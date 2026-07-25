use std::fs;
use std::path::Path;

use chrono::{DateTime, Utc};
use lineage_core::{normalize_repo_path, RepoBinding};
use lineage_git::{open_repo, resolve_repo_binding};
use lineage_retrieval::{
    CacheKey, ContextQuery, LocalRetriever, RetrievalCache, Retriever, LOCAL_RETRIEVER_VERSION,
};
use lineage_search::LineageIndex;
use sha2::{Digest, Sha256};

use crate::digest::{render_digest, select, Trigger};
use crate::events::{EventLog, Outcome};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Wall budget for the whole hook fire. The agent's tool call blocks on us,
/// so overrunning is worse than answering with nothing.
const DEFAULT_BUDGET_MS: u64 = 200;

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

    // The repo is derived from the file the agent read, not from cwd: hooks
    // may be installed user-level and sessions opened from a parent workspace,
    // where cwd is the wrong (or no) repo — gap 7. `repo_path` remains the
    // fallback for relative tool paths.
    let discover_from = Path::new(file_path)
        .parent()
        .filter(|p| p.is_absolute())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| repo_path.to_path_buf());
    let repo = open_repo(&discover_from)?;
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
    let cache = RetrievalCache::open(git_dir.join("lineage").join("oracle.db"))?;

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
    // This endpoint is the `PostToolUse`/`Read` trigger: mid-task, appended into
    // a tool result the agent is actively reading, so it takes the tight budget.
    let selected = select(&retrieval, Trigger::FileKeyed);
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

    let digest = render_digest(&relative_path, &selected, Trigger::FileKeyed);
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

/// `git lineage context chain <file>:<line>`: print the temporal chain for a
/// line, one hop per row. One live blame anchors the line at HEAD to its commit;
/// every subsequent hop is resolved from the index tables (`line_history` takes
/// no repo, so it cannot blame). Mirrors `chain.sh`'s columns for parity.
pub fn chain(repo_path: &Path, target: &str) -> Result<()> {
    let (file_path, line) = parse_chain_target(target)?;
    let repo = open_repo(repo_path)?;

    let head = repo
        .inner()
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id().to_string())
        .unwrap_or_default();

    // The single allowed live blame: resolve the line at HEAD to its introducing
    // commit and that line's position within it — the anchor the indexed walk
    // starts from (`line_history` takes no repo, so it cannot blame).
    let anchor = lineage_git::resolve_anchor(repo.inner(), &head, &file_path, line)?;
    let Some((anchor_commit, anchor_line)) = anchor else {
        println!(
            "Temporal chain for {file_path}:{line} (HEAD={})",
            short(&head)
        );
        println!("no chain (line has no blame at HEAD)");
        return Ok(());
    };

    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    let hops = index.line_history(&file_path, anchor_line, &anchor_commit)?;

    println!(
        "Temporal chain for {file_path}:{line} (HEAD={})",
        short(&head)
    );
    if hops.is_empty() {
        println!("no chain (line not on indexed lineage history)");
        return Ok(());
    }
    for (i, hop) in hops.iter().enumerate() {
        print_hop(i + 1, hop);
    }
    println!("Chain length: {} hops", hops.len());
    Ok(())
}

fn print_hop(hop_num: usize, hop: &lineage_search::Hop) {
    let date = DateTime::<Utc>::from_timestamp(hop.committed_at, 0)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "-".to_string());
    let (session, turn, conf) = match (&hop.session_id, &hop.turn_id, &hop.confidence) {
        (Some(s), Some(t), Some(c)) => (s.clone(), t.clone(), c.clone()),
        _ => (
            "-".to_string(),
            "-".to_string(),
            format!("DARK({})", hop.hop_kind),
        ),
    };
    println!(
        "{:<4} {:<10} {:<11} {:<28} {:<32} {:<14} {}:{}",
        hop_num,
        &hop.commit_sha[..hop.commit_sha.len().min(8)],
        date,
        session,
        turn,
        conf,
        hop.file_path,
        hop.start_line,
    );
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(9)]
}

fn parse_chain_target(target: &str) -> Result<(String, u32)> {
    let (path, line_str) = target
        .rsplit_once(':')
        .ok_or("target must be <file>:<line>")?;
    let line: u32 = line_str.parse().map_err(|_| "line must be a number")?;
    Ok((normalize_repo_path(path, None), line))
}

/// Claude Code `SessionStart` endpoint: teach the traversal vocabulary once per
/// session. MCP-connected agents discover the verbs free via `tools/list`; a CLI
/// session has no such channel, so without this it would have capability it
/// could not find.
///
/// The hook *states a capability*, it does not instruct — an agent is never told
/// to use lineage, which is what keeps the A/B harness's treatment arm honest.
///
/// Fails open like the others: a payload that does not parse still gets the
/// vocabulary, and nothing here can exit nonzero.
pub fn hook_claude_session_start(repo_path: &Path, input: &str, now_unix: i64) -> Option<String> {
    // The payload is not read for content — the vocabulary is the same whatever
    // the session or its `source` (startup/resume/clear/compact). Parsing it
    // only to reject a malformed one would make the hook *more* fragile for no
    // gain, so a bad payload still gets the vocabulary.
    let source = serde_json::from_str::<serde_json::Value>(input)
        .ok()
        .and_then(|event| event["source"].as_str().map(str::to_string));

    if let Some(log) = EventLog::for_repo_path(repo_path) {
        log.append(
            hook_timestamp(now_unix),
            "context_session_start",
            Outcome::Ok,
            serde_json::json!({ "harness": HOOK_HARNESS, "source": source }),
        );
    }

    let output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": crate::digest::verb_vocabulary(),
        }
    });
    Some(output.to_string())
}

