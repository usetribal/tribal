use std::fs;
use std::path::Path;

use lineage_cli::context_cmd;
use lineage_core::{AgentKind, Artifact, ArtifactKind, Conversation, LineageId, Role, Turn};
use lineage_git::write_conversation;
use lineage_search::LineageIndex;

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git2::Repository::init(dir.path()).unwrap();
    dir
}

fn store_session_touching(repo_root: &Path, file_path: &str) -> Conversation {
    let repo = git2::Repository::open(repo_root).unwrap();
    let mut conv = Conversation::new(AgentKind::Claude, repo_root.to_string_lossy());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "Introduce rate limiting on login".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: String::new(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![Artifact {
            kind: ArtifactKind::FileEdit,
            path: file_path.into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: None,
            resolve: None,
        }],
    });
    write_conversation(&repo, &conv).unwrap();
    let index = LineageIndex::open(repo.path().join("lineage").join("index.db")).unwrap();
    index.index_conversation(&conv).unwrap();
    conv
}

fn hook_input(file_path: &Path) -> String {
    serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Read",
        "tool_input": { "file_path": file_path.to_string_lossy() },
        "tool_response": "1: fn login() {}",
    })
    .to_string()
}

#[test]
fn covered_file_injects_attributed_digest_and_logs_it() {
    let dir = init_repo();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    let file = dir.path().join("src/auth.rs");
    fs::write(&file, "fn login() {}\n").unwrap();
    store_session_touching(dir.path(), "src/auth.rs");

    let output = context_cmd::hook_claude(dir.path(), &hook_input(&file), 1_753_000_000)
        .expect("covered file should inject");

    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    let updated = parsed["hookSpecificOutput"]["updatedToolOutput"]
        .as_str()
        .unwrap();
    assert!(updated.starts_with("1: fn login() {}"));
    assert!(updated.contains("Lineage: 1 past session(s) touched src/auth.rs"));
    assert!(updated.contains("claude session"));
    assert!(updated.contains("Introduce rate limiting on login"));

    // The injection is recorded in the event log (diagnostics-v0), not a
    // separate context-log file.
    let log = fs::read_to_string(dir.path().join(".git/lineage/events.jsonl")).unwrap();
    let entry: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!(entry["schema_version"], "lineage-events-v0");
    assert_eq!(entry["op"], "context_hook");
    assert_eq!(entry["outcome"], "ok");
    assert_eq!(
        entry["ts"],
        chrono::DateTime::from_timestamp(1_753_000_000, 0)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    assert_eq!(entry["detail"]["file_path"], "src/auth.rs");
    assert_eq!(entry["detail"]["strength"], "low");
    assert!(!dir.path().join(".git/lineage/context-log.jsonl").exists());
}

#[test]
fn repeat_fire_answers_from_cache_with_identical_output() {
    let dir = init_repo();
    let file = dir.path().join("lib.rs");
    fs::write(&file, "pub fn f() {}\n").unwrap();
    store_session_touching(dir.path(), "lib.rs");

    let first = context_cmd::hook_claude(dir.path(), &hook_input(&file), 1).unwrap();
    assert!(dir.path().join(".git/lineage/oracle.db").exists());
    let second = context_cmd::hook_claude(dir.path(), &hook_input(&file), 2).unwrap();
    assert_eq!(first, second);
}

#[test]
fn uncovered_file_and_foreign_events_stay_silent() {
    let dir = init_repo();
    let file = dir.path().join("untouched.rs");
    fs::write(&file, "// nothing\n").unwrap();
    store_session_touching(dir.path(), "src/auth.rs");

    // No provenance for this file: silent, with the reason on record.
    assert_eq!(
        context_cmd::hook_claude(dir.path(), &hook_input(&file), 0),
        None
    );

    // Non-Read tools are not injection triggers.
    let mut event: serde_json::Value = serde_json::from_str(&hook_input(&file)).unwrap();
    event["tool_name"] = "Bash".into();
    assert_eq!(
        context_cmd::hook_claude(dir.path(), &event.to_string(), 0),
        None
    );

    // Malformed payloads fail open, never error.
    assert_eq!(context_cmd::hook_claude(dir.path(), "not json", 0), None);

    // A fired-but-silent hook logs its reason; not-lineage-relevant fires
    // (non-Read tool, malformed payload) leave no entry at all
    // (diagnostics-v0 "Silent-fire reasons").
    let log = fs::read_to_string(dir.path().join(".git/lineage/events.jsonl")).unwrap();
    let entries: Vec<serde_json::Value> = log
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .filter(|e: &serde_json::Value| e["op"] == "context_hook")
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["outcome"], "silent");
    assert_eq!(entries[0]["detail"]["reason"], "no_evidence");
}

