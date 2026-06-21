use std::process::Command;

use lineage_core::{
    AgentKind, Artifact, ArtifactKind, ArtifactResolve, Conversation, LineageId, ResolveStrategy,
    Role, Turn,
};
use lineage_git::{blame_with_lineage, open_repo, persist_conversation};

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
    std::fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
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
fn blame_falls_back_to_artifact_overlap() {
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
    conv.commit_shas.push(sha);
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "added main".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![lineage_core::Artifact {
            kind: lineage_core::ArtifactKind::FileEdit,
            path: "main.rs".into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: Some([1, 1]),
            resolve: None,
        }],
    });
    persist_conversation(inner, &conv).unwrap();

    let result = blame_with_lineage(inner, std::path::Path::new("main.rs"), 1).unwrap();
    assert_eq!(result.line, 1);
    assert!(!result.sessions.is_empty() || !result.matches.is_empty());
}

#[test]
fn blame_returns_materialized_line_objects() {
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
    conv.commit_shas.push(sha);
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "added main".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![Artifact {
            kind: ArtifactKind::FileEdit,
            path: "main.rs".into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: Some([1, 1]),
            resolve: None,
        }],
    });
    persist_conversation(inner, &conv).unwrap();

    let result = blame_with_lineage(inner, std::path::Path::new("main.rs"), 1).unwrap();
    assert!(
        !result.line_objects.is_empty(),
        "expected materialized line objects"
    );
    assert!(!result.matches.is_empty());
}

#[test]
fn blame_resolves_absolute_path_artifacts() {
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
    let abs_path = dir.path().join("main.rs").display().to_string();

    let mut conv = Conversation::new(AgentKind::Cursor, dir.path().display().to_string());
    conv.commit_shas.push(sha);
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "added main".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![Artifact {
            kind: ArtifactKind::Diff,
            path: abs_path,
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: None,
            resolve: Some(ArtifactResolve {
                strategy: ResolveStrategy::OldString,
                old_string: Some("fn main() {}".into()),
                patch: None,
            }),
        }],
    });
    persist_conversation(inner, &conv).unwrap();

    let result = blame_with_lineage(inner, std::path::Path::new("main.rs"), 1).unwrap();
    assert!(
        !result.line_objects.is_empty() || !result.matches.is_empty(),
        "expected blame to resolve absolute artifact paths"
    );
}

#[test]
fn blame_missing_file_errors() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let err = blame_with_lineage(repo.inner(), std::path::Path::new("missing.rs"), 1);
    assert!(err.is_err());
}
