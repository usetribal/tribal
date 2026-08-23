//! Throwaway diagnostic: run every adapter against every real local session
//! directory on this machine (not just the hand-written fixtures) and report
//! parse failures / suspicious output. Read-only — writes nothing to git.
//!
//! Usage: cargo run -p lineage-adapters --example probe_real_sessions

use std::path::{Path, PathBuf};

use lineage_adapters::{ClaudeAdapter, CodexAdapter, CursorAdapter};
use lineage_agent::{SessionReader, SessionRef};
use lineage_core::AgentKind;

fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME not set"))
}

struct Report {
    agent: &'static str,
    total_sessions: usize,
    read_ok: usize,
    read_err: usize,
    empty_turns: usize,
    empty_content_turns: usize,
    unresolved_tool_calls: usize,
    missing_timestamps: usize,
    errors: Vec<String>,
    by_name: std::collections::BTreeMap<String, usize>,
}

impl Report {
    fn new(agent: &'static str) -> Self {
        Self {
            agent,
            total_sessions: 0,
            read_ok: 0,
            read_err: 0,
            empty_turns: 0,
            empty_content_turns: 0,
            unresolved_tool_calls: 0,
            missing_timestamps: 0,
            errors: Vec::new(),
            by_name: std::collections::BTreeMap::new(),
        }
    }

    fn print(&self) {
        println!("\n=== {} ===", self.agent);
        println!("  sessions discovered: {}", self.total_sessions);
        println!("  read ok:             {}", self.read_ok);
        println!("  read err:            {}", self.read_err);
        println!("  sessions w/ 0 turns: {}", self.empty_turns);
        println!(
            "  turns w/ empty content & no tool_calls: {}",
            self.empty_content_turns
        );
        println!(
            "  tool_calls missing target: {}",
            self.unresolved_tool_calls
        );
        println!("  turns missing timestamp:   {}", self.missing_timestamps);
        if !self.by_name.is_empty() {
            println!("  --- unresolved by tool name ---");
            for (name, count) in &self.by_name {
                println!("    {count:>5}  {name}");
            }
        }
        if !self.errors.is_empty() {
            println!("  --- first {} errors ---", self.errors.len().min(15));
            for e in self.errors.iter().take(15) {
                println!("    {e}");
            }
        }
    }
}

/// Claude Code's directory-name encoding (`/` and also `.`/`_` all collapse to
/// `-`) is deliberately lossy and non-invertible — decoding the project dir
/// name back to a path guesses wrong whenever the real path contains a dash,
/// dot, or underscore (confirmed: a repo directory with a dash in its own
/// name decodes to a path with a slash where the dash was, which exists
/// nowhere). The transcript's own `cwd` field is the authoritative source
/// instead, so this reads it straight out of each file rather than
/// inverting the encoding.
fn real_cwd_from_transcript(path: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines().take(5) {
        let v: serde_json::Value = serde_json::from_str(line).ok()?;
        if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
            return Some(PathBuf::from(cwd));
        }
    }
    None
}

fn probe_claude(home: &Path) -> Report {
    let mut report = Report::new("claude");
    let mut seen_paths = std::collections::HashSet::new();
    let projects = home.join(".claude").join("projects");
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return report;
    };

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        for file_entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = file_entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".jsonl") || name == "history.jsonl" {
                continue;
            }
            if !seen_paths.insert(path.clone()) {
                continue;
            }
            // Skip transcripts this same tool wrote out (tribal fork /
            // render_claude_transcript) — testing format interop against our
            // own rendered output would be circular, not a real-vendor check.
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if text.contains("this turn used tools, recorded here as history") {
                continue;
            }
            let Some(cwd) = real_cwd_from_transcript(&path) else {
                continue; // no cwd recorded at all — nothing to scope the adapter to
            };
            report.total_sessions += 1;
            let adapter = ClaudeAdapter::new(&cwd);
            let session = SessionRef {
                id_hint: path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string(),
                agent: AgentKind::Claude,
                source_path: path,
                started_at: None,
            };
            inspect_session(&adapter, &session, &mut report);
        }
    }
    report
}

fn probe_codex(home: &Path) -> Report {
    let mut report = Report::new("codex");
    // Codex sessions are global under ~/.codex/sessions; workspace_root only
    // affects filtering by cwd match, so use $HOME as the root to include
    // everything discover() can see, then read regardless of match.
    let adapter = CodexAdapter::new(home);
    let global = home.join(".codex").join("sessions");
    let mut seen = std::collections::HashSet::new();
    for entry in walkdir::WalkDir::new(&global).into_iter().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !name.starts_with("rollout-") || !name.ends_with(".jsonl") {
            continue;
        }
        if !seen.insert(path.to_path_buf()) {
            continue;
        }
        report.total_sessions += 1;
        let session = SessionRef {
            id_hint: path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string(),
            agent: AgentKind::Codex,
            source_path: path.to_path_buf(),
            started_at: None,
        };
        inspect_session(&adapter, &session, &mut report);
    }
    report
}

fn probe_cursor(home: &Path) -> Report {
    let mut report = Report::new("cursor");
    let projects = home.join(".cursor").join("projects");
    let mut seen = std::collections::HashSet::new();
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return report;
    };
    for entry in entries.flatten() {
        let transcripts_dir = entry.path().join("agent-transcripts");
        if !transcripts_dir.is_dir() {
            continue;
        }
        // workspace_root doesn't gate cursor discovery (it just points at
        // .cursor/projects/*/agent-transcripts under it), so use the
        // project's parent dir stand-in: home itself works since we walk
        // transcripts_dir directly below instead of via adapter.discover().
        let adapter = CursorAdapter::new(home);
        for e in walkdir::WalkDir::new(&transcripts_dir)
            .into_iter()
            .flatten()
        {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
                continue;
            }
            if !seen.insert(path.to_path_buf()) {
                continue;
            }
            report.total_sessions += 1;
            let session = SessionRef {
                id_hint: path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string(),
                agent: AgentKind::Cursor,
                source_path: path.to_path_buf(),
                started_at: None,
            };
            inspect_session(&adapter, &session, &mut report);
        }
    }
    report
}

fn inspect_session(adapter: &impl SessionReader, session: &SessionRef, report: &mut Report) {
    match adapter.read(session) {
        Ok(conv) => {
            report.read_ok += 1;
            if conv.turns.is_empty() {
                report.empty_turns += 1;
            }
            for t in &conv.turns {
                if t.content.trim().is_empty() && t.tool_calls.is_empty() {
                    report.empty_content_turns += 1;
                }
                if t.timestamp.is_none() {
                    report.missing_timestamps += 1;
                }
                for tc in &t.tool_calls {
                    // tool_result carries an answer, not a call — it never
                    // names a target by design (ToolTarget documents "what a
                    // call acted on"), so counting it here would conflate a
                    // real gap with expected shape.
                    if tc.target.is_none() && tc.name != "tool_result" {
                        report.unresolved_tool_calls += 1;
                        *report.by_name.entry(tc.name.clone()).or_insert(0) += 1;
                    }
                }
            }
        }
        Err(e) => {
            report.read_err += 1;
            report
                .errors
                .push(format!("{}: {e}", session.source_path.display()));
        }
    }
}

fn main() {
    let home = home_dir();
    let claude = probe_claude(&home);
    let codex = probe_codex(&home);
    let cursor = probe_cursor(&home);

    claude.print();
    codex.print();
    cursor.print();
}
