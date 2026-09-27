//! `Conversation` -> Claude Code transcript JSONL.
//!
//! Claude Code resolves a session as
//! `~/.claude/projects/<claude_project_key(cwd)>/<sessionId>.jsonl` — no registry
//! and no server handshake — so a transcript written there is resumable even if
//! the installation has never seen the session id.
//!
//! Tool activity is narrated as prose rather than replayed as `tool_use` blocks;
//! `transcript_writing` holds that rule and the reasons for it.

use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use lineage_agent::RenderedTranscript;
use lineage_core::Conversation;
use serde_json::json;

use crate::path_util::claude_project_dir;
use crate::transcript_writing::{mint_uuid, narrate};

/// Every session id Claude Code writes for itself is a well-formed v4, so an id
/// that is merely UUID-shaped sits outside the set the harness is known to accept.
const SESSION_ID_VERSION: u8 = 4;

/// Claude resolves records by walking `parentUuid` back from the last line and
/// drops anything unreachable, so every record carries the previous record's
/// `uuid` and the first carries `null`.
struct RecordChain {
    session_id: String,
    parent_uuid: Option<String>,
    lines: Vec<String>,
}

impl RecordChain {
    fn new(session_id: String) -> Self {
        Self {
            session_id,
            parent_uuid: None,
            lines: Vec::new(),
        }
    }

    fn push(&mut self, role: &str, text: &str, timestamp: DateTime<Utc>) {
        let uuid = mint_uuid(SESSION_ID_VERSION);
        let record = json!({
            "parentUuid": self.parent_uuid,
            "uuid": uuid,
            "sessionId": self.session_id,
            "timestamp": timestamp.to_rfc3339_opts(SecondsFormat::Millis, true),
            "type": role,
            "message": {
                "role": role,
                "content": [{ "type": "text", "text": text }],
            },
        });
        self.lines.push(record.to_string());
        self.parent_uuid = Some(uuid);
    }

    fn into_jsonl(self) -> String {
        let mut out = self.lines.join("\n");
        out.push('\n');
        out
    }
}

/// Renders `conversation` as a resumable Claude transcript under `home`.
///
/// `workspace_root` is the cwd the resumed session will be launched from: the
/// project key is derived from it, so a transcript written for one workspace is
/// invisible from another.
pub fn render_claude_transcript(
    conversation: &Conversation,
    home: &Path,
    workspace_root: &Path,
) -> RenderedTranscript {
    let session_id = mint_uuid(SESSION_ID_VERSION);
    let path = transcript_path(home, workspace_root, &session_id);

    let mut chain = RecordChain::new(session_id.clone());
    let base_timestamp = conversation.started_at;

    for turn in &conversation.turns {
        for (speaker, text) in narrate(turn) {
            chain.push(
                speaker.as_str(),
                &text,
                turn.timestamp.unwrap_or(base_timestamp),
            );
        }
    }

    RenderedTranscript {
        path,
        // `--fork-session` is deliberately absent. It exists so Claude mints a
        // new id instead of writing back into the source file, and the fork has
        // already happened here: this transcript is a fresh id in a fresh file,
        // so the source is untouchable regardless of the flag.
        resume_command: format!("claude --resume {session_id}"),
        resume_cwd: workspace_root.to_path_buf(),
        contents: chain.into_jsonl(),
        session_handle: session_id,
    }
}

