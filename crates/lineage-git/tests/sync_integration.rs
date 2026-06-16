use std::process::Command;

use lineage_core::{AgentKind, Artifact, ArtifactKind, Conversation, LineageId, Role, Turn};
use lineage_git::{assemble_batch, open_repo, persist_conversation, LineageRepo};

fn init_test_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.email", "test@test.dev"],
        vec!["config", "user.name", "Test"],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(dir.path())
            .output()
            .unwrap();
    }
    std::fs::write(dir.path().join("README.md"), "hello").unwrap();
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-m", "init"]);
    git(
        &dir,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/Acme/Widgets.git",
        ],
    );
    dir
}

fn git(dir: &tempfile::TempDir, args: &[&str]) {
    Command::new("git")
        .args(args)
        .current_dir(dir.path())
        .output()
        .unwrap();
}

fn head_sha(repo: &LineageRepo) -> String {
    repo.inner()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string()
}

fn seed_conversation(repo: &LineageRepo, sha: &str, private: bool) -> Conversation {
    let mut conv = Conversation::new(AgentKind::Claude, "/tmp/proj");
    conv.private = private;
    conv.commit_shas.push(sha.to_string());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "edited README.md".into(),
        tool_calls: vec![],
        model: Some("claude-sonnet-4".into()),
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
    persist_conversation(repo.inner(), &conv).unwrap();
    conv
}

#[test]
fn assembles_batch_with_repo_binding_and_links() {
    let tmp = init_test_repo();
    let repo = open_repo(tmp.path()).unwrap();
    let sha = head_sha(&repo);
    let conv = seed_conversation(&repo, &sha, false);

    let batch = assemble_batch(repo.inner(), "origin", vec![conv.clone()]).unwrap();

    assert_eq!(batch.schema_version, "sync-batch-v0");
    assert_eq!(batch.repo.normalized_remote_url, "github.com/acme/widgets");
    assert_eq!(batch.repo.root_commit_sha, sha);
    assert!(batch.repo.server_repo_id.is_none());
    assert_eq!(batch.conversations.len(), 1);

    // persist_conversation materializes line objects from the FileEdit artifact
    // and writes a note linking the session to its commit; both decompose into
    // the batch.
    assert!(
        !batch.line_objects.is_empty(),
        "expected materialized line objects"
    );
    assert!(
        batch
            .line_objects
            .iter()
            .all(|lo| lo.conversation_id == conv.id),
        "line objects belong to the synced conversation"
    );
    assert!(
        batch
            .session_commit_links
            .iter()
            .any(|l| l.conversation_id == conv.id && l.commit_sha == sha),
        "expected a session->commit link decomposed from the note"
    );
}

#[test]
fn excludes_line_objects_of_unsynced_sessions() {
    let tmp = init_test_repo();
    let repo = open_repo(tmp.path()).unwrap();
    let sha = head_sha(&repo);

    // A private session is persisted (its line objects/notes exist in git) but
    // the caller never hands it to assemble_batch — nothing of it may leak.
    let private = seed_conversation(&repo, &sha, true);
    let public = seed_conversation(&repo, &sha, false);

    let batch = assemble_batch(repo.inner(), "origin", vec![public.clone()]).unwrap();

    assert!(
        batch
            .line_objects
            .iter()
            .all(|lo| lo.conversation_id == public.id),
        "no line object from the unsynced private session"
    );
    assert!(
        batch
            .session_commit_links
            .iter()
            .all(|l| l.conversation_id == public.id),
        "no commit link from the unsynced private session"
    );
    assert!(!batch
        .session_commit_links
        .iter()
        .any(|l| l.conversation_id == private.id),);
}
