//! What the CLI has done to this machine, and what it must do after an upgrade.
//!
//! A change that renames or reshapes anything the CLI persists registers a
//! [`Migration`] here in the same pull request. `upgrade` then detects the old
//! shape and converts it, so a user carries their state across the change
//! instead of rebuilding it.
//!
//! Lives in the CLI rather than in `lineage-git` because a migration spans every
//! surface the CLI touches — the config directory, git config, hooks, and the
//! bundled agent skills — and this is the only crate that can see all of them.
//!
//! Two properties matter more than they might look:
//!
//! **Steps, not migrations, are the unit of record.** The substrates involved
//! (a home directory, `.git/config`, `.git/hooks/`, a worktree) share no
//! transaction, so a run cannot be made atomic — a failure partway through
//! cannot be rolled back. Recording each step as it lands makes the run
//! *resumable* instead: whatever failed is still pending, and running `upgrade`
//! again finishes the job. Steps are ordered so the ones with real data at stake
//! run first and the cheap re-runnable ones last.
//!
//! **Every step is idempotent.** A step that has nothing to do reports that it
//! has nothing to do; it never fails. This is what makes re-running safe, and it
//! is why a rename is written as an explicit three-case move (old only, both,
//! new only) rather than as "make the new thing exist".

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Where the applied-step record lives, inside the config directory.
const RECORD_FILE: &str = "migrations.json";

/// Where the last version to run on this machine is stamped.
const VERSION_STAMP_FILE: &str = "version.json";

/// What a step did, for the report `upgrade` prints.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The step ran and changed something. The string is shown to the user.
    Applied(String),
    /// There was nothing to do. Already migrated, or never affected.
    Skipped(String),
}

/// One indivisible piece of a migration, recorded on its own.
///
/// `detect` answers "is there anything to do here?" without changing anything,
/// which is what makes `--dry-run` honest rather than a rehearsal.
pub struct Step {
    pub id: &'static str,
    pub detect: fn(&Context) -> Result<bool>,
    pub apply: fn(&Context) -> Result<Outcome>,
}

/// A set of steps introduced together by one breaking change.
pub struct Migration {
    pub id: &'static str,
    /// The CLI version that introduced the change. Recorded with each applied
    /// step so the history reads as a version timeline, and so a future
    /// automatic upgrade can decide what to run without reworking this registry.
    pub introduced_in: &'static str,
    pub steps: &'static [Step],
}

/// What a step is given to work with.
///
/// The repository is optional because `upgrade` must run outside one: the
/// config directory is machine-global, and a user whose first act after
/// upgrading is `tribal upgrade` from their home directory should still get
/// their credentials moved.
pub struct Context {
    pub repo_path: Option<PathBuf>,
}

/// One applied step. Flat rather than nested under a migration so the file
/// stays append-friendly and a partially applied migration is representable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedStep {
    pub migration: String,
    pub step: String,
    pub version: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub applied: Vec<AppliedStep>,
}

impl Record {
    fn contains(&self, migration: &str, step: &str) -> bool {
        self.applied
            .iter()
            .any(|a| a.migration == migration && a.step == step)
    }
}

pub fn record_path() -> Result<PathBuf> {
    Ok(crate::auth::config_dir()?.join(RECORD_FILE))
}

