use std::fs;
use std::process::Command;

use lineage_cli::hooks_cmd;

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
fn install_hook_refuses_foreign_hook_without_force() {
    let dir = init_repo();
    let hooks = dir.path().join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    fs::write(hooks.join("pre-commit"), "#!/bin/sh\necho custom\n").unwrap();
    let err = hooks_cmd::install_hook(dir.path(), false);
    assert!(err.is_err());
    hooks_cmd::install_hook(dir.path(), true).unwrap();
    hooks_cmd::uninstall_hook(dir.path()).unwrap();
}
