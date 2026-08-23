use std::fs;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use chrono::Utc;
use lineage_core::LineageId;
use lineage_git::{link_recent_sessions_to_head, open_repo};
use lineage_search::LineageIndex;

use crate::events::{EventLog, Outcome};
use crate::ui;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const PRE_COMMIT_HOOK: &str = include_str!("../assets/hooks/pre-commit");
const POST_COMMIT_HOOK: &str = include_str!("../assets/hooks/post-commit");

pub fn install_hook(repo_path: &Path, force: bool) -> Result<()> {
    install_hook_impl(repo_path, force, true)
}

pub(crate) fn install_hook_quiet(repo_path: &Path, force: bool) -> Result<()> {
    install_hook_impl(repo_path, force, false)
}

fn install_hook_impl(repo_path: &Path, force: bool, verbose: bool) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let hooks_dir = repo.git_dir().join("hooks");
    fs::create_dir_all(&hooks_dir)?;

    install_one(&hooks_dir.join("pre-commit"), PRE_COMMIT_HOOK, force)?;
    install_one(&hooks_dir.join("post-commit"), POST_COMMIT_HOOK, force)?;

    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "install_hook",
        Outcome::Ok,
        serde_json::json!({ "hooks": ["pre-commit", "post-commit"], "forced": force }),
    );

    if verbose {
        ui::action("installed lineage hooks:");
        ui::row("pre-commit", "(import agent sessions)");
        ui::row("post-commit", "(link sessions to new commit)");
    }
    Ok(())
}

pub fn uninstall_hook(repo_path: &Path) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let hooks_dir = repo.git_dir().join("hooks");

    for name in ["pre-commit", "post-commit"] {
        let path = hooks_dir.join(name);
        if !path.exists() {
            continue;
        }
        let content = fs::read_to_string(&path)?;
        if content.contains("Lineage pre-commit hook")
            || content.contains("Lineage post-commit hook")
        {
            fs::remove_file(&path)?;
            ui::action(format!("removed {name}"));
        } else {
            ui::action(format!("skipped {name} (not installed by lineage)"));
        }
    }
    Ok(())
}

pub fn post_commit(repo_path: &Path) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let report = link_recent_sessions_to_head(repo.inner())?;
    if !report.linked.is_empty() {
        eprintln!("lineage: linked {} session(s) to HEAD", report.linked.len());
    }

    let head_sha = repo
        .inner()
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id().to_string());

    // Linking just materialized line objects at HEAD; mirror them so
    // `context chain` sees this commit without waiting for a full rebuild, and
    // record the session↔commit edges so `sessions-for-commit` resolves the
    // commit that was just made.
    if !report.linked.is_empty() {
        let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
        let linked_ids: Vec<LineageId> =
            report.linked.iter().map(|s| s.session_id.clone()).collect();
        index.populate_line_tables_for_sessions(repo.inner(), &linked_ids)?;
        if let Some(sha) = &head_sha {
            for id in &linked_ids {
                index.link_session_commit(id.as_str(), sha)?;
            }
        }
    }

    let sessions: Vec<serde_json::Value> = report
        .linked
        .iter()
        .map(|s| {
            serde_json::json!({
                "session_id": s.session_id.as_str(),
                "line_objects": s.line_objects,
                "basis": s.basis.as_str(),
            })
        })
        .collect();
    let skipped: Vec<&str> = report
        .skipped_no_overlap
        .iter()
        .map(|id| id.as_str())
        .collect();
    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "link",
        Outcome::Ok,
        serde_json::json!({
            "commit_sha": head_sha,
            "sessions": sessions,
            "skipped_no_overlap": skipped,
            "trigger": "post_commit",
        }),
    );
    Ok(())
}

fn install_one(path: &Path, content: &str, force: bool) -> Result<()> {
    if path.exists() && !force {
        let existing = fs::read_to_string(path)?;
        if !existing.contains("Lineage pre-commit hook")
            && !existing.contains("Lineage post-commit hook")
        {
            return Err(format!(
                "hook already exists at {} (use --force to overwrite)",
                path.display()
            )
            .into());
        }
    }
    fs::write(path, content)?;
    set_executable(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}
