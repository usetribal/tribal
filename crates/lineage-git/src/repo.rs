use std::path::{Path, PathBuf};

use git2::Repository;
use lineage_core::{workspace_root_for, LineageError, RepoPaths};

pub struct LineageRepo {
    repo: Repository,
    workdir: PathBuf,
}

impl LineageRepo {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, LineageError> {
        let repo = Repository::open(path.as_ref())
            .map_err(|e| LineageError::Other(format!("failed to open repo: {e}")))?;
        let workdir = repo
            .workdir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.as_ref().to_path_buf());
        Ok(Self { repo, workdir })
    }

    pub fn discover(start: impl AsRef<Path>) -> Result<Self, LineageError> {
        let repo = Repository::discover(start.as_ref())
            .map_err(|e| LineageError::Other(format!("not a git repository: {e}")))?;
        let workdir = repo
            .workdir()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| start.as_ref().to_path_buf());
        Ok(Self { repo, workdir })
    }

    pub fn inner(&self) -> &Repository {
        &self.repo
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    pub fn git_dir(&self) -> PathBuf {
        self.repo.path().to_path_buf()
    }

    pub fn lineage_cache_dir(&self) -> PathBuf {
        self.git_dir().join("lineage")
    }
}

/// Path normalization context for `repo`: its workdir plus the repo-relative
/// location of every linked worktree.
///
/// This is the only place that turns git's worktree registry into the prefixes
/// [`RepoPaths`] strips, so repository layout stays owned here and `lineage-core`
/// does pure string work. Reading the registry rather than matching a directory
/// name is what keeps the rewrite honest: a path is only rewritten when git
/// itself says a worktree lives there.
///
/// Resolve this **once per operation** and rebase it per session with
/// [`RepoPaths::with_workspace_root`] — the registry read is far more expensive
/// than the string comparison it feeds, and the prefixes do not vary by session.
pub fn repo_paths(repo: &Repository) -> RepoPaths {
    RepoPaths::new(repo.workdir(), worktree_prefixes(repo))
}

/// [`repo_paths`] already rebased onto one conversation's workspace root, for
/// callers handling a single session.
pub fn repo_paths_for_conversation(repo: &Repository, conversation_root: &str) -> RepoPaths {
    let workspace = workspace_root_for(conversation_root, repo.workdir());
    repo_paths(repo).with_workspace_root(&workspace)
}

/// Each linked worktree's location relative to the main workdir.
///
/// Only worktrees nested inside the workdir yield a prefix: those are the ones
/// whose paths can appear in a session as `.claude/worktrees/x/AGENTS.md`. A
/// sibling worktree sits outside the repo-relative namespace entirely, so no
/// prefix could strip it and it contributes nothing. A registered worktree whose
/// directory has since been removed still counts — the sessions recorded in it
/// are stored, and their paths must keep resolving.
fn worktree_prefixes(repo: &Repository) -> Vec<String> {
    let Some(workdir) = repo.workdir() else {
        return Vec::new();
    };
    let Ok(names) = repo.worktrees() else {
        return Vec::new();
    };
    names
        .iter()
        .flatten()
        .filter_map(|name| repo.find_worktree(name).ok())
        .filter_map(|worktree| prefix_under(worktree.path(), workdir))
        .collect()
}

/// `worktree_root` expressed relative to `workdir`, or `None` when it does not
/// sit underneath it. Both sides are canonicalized where possible so a symlinked
/// workdir (`/tmp` on macOS) does not hide the containment.
fn prefix_under(worktree_root: &Path, workdir: &Path) -> Option<String> {
    let root = canonical_display(worktree_root);
    let base = format!("{}/", canonical_display(workdir).trim_end_matches('/'));
    let rest = root.strip_prefix(&base)?.trim_matches('/');
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_string())
}

fn canonical_display(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
        .replace('\\', "/")
}

pub fn open_repo(path: impl AsRef<Path>) -> Result<LineageRepo, LineageError> {
    LineageRepo::discover(path)
}

pub fn find_repo(start: impl AsRef<Path>) -> Option<LineageRepo> {
    LineageRepo::discover(start).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn discover_and_cache_dir() {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let sub = dir.path().join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        let repo = find_repo(&sub).expect("repo");
        assert!(repo.git_dir().ends_with(".git"));
        assert!(repo.lineage_cache_dir().ends_with("lineage"));
    }
}
