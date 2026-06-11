use std::process::Command;

use lineage_core::{AgentKind, Artifact, ArtifactKind, Conversation, LineageId, Role, Turn};
use lineage_git::patch_id::build_patch_id_index;
use lineage_git::{best_commit_for_conversation, map_conversation_to_commits, open_repo};

fn init_repo_with_file() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.email", "t@t.dev"],
        vec!["config", "user.name", "T"],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(dir.path())
            .output()
            .unwrap();
    }
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/auth.rs"), "pub fn validate() {}\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "add auth"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let sha = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let sha = String::from_utf8(sha.stdout).unwrap().trim().to_string();
    (dir, sha)
}

#[test]
fn maps_session_to_commit_by_file_overlap() {
    let (tmp, sha) = init_repo_with_file();
    let repo = open_repo(tmp.path()).unwrap();
    let inner = repo.inner();

    let mut conv = Conversation::new(AgentKind::Claude, tmp.path().display().to_string());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: String::new(),
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
            line_range: None,
            resolve: None,
        }],
    });

    let m = best_commit_for_conversation(inner, &conv).unwrap().unwrap();
    assert_eq!(m.commit_sha, sha);
    assert!(m.score >= 0.25);

    let mapped = map_conversation_to_commits(inner, &conv, 5).unwrap();
    assert!(!mapped.is_empty());
    let index = build_patch_id_index(inner).unwrap();
    assert!(!index.is_empty());
}
