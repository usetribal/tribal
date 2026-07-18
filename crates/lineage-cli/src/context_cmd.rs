use std::fs;
use std::io::Write;
use std::path::Path;

use lineage_core::{normalize_repo_path, RepoBinding};
use lineage_git::{open_repo, resolve_repo_binding};
use lineage_oracle::{
    CacheKey, ContextQuery, Evidence, LocalRetriever, OracleCache, Retrieval, Retriever, Strength,
    LOCAL_RETRIEVER_VERSION,
};
use lineage_search::LineageIndex;
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Wall budget for the whole hook fire. The agent's tool call blocks on us,
/// so overrunning is worse than answering with nothing.
const DEFAULT_BUDGET_MS: u64 = 200;
/// Selection defaults from context-injection-v0 "Digest format".
const MIN_STRENGTH: Strength = Strength::Low;
const MAX_EVIDENCE_ENTRIES: usize = 3;
const MAX_DIGEST_BYTES: usize = 4096;

const CONTEXT_LOG_FILE: &str = "context-log.jsonl";

/// Claude Code PostToolUse endpoint. Any error, unmatched input, or empty
/// selection returns `None` — the adapter MUST fail open (spec: Trigger
/// protocol); the caller prints nothing and exits 0.
pub fn hook_claude(repo_path: &Path, input: &str, now_unix: i64) -> Option<String> {
    run_claude_hook(repo_path, input, now_unix).unwrap_or_default()
}

fn run_claude_hook(repo_path: &Path, input: &str, now_unix: i64) -> Result<Option<String>> {
    let event: serde_json::Value = serde_json::from_str(input)?;
    if event["tool_name"].as_str() != Some("Read") {
        return Ok(None);
    }
    let Some(file_path) = event["tool_input"]["file_path"].as_str() else {
        return Ok(None);
    };
    // Only a plain-text tool response can be appended to faithfully; any
    // other shape means silence rather than a mangled tool result.
    let Some(original_output) = event["tool_response"].as_str() else {
        return Ok(None);
    };

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

    let selected = select(&retrieval);
    if selected.is_empty() {
        return Ok(None);
    }

    let digest = render_digest(&relative_path, &selected);
    append_injection_log(
        &git_dir,
        &relative_path,
        &selected,
        retrieval.strength,
        now_unix,
    )?;

    let output = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "updatedToolOutput": format!("{original_output}\n\n{digest}"),
        }
    });
    Ok(Some(output.to_string()))
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

/// Append-only local record of every injection — the "no shadow
/// improvements" surface (spec: Injection log). Never syncs.
fn append_injection_log(
    git_dir: &Path,
    file_path: &str,
    selected: &[&Evidence],
    strength: Strength,
    now_unix: i64,
) -> Result<()> {
    let entry = serde_json::json!({
        "ts": now_unix,
        "file_path": file_path,
        "session_ids": selected.iter().map(|e| e.session_id.as_str()).collect::<Vec<_>>(),
        "strength": strength,
    });
    let dir = git_dir.join("lineage");
    fs::create_dir_all(&dir)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(CONTEXT_LOG_FILE))?;
    writeln!(file, "{entry}")?;
    Ok(())
}

/// `git lineage context log` — what was injected, newest last.
pub fn print_log(repo_path: &Path, limit: usize) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let path = repo.inner().path().join("lineage").join(CONTEXT_LOG_FILE);
    if !path.exists() {
        println!("no context injections recorded");
        return Ok(());
    }

    let contents = fs::read_to_string(path)?;
    let lines: Vec<&str> = contents.lines().collect();
    let start = lines.len().saturating_sub(limit);
    for line in &lines[start..] {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let sessions = entry["session_ids"]
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
            entry["ts"],
            entry["file_path"].as_str().unwrap_or("?"),
            entry["strength"].as_str().unwrap_or("?"),
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

fn group_has_lineage_hook(group: &serde_json::Value) -> bool {
    group["hooks"].as_array().into_iter().flatten().any(|hook| {
        hook["command"]
            .as_str()
            .is_some_and(|c| c.contains("git lineage context hook"))
    })
}
