use std::process::Command;

use lineage_core::{
    AgentKind, Conversation, LargeBlobBackend, LineageRepoConfig, Role, Turn, LINEAGE_CONFIG_SCHEMA,
};
use lineage_git::{
    hydrate_conversation, open_repo, persist_conversation, read_conversation,
    read_conversation_stored, write_repo_config,
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
fn lfs_compact_and_hydrate_round_trip() {
    let tmp = init_test_repo();
    let repo = open_repo(tmp.path()).unwrap();
    let inner = repo.inner();

    let config = LineageRepoConfig {
        large_blob_backend: LargeBlobBackend::Lfs,
        large_blob_threshold_bytes: 32,
        schema_version: LINEAGE_CONFIG_SCHEMA.into(),
        ..LineageRepoConfig::default()
    };
    write_repo_config(inner, &config).unwrap();

    let head_sha = inner
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();

    let large = "x".repeat(10_000);
    let mut conv = Conversation::new(AgentKind::Cursor, tmp.path().display().to_string());
    conv.commit_shas.push(head_sha);
    conv.turns.push(Turn {
        id: lineage_core::LineageId::new(),
        role: Role::User,
        content: large.clone(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });

    persist_conversation(inner, &conv).unwrap();

    let stored = read_conversation_stored(inner, &conv.id)
        .unwrap()
        .unwrap();
    assert!(stored.turns[0].content.starts_with("[blob:"));

    let mut hydrated = stored.clone();
    let report = hydrate_conversation(inner, &mut hydrated).unwrap();
    assert_eq!(report.hydrated_turns, 1);
    assert_eq!(hydrated.turns[0].content, large);

    let via_read = read_conversation(inner, &conv.id).unwrap().unwrap();
    assert_eq!(via_read.turns[0].content, large);
}
