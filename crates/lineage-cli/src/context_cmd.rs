use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use lineage_core::{normalize_repo_path, RepoBinding};
use lineage_git::{open_repo, resolve_repo_binding};
use lineage_oracle::{
    CacheKey, ContextQuery, Evidence, LocalRetriever, OracleCache, Retrieval, Retriever, Strength,
    LOCAL_RETRIEVER_VERSION,
};
use lineage_search::LineageIndex;
use sha2::{Digest, Sha256};

use crate::events::{EventLog, Outcome};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Wall budget for the whole hook fire. The agent's tool call blocks on us,
/// so overrunning is worse than answering with nothing.
const DEFAULT_BUDGET_MS: u64 = 200;
/// Selection defaults from context-injection-v0 "Digest format".
const MIN_STRENGTH: Strength = Strength::Low;
const MAX_EVIDENCE_ENTRIES: usize = 3;
const MAX_DIGEST_BYTES: usize = 4096;

const HOOK_HARNESS: &str = "claude";

/// Claude Code PostToolUse endpoint. Any error, unmatched input, or empty
/// selection returns `None` — the adapter MUST fail open (spec: Trigger
/// protocol); the caller prints nothing and exits 0. Every fired-but-silent
/// outcome is recorded in the event log with its reason (diagnostics-v0
/// "Silent-fire reasons"), so silence stays distinguishable from a hook that
/// never ran.
pub fn hook_claude(repo_path: &Path, input: &str, now_unix: i64) -> Option<String> {
    match run_claude_hook(repo_path, input, now_unix) {
        Ok(output) => output,
        Err(e) => {
            log_hook_error(repo_path, input, now_unix, e.as_ref());
            None
        }
    }
}

/// An `Err` from the hook still exits 0 and prints nothing; the reason and
/// message land in the event log instead. Reparses the input for the file
/// path because the error may have struck before any state was available.
fn log_hook_error(repo_path: &Path, input: &str, now_unix: i64, error: &dyn std::error::Error) {
    let Ok(event) = serde_json::from_str::<serde_json::Value>(input) else {
        return;
    };
    let Some(file_path) = event["tool_input"]["file_path"].as_str() else {
        return;
    };
    let Some(log) = EventLog::for_repo_path(repo_path) else {
        return;
    };
    log.append(
        hook_timestamp(now_unix),
        "context_hook",
        Outcome::Error,
        serde_json::json!({
            "file_path": file_path,
            "harness": HOOK_HARNESS,
            "reason": "error",
            "error": error.to_string(),
        }),
    );
}

/// The hook's injected clock is unix seconds; the event log speaks RFC 3339.
fn hook_timestamp(now_unix: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(now_unix, 0).unwrap_or_default()
}

fn run_claude_hook(repo_path: &Path, input: &str, now_unix: i64) -> Result<Option<String>> {
    let event: serde_json::Value = serde_json::from_str(input)?;
    if event["tool_name"].as_str() != Some("Read") {
        return Ok(None);
    }
    let Some(file_path) = event["tool_input"]["file_path"].as_str() else {
        return Ok(None);
    };
    // Only shapes we can append to faithfully are handled; anything else
    // means silence rather than a mangled tool result. Checked before
    // retrieval so unappendable events cost only the event-log write.
    if !response_is_appendable(&event["tool_response"]) {
        if let Some(log) = EventLog::for_repo_path(repo_path) {
            log_silent(&log, file_path, now_unix, "unappendable_shape");
        }
        return Ok(None);
    }

    let repo = open_repo(repo_path)?;
    let workdir = repo.workdir().to_path_buf();

    let content = fs::read(
        workdir.join(
            Path::new(file_path)
                .strip_prefix(&workdir)
                .unwrap_or(Path::new(file_path)),
        ),
    )?;
    let file_blob_sha = format!("{:x}", Sha256::digest(&content));
    let relative_path = normalize_repo_path(file_path, Some(&workdir));

    let git_dir = repo.git_dir();
    let index = LineageIndex::open(git_dir.join("lineage").join("index.db"))?;
    let cache = OracleCache::open(git_dir.join("lineage").join("oracle.db"))?;

    let key = CacheKey {
        file_path: &relative_path,
        file_blob_sha: &file_blob_sha,
        corpus_generation: index.generation()?,
        retriever_version: LOCAL_RETRIEVER_VERSION,
    };

    let retrieval = match cache.get(&key)? {
        Some(cached) => cached,
        None => {
            let query = ContextQuery {
                file_path: relative_path.clone(),
                file_blob_sha: file_blob_sha.clone(),
                repo: repo_binding_or_local(&repo),
                budget_ms: Some(DEFAULT_BUDGET_MS),
            };
            let retriever = LocalRetriever::new(repo.inner(), &index);
            let retrieval = retriever.retrieve(&query)?;
            cache.put(&key, &retrieval, now_unix)?;
            retrieval
        }
    };

    let log = EventLog::for_git_dir(&git_dir);
    let selected = select(&retrieval);
    if selected.is_empty() {
        // Truncation only explains an empty result; a nonempty one that all
        // fell below the floor is below_floor even if time also ran out.
        let reason = if !retrieval.evidence.is_empty() {
            "below_floor"
        } else if retrieval.truncated {
            "over_budget"
        } else {
            "no_evidence"
        };
        log_silent(&log, &relative_path, now_unix, reason);
        return Ok(None);
    }

    let digest = render_digest(&relative_path, &selected);
    let Some(updated) = append_digest_to_response(&event["tool_response"], &digest) else {
        log_silent(&log, &relative_path, now_unix, "unappendable_shape");
        return Ok(None);
    };
    log.append(
        hook_timestamp(now_unix),
        "context_hook",
        Outcome::Ok,
        serde_json::json!({
            "file_path": relative_path,
            "harness": HOOK_HARNESS,
            "session_ids": selected.iter().map(|e| e.session_id.as_str()).collect::<Vec<_>>(),
            "strength": retrieval.strength,
        }),
    );

    let output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "updatedToolOutput": updated,
        }
    });
    Ok(Some(output.to_string()))
}

