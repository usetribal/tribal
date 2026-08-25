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

/// A skill installed before 0.6 tells an agent to prefer `--json` and never
/// mentions `--no-interactive`, so `tribal list` opens a TUI the agent cannot
/// drive. Detected by what the copy lacks rather than a retired string: the old
/// advice was not wrong, it was incomplete.
#[test]
fn skills_predating_the_headless_switch_are_updated() {
    let (_home, home) = HomeGuard::new();
    let repo = init_repo(&home);
    let skill = repo.join(".claude/skills/lineage/SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(
        &skill,
        "---\nname: lineage\n---\nRetrieve context (prefer `--json`): `tribal list --json`.\n",
    )
    .unwrap();

    migrate::apply_pending(&repo_context(&repo)).unwrap();

    let rewritten = fs::read_to_string(&skill).unwrap();
    assert!(
        rewritten.contains("--no-interactive"),
        "the agent must be told the headless switch: {rewritten}"
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

// --- Running without being asked -------------------------------------------

/// A fresh install. The detectors find no old state, so nothing is converted
/// and there is no upgrade to announce — but the stamp is written, so the next
/// command takes the cheap path.
#[test]
fn a_first_run_on_a_clean_machine_reports_nothing_and_stamps() {
    let (_home, path) = HomeGuard::new();

    let outcome = migrate::run_pending_for_version(&context(), "0.6.0").unwrap();

    assert!(matches!(outcome, migrate::AutoUpgrade::UpToDate));
    assert!(
        migrate::pending(&context()).unwrap().is_empty(),
        "a machine with no old state has nothing pending after a first run"
    );
    assert!(new_config(&path).join("version.json").exists());
}

/// The upgrade that matters most: a user on a release older than the stamp
/// itself. There is no stamp to compare against, so the migrations must run on
/// the strength of the detectors alone — and the login must survive.
#[test]
fn an_unstamped_machine_still_migrates_its_old_state() {
    let (_home, path) = HomeGuard::new();
    seed_legacy_config(&path);

    migrate::run_pending_for_version(&context(), "0.6.0").unwrap();

    assert!(
        new_config(&path).join("credentials.json").exists(),
        "stamping must not create the config directory before 0001 can move it"
    );
    assert!(!legacy_config(&path).exists());
}

/// The steady state, and the reason the check is cheap: the same version
/// running again does no work at all.
#[test]
fn the_same_version_running_again_is_up_to_date() {
    let (_home, _path) = HomeGuard::new();

    migrate::run_pending_for_version(&context(), "0.6.0").unwrap();
    let second = migrate::run_pending_for_version(&context(), "0.6.0").unwrap();

    assert!(matches!(second, migrate::AutoUpgrade::UpToDate));
}

/// The upgrade a user never asks for: a new binary meets a machine that ran an
/// older one, and says so on the first command they happen to run.
#[test]
fn a_new_version_applies_pending_migrations_and_reports_the_move() {
    let (_home, _path) = HomeGuard::new();
    migrate::run_pending_for_version(&context(), "0.5.0").unwrap();

    let outcome = migrate::run_pending_for_version(&context(), "0.6.0").unwrap();

    let migrate::AutoUpgrade::Upgraded { from, to, .. } = outcome else {
        panic!("a version change must report the upgrade it performed");
    };
    assert_eq!(from, "0.5.0");
    assert_eq!(to, "0.6.0");
}

/// A version bump that carried no migration still stamps, so the expensive
/// detection runs once rather than on every command from then on.
#[test]
fn a_version_change_with_nothing_to_migrate_still_stamps() {
    let (_home, _path) = HomeGuard::new();
    migrate::run_pending_for_version(&context(), "0.5.0").unwrap();

    let outcome = migrate::run_pending_for_version(&context(), "0.6.0").unwrap();
    assert!(matches!(outcome, migrate::AutoUpgrade::Upgraded { .. }));

    let third = migrate::run_pending_for_version(&context(), "0.6.0").unwrap();
    assert!(matches!(third, migrate::AutoUpgrade::UpToDate));
}

/// Running an older binary after a newer one is a version change, not an
/// upgrade. The steps still run — state a newer version wrote may need them —
/// but announcing "upgrading from v0.6.0 to v0.5.1" states the direction
/// backwards, so there is nothing to report.
#[test]
fn an_older_binary_after_a_newer_one_reports_no_upgrade() {
    let (_home, _path) = HomeGuard::new();
    migrate::run_pending_for_version(&context(), "0.6.0").unwrap();

    let outcome = migrate::run_pending_for_version(&context(), "0.5.1").unwrap();

    assert!(
        matches!(outcome, migrate::AutoUpgrade::UpToDate),
        "a downgrade must not announce itself as an upgrade"
    );
}
