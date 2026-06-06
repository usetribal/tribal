use std::process::Command;

use lineage_core::{LargeBlobBackend, LineageRepoConfig, LfsTransport, LINEAGE_CONFIG_SCHEMA};
use lineage_git::{lfs_push, open_repo, write_repo_config};

fn init_repo_with_remote() -> tempfile::TempDir {
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
    Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/example/lineage.git",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

#[test]
fn lfs_push_and_fetch_use_refs_transport() {
    let dir = init_repo_with_remote();
    let repo = open_repo(dir.path()).unwrap();
    let inner = repo.inner();

    let config = LineageRepoConfig {
        lfs_transport: LfsTransport::Refs,
        large_blob_backend: LargeBlobBackend::Lfs,
        schema_version: LINEAGE_CONFIG_SCHEMA.into(),
        ..LineageRepoConfig::default()
    };
    write_repo_config(inner, &config).unwrap();

    let push = lfs_push(inner, "origin").unwrap();
    assert_eq!(push.method, "refs");
}