/// The record as stored, or an empty one for anything unreadable.
///
/// Corruption is treated as absence deliberately, matching the repo registry:
/// every step is idempotent, so the worst case of re-running one is that it
/// reports it had nothing to do.
pub fn load_record() -> Record {
    let Ok(path) = record_path() else {
        return Record::default();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return Record::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_record(record: &Record) -> Result<()> {
    let path = record_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(record)?)?;
    Ok(())
}

/// A step that has run, and the report line for it.
pub struct StepReport {
    pub migration: &'static str,
    pub step: &'static str,
    pub outcome: Outcome,
}

/// Steps that would run, without running them.
pub fn pending(context: &Context) -> Result<Vec<(&'static str, &'static str)>> {
    let record = load_record();
    let mut out = Vec::new();
    for migration in MIGRATIONS {
        for step in migration.steps {
            if record.contains(migration.id, step.id) {
                continue;
            }
            if (step.detect)(context)? {
                out.push((migration.id, step.id));
            }
        }
    }
    Ok(out)
}

/// Apply every pending step, in registry order, recording each as it lands.
///
/// The record is written after each step rather than once at the end: a failure
/// partway through must leave the steps that did succeed marked done, or a
/// re-run would repeat them. It stops at the first failure so a broken step
/// cannot cascade into ones that assume it ran.
pub fn apply_pending(context: &Context) -> Result<Vec<StepReport>> {
    let mut reports = Vec::new();

    for migration in MIGRATIONS {
        for step in migration.steps {
            let mut record = load_record();
            if record.contains(migration.id, step.id) {
                continue;
            }

            // A step with nothing left to detect is finished, not absent: either
            // it never applied here, or it applied and was interrupted before it
            // could be recorded. Both are terminal, so record it and move on —
            // otherwise an interrupted step stays pending forever and `--dry-run`
            // keeps promising work that will never happen.
            let outcome = if (step.detect)(context)? {
                (step.apply)(context)?
            } else {
                Outcome::Skipped("nothing to do".into())
            };
            record.applied.push(AppliedStep {
                migration: migration.id.to_string(),
                step: step.id.to_string(),
                version: migration.introduced_in.to_string(),
                at: Utc::now(),
            });
            save_record(&record)?;

            reports.push(StepReport {
                migration: migration.id,
                step: step.id,
                outcome,
            });
        }
    }
    Ok(reports)
}

/// Every repository this step should touch: the ones the registry knows about,
/// plus the current one, which may not be registered yet.
///
/// Paths that no longer resolve to a repository are dropped rather than failing
/// the run — a recorded checkout can have been deleted or moved since, and one
/// stale entry must not block the rest of the machine from migrating.
fn repos_to_stamp(context: &Context) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();

    let registry = crate::repo_registry::load();
    let candidates = registry
        .repos
        .values()
        .map(|entry| entry.path.clone())
        .chain(context.repo_path.clone());

    for path in candidates {
        if !path.join(".git").exists() {
            continue;
        }
        let key = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if seen.insert(key) {
            out.push(path);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Running migrations without being asked
// ---------------------------------------------------------------------------

/// The version that last ran on this machine.
///
/// This exists to keep the automatic check cheap. Asking [`pending`] whether
/// there is work opens a git config in every registered repository, which is
/// far too much to do on every invocation of every command. The version a
/// binary was built as is a fact it already knows, so comparing it against the
/// last one to run is a string compare — and a migration can only ever become
/// pending when that string changes.
#[derive(Debug, Serialize, Deserialize)]
struct VersionStamp {
    version: String,
}

fn version_stamp_path() -> Result<PathBuf> {
    Ok(crate::auth::config_dir()?.join(VERSION_STAMP_FILE))
}

fn load_version_stamp() -> Option<String> {
    let path = version_stamp_path().ok()?;
    let text = fs::read_to_string(&path).ok()?;
    let stamp: VersionStamp = serde_json::from_str(&text).ok()?;
    Some(stamp.version)
}

fn save_version_stamp(version: &str) -> Result<()> {
    let path = version_stamp_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let stamp = VersionStamp {
        version: version.to_string(),
    };
    fs::write(&path, serde_json::to_string_pretty(&stamp)?)?;
    Ok(())
}

/// What [`run_pending_for_version`] did, so the caller can report it.
pub enum AutoUpgrade {
    /// The stamp matched: this version has run here before.
    UpToDate,
    /// The version changed and migrations were applied.
    Upgraded {
        from: String,
        to: String,
        reports: Vec<StepReport>,
    },
}

/// Apply any migrations this version introduced, if it has not run here before.
///
/// Called once per invocation from the dispatcher so a user never has to know
/// `upgrade` exists: the release that needs a migration runs it the first time
/// any command is used. Safe to do automatically only because every step is
/// idempotent and the run is resumable — the same properties that make
/// `upgrade` safe to re-run by hand.
///
/// A machine with no stamp has never run a version that writes one, which
/// covers both a fresh install and every release before this check existed.
/// Those are indistinguishable here and must not be told apart by guessing: the
/// unstamped machine runs the migrations and is stamped afterwards, because
/// `0001`'s detectors already answer "is there old state here?" directly, and
/// answer it with "no" on a fresh install.
///
/// The stamp is written only after migrations have run, on every path. Writing
/// it first would create the config directory, and `0001` moves the old
/// directory only when the new one does not yet exist — so an eager stamp would
/// strand the user's login at the old path permanently.
pub fn run_pending_for_version(context: &Context, current: &str) -> Result<AutoUpgrade> {
    let previous = load_version_stamp();
    if previous.as_deref() == Some(current) {
        return Ok(AutoUpgrade::UpToDate);
    }

    let reports = apply_pending(context)?;
    // Stamped only after the run, so an upgrade that fails partway is retried
    // on the next command rather than being recorded as done.
    save_version_stamp(current)?;

    // A machine seeing this for the first time has nothing to report: either it
    // is a fresh install, or its migrations were already applied by an explicit
    // `upgrade`. Announcing an upgrade "from" a version we never saw would be
    // an invention.
    let Some(previous) = previous else {
        return Ok(AutoUpgrade::UpToDate);
    };
    Ok(AutoUpgrade::Upgraded {
        from: previous,
        to: current.to_string(),
        reports,
    })
}

// ---------------------------------------------------------------------------
// 0001 — the `git lineage` to `tribal` rename
// ---------------------------------------------------------------------------

/// The config directory as it was named before the rename.
fn legacy_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var(crate::auth::CONFIG_DIR_ENV) {
        if !dir.is_empty() {
            // An explicit override names one directory; there is no legacy
            // counterpart to move from, and guessing one would move state the
            // user deliberately placed.
            return None;
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("lineage"));
        }
    }
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config").join("lineage"))
}

fn detect_config_dir(_: &Context) -> Result<bool> {
    let Some(old) = legacy_config_dir() else {
        return Ok(false);
    };
    Ok(old.is_dir() && !crate::auth::config_dir()?.is_dir())
}

/// Move `~/.config/lineage` to `~/.config/tribal`.
///
/// Runs first of all steps because the applied-step record lives in this
/// directory: moving it afterwards would strand the record written by earlier
/// steps at the old path.
///
/// The three cases of a rename are spelled out rather than inferred. "Both
/// exist" keeps the new directory and leaves the old one in place — the user
/// has state at both paths, and picking one to delete is not a decision a
/// migration should make silently.
fn apply_config_dir(_: &Context) -> Result<Outcome> {
    let Some(old) = legacy_config_dir() else {
        return Ok(Outcome::Skipped("config directory is overridden".into()));
    };
    let new = crate::auth::config_dir()?;

    if !old.is_dir() {
        return Ok(Outcome::Skipped("no config directory to move".into()));
    }
    if new.is_dir() {
        return Ok(Outcome::Skipped(format!(
            "{} already exists; left {} in place",
            new.display(),
            old.display()
        )));
    }

    if let Some(parent) = new.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&old, &new)?;
    Ok(Outcome::Applied(format!(
        "moved {} to {}",
        old.display(),
        new.display()
    )))
}