const CLAUDE_SETTINGS_FILE: &str = ".claude/settings.json";
const HOOK_COMMAND: &str = "git lineage context hook claude";
const SESSION_START_HOOK_COMMAND: &str = "git lineage context hook claude-session-start";

/// Wires the injection endpoint into Claude Code's project settings.
/// Idempotent, and merges into existing settings rather than replacing them —
/// the file is shared user configuration, not ours.
pub fn install_claude_agent_hook(repo_path: &Path) -> Result<bool> {
    let installed = install_claude_agent_hook_at(&repo_path.join(CLAUDE_SETTINGS_FILE))?;
    if let Some(log) = EventLog::for_repo_path(repo_path) {
        log.append(
            Utc::now(),
            "install_claude_agent_hook",
            Outcome::Ok,
            serde_json::json!({ "already_installed": !installed }),
        );
    }
    Ok(installed)
}

/// User-level install (`~/.claude/settings.json`): one wiring covers every
/// repo, because the hook derives its repo from the file the agent read and
/// fails open outside lineage repos — the gap 7 answer for parent-workspace
/// sessions whose project root never loads a nested repo's settings.
pub fn install_claude_agent_hook_user() -> Result<bool> {
    install_claude_agent_hook_at(&user_settings_path()?)
}

pub fn uninstall_claude_agent_hook_user() -> Result<bool> {
    uninstall_claude_agent_hook_at(&user_settings_path()?)
}

fn user_settings_path() -> Result<std::path::PathBuf> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(Path::new(&home).join(CLAUDE_SETTINGS_FILE))
}

/// The two hook groups lineage owns: the file-keyed injection endpoint and the
/// once-per-session vocabulary. `SessionStart` groups carry no matcher — there
/// is no tool to match on.
const LINEAGE_HOOK_GROUPS: &[(&str, Option<&str>, &str)] = &[
    ("PostToolUse", Some("Read"), HOOK_COMMAND),
    ("SessionStart", None, SESSION_START_HOOK_COMMAND),
];

fn install_claude_agent_hook_at(path: &Path) -> Result<bool> {
    let path = path.to_path_buf();
    let mut settings: serde_json::Value = match fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents)
            .map_err(|e| format!("{CLAUDE_SETTINGS_FILE} is not valid JSON: {e}"))?,
        Err(_) => serde_json::json!({}),
    };

    let mut wrote_any = false;
    for (event, matcher, command) in LINEAGE_HOOK_GROUPS {
        wrote_any |= add_hook_group(&mut settings, event, *matcher, command)?;
    }
    // Nothing to write means every group was already present; leaving the file
    // untouched keeps install idempotent down to its mtime.
    if !wrote_any {
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, format!("{:#}\n", settings))?;
    Ok(true)
}

/// Append one lineage hook group under `hooks.<event>`, or report that an
/// equivalent one is already there. Merges into existing settings rather than
/// replacing them — the file is shared user configuration, not ours.
fn add_hook_group(
    settings: &mut serde_json::Value,
    event: &str,
    matcher: Option<&str>,
    command: &str,
) -> Result<bool> {
    let entry = settings
        .as_object_mut()
        .ok_or(format!("{CLAUDE_SETTINGS_FILE} root is not an object"))?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("settings 'hooks' is not an object")?
        .entry(event.to_string())
        .or_insert_with(|| serde_json::json!([]));
    let groups = entry
        .as_array_mut()
        .ok_or(format!("settings 'hooks.{event}' is not an array"))?;

    if groups
        .iter()
        .any(|group| group_has_lineage_command(group, command))
    {
        return Ok(false);
    }

    let mut group = serde_json::json!({
        "hooks": [{ "type": "command", "command": command }],
    });
    if let Some(matcher) = matcher {
        group["matcher"] = serde_json::json!(matcher);
    }
    groups.push(group);
    Ok(true)
}

/// Removes only lineage-owned wiring; everything else in the file survives.
pub fn uninstall_claude_agent_hook(repo_path: &Path) -> Result<bool> {
    uninstall_claude_agent_hook_at(&repo_path.join(CLAUDE_SETTINGS_FILE))
}

fn uninstall_claude_agent_hook_at(path: &Path) -> Result<bool> {
    let path = path.to_path_buf();
    let Ok(contents) = fs::read_to_string(&path) else {
        return Ok(false);
    };
    let mut settings: serde_json::Value = serde_json::from_str(&contents)?;

    let mut removed_any = false;
    for (event, _, _) in LINEAGE_HOOK_GROUPS {
        let Some(groups) = settings
            .get_mut("hooks")
            .and_then(|hooks| hooks.get_mut(*event))
            .and_then(|groups| groups.as_array_mut())
        else {
            continue;
        };
        let before = groups.len();
        groups.retain(|group| !group_has_lineage_hook(group));
        removed_any |= groups.len() != before;
    }
    if !removed_any {
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

/// Any lineage-owned hook group — what uninstall and doctor look for.
fn group_has_lineage_hook(group: &serde_json::Value) -> bool {
    group["hooks"].as_array().into_iter().flatten().any(|hook| {
        hook["command"]
            .as_str()
            .is_some_and(|c| c.contains("git lineage context hook"))
    })
}

/// One *specific* lineage hook group. Install matches on the exact command
/// because the two groups share a prefix: a repo with the file-keyed hook
/// already installed must still gain the `SessionStart` one.
fn group_has_lineage_command(group: &serde_json::Value, command: &str) -> bool {
    group["hooks"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|hook| hook["command"].as_str() == Some(command))
}
