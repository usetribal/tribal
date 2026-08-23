use std::process::Command;

use lineage_core::{AgentKind, Conversation, LineageId, Role, Turn};
use lineage_git::{list_session_ids, open_repo, persist_conversation};
use lineage_mcp::server::handle_request;
use serde_json::json;

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "t@t.dev"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "T"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("f.txt"), "x\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

#[tokio::test]
async fn mcp_initialize_and_tools_list() {
    let dir = init_repo();
    let init = handle_request(dir.path(), "initialize", &json!({}))
        .await
        .unwrap();
    assert_eq!(init["serverInfo"]["name"], "lineage-mcp");

    let tools = handle_request(dir.path(), "tools/list", &json!({}))
        .await
        .unwrap();
    let names: Vec<_> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"lineage_doctor"));
    assert!(names.contains(&"lineage_search"));
}

/// The MCP half of the anti-drift guarantee: the tool set carries exactly the
/// verb registry, plus the registered non-traversal capability. The CLI half
/// lives in `lineage-cli/tests/verb_registry.rs`; a capability reaching one
/// surface and not the other is what this pair forbids, and `tools/list` is also
/// verb discovery for free on this path.
#[tokio::test]
async fn tools_list_carries_the_whole_verb_registry() {
    let dir = init_repo();
    let tools = handle_request(dir.path(), "tools/list", &json!({}))
        .await
        .unwrap();
    let names: Vec<String> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();

    // Continuation is checked alongside the traversal verbs rather than in its
    // own test: it is registered for exactly the same guarantee, and a separate
    // test would be one somebody could delete without the pairing failing.
    for verb in lineage_retrieval::VERBS
        .iter()
        .chain(std::iter::once(&lineage_retrieval::CONTINUE_SESSION))
    {
        assert!(
            names.contains(&verb.mcp.to_string()),
            "capability {} is in the registry but not an MCP tool (have: {names:?})",
            verb.relation,
        );
    }

    // The other direction: every `lineage_`-prefixed tool is either pre-existing
    // plumbing or a registry verb, so a traversal tool cannot be added here
    // without being registered.
    const NON_VERB_TOOLS: &[&str] = &[
        "lineage_list_sessions",
        "lineage_get_session",
        "lineage_blame_line",
        "lineage_search",
        "lineage_doctor",
        "lineage_materialize",
        "lineage_rebuild_index",
        "lineage_export",
        "lineage_remap",
    ];
    for name in &names {
        let known = NON_VERB_TOOLS.contains(&name.as_str())
            || lineage_retrieval::VERBS.iter().any(|v| v.mcp == *name)
            || lineage_retrieval::CONTINUE_SESSION.mcp == *name;
        assert!(known, "tool {name} is neither plumbing nor a registry verb");
    }
}

/// Each verb answers over MCP, and a private session never appears in any of
/// them — the gate runs inside the primitive, so this surface cannot bypass it.
#[tokio::test]
async fn traversal_verbs_answer_and_never_emit_private_sessions() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let head = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let mut public = session_saying(dir.path(), &["we chose redis for the cache", "and why"]);
    public.commit_shas.push(head.clone());
    let mut private = session_saying(dir.path(), &["the private redis decision"]);
    private.private = true;
    private.commit_shas.push(head.clone());
    for conv in [&public, &private] {
        persist_conversation(repo.inner(), conv).unwrap();
        lineage_search::LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))
            .unwrap()
            .index_conversation(conv)
            .unwrap();
    }

    let search = call_tool(
        dir.path(),
        "lineage_search_within",
        json!({
            "session_ids": [public.id.as_str(), private.id.as_str()],
            "query": "redis",
        }),
    )
    .await;
    assert!(search.contains(public.id.as_str()));
    assert!(
        !search.contains(private.id.as_str()),
        "a private session must never surface: {search}"
    );

    let around = call_tool(
        dir.path(),
        "lineage_turns_around",
        json!({ "turn_id": public.turns[0].id.as_str(), "radius": 1 }),
    )
    .await;
    assert!(around.contains("and why"), "neighbour turn is present");

    let private_around = call_tool(
        dir.path(),
        "lineage_turns_around",
        json!({ "turn_id": private.turns[0].id.as_str(), "radius": 1 }),
    )
    .await;
    assert_eq!(private_around.trim(), "[]");

    let sessions = call_tool(
        dir.path(),
        "lineage_sessions_for_commit",
        json!({ "commit_sha": head }),
    )
    .await;
    assert!(sessions.contains(public.id.as_str()));
    assert!(!sessions.contains(private.id.as_str()));

    // No line objects were materialized, so the honest answer is an empty list.
    let produced = call_tool(
        dir.path(),
        "lineage_produced_by",
        json!({ "turn_id": public.turns[0].id.as_str() }),
    )
    .await;
    assert_eq!(produced.trim(), "[]");
}

