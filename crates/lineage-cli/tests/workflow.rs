use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lineage_cli::{commands, hooks_cmd, init_cmd, skill_cmd};
use lineage_core::{AgentKind, Conversation, LineageId, LineageRepoConfig, Role, Turn};
use lineage_git::{
    open_repo, persist_conversation, read_conversation, PROMPTED_BY_EMAIL, PROMPTED_BY_NAME,
};

fn init_repo() -> tempfile::TempDir {
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
    std::fs::write(dir.path().join("src.txt"), "hello\n").unwrap();
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

fn seed_session(dir: &tempfile::TempDir) -> String {
    let repo = open_repo(dir.path()).unwrap();
    let sha = repo
        .inner()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    let mut conv = Conversation::new(AgentKind::Claude, dir.path().display().to_string());
    conv.commit_shas.push(sha);
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "add authentication middleware".into(),
        tool_calls: vec![],
        model: Some("claude-sonnet".into()),
        timestamp: None,
        artifacts: vec![],
    });
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "Updated src.txt".into(),
        tool_calls: vec![],
        model: Some("claude-sonnet".into()),
        timestamp: None,
        artifacts: vec![lineage_core::Artifact {
            kind: lineage_core::ArtifactKind::FileEdit,
            path: "src.txt".into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: Some([1, 1]),
            resolve: None,
        }],
    });
    persist_conversation(repo.inner(), &conv).unwrap();
    conv.id.to_string()
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn install_cursor_fixture(dir: &Path) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/cursor-history/.cursor");
    let dest = dir.join(".cursor");
    copy_dir_all(&fixture, &dest).unwrap();
}

#[test]
fn cli_import_and_hooks() {
    let dir = init_repo();
    install_cursor_fixture(dir.path());
    commands::init_config(dir.path()).unwrap();
    commands::import(dir.path(), &["cursor".into()], None, true, false).unwrap();
    commands::import(
        dir.path(),
        &["cursor".into()],
        Some("2099-01-01"),
        true,
        true,
    )
    .unwrap();

    hooks_cmd::install_hook(dir.path(), true).unwrap();
    hooks_cmd::post_commit(dir.path()).unwrap();
    hooks_cmd::uninstall_hook(dir.path()).unwrap();
}

