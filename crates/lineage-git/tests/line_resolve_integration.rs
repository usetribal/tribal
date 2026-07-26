use std::fs;
use std::path::Path;
use std::process::Command;

use git2::Repository;
use lineage_core::{
    AgentKind, Artifact, ArtifactKind, ArtifactResolve, Confidence, Conversation, LineageId,
    ResolveStrategy, Role, Turn, CONVERSATION_SCHEMA,
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
    let commit_sha = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

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
        fork_origin: None,
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
                    new_string: None,
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

    let objects =
        materialize_line_objects(&repo, &conversation, &commit_sha, Confidence::Exact).unwrap();
    assert!(!objects.is_empty());
    let obj = &objects[0];
    assert_eq!(obj.file_path, "src/auth.rs");
    assert!(obj.line_range[0] >= 1);
    assert!(obj.contains_line(obj.line_range[0]));
}

#[test]
fn materializes_citation_line_range() {
    let (_dir, repo) = init_repo();
    let commit_sha = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

    let conversation = Conversation {
        schema_version: CONVERSATION_SCHEMA.into(),
        id: LineageId::from("citation-session"),
        agent: AgentKind::Cursor,
        started_at: chrono::Utc::now(),
        ended_at: None,
        workspace_root: ".".into(),
        parent_session_id: None,
        fork_origin: None,
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
                    new_string: None,
                    patch: None,
                }),
            }],
        }],
        commit_shas: vec![commit_sha.clone()],
        metadata: Default::default(),
    };

    let objects =
        materialize_line_objects(&repo, &conversation, &commit_sha, Confidence::Exact).unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].line_range, [2, 2]);
    assert_eq!(objects[0].confidence, Confidence::Exact);
}