#[test]
fn unappendable_response_shape_is_silent_with_reason() {
    let dir = init_repo();
    let file = dir.path().join("lib.rs");
    fs::write(&file, "pub fn f() {}\n").unwrap();
    store_session_touching(dir.path(), "lib.rs");

    let mut event: serde_json::Value = serde_json::from_str(&hook_input(&file)).unwrap();
    event["tool_response"] = serde_json::json!({ "unexpected": true });
    assert_eq!(
        context_cmd::hook_claude(dir.path(), &event.to_string(), 0),
        None
    );

    let log = fs::read_to_string(dir.path().join(".git/lineage/events.jsonl")).unwrap();
    let entry: serde_json::Value = serde_json::from_str(log.lines().last().unwrap()).unwrap();
    assert_eq!(entry["op"], "context_hook");
    assert_eq!(entry["outcome"], "silent");
    assert_eq!(entry["detail"]["reason"], "unappendable_shape");
}

#[test]
fn private_sessions_stay_silent_through_the_full_hook_path() {
    let dir = init_repo();
    let file = dir.path().join("secret.rs");
    fs::write(&file, "// sensitive\n").unwrap();

    let repo = git2::Repository::open(dir.path()).unwrap();
    let mut conv = Conversation::new(AgentKind::Claude, dir.path().to_string_lossy());
    conv.private = true;
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: String::new(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![Artifact {
            kind: ArtifactKind::FileEdit,
            path: "secret.rs".into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: None,
            resolve: None,
        }],
    });
    write_conversation(&repo, &conv).unwrap();
    let index = LineageIndex::open(repo.path().join("lineage").join("index.db")).unwrap();
    index.index_conversation(&conv).unwrap();

    assert_eq!(
        context_cmd::hook_claude(dir.path(), &hook_input(&file), 0),
        None
    );
}