#[test]
fn cli_workflow_covers_commands() {
    let dir = init_repo();
    commands::init_config(dir.path()).unwrap();
    lineage_cli::doctor_cmd::run(
        dir.path(),
        &lineage_cli::doctor_cmd::DoctorArgs {
            json: false,
            sections: vec![],
            activity_limit: 20,
        },
    )
    .unwrap();

    let session_id = seed_session(&dir);
    let sha = open_repo(dir.path())
        .unwrap()
        .inner()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    // Use a local bare repo as the remote so the lfs push/fetch calls below stay hermetic.
    let remote_dir = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init", "--bare"])
        .current_dir(remote_dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            remote_dir.path().to_str().unwrap(),
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    commands::list(dir.path(), None, false).unwrap();
    commands::list(dir.path(), None, true).unwrap();
    commands::list(dir.path(), Some(&sha), true).unwrap();
    commands::link(dir.path(), &session_id, &sha).unwrap();
    commands::show(dir.path(), &session_id, false, false).unwrap();
    commands::show(dir.path(), &session_id, true, true).unwrap();
    commands::blame(dir.path(), "src.txt:1", false).unwrap();
    commands::blame(dir.path(), "src.txt:1", true).unwrap();
    commands::search(dir.path(), "authentication").unwrap();
    commands::rebuild_index(dir.path(), false).unwrap();
    commands::export(dir.path(), true, "json").unwrap();
    commands::export(dir.path(), false, "jsonl").unwrap();
    commands::materialize(dir.path(), None, Some(&session_id)).unwrap();
    commands::remap(dir.path()).unwrap();
    commands::lfs_status_cmd(dir.path()).unwrap();
    let _ = commands::lfs_push_cmd(dir.path(), Some("origin"));
    let _ = commands::lfs_fetch_cmd(dir.path(), Some("origin"));
    commands::gc_cmd(dir.path()).unwrap();
    commands::delete_session_cmd(dir.path(), &session_id, true).unwrap();
}

#[test]
fn cli_export_rejects_unknown_format() {
    let dir = init_repo();
    commands::init_config(dir.path()).unwrap();
    assert!(commands::export(dir.path(), false, "yaml").is_err());
}

#[test]
fn cli_sync_requires_a_token_or_login() {
    let dir = init_repo();
    commands::init_config(dir.path()).unwrap();
    // No --token, no LINEAGE_TOKEN, and no stored login (credentials isolated to
    // an empty temp dir): the command must fail pointing at `login`.
    let config_dir = tempfile::tempdir().unwrap();
    std::env::set_var(lineage_cli::auth::CONFIG_DIR_ENV, config_dir.path());
    std::env::remove_var("LINEAGE_TOKEN");
    let err = commands::sync(dir.path(), Some("http://127.0.0.1:0"), None, "origin")
        .expect_err("sync without a token or login should fail");
    assert!(err.to_string().contains("login"), "got: {err}");
}

#[test]
fn cli_sync_assembles_then_fails_on_unreachable_server() {
    let dir = init_repo();
    commands::init_config(dir.path()).unwrap();
    seed_session(&dir);
    Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();
    // Reaching the transport error proves assembly + repo binding succeeded; the
    // port-0 address is guaranteed unbindable, so the POST cannot connect.
    let err = commands::sync(
        dir.path(),
        Some("http://127.0.0.1:0"),
        Some("dev-token"),
        "origin",
    )
    .expect_err("sync to an unreachable server should fail");
    assert!(
        err.to_string().contains("sync request failed"),
        "got: {err}"
    );
}

#[test]
fn cli_search_no_results() {
    let dir = init_repo();
    commands::init_config(dir.path()).unwrap();
    commands::search(dir.path(), "zzznomatchzzz").unwrap();
}

#[test]
fn cli_import_all_agents() {
    let dir = init_repo();
    install_cursor_fixture(dir.path());
    commands::init_config(dir.path()).unwrap();
    commands::import(dir.path(), &["all".into()], None, true, false).unwrap();
}

#[test]
fn cli_import_skips_non_code_when_configured() {
    let dir = init_repo();
    install_cursor_fixture(dir.path());
    commands::init_config(dir.path()).unwrap();
    let repo = open_repo(dir.path()).unwrap();
    let base = lineage_git::read_repo_config(repo.inner()).unwrap();
    let config = LineageRepoConfig {
        import_only_code_sessions: true,
        ..base
    };
    lineage_git::write_repo_config(repo.inner(), &config).unwrap();
    let chat_only = dir.path().join(".cursor/agent-transcripts/chat-only.jsonl");
    std::fs::create_dir_all(chat_only.parent().unwrap()).unwrap();
    std::fs::write(chat_only, r#"{"role":"user","content":"hello"}"#).unwrap();
    commands::import(dir.path(), &["cursor".into()], None, false, false).unwrap();
}

#[test]
fn import_stamps_prompted_by_from_git_config() {
    let dir = init_repo();
    install_cursor_fixture(dir.path());
    commands::init_config(dir.path()).unwrap();
    commands::import(dir.path(), &["cursor".into()], None, false, false).unwrap();

    let repo = open_repo(dir.path()).unwrap();
    let ids = lineage_git::list_session_ids(repo.inner()).unwrap();
    assert!(!ids.is_empty(), "expected imported session");
    let conv = read_conversation(repo.inner(), &ids[0])
        .unwrap()
        .expect("session blob");
    assert_eq!(
        conv.metadata
            .get(PROMPTED_BY_EMAIL)
            .and_then(|v| v.as_str()),
        Some("test@test.dev")
    );
    assert_eq!(
        conv.metadata.get(PROMPTED_BY_NAME).and_then(|v| v.as_str()),
        Some("Test")
    );
}

#[test]
fn init_non_interactive_installs_defaults() {
    let dir = init_repo();
    init_cmd::init(
        dir.path(),
        init_cmd::InitOptions {
            yes: true,
            no_import: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(dir.path().join(".cursor/skills/lineage/SKILL.md").is_file());
    assert!(dir.path().join(".git/hooks/pre-commit").is_file());
}

#[test]
fn init_skill_installs_all_targets_by_default() {
    let dir = init_repo();
    skill_cmd::init_skill(dir.path(), &[], false).unwrap();
    assert!(dir.path().join(".cursor/skills/lineage/SKILL.md").is_file());
    assert!(dir.path().join(".claude/skills/lineage/SKILL.md").is_file());
    assert!(dir.path().join(".agents/skills/lineage/SKILL.md").is_file());
}

#[test]
fn init_skill_multiselect_targets() {
    let dir = init_repo();
    skill_cmd::init_skill(dir.path(), &["cursor".into(), "claude".into()], false).unwrap();
    assert!(dir.path().join(".cursor/skills/lineage/SKILL.md").is_file());
    assert!(dir.path().join(".claude/skills/lineage/SKILL.md").is_file());
    assert!(!dir.path().join(".agents/skills/lineage/SKILL.md").exists());
}

#[test]
fn init_skill_agents_alias_installs_codex_path() {
    let dir = init_repo();
    skill_cmd::init_skill(dir.path(), &["agents".into()], false).unwrap();
    assert!(dir.path().join(".agents/skills/lineage/SKILL.md").is_file());
    assert!(!dir.path().join(".cursor/skills/lineage/SKILL.md").exists());
}

#[test]
fn init_skill_refuses_without_force_when_present() {
    let dir = init_repo();
    skill_cmd::init_skill(dir.path(), &["cursor".into()], false).unwrap();
    assert!(skill_cmd::init_skill(dir.path(), &["cursor".into()], false).is_err());
}
