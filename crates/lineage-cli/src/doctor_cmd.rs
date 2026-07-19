//! `git lineage doctor` — the five-section diagnosis report defined by
//! `specs/diagnostics-v0.md`, assembled from refs, config, the search index,
//! and the event log. Read-only: inspecting a repo never repairs it.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use lineage_core::Conversation;
use lineage_git::{
    audit_materialization, list_notes, list_session_ids, open_repo, read_conversation_stored,
    run_doctor, run_doctor_refs, LineageRepo,
};

use crate::context_cmd;
use crate::events::EventLog;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub const DOCTOR_SCHEMA_VERSION: &str = "lineage-doctor-v0";
const SECTIONS: [&str; 5] = ["setup", "capture", "materialization", "links", "activity"];
pub const DEFAULT_ACTIVITY_LIMIT: usize = 20;

pub struct DoctorArgs {
    pub json: bool,
    pub sections: Vec<String>,
    pub activity_limit: usize,
}

pub fn run(repo_path: &Path, args: &DoctorArgs) -> Result<()> {
    for section in &args.sections {
        if !SECTIONS.contains(&section.as_str()) {
            return Err(format!(
                "unknown section: {section} (sections: {})",
                SECTIONS.join(", ")
            )
            .into());
        }
    }

    let report = doctor_report_sections(repo_path, &args.sections, args.activity_limit)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    render_text(&report);
    Ok(())
}

/// The full `--json` object, unfiltered. Also the MCP `lineage_doctor` payload.
pub fn doctor_report(repo_path: &Path, activity_limit: usize) -> Result<serde_json::Value> {
    doctor_report_sections(repo_path, &[], activity_limit)
}

/// Builds only the requested sections (empty = all): `--section` filtering
/// happens before the work, not after, so a cheap section is a cheap command —
/// capture's per-session LFS scan and materialization's commit diffs are the
/// expensive passes and must not run when their section is filtered out.
fn doctor_report_sections(
    repo_path: &Path,
    sections: &[String],
    activity_limit: usize,
) -> Result<serde_json::Value> {
    let wants = |name: &str| sections.is_empty() || sections.iter().any(|section| section == name);
    let repo = open_repo(repo_path)?;

    let conversations = if wants("setup") || wants("capture") || wants("links") {
        load_conversations(&repo)?
    } else {
        Vec::new()
    };
    let events = if wants("capture") || wants("links") || wants("activity") {
        EventLog::for_git_dir(&repo.git_dir()).read_entries()
    } else {
        Vec::new()
    };

    let mut report = serde_json::json!({ "schema_version": DOCTOR_SCHEMA_VERSION });
    if wants("setup") {
        let refs = run_doctor_refs(&repo)?;
        report["setup"] = setup_section(&repo, &refs, &conversations);
    }
    if wants("capture") {
        let full = run_doctor(&repo)?;
        report["capture"] = capture_section(&repo, &full, &conversations, &events);
    }
    if wants("materialization") {
        report["materialization"] = materialization_section(&repo)?;
    }
    if wants("links") {
        report["links"] = links_section(&repo, &conversations, &events)?;
    }
    if wants("activity") {
        report["activity"] = activity_tail(&events, activity_limit);
    }
    Ok(report)
}

fn load_conversations(repo: &LineageRepo) -> Result<Vec<Conversation>> {
    let mut out = Vec::new();
    for id in list_session_ids(repo.inner())? {
        if let Some(conv) = read_conversation_stored(repo.inner(), &id)? {
            out.push(conv);
        }
    }
    Ok(out)
}

fn setup_section(
    repo: &LineageRepo,
    base: &lineage_git::DoctorReport,
    conversations: &[Conversation],
) -> serde_json::Value {
    let index_schema =
        lineage_search::inspect_schema(repo.git_dir().join("lineage").join("index.db"))
            .map(|info| {
                serde_json::json!({
                    "has_session_files": info.has_session_files,
                    "has_index_meta": info.has_index_meta,
                    "generation": info.generation,
                })
            })
            .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }));

    let mut warnings = base.warnings.clone();
    if let Some(false) = index_schema["has_session_files"].as_bool() {
        warnings.push(
            "search index predates the current schema (missing session_files); \
             run: git lineage rebuild-index"
                .into(),
        );
    }

    let (settings_present, hook_registered) = context_cmd::claude_hook_status(repo.workdir());
    let loadable = hook_loadable_from_session_roots(repo, conversations, hook_registered);
    if !loadable {
        warnings.push(
            "context hook is not loadable from every session root: sessions were opened \
             from a directory whose .claude/settings.json does not register the hook"
                .into(),
        );
    }

    let hooks_dir = repo.git_dir().join("hooks");
    serde_json::json!({
        "binary_version": env!("CARGO_PKG_VERSION"),
        "is_git_repo": base.is_git_repo,
        "notes_ref_ok": base.notes_ref_ok,
        "index_ref_ok": base.index_ref_ok,
        "config_ref_ok": base.config_ref_ok,
        "index_schema": index_schema,
        "hook_wiring": {
            "claude_settings_present": settings_present,
            "lineage_hook_registered": hook_registered,
            "loadable_from_session_root": loadable,
        },
        "git_hooks": {
            "pre_commit_installed": lineage_hook_installed(&hooks_dir.join("pre-commit")),
            "post_commit_installed": lineage_hook_installed(&hooks_dir.join("post-commit")),
        },
        "warnings": warnings,
    })
}