#[test]
fn agent_hook_install_is_idempotent_and_merge_preserving() {
    let dir = init_repo();
    let settings_path = dir.path().join(".claude/settings.json");
    fs::create_dir_all(dir.path().join(".claude")).unwrap();
    fs::write(
        &settings_path,
        r#"{"model": "opus", "hooks": {"PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}]}]}}"#,
    )
    .unwrap();

    assert!(context_cmd::install_claude_agent_hook(dir.path()).unwrap());
    // Second install is a no-op, not a duplicate entry.
    assert!(!context_cmd::install_claude_agent_hook(dir.path()).unwrap());

    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(settings["model"], "opus");
    let groups = settings["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["matcher"], "Bash");
    assert_eq!(
        groups[1]["hooks"][0]["command"],
        "git lineage context hook claude"
    );
}

#[test]
fn install_writes_both_hook_groups_and_backfills_a_missing_one() {
    let dir = init_repo();
    let settings_path = dir.path().join(".claude/settings.json");
    context_cmd::install_claude_agent_hook(dir.path()).unwrap();

    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    let session_start = settings["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(session_start.len(), 1);
    assert_eq!(
        session_start[0]["hooks"][0]["command"],
        "git lineage context hook claude-session-start"
    );
    // SessionStart has no tool to match on, so the group carries no matcher.
    assert!(session_start[0].get("matcher").is_none());

    // A repo wired by an older binary has only the PostToolUse group; install
    // must add the missing one rather than see the shared prefix and stop.
    fs::write(
        &settings_path,
        r#"{"hooks": {"PostToolUse": [{"matcher": "Read", "hooks": [{"type": "command", "command": "git lineage context hook claude"}]}]}}"#,
    )
    .unwrap();
    assert!(context_cmd::install_claude_agent_hook(dir.path()).unwrap());
    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(
        settings["hooks"]["PostToolUse"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        settings["hooks"]["SessionStart"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn session_start_hook_emits_the_verb_vocabulary() {
    let dir = init_repo();
    let input = serde_json::json!({
        "hook_event_name": "SessionStart",
        "session_id": "abc123",
        "source": "startup",
        "cwd": dir.path().to_string_lossy(),
    })
    .to_string();

    let output = context_cmd::hook_claude_session_start(dir.path(), &input, 0).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(
        parsed["hookSpecificOutput"]["hookEventName"],
        "SessionStart"
    );
    let context = parsed["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    for verb in lineage_retrieval::VERBS {
        assert!(
            context.contains(verb.cli),
            "vocabulary omits {}: {context}",
            verb.relation
        );
    }
    // It states a capability; it must never instruct the agent to use lineage,
    // or the A/B harness measures the prompt instead of the tool.
    assert!(!context.to_lowercase().contains("you should"));
    assert!(!context.to_lowercase().contains("always"));
}

/// Fail-open: a payload we cannot parse still gets the vocabulary rather than
/// killing the session that injection exists to help.
#[test]
fn session_start_hook_fails_open_on_a_malformed_payload() {
    let dir = init_repo();
    let output = context_cmd::hook_claude_session_start(dir.path(), "not json at all", 0).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert!(parsed["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap()
        .contains("git lineage context"));
}

#[test]
fn agent_hook_uninstall_removes_only_lineage_wiring() {
    let dir = init_repo();
    let settings_path = dir.path().join(".claude/settings.json");
    fs::create_dir_all(dir.path().join(".claude")).unwrap();
    fs::write(
        &settings_path,
        r#"{"hooks": {"PostToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}]}]}}"#,
    )
    .unwrap();
    context_cmd::install_claude_agent_hook(dir.path()).unwrap();

    assert!(context_cmd::uninstall_claude_agent_hook(dir.path()).unwrap());
    // Nothing of ours left; the user's own hook untouched.
    assert!(!context_cmd::uninstall_claude_agent_hook(dir.path()).unwrap());
    let settings: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    let groups = settings["hooks"]["PostToolUse"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["matcher"], "Bash");
    // Both groups go, and the now-empty SessionStart key is left in place
    // rather than pruned — it is the user's file, not ours to tidy.
    assert!(settings["hooks"]["SessionStart"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn structured_read_response_gets_digest_inside_file_content() {
    let dir = init_repo();
    let file = dir.path().join("main.rs");
    fs::write(&file, "fn main() {}\n").unwrap();
    store_session_touching(dir.path(), "main.rs");

    // The shape Claude Code actually sends for Read (observed live).
    let input = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "tool_name": "Read",
        "tool_input": { "file_path": file.to_string_lossy() },
        "tool_response": {
            "type": "text",
            "file": {
                "filePath": file.to_string_lossy(),
                "content": "fn main() {}\n",
                "numLines": 2,
                "startLine": 1,
                "totalLines": 2
            }
        },
    })
    .to_string();

    let output = context_cmd::hook_claude(dir.path(), &input, 0).expect("should inject");
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    let updated = &parsed["hookSpecificOutput"]["updatedToolOutput"];
    // Shape preserved, digest inside file.content.
    assert_eq!(updated["type"], "text");
    assert_eq!(updated["file"]["numLines"], 2);
    let content = updated["file"]["content"].as_str().unwrap();
    assert!(content.starts_with("fn main() {}\n"));
    assert!(content.contains("Lineage: 1 past session(s) touched main.rs"));
}
