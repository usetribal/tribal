use std::process::Command;

use lineage_core::{
    AgentKind, Artifact, ArtifactKind, Conversation, LargeBlobBackend, LineageRepoConfig,
    LineageId, Role, Turn, LINEAGE_CONFIG_SCHEMA,
};
use lineage_git::{
    best_commit_for_conversation, find_repo, hydrate_conversation, hydrate_media_artifacts,
    indexable_body, link_session_to_commit, list_session_ids, map_commit_to_sessions,
    materialize_line_objects, open_repo, persist_conversation, read_conversation,
    read_repo_config, run_doctor, write_repo_config,
};
use lineage_core::Confidence;

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
    std::fs::write(dir.path().join("api.rs"), "let v = 1;\n").unwrap();
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
fn exercises_public_git_api_surface() {
    let dir = init_repo();
    assert!(find_repo(dir.path()).is_some());
    let repo = open_repo(dir.path()).unwrap();
    let inner = repo.inner();
    let sha = inner
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

    let config = LineageRepoConfig {
        large_blob_backend: LargeBlobBackend::Lfs,
        large_blob_threshold_bytes: 64,
        schema_version: LINEAGE_CONFIG_SCHEMA.into(),
        ..LineageRepoConfig::default()
    };
    write_repo_config(inner, &config).unwrap();
    assert!(read_repo_config(inner).unwrap().large_blob_threshold_bytes == 64);

    let mut conv = Conversation::new(AgentKind::Claude, dir.path().display().to_string());
    conv.commit_shas.push(sha.clone());
    conv.metadata.insert(
        "git_branch".into(),
        serde_json::Value::String("main".into()),
    );
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "update api.rs".into(),
        tool_calls: vec![lineage_core::ToolCall {
            id: "t1".into(),
            name: "edit".into(),
            arguments: r#"{"path":"api.rs"}"#.into(),
            result: Some("y".repeat(200)),
        }],
        model: None,
        timestamp: None,
        artifacts: vec![Artifact {
            kind: ArtifactKind::FileEdit,
            path: "api.rs".into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: Some([1, 1]),
            resolve: None,
        }],
    });
    persist_conversation(inner, &conv).unwrap();
    assert!(!list_session_ids(inner).unwrap().is_empty());

    let _ = best_commit_for_conversation(inner, &conv).unwrap();
    let _ = map_commit_to_sessions(inner, &sha).unwrap();
    let _ = link_session_to_commit(inner, &conv.id, &sha).unwrap();
    let _ = materialize_line_objects(inner, &conv, &sha, Confidence::Exact).unwrap();

    let mut loaded = read_conversation(inner, &conv.id).unwrap().unwrap();
    assert!(indexable_body(&loaded).contains("api.rs"));
    let _ = hydrate_conversation(inner, &mut loaded).unwrap();
    let _ = hydrate_media_artifacts(inner, &mut loaded).unwrap();
    let _ = run_doctor(&repo).unwrap();
}
