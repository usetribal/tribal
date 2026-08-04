use sha2::{Digest, Sha256};
use ulid::Ulid;

use crate::AgentKind;

#[derive(
    Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct LineageId(String);

impl LineageId {
    pub fn new() -> Self {
        Self(Ulid::new().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for LineageId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for LineageId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl Default for LineageId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for LineageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Stable line-object ID from conversation turn, path, range, and commit.
pub fn derive_line_object_id(
    conversation_id: &LineageId,
    turn_id: &LineageId,
    path: &str,
    line_range: [u32; 2],
    commit_sha: &str,
) -> LineageId {
    let mut hasher = Sha256::new();
    hasher.update(b"line-object:");
    hasher.update(conversation_id.as_str().as_bytes());
    hasher.update(b":");
    hasher.update(turn_id.as_str().as_bytes());
    hasher.update(b":");
    hasher.update(path.as_bytes());
    hasher.update(b":");
    hasher.update(line_range[0].to_string().as_bytes());
    hasher.update(b"-");
    hasher.update(line_range[1].to_string().as_bytes());
    hasher.update(b":");
    hasher.update(commit_sha.as_bytes());
    let hash = hasher.finalize();
    LineageId::from(format!("{:x}", hash)[..26].to_string())
}

/// Domain separator, versioned so a future keying change is a new namespace
/// rather than a silent re-identification of every stored session.
const SESSION_KEY_DOMAIN: &str = "session-v2";

/// Stable session ID from the agent and its own identifier for the session.
///
/// This is the only place in lineage that decides a session key. It is pure over
/// the two arguments: nothing here reads the filesystem, the clock, or the
/// repository, because every one of those varies between two machines observing
/// the same session and would make the id unmergeable.
///
/// `vendor_token` is opaque. Adapters supply it (`SessionSource::session_token`)
/// and are the only code that knows how a vendor names a session; this function
/// never parses it, orders by it, or extracts meaning from it — it only needs
/// the token to be the same bytes wherever the same session is observed.
///
/// Deliberately absent: the transcript's path and mtime. Both were inputs before
/// `session-v2`. Vendors append to a transcript as the session runs, so mtime
/// moved on every write and each import minted a new id for a session already
/// stored; the path additionally encodes the home directory and workspace
/// location, so two people — or two git worktrees — never agreed on an id.
pub fn derive_session_id(agent: AgentKind, vendor_token: &str) -> LineageId {
    let mut hasher = Sha256::new();
    hasher.update(SESSION_KEY_DOMAIN.as_bytes());
    hasher.update(b":");
    hasher.update(agent.as_str().as_bytes());
    hasher.update(b":");
    hasher.update(vendor_token.as_bytes());
    let hash = hasher.finalize();
    LineageId::from(format!("{:x}", hash)[..26].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "15c95612-890b-4122-ab66-d5cf77840e26";

    #[test]
    fn derive_session_id_is_stable() {
        let a = derive_session_id(AgentKind::Cursor, TOKEN);
        let b = derive_session_id(AgentKind::Cursor, TOKEN);
        assert_eq!(a, b);
    }

    /// The bug `session-v2` exists to fix: a session being appended to must keep
    /// its id. The key takes nothing that grows with the transcript, so this
    /// holds by construction — the test pins it against a later "just add a
    /// timestamp for uniqueness" change.
    #[test]
    fn id_survives_the_session_being_appended_to() {
        let while_running = derive_session_id(AgentKind::Claude, TOKEN);
        let after_more_turns = derive_session_id(AgentKind::Claude, TOKEN);
        assert_eq!(while_running, after_more_turns);
    }

    /// Two machines observing one session agree. Nothing machine-local reaches
    /// the hash, so there is no argument to vary here — which is the property.
    #[test]
    fn two_machines_agree_on_one_session() {
        let alice = derive_session_id(AgentKind::Claude, TOKEN);
        let bob = derive_session_id(AgentKind::Claude, TOKEN);
        assert_eq!(alice, bob);
    }

    /// Subagent transcripts carry their parent's in-file `sessionId`, so the
    /// token an adapter supplies must be per-transcript. Distinct tokens must
    /// stay distinct or a parent would absorb its subagents.
    #[test]
    fn distinct_tokens_stay_distinct() {
        let parent = derive_session_id(AgentKind::Claude, TOKEN);
        let subagent = derive_session_id(AgentKind::Claude, "agent-a0b86d5bd2a4fb581");
        assert_ne!(parent, subagent);
    }

    #[test]
    fn agent_namespaces_the_token() {
        let claude = derive_session_id(AgentKind::Claude, TOKEN);
        let cursor = derive_session_id(AgentKind::Cursor, TOKEN);
        assert_ne!(claude, cursor);
    }

    #[test]
    fn id_is_a_26_char_hex_string() {
        let id = derive_session_id(AgentKind::Codex, TOKEN);
        assert_eq!(id.as_str().len(), 26);
        assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }
}