fn repository_count(count: usize) -> String {
    if count == 1 {
        return "1 repository".into();
    }
    format!("{count} repositories")
}

const LEGACY_SERVER_REPO_ID_KEY: &str = "lineage.serverRepoId";

fn detect_git_config(context: &Context) -> Result<bool> {
    Ok(repos_to_stamp(context)
        .iter()
        .any(|path| read_legacy_repo_id(path).is_some()))
}

fn read_legacy_repo_id(repo_path: &Path) -> Option<String> {
    let repo = lineage_git::open_repo(repo_path).ok()?;
    lineage_git::read_git_config(repo.inner(), LEGACY_SERVER_REPO_ID_KEY)
}

/// Rewrite `lineage.serverRepoId` to `tribal.serverRepoId`.
///
/// The value is a server-issued binding cached per checkout, so it is copied
/// key-to-key rather than re-fetched. The old key is removed once the new one
/// holds the value; leaving both would let a later reader pick the stale one.
fn apply_git_config(context: &Context) -> Result<Outcome> {
    let mut moved = Vec::new();

    for path in repos_to_stamp(context) {
        let Some(value) = read_legacy_repo_id(&path) else {
            continue;
        };
        let repo = lineage_git::open_repo(&path)?;
        if lineage_git::read_git_config(repo.inner(), lineage_git::SERVER_REPO_ID_KEY).is_none() {
            lineage_git::write_git_config(repo.inner(), lineage_git::SERVER_REPO_ID_KEY, &value)?;
        }
        lineage_git::remove_git_config(repo.inner(), LEGACY_SERVER_REPO_ID_KEY)?;
        moved.push(path.display().to_string());
    }

    if moved.is_empty() {
        return Ok(Outcome::Skipped("no repository carried the old key".into()));
    }
    Ok(Outcome::Applied(format!(
        "moved the server repo id key in {}",
        repository_count(moved.len())
    )))
}

