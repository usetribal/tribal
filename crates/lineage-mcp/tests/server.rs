use std::process::Command;

use lineage_core::{AgentKind, Conversation, LineageId, Role, Turn};
use lineage_git::{open_repo, persist_conversation};
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