fn log_silent(log: &EventLog, file_path: &str, now_unix: i64, reason: &str) {
    log.append(
        hook_timestamp(now_unix),
        "context_hook",
        Outcome::Silent,
        serde_json::json!({
            "file_path": file_path,
            "harness": HOOK_HARNESS,
            "reason": reason,
        }),
    );
}

/// Claude Code's Read returns `{"type":"text","file":{"content",…}}` (observed
/// live, 2026-07-18); older/simpler payloads may carry a bare string. Both are
/// appended to in place so the tool result stays shape-identical.
fn response_is_appendable(response: &serde_json::Value) -> bool {
    response.is_string() || response["file"]["content"].is_string()
}

fn append_digest_to_response(
    response: &serde_json::Value,
    digest: &str,
) -> Option<serde_json::Value> {
    if let Some(text) = response.as_str() {
        return Some(serde_json::Value::String(format!("{text}\n\n{digest}")));
    }
    let content = response["file"]["content"].as_str()?;
    let mut updated = response.clone();
    updated["file"]["content"] = serde_json::Value::String(format!("{content}\n\n{digest}"));
    Some(updated)
}

/// A repo with no remote still gets local retrieval; the binding only
/// matters once a remote retriever answers the query.
fn repo_binding_or_local(repo: &lineage_git::LineageRepo) -> RepoBinding {
    resolve_repo_binding(repo.inner(), "origin").unwrap_or(RepoBinding {
        normalized_remote_url: String::new(),
        root_commit_sha: String::new(),
        server_repo_id: None,
    })
}

/// The selector: presentation policy over an already-final retrieval.
/// Evidence arrives strongest-first, so truncation keeps the best entries.
fn select(retrieval: &Retrieval) -> Vec<&Evidence> {
    retrieval
        .evidence
        .iter()
        .filter(|e| e.strength >= MIN_STRENGTH)
        .take(MAX_EVIDENCE_ENTRIES)
        .collect()
}

fn render_digest(file_path: &str, selected: &[&Evidence]) -> String {
    let mut digest = format!(
        "Lineage: {} past session(s) touched {file_path} — details below.\n",
        selected.len(),
    );
    for evidence in selected {
        digest.push_str(&format!("- {}", evidence.attribution));
        if !evidence.line_ranges.is_empty() {
            let ranges: Vec<String> = evidence
                .line_ranges
                .iter()
                .map(|[start, end]| format!("{start}-{end}"))
                .collect();
            digest.push_str(&format!(" (lines {})", ranges.join(", ")));
        }
        digest.push('\n');
        for line in evidence.summary.lines() {
            digest.push_str(&format!("  {line}\n"));
        }
    }
    truncate_to_bytes(&digest, MAX_DIGEST_BYTES)
}

