use std::path::{Path, PathBuf};

use git2::Repository;
use lineage_core::LineageError;

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