fn hook_is_stale(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    // Only a hook this CLI wrote is ours to rewrite, and only one still naming
    // the old binary needs it.
    content.contains("Lineage pre-commit hook") || content.contains("Lineage post-commit hook")
}

fn stale_hooks(repo_path: &Path) -> Vec<PathBuf> {
    let hooks = repo_path.join(".git").join("hooks");
    ["pre-commit", "post-commit"]
        .iter()
        .map(|name| hooks.join(name))
        .filter(|path| hook_is_stale(path) && !names_new_binary(path))
        .collect()
}

fn names_new_binary(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|content| content.contains("command -v tribal"))
        .unwrap_or(false)
}

fn detect_hooks(context: &Context) -> Result<bool> {
    Ok(repos_to_stamp(context)
        .iter()
        .any(|path| !stale_hooks(path).is_empty()))
}

/// Re-stamp installed hooks so they invoke `tribal`.
///
/// `install_hook_quiet` overwrites in place when the existing file carries the
/// Lineage marker, so a hook this CLI installed is replaced without `--force`
/// while one the user wrote themselves is left alone.
fn apply_hooks(context: &Context) -> Result<Outcome> {
    let mut stamped = 0usize;

    for path in repos_to_stamp(context) {
        if stale_hooks(&path).is_empty() {
            continue;
        }
        match crate::hooks_cmd::install_hook_quiet(&path, false) {
            Ok(()) => stamped += 1,
            // One repository whose hooks cannot be rewritten (a permission
            // problem, a hook replaced by hand) must not stop the others.
            Err(error) => tracing::warn!("could not restamp hooks in {}: {error}", path.display()),
        }
    }

    if stamped == 0 {
        return Ok(Outcome::Skipped(
            "no installed hooks needed rewriting".into(),
        ));
    }
    Ok(Outcome::Applied(format!(
        "rewrote hooks in {}",
        repository_count(stamped)
    )))
}

/// Agent skill files this CLI installed, that still name the old command.
/// Installed skills that predate the headless switch.
///
/// Detected by absence rather than by a retired string: the 0.5 skill told an
/// agent to prefer `--json`, which is still valid advice, so there is nothing
/// stale to match on. What a pre-0.6 copy lacks is any mention of
/// `--no-interactive`, and an agent reading one will open a TUI it cannot drive.
fn skills_without_headless_switch(repo_path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in [".cursor/skills", ".claude/skills", ".agents/skills"] {
        let path = repo_path.join(dir).join("lineage").join("SKILL.md");
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if !content.contains("--no-interactive") {
            out.push(path);
        }
    }
    out
}

fn detect_headless_skills(context: &Context) -> Result<bool> {
    Ok(repos_to_stamp(context)
        .iter()
        .any(|path| !skills_without_headless_switch(path).is_empty()))
}