fn truncate_to_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// `git lineage context log` — what was injected, newest last. The injection
/// log required by context-injection-v0 is a view over the event log's
/// `context_hook` entries with `outcome: "ok"` (spec: diagnostics-v0), not a
/// separate file.
pub fn print_log(repo_path: &Path, limit: usize) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let injections: Vec<serde_json::Value> = EventLog::for_git_dir(&repo.git_dir())
        .read_entries()
        .into_iter()
        .filter(|e| e["op"] == "context_hook" && e["outcome"] == "ok")
        .collect();
    if injections.is_empty() {
        println!("no context injections recorded");
        return Ok(());
    }

    let start = injections.len().saturating_sub(limit);
    for entry in &injections[start..] {
        let detail = &entry["detail"];
        let sessions = detail["session_ids"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        println!(
            "{}  {}  [{}]  sessions: {}",
            entry["ts"].as_str().unwrap_or("?"),
            detail["file_path"].as_str().unwrap_or("?"),
            detail["strength"].as_str().unwrap_or("?"),
            sessions,
        );
    }
    Ok(())
}

const CLAUDE_SETTINGS_FILE: &str = ".claude/settings.json";
const HOOK_COMMAND: &str = "git lineage context hook claude";

/// Wires the injection endpoint into Claude Code's project settings.
/// Idempotent, and merges into existing settings rather than replacing them —
/// the file is shared user configuration, not ours.
pub fn install_claude_agent_hook(repo_path: &Path) -> Result<bool> {
    let path = repo_path.join(CLAUDE_SETTINGS_FILE);
    let mut settings: serde_json::Value = match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map_err(|e| format!("{CLAUDE_SETTINGS_FILE} is not valid JSON: {e}"))?,
        Err(_) => serde_json::json!({}),
    };

    let post_tool_use = settings
        .as_object_mut()
        .ok_or(format!("{CLAUDE_SETTINGS_FILE} root is not an object"))?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("settings 'hooks' is not an object")?
        .entry("PostToolUse")
        .or_insert_with(|| serde_json::json!([]));
    let groups = post_tool_use
        .as_array_mut()
        .ok_or("settings 'hooks.PostToolUse' is not an array")?;

    if groups.iter().any(group_has_lineage_hook) {
        if let Some(log) = EventLog::for_repo_path(repo_path) {
            log.append(
                Utc::now(),
                "install_claude_agent_hook",
                Outcome::Ok,
                serde_json::json!({ "already_installed": true }),
            );
        }
        return Ok(false);
    }

    groups.push(serde_json::json!({
        "matcher": "Read",
        "hooks": [{ "type": "command", "command": HOOK_COMMAND }],
    }));

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, format!("{:#}\n", settings))?;

    if let Some(log) = EventLog::for_repo_path(repo_path) {
        log.append(
            Utc::now(),
            "install_claude_agent_hook",
            Outcome::Ok,
            serde_json::json!({ "already_installed": false }),
        );
    }
    Ok(true)
}

/// Removes only lineage-owned wiring; everything else in the file survives.
pub fn uninstall_claude_agent_hook(repo_path: &Path) -> Result<bool> {
    let path = repo_path.join(CLAUDE_SETTINGS_FILE);
    let Ok(contents) = fs::read_to_string(&path) else {
        return Ok(false);
    };
    let mut settings: serde_json::Value = serde_json::from_str(&contents)?;

    let Some(groups) = settings
        .get_mut("hooks")
        .and_then(|h| h.get_mut("PostToolUse"))
        .and_then(|p| p.as_array_mut())
    else {
        return Ok(false);
    };

    let before = groups.len();
    groups.retain(|group| !group_has_lineage_hook(group));
    if groups.len() == before {
        return Ok(false);
    }

    fs::write(&path, format!("{:#}\n", settings))?;
    Ok(true)
}

/// `(settings_present, hook_registered)` for the Claude settings at `root` —
/// doctor's view of whether a session opened at that directory loads the hook.
pub(crate) fn claude_hook_status(root: &Path) -> (bool, bool) {
    let Ok(contents) = fs::read_to_string(root.join(CLAUDE_SETTINGS_FILE)) else {
        return (false, false);
    };
    let Ok(settings) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return (true, false);
    };
    let registered = settings["hooks"]["PostToolUse"]
        .as_array()
        .into_iter()
        .flatten()
        .any(group_has_lineage_hook);
    (true, registered)
}

fn group_has_lineage_hook(group: &serde_json::Value) -> bool {
    group["hooks"].as_array().into_iter().flatten().any(|hook| {
        hook["command"]
            .as_str()
            .is_some_and(|c| c.contains("git lineage context hook"))
    })
}