pub fn transcript_path(home: &Path, workspace_root: &Path, session_id: &str) -> PathBuf {
    claude_project_dir(home, workspace_root).join(format!("{session_id}.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{AgentKind, LineageId, Role, ToolCall, Turn};
    use serde_json::Value;

    fn turn(role: Role, content: &str) -> Turn {
        Turn {
            id: LineageId::new(),
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            model: None,
            timestamp: Some(Utc::now()),
            artifacts: Vec::new(),
        }
    }

    fn conversation(turns: Vec<Turn>) -> Conversation {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/workspace");
        conv.turns = turns;
        conv
    }

    fn records(rendered: &RenderedTranscript) -> Vec<Value> {
        rendered
            .contents
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("record is valid json"))
            .collect()
    }

    #[test]
    fn parent_uuid_chains_from_null_through_every_record() {
        let conv = conversation(vec![
            turn(Role::User, "fix the auth bug"),
            turn(Role::Assistant, "found it"),
            turn(Role::User, "ship it"),
        ]);
        let rendered =
            render_claude_transcript(&conv, Path::new("/home/bob"), Path::new("/tmp/workspace"));
        let records = records(&rendered);

        assert_eq!(records.len(), 3);
        assert!(records[0]["parentUuid"].is_null());
        for pair in records.windows(2) {
            assert_eq!(pair[1]["parentUuid"], pair[0]["uuid"]);
        }
    }

    #[test]
    fn every_record_carries_the_required_fields() {
        let conv = conversation(vec![turn(Role::User, "hello"), turn(Role::Assistant, "hi")]);
        let rendered =
            render_claude_transcript(&conv, Path::new("/home/bob"), Path::new("/tmp/workspace"));

        for record in records(&rendered) {
            assert!(record.get("uuid").and_then(|v| v.as_str()).is_some());
            assert!(record.get("timestamp").and_then(|v| v.as_str()).is_some());
            assert_eq!(record["sessionId"], rendered.session_handle.as_str());
            assert!(record.get("parentUuid").is_some());
            assert!(record["message"]["role"].is_string());
            assert!(record["message"]["content"].is_array());
        }
    }

    #[test]
    fn session_id_is_freshly_minted_every_render() {
        let conv = conversation(vec![turn(Role::User, "hello")]);
        let first =
            render_claude_transcript(&conv, Path::new("/home/bob"), Path::new("/tmp/workspace"));
        let second =
            render_claude_transcript(&conv, Path::new("/home/bob"), Path::new("/tmp/workspace"));

        assert_ne!(first.session_handle, second.session_handle);
        // The vendor id of the source session must never be reused: two users
        // on one machine would collide.
        assert_ne!(first.session_handle, conv.id.as_str());
        assert_eq!(first.session_handle.len(), 36);
    }

    #[test]
    fn the_session_id_is_a_v4_uuid() {
        let conv = conversation(vec![turn(Role::User, "hello")]);
        let rendered =
            render_claude_transcript(&conv, Path::new("/home/bob"), Path::new("/tmp/workspace"));

        assert_eq!(rendered.session_handle.chars().nth(14), Some('4'));
    }

    #[test]
    fn path_is_the_project_key_directory_and_the_minted_id() {
        let conv = conversation(vec![turn(Role::User, "hello")]);
        let rendered =
            render_claude_transcript(&conv, Path::new("/home/bob"), Path::new("/tmp/workspace"));

        assert_eq!(
            rendered.path.file_name().unwrap().to_str().unwrap(),
            format!("{}.jsonl", rendered.session_handle)
        );
        assert!(rendered.path.starts_with("/home/bob/.claude/projects/"));
    }

    /// The caller prints this verbatim, so the id in it has to be the id the
    /// file was written under — a mismatch resolves to nothing with no error.
    #[test]
    fn the_resume_command_names_the_minted_handle_and_the_workspace() {
        let conv = conversation(vec![turn(Role::User, "hello")]);
        let rendered =
            render_claude_transcript(&conv, Path::new("/home/bob"), Path::new("/tmp/workspace"));

        assert_eq!(
            rendered.resume_command,
            format!("claude --resume {}", rendered.session_handle)
        );
        // Claude derives the project key from the launch directory, so running
        // the command anywhere else finds nothing.
        assert_eq!(rendered.resume_cwd, Path::new("/tmp/workspace"));
    }

    #[test]
    fn tool_role_turns_flatten_to_prose_without_tool_blocks() {
        let mut tool_turn = turn(Role::Tool, "");
        tool_turn.tool_calls.push(ToolCall {
            id: "tu-1".into(),
            name: "tool_result".into(),
            arguments: String::new(),
            result: Some("pub mod auth;".into()),
            target: None,
        });
        let conv = conversation(vec![turn(Role::User, "read auth.rs"), tool_turn]);
        let rendered =
            render_claude_transcript(&conv, Path::new("/home/bob"), Path::new("/tmp/workspace"));

        assert!(!rendered.contents.contains("tool_use"));
        assert!(!rendered.contents.contains("tool_result"));
        assert!(!rendered.contents.contains("tool_use_id"));

        let records = records(&rendered);
        assert_eq!(records.len(), 2);
        // A tool result is not something the user said, so it is labelled.
        assert_eq!(records[1]["type"], "user");
        let text = records[1]["message"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("tool output from the original session"));
        assert!(text.contains("pub mod auth;"));
    }

    #[test]
    fn assistant_tool_calls_become_narrated_history() {
        let mut assistant = turn(Role::Assistant, "Let me read the auth module.");
        assistant.tool_calls.push(ToolCall {
            id: "tu-1".into(),
            name: "Read".into(),
            arguments: r#"{"file_path":"src/auth.rs"}"#.into(),
            result: None,
            target: None,
        });
        let conv = conversation(vec![assistant]);
        let rendered =
            render_claude_transcript(&conv, Path::new("/home/bob"), Path::new("/tmp/workspace"));

        let records = records(&rendered);
        let text = records[0]["message"]["content"][0]["text"]
            .as_str()
            .unwrap();
        assert!(text.contains("Let me read the auth module."));
        assert!(text.contains("Read src/auth.rs"));
        assert!(!rendered.contents.contains("tool_use"));
    }

    #[test]
    fn empty_and_system_turns_are_dropped_so_the_chain_stays_walkable() {
        let conv = conversation(vec![
            turn(Role::User, "hello"),
            turn(Role::System, "tribal internal note"),
            turn(Role::Assistant, "   "),
            turn(Role::Assistant, "hi"),
        ]);
        let rendered =
            render_claude_transcript(&conv, Path::new("/home/bob"), Path::new("/tmp/workspace"));
        let records = records(&rendered);

        assert_eq!(records.len(), 2);
        assert!(records[0]["parentUuid"].is_null());
        assert_eq!(records[1]["parentUuid"], records[0]["uuid"]);
    }
}
