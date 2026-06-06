use std::fs;
use std::process::Command;

use git2::Repository;
use lineage_core::{
    AgentKind, Artifact, ArtifactKind, ArtifactResolve, Confidence, Conversation, LineageId, Role,
    ResolveStrategy, Turn, CONVERSATION_SCHEMA,
};
use lineage_git::{materialize_line_objects, persist_conversation};

fn init_repo() -> (tempfile::TempDir, Repository) {
    let dir = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/auth.rs"),
        "pub mod auth;\npub fn validate() {}\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", "src/auth.rs"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-qm", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let repo = Repository::open(dir.path()).unwrap();
    (dir, repo)
}

#[test]
fn materializes_old_string_line_objects() {
    let (_dir, repo) = init_repo();
    let commit_sha = repo.head().unwrap().peel_to_commit().unwrap().id().to_string();

    let conv_id = LineageId::from("test-session");
    let turn_id = LineageId::from("turn-1");
    let conversation = Conversation {
        schema_version: CONVERSATION_SCHEMA.into(),
        id: conv_id,
        agent: AgentKind::Cursor,
        started_at: chrono::Utc::now(),
        ended_at: None,
        workspace_root: ".".into(),
        parent_session_id: None,
        private: false,
        turns: vec![Turn {
            id: turn_id,
            role: Role::Assistant,
            content: "updated auth".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![Artifact {
                kind: ArtifactKind::Diff,
                path: "src/auth.rs".into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: None,
                resolve: Some(ArtifactResolve {
                    strategy: ResolveStrategy::OldString,
                    old_string: Some("pub fn validate() {}".into()),
                    patch: None,
                }),
            }],
        }],
        commit_shas: vec![commit_sha.clone()],
        metadata: Default::default(),
    };

    let result = persist_conversation(&repo, &conversation).unwrap();
    assert!(
        result.line_objects_written >= 1,
        "expected line objects, got {}",
        result.line_objects_written
    );

    let objects = materialize_line_objects(&repo, &conversation, &commit_sha, Confidence::Exact)
        .unwrap();
    assert!(!objects.is_empty());
    let obj = &objects[0];
    assert_eq!(obj.file_path, "src/auth.rs");
    assert!(obj.line_range[0] >= 1);
    assert!(obj.contains_line(obj.line_range[0]));
}

#[test]
fn materializes_citation_line_range() {
    let (_dir, repo) = init_repo();
    let commit_sha = repo.head().unwrap().peel_to_commit().unwrap().id().to_string();

    let conversation = Conversation {
        schema_version: CONVERSATION_SCHEMA.into(),
        id: LineageId::from("citation-session"),
        agent: AgentKind::Cursor,
        started_at: chrono::Utc::now(),
        ended_at: None,
        workspace_root: ".".into(),
        parent_session_id: None,
        private: false,
        turns: vec![Turn {
            id: LineageId::from("turn-cite"),
            role: Role::Assistant,
            content: "see `2:2:src/auth.rs`".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![Artifact {
                kind: ArtifactKind::FileEdit,
                path: "src/auth.rs".into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: Some([2, 2]),
                resolve: Some(ArtifactResolve {
                    strategy: ResolveStrategy::Citation,
                    old_string: None,
                    patch: None,
                }),
            }],
        }],
        commit_shas: vec![commit_sha.clone()],
        metadata: Default::default(),
    };

    let objects = materialize_line_objects(&repo, &conversation, &commit_sha, Confidence::Exact)
        .unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].line_range, [2, 2]);
    assert_eq!(objects[0].confidence, Confidence::Exact);
}
