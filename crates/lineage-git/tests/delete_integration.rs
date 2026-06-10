use std::process::Command;

use lineage_core::{AgentKind, Artifact, ArtifactKind, Conversation, LineageId, Role, Turn};
use lineage_git::{
    delete_session, list_session_ids, open_repo, persist_conversation, read_line_object,
    read_note_for_commit,
};

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
fn deletes_session_ref_notes_and_line_objects() {
    let tmp = init_test_repo();
    let repo = open_repo(tmp.path()).unwrap();
    let inner = repo.inner();

    let sha = inner
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

    let mut conv = Conversation::new(AgentKind::Cursor, tmp.path().display().to_string());
    conv.commit_shas.push(sha.clone());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "edited src/main.rs".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![Artifact {
            kind: ArtifactKind::FileEdit,
            path: "README.md".into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: Some([1, 1]),
            resolve: None,
        }],
    });

    persist_conversation(inner, &conv).unwrap();
    let line_id = read_note_for_commit(inner, &sha)
        .unwrap()
        .unwrap()
        .line_object_ids[0]
        .clone();
    assert!(read_line_object(inner, &line_id).unwrap().is_some());

    let report = delete_session(inner, &conv.id, false).unwrap();
    assert_eq!(report.notes_updated, 1);
    assert!(report.line_objects_deleted >= 1);
    assert!(!list_session_ids(inner).unwrap().contains(&conv.id));
    assert!(read_line_object(inner, &line_id).unwrap().is_none());
    let note = read_note_for_commit(inner, &sha).unwrap().unwrap();
    assert!(!note.session_ids.contains(&conv.id));
    assert!(!note.line_object_ids.contains(&line_id));
}