/// The hook only fires for sessions whose root's Claude settings register it.
/// Every distinct workspace root among stored sessions must be wired; with no
/// sessions the repo root stands in.
fn hook_loadable_from_session_roots(
    repo: &LineageRepo,
    conversations: &[Conversation],
    repo_root_registered: bool,
) -> bool {
    let repo_root = canonical(repo.workdir());
    let mut roots: Vec<String> = conversations
        .iter()
        .map(|c| canonical(Path::new(&c.workspace_root)))
        .collect();
    roots.sort();
    roots.dedup();
    if roots.is_empty() {
        return repo_root_registered;
    }
    roots.iter().all(|root| {
        if *root == repo_root {
            repo_root_registered
        } else {
            context_cmd::claude_hook_status(Path::new(root)).1
        }
    })
}

fn lineage_hook_installed(path: &Path) -> bool {
    fs::read_to_string(path).is_ok_and(|content| content.contains("Lineage"))
}

fn capture_section(
    repo: &LineageRepo,
    base: &lineage_git::DoctorReport,
    conversations: &[Conversation],
    events: &[serde_json::Value],
) -> serde_json::Value {
    let discovered = events
        .iter()
        .rev()
        .find(|e| e["op"] == "import")
        .map(|e| e["detail"]["discovered"].clone())
        .unwrap_or_else(|| serde_json::json!({}));

    serde_json::json!({
        "sessions_discovered": discovered,
        "sessions_imported": conversations.len(),
        "workspace_mismatches": workspace_mismatches(conversations, &canonical(repo.workdir())),
        "broken_sessions": base.broken_sessions,
        "missing_lfs_blobs": base.missing_lfs_blobs,
    })
}

fn workspace_mismatches(conversations: &[Conversation], repo_root: &str) -> Vec<serde_json::Value> {
    conversations
        .iter()
        .filter_map(|c| {
            let session_root = canonical(Path::new(&c.workspace_root));
            if session_root == repo_root {
                return None;
            }
            Some(serde_json::json!({
                "session_id": c.id.as_str(),
                "workspace_root": session_root,
                "repo_root": repo_root,
            }))
        })
        .collect()
}

fn materialization_section(repo: &LineageRepo) -> Result<serde_json::Value> {
    let funnel = audit_materialization(repo.inner())?;
    Ok(serde_json::json!({
        "total_artifacts": funnel.total_artifacts,
        "resolvable": funnel.resolvable,
        "resolved": funnel.resolved,
        "line_objects": funnel.line_objects,
        "failure_reasons": {
            "no_resolve_payload": funnel.no_resolve_payload,
            "missing_old_string": funnel.missing_old_string,
            "old_string_not_found": funnel.old_string_not_found,
            "commit_not_linked": funnel.commit_not_linked,
        },
    }))
}

fn links_section(
    repo: &LineageRepo,
    conversations: &[Conversation],
    events: &[serde_json::Value],
) -> Result<serde_json::Value> {
    let mut triggers: BTreeMap<(String, String), String> = BTreeMap::new();
    for event in events.iter().filter(|e| e["op"] == "link") {
        let Some(commit) = event["detail"]["commit_sha"].as_str() else {
            continue;
        };
        let trigger = event["detail"]["trigger"].as_str().unwrap_or("unknown");
        for session in event["detail"]["sessions"].as_array().into_iter().flatten() {
            if let Some(id) = session["session_id"].as_str() {
                triggers.insert((commit.into(), id.into()), trigger.into());
            }
        }
    }
    let auto_matched: Vec<&str> = conversations
        .iter()
        .filter(|c| c.metadata.contains_key("commit_match_score"))
        .map(|c| c.id.as_str())
        .collect();

    let mut out = Vec::new();
    for note in list_notes(repo.inner())? {
        let sessions: Vec<serde_json::Value> = note
            .session_ids
            .iter()
            .map(|id| {
                let established_by = triggers
                    .get(&(note.commit_sha.clone(), id.to_string()))
                    .cloned()
                    .unwrap_or_else(|| {
                        if auto_matched.contains(&id.as_str()) {
                            "auto_match".into()
                        } else {
                            "unknown".into()
                        }
                    });
                serde_json::json!({
                    "session_id": id.as_str(),
                    "established_by": established_by,
                })
            })
            .collect();
        out.push(serde_json::json!({
            "commit_sha": note.commit_sha,
            "sessions": sessions,
        }));
    }
    Ok(serde_json::Value::Array(out))
}

