use std::process::Command;

use lineage_core::{AgentKind, Conversation, LineageId, Role, Turn};
use lineage_git::{list_session_ids, open_repo, persist_conversation, read_conversation};

fn init_test_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@test.dev"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("README.md"), "hello").unwrap();
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

#[test]
fn persist_and_read_conversation() {
    let tmp = init_test_repo();
    let repo = open_repo(tmp.path()).unwrap();
    let inner = repo.inner();

    let head_sha = inner
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

    let mut conv = Conversation::new(AgentKind::Cursor, tmp.path().display().to_string());
    conv.commit_shas.push(head_sha);
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "add auth".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });

    let result = persist_conversation(inner, &conv).unwrap();
    assert_eq!(result.session_id, conv.id);

    let ids = list_session_ids(inner).unwrap();
    assert_eq!(ids.len(), 1);

    let loaded = read_conversation(inner, &conv.id).unwrap().unwrap();
    assert_eq!(loaded.turns.len(), 1);
    assert_eq!(loaded.turns[0].content, "add auth");
}

/// A session may name history this checkout has not fetched: a fork of a
/// teammate's session whose commits are on an unmerged branch, or a pull that
/// ran before they landed. Persisting must still write the session — the
/// transcript is fully in hand, and only the commit link is deferred.
#[test]
fn persisting_skips_a_commit_this_checkout_does_not_have() {
    let tmp = init_test_repo();
    let repo = open_repo(tmp.path()).unwrap();
    let inner = repo.inner();

    let head_sha = inner
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    // Well-formed but absent — the shape a real unfetched commit arrives in.
    let unfetched_sha = "5c3eb94beef24763b14d4ab527c41c2603afc759";

    let mut conv = Conversation::new(AgentKind::Claude, tmp.path().display().to_string());
    conv.commit_shas.push(unfetched_sha.into());
    conv.commit_shas.push(head_sha);
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "pick up where you left off".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });

    let result = persist_conversation(inner, &conv).expect("an unfetched commit must not fail");

    // The commit that is here is still linked; the one that is not is skipped.
    assert_eq!(result.commits_linked, 1);

    let loaded = read_conversation(inner, &conv.id).unwrap().unwrap();
    assert_eq!(loaded.turns.len(), 1);
}
