use std::process::Command;

use chrono::Utc;
use lineage_core::{
    AgentKind, Artifact, ArtifactKind, Conversation, LineageId, PullOrigin, Role, Turn,
};
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

fn mark_pulled(conv: &mut Conversation) {
    conv.pull_origin = Some(PullOrigin {
        server: "https://lineage.example".into(),
        tenant: Some("acme".into()),
        pulled_at: Utc::now(),
        lineage_version: "0.0.0-test".into(),
    });
}

#[test]
fn excludes_pulled_sessions_and_keeps_locally_imported_ones() {
    let tmp = init_test_repo();
    let repo = open_repo(tmp.path()).unwrap();
    let sha = head_sha(&repo);

    let mut pulled = seed_conversation(&repo, &sha, false);
    mark_pulled(&mut pulled);
    let local = seed_conversation(&repo, &sha, false);

    let batch =
        assemble_batch(repo.inner(), "origin", vec![pulled.clone(), local.clone()]).unwrap();

    assert!(
        !batch.conversations.iter().any(|c| c.id == pulled.id),
        "a session pulled from a server must not be pushed back to it"
    );
    assert!(
        batch.conversations.iter().any(|c| c.id == local.id),
        "a locally imported session still pushes"
    );
    assert!(
        batch
            .line_objects
            .iter()
            .all(|lo| lo.conversation_id == local.id),
        "dependents of the pulled session are dropped with it"
    );
}

/// Regression: Bob pulls Alice's session, then forks it. The fork is Bob's own
/// new session and the server has never seen it, so it must push even though its
/// parent was pulled.
#[test]
fn includes_a_fork_of_a_pulled_session() {
    let tmp = init_test_repo();
    let repo = open_repo(tmp.path()).unwrap();
    let sha = head_sha(&repo);

    let mut pulled = seed_conversation(&repo, &sha, false);
    mark_pulled(&mut pulled);

    let mut fork = Conversation::fork_from(&pulled, "bob-handle".into());
    fork.commit_shas.push(sha.to_string());
    persist_conversation(repo.inner(), &fork).unwrap();

    assert!(fork.pull_origin.is_none(), "a fork is not itself pulled");

    let batch = assemble_batch(repo.inner(), "origin", vec![pulled.clone(), fork.clone()]).unwrap();

    let pushed = batch
        .conversations
        .iter()
        .find(|c| c.id == fork.id)
        .expect("the fork of a pulled session is the forker's own work and must push");
    assert_eq!(
        pushed.fork_origin.as_ref().map(|o| &o.source_session_id),
        Some(&pulled.id),
        "the fork edge back to the pulled parent survives assembly"
    );
    assert!(!batch.conversations.iter().any(|c| c.id == pulled.id));
}

#[test]
fn an_all_pulled_batch_assembles_empty_rather_than_failing() {
    let tmp = init_test_repo();
    let repo = open_repo(tmp.path()).unwrap();
    let sha = head_sha(&repo);

    let mut first = seed_conversation(&repo, &sha, false);
    let mut second = seed_conversation(&repo, &sha, false);
    mark_pulled(&mut first);
    mark_pulled(&mut second);

    let batch = assemble_batch(repo.inner(), "origin", vec![first, second]).unwrap();

    assert!(batch.conversations.is_empty());
    assert!(batch.line_objects.is_empty());
    assert!(batch.session_commit_links.is_empty());
    assert!(batch.blobs.is_empty());
}
