use git2::Repository;
use lineage_core::{Conversation, LineageError};
use serde_json::Value;

use crate::refs::read_conversation;

pub const PROMPTED_BY_EMAIL: &str = "prompted_by_email";
pub const PROMPTED_BY_NAME: &str = "prompted_by_name";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitIdentity {
    pub name: Option<String>,
    pub email: Option<String>,
}

pub fn repo_git_identity(repo: &Repository) -> GitIdentity {
    let Ok(config) = repo.config() else {
        return GitIdentity::default();
    };
    GitIdentity {
        name: config.get_string("user.name").ok().filter(|s| !s.is_empty()),
        email: config
            .get_string("user.email")
            .ok()
            .filter(|s| !s.is_empty()),
    }
}

/// Records who prompted the session using git `user.email` / `user.name`.
///
/// Preserves existing values on re-ingest so the original author is kept when
/// sessions are refreshed or ingested by someone else.
pub fn stamp_prompted_by(repo: &Repository, conversation: &mut Conversation) -> Result<(), LineageError> {
    if conversation.metadata.contains_key(PROMPTED_BY_EMAIL) {
        return Ok(());
    }

    if let Some(existing) = read_conversation(repo, &conversation.id)? {
        if let Some(value) = existing.metadata.get(PROMPTED_BY_EMAIL) {
            conversation.metadata.insert(PROMPTED_BY_EMAIL.into(), value.clone());
        }
        if let Some(value) = existing.metadata.get(PROMPTED_BY_NAME) {
            conversation.metadata.insert(PROMPTED_BY_NAME.into(), value.clone());
        }
        if conversation.metadata.contains_key(PROMPTED_BY_EMAIL) {
            return Ok(());
        }
    }

    let identity = repo_git_identity(repo);
    if let Some(email) = identity.email {
        conversation.metadata.insert(
            PROMPTED_BY_EMAIL.into(),
            Value::String(email),
        );
    }
    if let Some(name) = identity.name {
        conversation.metadata.insert(PROMPTED_BY_NAME.into(), Value::String(name));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{AgentKind, Conversation};
    use std::process::Command;

    fn test_repo(email: &str, name: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", email])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", name])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn reads_repo_git_identity() {
        let dir = test_repo("alice@team.dev", "Alice");
        let repo = Repository::open(dir.path()).unwrap();
        let id = repo_git_identity(&repo);
        assert_eq!(id.email.as_deref(), Some("alice@team.dev"));
        assert_eq!(id.name.as_deref(), Some("Alice"));
    }

    #[test]
    fn stamp_prompted_by_uses_git_config() {
        let dir = test_repo("bob@team.dev", "Bob");
        let repo = Repository::open(dir.path()).unwrap();
        let mut conv = Conversation::new(AgentKind::Cursor, dir.path().display().to_string());
        stamp_prompted_by(&repo, &mut conv).unwrap();
        assert_eq!(
            conv.metadata
                .get(PROMPTED_BY_EMAIL)
                .and_then(|v| v.as_str()),
            Some("bob@team.dev")
        );
        assert_eq!(
            conv.metadata.get(PROMPTED_BY_NAME).and_then(|v| v.as_str()),
            Some("Bob")
        );
    }
}