#[test]
fn materializes_absolute_path_artifacts() {
    let (dir, repo) = init_repo();
    let commit_sha = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    let workspace = dir.path().display().to_string();
    let abs_path = dir.path().join("src/auth.rs").display().to_string();

    let conversation = Conversation {
        schema_version: CONVERSATION_SCHEMA.into(),
        id: LineageId::from("abs-path-session"),
        agent: AgentKind::Cursor,
        started_at: chrono::Utc::now(),
        ended_at: None,
        workspace_root: workspace,
        parent_session_id: None,
        fork_origin: None,
        private: false,
        turns: vec![Turn {
            id: LineageId::from("turn-abs"),
            role: Role::Assistant,
            content: "updated auth".into(),
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
                    old_string: Some("pub fn validate() {}".into()),
                    new_string: None,
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
        "expected line objects for absolute path, got {}",
        result.line_objects_written
    );
    let objects =
        materialize_line_objects(&repo, &conversation, &commit_sha, Confidence::Exact).unwrap();
    assert_eq!(objects[0].file_path, "src/auth.rs");
}

#[test]
fn skips_artifacts_for_files_not_in_commit_diff() {
    let (dir, repo) = init_repo();
    let commit_sha = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    fs::write(dir.path().join("src/other.rs"), "fn other() {}\n").unwrap();

    let conversation = Conversation {
        schema_version: CONVERSATION_SCHEMA.into(),
        id: LineageId::from("skip-session"),
        agent: AgentKind::Cursor,
        started_at: chrono::Utc::now(),
        ended_at: None,
        workspace_root: dir.path().display().to_string(),
        parent_session_id: None,
        fork_origin: None,
        private: false,
        turns: vec![Turn {
            id: LineageId::from("turn-skip"),
            role: Role::Assistant,
            content: "edit other".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![Artifact {
                kind: ArtifactKind::FileEdit,
                path: "src/other.rs".into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: Some([1, 1]),
                resolve: None,
            }],
        }],
        commit_shas: vec![commit_sha.clone()],
        metadata: Default::default(),
    };

    let objects =
        materialize_line_objects(&repo, &conversation, &commit_sha, Confidence::Exact).unwrap();
    assert!(
        objects.is_empty(),
        "expected no line objects when file was not in commit diff"
    );
}

#[test]
fn materializes_worktree_prefixed_artifact_paths() {
    // Sessions run in a linked worktree sometimes record an edit relative to
    // the *main* workdir — `.claude/worktrees/feature/src/auth.rs` for what is
    // really `src/auth.rs`. That path is already relative, so plain
    // normalization left it untouched, it matched no file in the commit diff,
    // and every edit the session made was dropped.
    let (dir, repo) = init_repo();
    let commit_sha = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

    // Nested inside the main workdir, matching the layout that produces these
    // paths (`.claude/worktrees/<name>`).
    let worktree_rel = ".claude/worktrees/feature";
    let worktree_path = dir.path().join(worktree_rel);
    Command::new("git")
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            worktree_path.to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let conversation = Conversation {
        schema_version: CONVERSATION_SCHEMA.into(),
        id: LineageId::from("worktree-session"),
        agent: AgentKind::Claude,
        started_at: chrono::Utc::now(),
        ended_at: None,
        workspace_root: worktree_path.to_string_lossy().into_owned(),
        parent_session_id: None,
        fork_origin: None,
        private: false,
        turns: vec![Turn {
            id: LineageId::from("turn-1"),
            role: Role::Assistant,
            content: "updated auth from the worktree".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![Artifact {
                kind: ArtifactKind::Diff,
                // Repo-relative but prefixed by the worktree's own location —
                // the shape that silently matched nothing.
                path: format!("{worktree_rel}/src/auth.rs"),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: None,
                resolve: Some(ArtifactResolve {
                    strategy: ResolveStrategy::OldString,
                    old_string: Some("pub fn validate() {}".into()),
                    new_string: None,
                    patch: None,
                }),
            }],
        }],
        commit_shas: vec![commit_sha.clone()],
        metadata: Default::default(),
    };

    let objects =
        materialize_line_objects(&repo, &conversation, &commit_sha, Confidence::Exact).unwrap();
    assert!(
        !objects.is_empty(),
        "expected the worktree session to materialize against the main workdir"
    );
    assert_eq!(
        objects[0].file_path, "src/auth.rs",
        "worktree prefix must be stripped to a repo-relative path"
    );
}

/// A session whose single artifact edits `artifact_path`, anchored on text the
/// fixture repo's `src/auth.rs` contains.
fn session_editing(workspace_root: &str, artifact_path: &str, commit_sha: &str) -> Conversation {
    Conversation {
        schema_version: CONVERSATION_SCHEMA.into(),
        id: LineageId::from("deleted-worktree-session"),
        agent: AgentKind::Claude,
        started_at: chrono::Utc::now(),
        ended_at: None,
        workspace_root: workspace_root.to_string(),
        parent_session_id: None,
        fork_origin: None,
        private: false,
        turns: vec![Turn {
            id: LineageId::from("turn-1"),
            role: Role::Assistant,
            content: "updated auth".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![Artifact {
                kind: ArtifactKind::Diff,
                path: artifact_path.to_string(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: None,
                resolve: Some(ArtifactResolve {
                    strategy: ResolveStrategy::OldString,
                    old_string: Some("pub fn validate() {}".into()),
                    new_string: None,
                    patch: None,
                }),
            }],
        }],
        commit_shas: vec![commit_sha.to_string()],
        metadata: Default::default(),
    }
}

/// Add a worktree and then prune it, leaving git with no record it existed —
/// the state that makes its recorded paths unrecoverable from the registry.
fn add_then_delete_worktree(dir: &Path, relative: &str) {
    let path = dir.join(relative);
    Command::new("git")
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "gone",
            path.to_str().unwrap(),
        ])
        .current_dir(dir)
        .output()
        .unwrap();
    std::fs::remove_dir_all(&path).unwrap();
    Command::new("git")
        .args(["worktree", "prune"])
        .current_dir(dir)
        .output()
        .unwrap();
}

#[test]
fn recovers_paths_prefixed_by_a_deleted_worktree() {
    // A worktree is usually removed once its branch merges, so git can no
    // longer vouch for the prefix.
    let (dir, repo) = init_repo();
    let commit_sha = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    add_then_delete_worktree(dir.path(), ".claude/worktrees/gone");

    let conversation = session_editing(
        dir.path().to_str().unwrap(),
        ".claude/worktrees/gone/src/auth.rs",
        &commit_sha,
    );

    let objects =
        materialize_line_objects(&repo, &conversation, &commit_sha, Confidence::Exact).unwrap();
    assert!(
        !objects.is_empty(),
        "expected the deleted-worktree path to be recovered"
    );
    assert_eq!(objects[0].file_path, "src/auth.rs");
    assert_eq!(
        objects[0].confidence,
        Confidence::Heuristic,
        "a path recovered by inference must not claim exact provenance"
    );
}

#[test]
fn leaves_a_prefixed_path_alone_when_the_remainder_is_not_in_the_commit() {
    // Containment is the only evidence the worktree existed; without it the
    // prefix is presumed to be a real directory and the artifact is dropped.
    let (dir, repo) = init_repo();
    let commit_sha = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    add_then_delete_worktree(dir.path(), ".claude/worktrees/gone");

    let conversation = session_editing(
        dir.path().to_str().unwrap(),
        ".claude/worktrees/gone/src/nowhere.rs",
        &commit_sha,
    );

    let objects =
        materialize_line_objects(&repo, &conversation, &commit_sha, Confidence::Exact).unwrap();
    assert!(
        objects.is_empty(),
        "a remainder that names no file in the commit must not be recovered"
    );
}

#[test]
fn leaves_a_prefixed_path_alone_when_it_is_itself_tracked() {
    // A repository that genuinely tracks files under a worktree-shaped
    // directory keeps its own paths, even though the suffix also resolves.
    let (dir, repo) = init_repo();
    let nested = dir.path().join(".claude/worktrees/gone/src");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("auth.rs"), "pub fn validate() {}\n").unwrap();
    Command::new("git")
        .args(["add", "-A", "-f"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-qm", "track a worktree-shaped path"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let commit_sha = repo
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

    let conversation = session_editing(
        dir.path().to_str().unwrap(),
        ".claude/worktrees/gone/src/auth.rs",
        &commit_sha,
    );

    let objects =
        materialize_line_objects(&repo, &conversation, &commit_sha, Confidence::Exact).unwrap();
    assert!(!objects.is_empty(), "the tracked path itself must resolve");
    assert_eq!(
        objects[0].file_path, ".claude/worktrees/gone/src/auth.rs",
        "a tracked worktree-shaped path must not be stripped"
    );
    assert_eq!(
        objects[0].confidence,
        Confidence::Exact,
        "no inference happened, so confidence must not be degraded"
    );
}

#[test]
fn new_string_resolves_where_old_string_is_gone() {
    // A real replacement: the committed file contains only the post-image.
    let content = "fn setup() {}\nfn login(user: &User) {}\nfn teardown() {}\n";
    // Pre-image no longer present:
    assert!(lineage_git::resolve_old_string(content, "fn login() {}").is_empty());
    // Post-image locates the edit exactly:
    let matches = lineage_git::resolve_old_string(content, "fn login(user: &User) {}");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].0, [2, 2]);
}
