use std::process::Command;

use lineage_core::{AgentKind, Artifact, ArtifactKind, Conversation, LineageId, Role, Turn};
use lineage_git::{
    link_all_sessions_to_head, link_recent_sessions_to_head, open_repo, persist_conversation,
    read_line_object, write_last_import,
};

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
    std::fs::write(dir.path().join("lib.rs"), "pub fn ok() {}\n").unwrap();
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
fn hooks_link_sessions_to_head() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let inner = repo.inner();
    let sha = inner
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

    let mut conv = Conversation::new(AgentKind::Cursor, dir.path().display().to_string());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "edit lib.rs".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![Artifact {
            kind: ArtifactKind::FileEdit,
            path: "lib.rs".into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: Some([1, 1]),
            resolve: None,
        }],
    });
    persist_conversation(inner, &conv).unwrap();

    let linked = link_all_sessions_to_head(inner).unwrap();
    assert_eq!(linked.len(), 1);
    assert_eq!(linked[0].session_id, conv.id);
    assert!(
        linked[0].line_objects > 0,
        "linked session should report its materialized line objects"
    );

    write_last_import(
        inner,
        &lineage_core::LastImportState::new(vec![conv.id.clone()]),
    )
    .unwrap();
    let recent = link_recent_sessions_to_head(inner).unwrap();
    assert_eq!(recent.len(), 1);

    let note = lineage_git::read_note_for_commit(inner, &sha)
        .unwrap()
        .unwrap();
    assert!(note.session_ids.contains(&conv.id));
    assert!(
        !note.line_object_ids.is_empty(),
        "hooks should materialize line objects onto the commit note"
    );
    let line_id = &note.line_object_ids[0];
    let obj = read_line_object(inner, line_id)
        .unwrap()
        .expect("line object ref should resolve");
    assert_eq!(obj.file_path, "lib.rs");
    assert_eq!(obj.line_range, [1, 1]);
}
