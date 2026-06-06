use std::process::Command;

use lineage_core::{
    AgentKind, Artifact, ArtifactKind, Conversation, LineageId, Role, Turn,
};
use lineage_git::{
    blame_with_lineage, ensure_gitattributes, hydrate_conversation, hydrate_media_artifacts,
    lfs_status, purge_orphans, read_note_for_commit, run_doctor, open_repo, persist_conversation,
    remap_orphaned_commits, write_last_ingest, read_last_ingest, write_note_for_commit,
    LINEAGE_MEDIA_DIR,
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
    std::fs::write(dir.path().join("app.rs"), "fn main() {}\n").unwrap();
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
fn full_workflow_exercises_git_modules() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let inner = repo.inner();
    ensure_gitattributes(inner).unwrap();
    assert!(dir.path().join(".gitattributes").exists());

    let sha = inner
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

    let mut conv = Conversation::new(AgentKind::Cursor, dir.path().display().to_string());
    conv.commit_shas.push(sha.clone());
    conv.metadata.insert(
        "git_branch".into(),
        serde_json::Value::String("main".into()),
    );
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "![diagram](data:image/png;base64,iVBORw0KGgo=)".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![Artifact {
            kind: ArtifactKind::FileEdit,
            path: "app.rs".into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: Some([1, 1]),
            resolve: None,
        }],
    });
    persist_conversation(inner, &conv).unwrap();

    write_last_ingest(inner, &lineage_core::LastIngestState::new(vec![conv.id.clone()])).unwrap();
    assert!(read_last_ingest(inner).unwrap().ingested_at.is_some());

    let doctor = run_doctor(&repo).unwrap();
    assert!(doctor.is_git_repo);

    let status = lfs_status(inner).unwrap();
    let _ = status.referenced;

    let mut loaded = lineage_git::read_conversation_stored(inner, &conv.id)
        .unwrap()
        .unwrap();
    hydrate_conversation(inner, &mut loaded).unwrap();
    let _ = hydrate_media_artifacts(inner, &mut loaded);

    let blame = blame_with_lineage(inner, std::path::Path::new("app.rs"), 1).unwrap();
    assert_eq!(blame.line, 1);

    write_note_for_commit(inner, &sha, &[conv.id.clone()], &[], None).unwrap();
    let note = read_note_for_commit(inner, &sha).unwrap().unwrap();
    assert!(note.session_ids.contains(&conv.id));

    let _ = remap_orphaned_commits(inner);
    let _ = purge_orphans(inner);

    assert!(LINEAGE_MEDIA_DIR.contains("media"));
}