/// Reinstall the lineage skill wherever it predates `--no-interactive`.
///
/// Forced for the same reason the rename step is: the detector has already
/// established these copies are ours and out of date.
fn apply_headless_skills(context: &Context) -> Result<Outcome> {
    let mut stamped = 0usize;

    for path in repos_to_stamp(context) {
        let stale = skills_without_headless_switch(&path);
        if stale.is_empty() {
            continue;
        }
        let targets: Vec<String> = stale
            .iter()
            .filter_map(|p| target_of(&path, p))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        match crate::skill_cmd::init_skill_quiet(&path, &targets, true) {
            Ok(()) => stamped += 1,
            Err(error) => tracing::warn!("could not restamp skills in {}: {error}", path.display()),
        }
    }

    if stamped == 0 {
        return Ok(Outcome::Skipped(
            "installed skills already document --no-interactive".into(),
        ));
    }
    Ok(Outcome::Applied(format!(
        "updated agent skills for --no-interactive in {}",
        repository_count(stamped)
    )))
}

fn stale_skills(repo_path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in [".cursor/skills", ".claude/skills", ".agents/skills"] {
        for skill in ["lineage", "share"] {
            let path = repo_path.join(dir).join(skill).join("SKILL.md");
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            if content.contains("git lineage") {
                out.push(path);
            }
        }
    }
    out
}

fn detect_skills(context: &Context) -> Result<bool> {
    Ok(repos_to_stamp(context)
        .iter()
        .any(|path| !stale_skills(path).is_empty()))
}

/// Rewrite installed skill files so agents are told the current command.
///
/// Runs last: a stale skill costs an agent one failed command, where the
/// earlier steps carry credentials and server bindings. Forced, because the
/// detector has already established these copies are ours and out of date.
fn apply_skills(context: &Context) -> Result<Outcome> {
    let mut stamped = 0usize;

    for path in repos_to_stamp(context) {
        let stale = stale_skills(&path);
        if stale.is_empty() {
            continue;
        }
        // Only the targets that actually have stale copies, so the step never
        // installs a skill into a repository that had not opted into it.
        let targets: Vec<String> = stale
            .iter()
            .filter_map(|p| target_of(&path, p))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        match crate::skill_cmd::init_skill_quiet(&path, &targets, true) {
            Ok(()) => stamped += 1,
            Err(error) => tracing::warn!("could not restamp skills in {}: {error}", path.display()),
        }
    }

    if stamped == 0 {
        return Ok(Outcome::Skipped(
            "no installed skills needed rewriting".into(),
        ));
    }
    Ok(Outcome::Applied(format!(
        "rewrote agent skills in {}",
        repository_count(stamped)
    )))
}

fn target_of(repo_path: &Path, skill_path: &Path) -> Option<String> {
    let rel = skill_path.strip_prefix(repo_path).ok()?;
    let first = rel.components().next()?.as_os_str().to_str()?;
    match first {
        ".cursor" => Some("cursor".into()),
        ".claude" => Some("claude".into()),
        ".agents" => Some("codex".into()),
        _ => None,
    }
}

/// Every migration, oldest first. Order within a migration is the order its
/// steps run in, so the riskiest work happens while the least has been changed.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        id: "0001-tribal-rename",
        introduced_in: "0.5.0",
        steps: &[
            Step {
                id: "config-dir",
                detect: detect_config_dir,
                apply: apply_config_dir,
            },
            Step {
                id: "git-config",
                detect: detect_git_config,
                apply: apply_git_config,
            },
            Step {
                id: "hooks",
                detect: detect_hooks,
                apply: apply_hooks,
            },
            Step {
                id: "skills",
                detect: detect_skills,
                apply: apply_skills,
            },
        ],
    },
    Migration {
        id: "0002-headless-switch",
        introduced_in: "0.6.0",
        steps: &[Step {
            id: "skills-headless",
            detect: detect_headless_skills,
            apply: apply_headless_skills,
        }],
    },
];