fn activity_tail(events: &[serde_json::Value], limit: usize) -> serde_json::Value {
    let start = events.len().saturating_sub(limit);
    serde_json::Value::Array(events[start..].to_vec())
}

fn canonical(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

fn render_text(report: &serde_json::Value) {
    println!("Lineage doctor");

    if let Some(setup) = report.get("setup") {
        println!("setup");
        println!("  git-lineage:     {}", str_of(&setup["binary_version"]));
        println!("  git repo:        {}", setup["is_git_repo"]);
        println!("  notes ref:       {}", setup["notes_ref_ok"]);
        println!("  index ref:       {}", setup["index_ref_ok"]);
        println!("  config ref:      {}", setup["config_ref_ok"]);
        println!(
            "  index schema:    session_files={} index_meta={} generation={}",
            setup["index_schema"]["has_session_files"],
            setup["index_schema"]["has_index_meta"],
            setup["index_schema"]["generation"]
        );
        println!(
            "  context hook:    settings={} registered={} loadable_from_session_root={}",
            setup["hook_wiring"]["claude_settings_present"],
            setup["hook_wiring"]["lineage_hook_registered"],
            setup["hook_wiring"]["loadable_from_session_root"]
        );
        println!(
            "  git hooks:       pre-commit={} post-commit={}",
            setup["git_hooks"]["pre_commit_installed"], setup["git_hooks"]["post_commit_installed"]
        );
        for warning in setup["warnings"].as_array().into_iter().flatten() {
            println!("  warning: {}", str_of(warning));
        }
    }

    if let Some(capture) = report.get("capture") {
        println!("capture");
        println!("  imported:        {}", capture["sessions_imported"]);
        if let Some(discovered) = capture["sessions_discovered"].as_object() {
            for (agent, count) in discovered {
                println!("  discovered:      {agent} {count}");
            }
        }
        for m in capture["workspace_mismatches"]
            .as_array()
            .into_iter()
            .flatten()
        {
            println!(
                "  workspace mismatch: {} at {}",
                str_of(&m["session_id"]),
                str_of(&m["workspace_root"])
            );
        }
        let broken = capture["broken_sessions"].as_array().map_or(0, Vec::len);
        if broken > 0 {
            println!("  broken sessions: {broken}");
        }
        let missing = capture["missing_lfs_blobs"].as_array().map_or(0, Vec::len);
        if missing > 0 {
            println!("  missing LFS:     {missing}");
        }
    }

    if let Some(m) = report.get("materialization") {
        println!("materialization");
        println!(
            "  funnel:          {} artifacts -> {} resolvable -> {} resolved -> {} line objects",
            m["total_artifacts"], m["resolvable"], m["resolved"], m["line_objects"]
        );
        if let Some(reasons) = m["failure_reasons"].as_object() {
            for (reason, count) in reasons {
                if count.as_u64().unwrap_or(0) > 0 {
                    println!("  loss:            {reason} {count}");
                }
            }
        }
    }

    if let Some(links) = report.get("links").and_then(|l| l.as_array()) {
        println!("links");
        for link in links {
            let sha = str_of(&link["commit_sha"]);
            let sessions = link["sessions"].as_array().map_or(0, Vec::len);
            println!("  {}: {} session(s)", &sha[..sha.len().min(8)], sessions);
        }
    }

    if let Some(activity) = report.get("activity").and_then(|a| a.as_array()) {
        println!("activity");
        for entry in activity {
            println!(
                "  {}  {}  [{}]",
                str_of(&entry["ts"]),
                str_of(&entry["op"]),
                str_of(&entry["outcome"])
            );
        }
    }
}

fn str_of(value: &serde_json::Value) -> &str {
    value.as_str().unwrap_or("?")
}
