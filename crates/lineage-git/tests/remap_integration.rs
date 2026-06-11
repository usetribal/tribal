use std::process::Command;

use lineage_core::{AgentKind, Conversation, LineageId, Role, Turn};
use lineage_git::{open_repo, persist_conversation, remap_orphaned_commits};

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
    std::fs::write(dir.path().join("x.rs"), "1\n").unwrap();
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
fn remap_replaces_missing_commit_with_head() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let inner = repo.inner();
    let head = inner
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

    let mut conv = Conversation::new(AgentKind::Claude, dir.path().display().to_string());
    conv.commit_shas.push(head.clone());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "change x.rs".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![lineage_core::Artifact {
            kind: lineage_core::ArtifactKind::FileEdit,
            path: "x.rs".into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: Some([1, 1]),
            resolve: None,
        }],
    });
    persist_conversation(inner, &conv).unwrap();
    let mut stored = lineage_git::read_conversation_stored(inner, &conv.id)
        .unwrap()
        .unwrap();
    stored.commit_shas = vec!["0000000000000000000000000000000000000001".into()];
    lineage_git::write_conversation(inner, &stored).unwrap();

    let report = remap_orphaned_commits(inner).unwrap();
    assert_eq!(report.remapped_commits, 1);

    let updated = lineage_git::read_conversation_stored(inner, &conv.id)
        .unwrap()
        .unwrap();
    assert!(updated.commit_shas.contains(&head));
}
