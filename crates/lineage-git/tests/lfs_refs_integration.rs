use std::process::Command;

use lineage_core::{
    AgentKind, Conversation, LargeBlobBackend, LineageRepoConfig, Role, Turn, LINEAGE_CONFIG_SCHEMA,
};
use lineage_git::{
    collect_all_blob_refs, collect_blob_refs_from_conversation, lfs_data_ref, lfs_pointer_ref,
    list_lfs_data_refs, open_repo, persist_conversation, read_lfs_data_from_ref,
    read_lfs_pointer_ref, write_lfs_data_ref, write_lfs_pointer_ref, write_repo_config,
};

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

#[test]
fn lfs_refs_round_trip_and_collect() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let inner = repo.inner();
    let oid = "abc123";

    write_lfs_pointer_ref(inner, oid, 12).unwrap();
    write_lfs_data_ref(inner, oid, b"payload").unwrap();
    assert!(read_lfs_pointer_ref(inner, oid).unwrap().is_some());
    assert_eq!(
        read_lfs_data_from_ref(inner, oid).unwrap().unwrap(),
        b"payload"
    );
    assert_eq!(lfs_pointer_ref(oid), "refs/lineage/lfs/abc123");
    assert_eq!(lfs_data_ref(oid), "refs/lineage/lfs-data/abc123");
    assert_eq!(list_lfs_data_refs(inner).unwrap(), vec![oid.to_string()]);

    let config = LineageRepoConfig {
        large_blob_backend: LargeBlobBackend::Lfs,
        large_blob_threshold_bytes: 8,
        schema_version: LINEAGE_CONFIG_SCHEMA.into(),
        ..LineageRepoConfig::default()
    };
    write_repo_config(inner, &config).unwrap();

    let mut conv = Conversation::new(AgentKind::Cursor, dir.path().display().to_string());
    conv.turns.push(Turn {
        id: lineage_core::LineageId::new(),
        role: Role::User,
        content: "z".repeat(50),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    persist_conversation(inner, &conv).unwrap();
    let stored = lineage_git::read_conversation_stored(inner, &conv.id)
        .unwrap()
        .unwrap();
    let refs = collect_blob_refs_from_conversation(&stored);
    assert!(!refs.is_empty());
    assert!(!collect_all_blob_refs(inner).unwrap().is_empty());
}