/// The MCP continuation tool briefs and does not fork: no transcript is written
/// and no fork edge is recorded. There is nobody at a terminal to act on a
/// printed resume command here, and writing into the caller's harness state as a
/// side effect of a tool call is a thing to choose.
#[tokio::test]
async fn fork_brief_returns_a_block_and_writes_nothing() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let conv = session_saying(dir.path(), &["make the cache write through"]);
    persist_conversation(repo.inner(), &conv).unwrap();

    let before = list_session_ids(repo.inner()).unwrap().len();
    let block = call_tool(
        dir.path(),
        lineage_retrieval::CONTINUE_SESSION.mcp,
        json!({ "session_id": conv.id.as_str() }),
    )
    .await;

    assert!(block.contains(conv.id.as_str()));
    assert!(block.contains("make the cache write through"));
    assert!(
        block.contains(lineage_cli::brief::TASK_SLOT_MARKER),
        "the block ends with the empty task slot: {block}"
    );

    // No new session means no fork edge — the block is a context load, not a
    // fork.
    assert_eq!(list_session_ids(repo.inner()).unwrap().len(), before);

    // The brief goes to a subagent exploring one session somebody already chose;
    // offering `fork` there invites it to fork again from inside a fork.
    assert!(
        !block.contains("tribal fork"),
        "the brief withholds continuation: {block}"
    );
}

/// The privacy gate the traversal verbs run applies here too — briefing a
/// private session would launder it through a different tool.
#[tokio::test]
async fn fork_brief_refuses_a_private_session() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let mut private = session_saying(dir.path(), &["the private decision"]);
    private.private = true;
    persist_conversation(repo.inner(), &private).unwrap();

    let err = handle_request(
        dir.path(),
        "tools/call",
        &json!({
            "name": lineage_retrieval::CONTINUE_SESSION.mcp,
            "arguments": { "session_id": private.id.as_str() },
        }),
    )
    .await
    .unwrap_err();
    assert!(err.contains("private"), "{err}");
}

/// The text payload of one `tools/call`, which is where every tool puts its
/// answer.
async fn call_tool(
    repo_path: &std::path::Path,
    name: &str,
    arguments: serde_json::Value,
) -> String {
    let result = handle_request(
        repo_path,
        "tools/call",
        &json!({ "name": name, "arguments": arguments }),
    )
    .await
    .unwrap_or_else(|e| panic!("{name} failed: {e}"));
    result["content"][0]["text"].as_str().unwrap().to_string()
}

fn session_saying(workspace_root: &std::path::Path, prompts: &[&str]) -> Conversation {
    let mut conv = Conversation::new(AgentKind::Claude, workspace_root.to_string_lossy());
    for prompt in prompts {
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: (*prompt).into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
    }
    conv
}

#[tokio::test]
async fn mcp_tool_calls_on_repo_with_session() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let sha = repo
        .inner()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    let mut conv = Conversation::new(AgentKind::Cursor, dir.path().display().to_string());
    conv.commit_shas.push(sha);
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "edit f.txt".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    persist_conversation(repo.inner(), &conv).unwrap();
    let sid = conv.id.to_string();

    let doctor = handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_doctor", "arguments": {} }),
    )
    .await
    .unwrap();
    let doctor_report: serde_json::Value =
        serde_json::from_str(doctor["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(doctor_report["schema_version"], "lineage-doctor-v0");
    assert_eq!(doctor_report["capture"]["sessions_imported"], 1);
    assert!(doctor_report["materialization"]["failure_reasons"].is_object());
    // The MCP payload is the unfiltered report, so new sections reach agents
    // without the server enumerating them.
    assert!(doctor_report["coverage"].is_object());

    let list = handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_list_sessions", "arguments": {} }),
    )
    .await
    .unwrap();
    assert!(list["content"][0]["text"].as_str().unwrap().contains(&sid));

    let get = handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_get_session", "arguments": { "session_id": sid, "redact": false } }),
    )
    .await
    .unwrap();
    assert!(get["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("edit f.txt"));

    let blame = handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_blame_line", "arguments": { "path": "f.txt", "line": 1 } }),
    )
    .await
    .unwrap();
    assert!(blame["content"][0]["text"].is_string());

    handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_rebuild_index", "arguments": {} }),
    )
    .await
    .unwrap();

    let search = handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_search", "arguments": { "query": "edit", "limit": 5 } }),
    )
    .await
    .unwrap();
    assert!(search["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("edit"));

    handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_export", "arguments": { "redact": true, "format": "json" } }),
    )
    .await
    .unwrap();

    handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_materialize", "arguments": {} }),
    )
    .await
    .unwrap();

    handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_remap", "arguments": {} }),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn mcp_export_jsonl_and_materialize_session() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let mut conv = Conversation::new(AgentKind::Claude, dir.path().display().to_string());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "test export".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    persist_conversation(repo.inner(), &conv).unwrap();
    let sid = conv.id.to_string();

    let export = handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_export", "arguments": { "redact": false, "format": "jsonl" } }),
    )
    .await
    .unwrap();
    assert!(export["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("test export"));

    handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_materialize", "arguments": { "session_id": sid } }),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn mcp_unknown_tool_returns_error() {
    let dir = init_repo();
    let err = handle_request(
        dir.path(),
        "tools/call",
        &json!({ "name": "lineage_nonexistent", "arguments": {} }),
    )
    .await;
    assert!(err.is_err());
}

#[tokio::test]
async fn mcp_notifications_initialized_ok() {
    let dir = init_repo();
    let result = handle_request(dir.path(), "notifications/initialized", &json!({}))
        .await
        .unwrap();
    assert!(result.is_object());
}

#[tokio::test]
async fn mcp_unknown_method_returns_error() {
    let dir = init_repo();
    let err = handle_request(dir.path(), "bogus/method", &json!({})).await;
    assert!(err.is_err());
}
