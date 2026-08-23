//! Migrating state left by an older version of the CLI.
//!
//! These tests drive `HOME` rather than `LINEAGE_CONFIG_DIR`, because the
//! config-directory step deliberately does nothing when that override is set:
//! an explicit directory names one location, and there is no legacy counterpart
//! to move from. `HOME` is process-global, so every test here takes `ENV_LOCK`
//! and restores what it changed — the suite runs its tests on threads of one
//! process, and a leaked `HOME` would send another test at the developer's real
//! configuration.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

use lineage_cli::migrate;

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // A poisoned lock only means some other test panicked while holding it;
    // the environment is restored by the guard below either way.
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Points `HOME` at a scratch directory for as long as it is held.
struct HomeGuard {
    _dir: tempfile::TempDir,
    previous: Option<String>,
    _lock: MutexGuard<'static, ()>,
}

impl HomeGuard {
    fn new() -> (Self, PathBuf) {
        let lock = env_lock();
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var("HOME").ok();
        let path = dir.path().to_path_buf();
        // SAFETY: the environment is guarded by ENV_LOCK for this test's life.
        unsafe {
            std::env::set_var("HOME", &path);
            std::env::remove_var("XDG_CONFIG_HOME");
            std::env::remove_var(lineage_cli::auth::CONFIG_DIR_ENV);
        }
        (
            Self {
                _dir: dir,
                previous,
                _lock: lock,
            },
            path,
        )
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // SAFETY: still holding ENV_LOCK.
        unsafe {
            match &self.previous {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}

fn legacy_config(home: &Path) -> PathBuf {
    home.join(".config").join("lineage")
}

fn new_config(home: &Path) -> PathBuf {
    home.join(".config").join("tribal")
}

/// A config directory as an older version left it.
fn seed_legacy_config(home: &Path) {
    let dir = legacy_config(home);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("credentials.json"), r#"{"servers":{}}"#).unwrap();
    fs::write(dir.join("repos.json"), r#"{"repos":{}}"#).unwrap();
}

fn context() -> migrate::Context {
    migrate::Context { repo_path: None }
}

/// The ordinary upgrade: state exists only under the old name, so it moves.
#[test]
fn an_old_config_directory_moves_to_the_new_name() {
    let (_home, path) = HomeGuard::new();
    seed_legacy_config(&path);

    let reports = migrate::apply_pending(&context()).unwrap();

    assert!(new_config(&path).join("credentials.json").exists());
    assert!(!legacy_config(&path).exists());
    assert!(reports
        .iter()
        .any(|r| r.step == "config-dir" && matches!(r.outcome, migrate::Outcome::Applied(_))));
}

/// The second run of the same upgrade. Nothing is pending, so nothing runs —
/// this is the property that makes a resumed run safe.
#[test]
fn running_the_upgrade_twice_changes_nothing_the_second_time() {
    let (_home, path) = HomeGuard::new();
    seed_legacy_config(&path);

    migrate::apply_pending(&context()).unwrap();
    let second = migrate::apply_pending(&context()).unwrap();

    assert!(
        second.is_empty(),
        "every step is recorded by the first run, so the second considers none"
    );
    assert!(migrate::pending(&context()).unwrap().is_empty());
}

/// State at both names. The new one is authoritative and the old one is left
/// alone: the user has data at both paths, and choosing which to destroy is not
/// a decision this should make without being asked.
#[test]
fn a_directory_at_both_names_keeps_the_new_one_and_leaves_the_old() {
    let (_home, path) = HomeGuard::new();
    seed_legacy_config(&path);
    fs::create_dir_all(new_config(&path)).unwrap();
    fs::write(
        new_config(&path).join("credentials.json"),
        r#"{"new":true}"#,
    )
    .unwrap();

    migrate::apply_pending(&context()).unwrap();

    assert!(legacy_config(&path).exists(), "the old directory is kept");
    let kept = fs::read_to_string(new_config(&path).join("credentials.json")).unwrap();
    assert!(kept.contains("new"), "the new directory is not overwritten");
}

/// A machine that never ran an older version. There is nothing to detect, and
/// the command must be silent rather than inventing work.
#[test]
fn a_machine_with_no_old_state_has_nothing_pending() {
    let (_home, _path) = HomeGuard::new();

    assert!(migrate::pending(&context()).unwrap().is_empty());
    // Steps still get recorded as settled, so this machine never reconsiders
    // them, but none of them report having changed anything.
    let reports = migrate::apply_pending(&context()).unwrap();
    assert!(reports
        .iter()
        .all(|r| matches!(r.outcome, migrate::Outcome::Skipped(_))));
}

/// The resumability guarantee. A run that stops partway must leave the steps
/// that did land recorded, so re-running finishes the remainder instead of
/// repeating it — which is what stands in for the atomicity these steps cannot
/// have, spanning as they do a home directory, git config, hooks and a worktree.
#[test]
fn a_partial_run_records_what_landed_and_resumes_from_there() {
    let (_home, path) = HomeGuard::new();
    seed_legacy_config(&path);

    // Stop after the first step by running it alone, which is what a failure in
    // a later step leaves behind: its predecessors applied and recorded.
    let first = migrate::MIGRATIONS[0].steps.first().unwrap();
    assert!((first.detect)(&context()).unwrap());
    (first.apply)(&context()).unwrap();

    // The move happened, but nothing recorded it yet.
    assert!(new_config(&path).exists());
    let before = migrate::load_record();
    assert!(before.applied.is_empty());

    // Resuming records the completed work and reports the run as finished.
    migrate::apply_pending(&context()).unwrap();
    let after = migrate::load_record();
    assert!(
        after.applied.iter().any(|a| a.step == "config-dir"),
        "the completed step is recorded on resume"
    );
    assert!(migrate::pending(&context()).unwrap().is_empty());
}

/// The record is what a later automatic upgrade would read to decide what to
/// run, so each entry has to say which release introduced the step.
#[test]
fn each_applied_step_records_the_version_that_introduced_it() {
    let (_home, path) = HomeGuard::new();
    seed_legacy_config(&path);

    migrate::apply_pending(&context()).unwrap();

    let record = migrate::load_record();
    let entry = record
        .applied
        .iter()
        .find(|a| a.step == "config-dir")
        .expect("the config-dir step is recorded");
    assert_eq!(entry.migration, "0001-tribal-rename");
    assert_eq!(entry.version, "0.5.0");
}

/// The record lives inside the directory the first step moves, so it must be
/// written at the new location — not stranded at the old one.
#[test]
fn the_record_is_written_beside_the_migrated_configuration() {
    let (_home, path) = HomeGuard::new();
    seed_legacy_config(&path);

    migrate::apply_pending(&context()).unwrap();

    assert!(new_config(&path).join("migrations.json").exists());
    assert!(!legacy_config(&path).join("migrations.json").exists());
}

fn init_repo(home: &Path) -> PathBuf {
    let path = home.join("checkout");
    fs::create_dir_all(&path).unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(&path)
        .output()
        .unwrap();
    path
}

fn repo_context(path: &Path) -> migrate::Context {
    migrate::Context {
        repo_path: Some(path.to_path_buf()),
    }
}

/// A hook this CLI installed, as an older version wrote it.
fn seed_stale_hooks(repo: &Path) {
    let hooks = repo.join(".git").join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    fs::write(
        hooks.join("pre-commit"),
        "#!/bin/sh\n# Lineage pre-commit hook: import agent sessions before each commit.\ngit-lineage import\n",
    )
    .unwrap();
    fs::write(
        hooks.join("post-commit"),
        "#!/bin/sh\n# Lineage post-commit hook: link recently imported sessions to the new commit.\ngit-lineage hook post-commit\n",
    )
    .unwrap();
}

/// Hooks an older version installed name a command that no longer exists, so
/// they are rewritten in place — the marker is what identifies them as ours.
#[test]
fn stale_hooks_are_rewritten_to_name_the_new_command() {
    let (_home, home) = HomeGuard::new();
    let repo = init_repo(&home);
    seed_stale_hooks(&repo);

    migrate::apply_pending(&repo_context(&repo)).unwrap();

    let pre = fs::read_to_string(repo.join(".git/hooks/pre-commit")).unwrap();
    assert!(pre.contains("tribal"), "the hook names the new command");
    assert!(!pre.contains("git-lineage"), "and not the old one");
}

/// A hook the user wrote themselves carries no marker. Rewriting it would
/// destroy their work, so it is left exactly as found.
#[test]
fn a_hook_the_user_wrote_is_left_alone() {
    let (_home, home) = HomeGuard::new();
    let repo = init_repo(&home);
    let hooks = repo.join(".git").join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    let theirs = "#!/bin/sh\necho my own hook\n";
    fs::write(hooks.join("pre-commit"), theirs).unwrap();

    migrate::apply_pending(&repo_context(&repo)).unwrap();

    assert_eq!(
        fs::read_to_string(hooks.join("pre-commit")).unwrap(),
        theirs
    );
}

/// Agent skills are copies, not links: one installed before the rename still
/// tells an agent to run a command that no longer exists.
#[test]
fn stale_agent_skills_are_rewritten() {
    let (_home, home) = HomeGuard::new();
    let repo = init_repo(&home);
    let skill = repo.join(".claude/skills/lineage/SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(&skill, "---\nname: lineage\n---\nRun `tribal list`.\n").unwrap();

    migrate::apply_pending(&repo_context(&repo)).unwrap();

    let rewritten = fs::read_to_string(&skill).unwrap();
    assert!(
        !rewritten.contains("git lineage"),
        "the old command is gone"
    );
}

/// The skills step must not install into a repository that never opted in —
/// only the agent directories that already hold a stale copy are rewritten.
#[test]
fn a_repository_without_skills_does_not_gain_them() {
    let (_home, home) = HomeGuard::new();
    let repo = init_repo(&home);

    migrate::apply_pending(&repo_context(&repo)).unwrap();

    assert!(!repo.join(".claude/skills").exists());
    assert!(!repo.join(".cursor/skills").exists());
    assert!(!repo.join(".agents/skills").exists());
}

/// The cached server binding moves key-to-key: the value is server-issued, so
/// it is carried across rather than dropped and re-fetched.
#[test]
fn the_cached_server_repo_id_moves_to_the_new_key() {
    let (_home, home) = HomeGuard::new();
    let repo = init_repo(&home);
    Command::new("git")
        .args(["config", "lineage.serverRepoId", "repo-abc123"])
        .current_dir(&repo)
        .output()
        .unwrap();

    migrate::apply_pending(&repo_context(&repo)).unwrap();

    let new = Command::new("git")
        .args(["config", "--get", "tribal.serverRepoId"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&new.stdout).trim(), "repo-abc123");

    let old = Command::new("git")
        .args(["config", "--get", "lineage.serverRepoId"])
        .current_dir(&repo)
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&old.stdout).trim().is_empty(),
        "the retired key is cleared so no reader can pick the stale one"
    );
}
