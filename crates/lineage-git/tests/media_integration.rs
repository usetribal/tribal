use std::process::Command;

use lineage_core::{AgentKind, Conversation, LineageId, Role, Turn};
use lineage_git::{open_repo, persist_conversation, read_conversation_stored};

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
    std::fs::write(dir.path().join("README.md"), "hi").unwrap();
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
fn persist_externalizes_embedded_image() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let inner = repo.inner();

    let mut conv = Conversation::new(AgentKind::Cursor, dir.path().display().to_string());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "diagram: data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    persist_conversation(inner, &conv).unwrap();

    let stored = read_conversation_stored(inner, &conv.id).unwrap().unwrap();
    assert!(stored.turns[0]
        .artifacts
        .iter()
        .any(|a| a.blob_ref.is_some()));
    assert!(dir.path().join(".gitattributes").exists());
}
