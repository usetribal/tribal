//! Where this machine has checkouts of which repositories
//! (`~/.config/lineage/repos.json`).
//!
//! It exists for one caller: `fork <share-url>`, which is handed a repository
//! by name and has to land the session somewhere without asking. A receiver
//! who already has the repo cloned should not be made to clone it again, and
//! nothing else on the machine knows where their clones are.
//!
//! Kept beside `credentials.json` rather than in any repository, because the
//! question it answers ("where is `github.com/acme/widgets` on this box?") is
//! about the machine and is asked from a directory that may be none of them.
//!
//! Best-effort throughout: a missing, unreadable, or corrupt file is an empty
//! registry, and a failed write is a warning. The registry is a cache of a fact
//! git already holds — losing it costs a clone, never data — so no command may
//! fail because of it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use lineage_git::{normalize_remote_url, open_repo};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const REGISTRY_FILE: &str = "repos.json";

/// The remote a checkout is recorded under. Tribal identifies repositories by
/// their `origin` everywhere else (sync, pull, share), so a registry keyed on
/// anything else would not answer the question a share asks.
const REGISTRY_REMOTE: &str = "origin";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    /// Keyed by normalized remote URL (`github.com/<owner>/<name>`).
    #[serde(default)]
    pub repos: BTreeMap<String, RepoEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoEntry {
    pub path: PathBuf,
    pub last_used: DateTime<Utc>,
}

pub fn registry_path() -> Result<PathBuf> {
    Ok(crate::auth::config_dir()?.join(REGISTRY_FILE))
}

/// The registry as stored, or an empty one for anything that is not a readable
/// registry. Corruption is indistinguishable from absence on purpose: the only
/// recovery worth having is to re-record on the next command, which happens
/// anyway.
pub fn load() -> Registry {
    let Ok(path) = registry_path() else {
        return Registry::default();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return Registry::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save(registry: &Registry) -> Result<()> {
    let path = registry_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(registry)?)?;
    Ok(())
}

/// Record this checkout under its `origin`, if it is a repository with one.
///
/// Called once per invocation from the command dispatcher rather than from each
/// command, so the registry tracks "repositories lineage was used in" without
/// every new subcommand having to remember to say so.
pub fn record(repo_path: &Path, now: DateTime<Utc>) {
    let Some(url) = origin_url(repo_path) else {
        return;
    };
    let Some(workdir) = workdir_of(repo_path) else {
        return;
    };

    let mut registry = load();
    registry.repos.insert(
        url,
        RepoEntry {
            path: workdir,
            last_used: now,
        },
    );
    if let Err(error) = save(&registry) {
        tracing::warn!("repo registry write failed: {error}");
    }
}

/// The most recently used checkout of `normalized_url` that is still a git
/// repository with that origin.
pub fn lookup(normalized_url: &str) -> Option<PathBuf> {
    lookup_in(&load(), normalized_url)
}

/// The verification half, over a registry the caller supplies.
///
/// A recorded path can have been deleted, moved, or re-pointed at a different
/// remote since it was written, and landing someone else's session in a
/// directory that is no longer the named repository is worse than cloning it
/// afresh. So the entry is re-checked against git rather than trusted — the
/// registry is a hint about where to look, never an authority on what is there.
pub fn lookup_in(registry: &Registry, normalized_url: &str) -> Option<PathBuf> {
    let entry = registry.repos.get(normalized_url)?;
    if origin_url(&entry.path).as_deref() != Some(normalized_url) {
        return None;
    }
    Some(entry.path.clone())
}

/// The normalized `origin` of the repository containing `path`, or `None` when
/// there is no repository, no `origin`, or no URL on it.
pub fn origin_url(path: &Path) -> Option<String> {
    let repo = open_repo(path).ok()?;
    let remote = repo.inner().find_remote(REGISTRY_REMOTE).ok()?;
    Some(normalize_remote_url(remote.url()?))
}

fn workdir_of(path: &Path) -> Option<PathBuf> {
    let repo = open_repo(path).ok()?;
    Some(repo.workdir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_corrupt_registry_reads_as_an_empty_one_rather_than_failing() {
        assert!(serde_json::from_str::<Registry>("{ not json").is_err());
        let recovered: Registry = serde_json::from_str("{ not json").unwrap_or_default();
        assert!(recovered.repos.is_empty());
    }

    #[test]
    fn an_unknown_field_does_not_discard_the_entries_beside_it() {
        let registry: Registry = serde_json::from_str(
            r#"{"repos":{"github.com/acme/widgets":{"path":"/tmp/w","last_used":"2026-07-31T00:00:00Z","future":1}},"future":2}"#,
        )
        .unwrap_or_default();
        assert_eq!(registry.repos.len(), 1);
    }
}
