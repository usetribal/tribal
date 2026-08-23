//! Resolve a session from a lineage id, id prefix, or vendor harness id.
//!
//! Fork and resume today require the exact 26-character lineage id, but users
//! naturally copy Claude's UUID from their terminal. This module is the single
//! place that bridges the two naming schemes.

use git2::Repository;
use lineage_core::{display_title, Conversation, LineageError, LineageId};

use crate::refs::{list_session_ids, read_conversation};

const VENDOR_ID_KEYS: &[&str] = &["claude_session_id", "cursor_session_id", "codex_session_id"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCandidate {
    pub id: LineageId,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    NotFound(String),
    Ambiguous {
        message: String,
        candidates: Vec<SessionCandidate>,
    },
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NotFound(message) => write!(f, "{message}"),
            ResolveError::Ambiguous {
                message,
                candidates,
            } => {
                write!(f, "{message}")?;
                for (index, candidate) in candidates.iter().enumerate() {
                    write!(
                        f,
                        "\n  {}. {}  ({})",
                        index + 1,
                        candidate.title,
                        candidate.id
                    )?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// Resolve a user-supplied hint to a stored session id.
pub fn resolve_session(repo: &Repository, hint: &str) -> Result<LineageId, ResolveError> {
    let hint = hint.trim();
    if hint.is_empty() {
        return Err(ResolveError::NotFound(
            "session id required — `tribal list` shows what is here".into(),
        ));
    }

    let id = LineageId::from(hint);
    if read_conversation(repo, &id)
        .map_err(|e| ResolveError::NotFound(e.to_string()))?
        .is_some()
    {
        return Ok(id);
    }

    let sessions = load_sessions(repo).map_err(|e| ResolveError::NotFound(e.to_string()))?;

    // Tribal ids never contain hyphens; try prefix match before vendor ids do.
    if !hint.contains('-') {
        let matches: Vec<(&LineageId, &Conversation)> = sessions
            .iter()
            .filter(|(id, _)| id.as_str().starts_with(hint))
            .map(|(id, conv)| (id, conv))
            .collect();
        if !matches.is_empty() {
            return pick_unique(matches, hint);
        }
    }

    if hint.contains('-') {
        let normalized = normalize_vendor_hint(hint);
        let exact = is_full_vendor_uuid(hint);
        let matches: Vec<(&LineageId, &Conversation)> = sessions
            .iter()
            .filter(|(_, conv)| vendor_id_matches(conv, &normalized, exact))
            .map(|(id, conv)| (id, conv))
            .collect();
        return pick_unique(matches, hint);
    }

    Err(ResolveError::NotFound(format!(
        "no session matching '{hint}' in this repository's lineage refs. \
         `tribal list` shows what is here"
    )))
}

fn is_full_vendor_uuid(hint: &str) -> bool {
    hint.len() == 36 && hint.matches('-').count() == 4
}

pub fn load_sessions(repo: &Repository) -> Result<Vec<(LineageId, Conversation)>, LineageError> {
    let mut sessions = Vec::new();
    for id in list_session_ids(repo)? {
        if let Some(conv) = read_conversation(repo, &id)? {
            sessions.push((id, conv));
        }
    }
    Ok(sessions)
}

fn normalize_vendor_hint(hint: &str) -> String {
    hint.trim().to_ascii_lowercase()
}

fn vendor_id_matches(conv: &Conversation, normalized_hint: &str, exact: bool) -> bool {
    vendor_session_id(conv).is_some_and(|vendor_id| {
        let normalized = vendor_id.to_ascii_lowercase();
        if exact {
            normalized == normalized_hint
        } else {
            normalized == normalized_hint || normalized.starts_with(normalized_hint)
        }
    })
}

fn vendor_session_id(conv: &Conversation) -> Option<String> {
    for key in VENDOR_ID_KEYS {
        if let Some(value) = conv.metadata.get(*key).and_then(|v| v.as_str()) {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn pick_unique(
    matches: Vec<(&LineageId, &Conversation)>,
    hint: &str,
) -> Result<LineageId, ResolveError> {
    match matches.len() {
        0 => Err(ResolveError::NotFound(format!(
            "no session matching '{hint}' in this repository's lineage refs. \
             `tribal list` shows what is here"
        ))),
        1 => Ok(matches[0].0.clone()),
        _ => Err(ResolveError::Ambiguous {
            message: format!("'{hint}' matches {} sessions — pick one:", matches.len()),
            candidates: matches
                .into_iter()
                .map(|(id, conv)| SessionCandidate {
                    id: id.clone(),
                    title: display_title(conv),
                })
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use lineage_core::{AgentKind, Role, Turn};
    use tempfile::TempDir;

    use crate::{open_repo, persist_conversation};

    fn init_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    fn seed_session(dir: &TempDir, vendor_id: &str, summary: &str) -> LineageId {
        let repo = open_repo(dir.path()).unwrap();
        let mut conv = Conversation::new(AgentKind::Claude, dir.path().display().to_string());
        conv.metadata.insert(
            "claude_session_id".into(),
            serde_json::Value::String(vendor_id.into()),
        );
        conv.metadata.insert(
            lineage_core::SESSION_SUMMARY_KEY.into(),
            serde_json::Value::String(summary.into()),
        );
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: "hello".into(),
            tool_calls: vec![],
            model: None,
            timestamp: Some(Utc::now()),
            artifacts: vec![],
        });
        persist_conversation(repo.inner(), &conv).unwrap();
        conv.id
    }

    #[test]
    fn resolves_full_lineage_id() {
        let dir = init_repo();
        let id = seed_session(
            &dir,
            "550e8400-e29b-41d4-a716-446655440000",
            "Auth middleware",
        );
        let repo = open_repo(dir.path()).unwrap();
        assert_eq!(resolve_session(repo.inner(), id.as_str()).unwrap(), id);
    }

    #[test]
    fn resolves_lineage_id_prefix() {
        let dir = init_repo();
        let id = seed_session(
            &dir,
            "550e8400-e29b-41d4-a716-446655440000",
            "Auth middleware",
        );
        let repo = open_repo(dir.path()).unwrap();
        let prefix = &id.as_str()[..8];
        assert_eq!(resolve_session(repo.inner(), prefix).unwrap(), id);
    }

    #[test]
    fn resolves_claude_vendor_uuid() {
        let dir = init_repo();
        let vendor = "019fa49d-f719-43f8-8d60-e2c9d4cdab31";
        let id = seed_session(&dir, vendor, "Lineage RLS audit");
        let repo = open_repo(dir.path()).unwrap();
        assert_eq!(resolve_session(repo.inner(), vendor).unwrap(), id);
    }

    #[test]
    fn resolves_claude_vendor_uuid_prefix() {
        let dir = init_repo();
        let vendor = "019fa49d-f719-43f8-8d60-e2c9d4cdab31";
        let id = seed_session(&dir, vendor, "Lineage RLS audit");
        let repo = open_repo(dir.path()).unwrap();
        assert_eq!(resolve_session(repo.inner(), "019fa49d-f719").unwrap(), id);
    }

    #[test]
    fn ambiguous_vendor_prefix_lists_candidates() {
        let dir = init_repo();
        seed_session(
            &dir,
            "019fa49d-f719-43f8-8d60-e2c9d4cdab31",
            "Lineage RLS audit",
        );
        seed_session(
            &dir,
            "019fa49d-f719-aaaa-bbbb-cccc-ddddeeeeffff",
            "OSS and server reconciliation",
        );
        let repo = open_repo(dir.path()).unwrap();
        let err = resolve_session(repo.inner(), "019fa49d-f719").unwrap_err();
        assert!(matches!(err, ResolveError::Ambiguous { .. }));
    }
}
