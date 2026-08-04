//! `Conversation` -> Claude Code transcript JSONL.
//!
//! Claude Code resolves a session as
//! `~/.claude/projects/<claude_project_key(cwd)>/<sessionId>.jsonl` — no registry
//! and no server handshake — so a transcript written there is resumable even if
//! the installation has never seen the session id.
//!
//! What this deliberately does *not* do is reproduce tool state. A `tool_use`
//! block names a handle the resuming model may act as though it still holds, and
//! a `tool_result` paired to it asserts an outcome that is no longer inspectable
//! — a model reading either can conclude it already made edits it did not make.
//! Tool activity is therefore flattened into prose the model reads as history.
//! Honest narrative beats structure that lies.

use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use lineage_agent::RenderedTranscript;
use lineage_core::{Conversation, Role, Turn};
use serde_json::{json, Value};
use ulid::Ulid;

use crate::path_util::claude_project_dir;

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
        let uuid = mint_uuid();
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
    let session_id = mint_uuid();
    let path = transcript_path(home, workspace_root, &session_id);

    let mut chain = RecordChain::new(session_id.clone());
    let base_timestamp = conversation.started_at;

    for turn in &conversation.turns {
        let Some((role, text)) = narrate(turn) else {
            continue;
        };
        chain.push(role, &text, turn.timestamp.unwrap_or(base_timestamp));
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

/// One turn as a `(claude_role, text)` pair, or `None` when it carries nothing
/// worth a record. Claude only accepts `user` and `assistant`, so lineage's
/// four roles collapse onto two.
fn narrate(turn: &Turn) -> Option<(&'static str, String)> {
    let body = match turn.role {
        // A Tool turn is a tool *result*: Claude's parser re-roles the user
        // record that carried `tool_result` blocks. Replaying it as a user turn
        // verbatim would read as Alice having typed the tool's output, so it is
        // labelled as the recap it is.
        Role::Tool => tool_result_prose(turn),
        // System turns are lineage's own; attributing them to the user would be
        // a lie, and Claude has no system record type in a transcript.
        Role::System => return None,
        Role::User => turn.content.trim().to_string(),
        Role::Assistant => assistant_prose(turn),
    };

    let body = body.trim();
    if body.is_empty() {
        return None;
    }

    let role = match turn.role {
        Role::Assistant => "assistant",
        _ => "user",
    };
    Some((role, body.to_string()))
}

/// Assistant text plus a plain-language note of what it did, so the resumed
/// model knows work happened without being handed a handle to it.
fn assistant_prose(turn: &Turn) -> String {
    let mut parts = Vec::new();
    let text = turn.content.trim();
    if !text.is_empty() {
        parts.push(text.to_string());
    }

    let actions: Vec<String> = turn
        .tool_calls
        .iter()
        .filter(|call| call.name != "tool_result")
        .map(|call| format!("- {}{}", call.name, summarize_arguments(&call.arguments)))
        .collect();

    if !actions.is_empty() {
        parts.push(format!(
            "[lineage: this turn used tools, recorded here as history rather than replayable calls]\n{}",
            actions.join("\n")
        ));
    }

    parts.join("\n\n")
}

fn tool_result_prose(turn: &Turn) -> String {
    let mut parts = Vec::new();
    let text = turn.content.trim();
    if !text.is_empty() {
        parts.push(text.to_string());
    }

    for call in &turn.tool_calls {
        let Some(result) = call
            .result
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        else {
            continue;
        };
        parts.push(result.to_string());
    }

    if parts.is_empty() {
        return String::new();
    }

    format!(
        "[lineage: tool output from the original session]\n{}",
        parts.join("\n")
    )
}

/// Tool arguments are stored as a JSON string. A file path is the one detail
/// worth surfacing in prose; everything else would bloat the narrative without
/// helping the model orient.
fn summarize_arguments(arguments: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(arguments) else {
        return String::new();
    };
    let path = ["file_path", "path", "file", "command"]
        .iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_str()));
    match path {
        Some(path) => format!(" {path}"),
        None => String::new(),
    }
}

/// Claude session ids are UUIDs and the filename is the id, so the minted id
/// must look like one. A ULID is 128 bits from the same workspace dependency
/// lineage already uses for ids, formatted in UUID layout — adding a `uuid`
/// crate to mint a name would buy nothing.
///
/// The version and variant nibbles are stamped to v4 rather than left as ULID
/// bytes: every id Claude Code writes itself is a well-formed v4, so an id that
/// is merely UUID-*shaped* sits outside the set the harness has been observed to
/// accept. A ULID's leading 48 bits are a timestamp, so without this the version
/// nibble would be whatever the clock produced. Stamping costs nothing and keeps
/// the minted id inside the shape the format documents.
fn mint_uuid() -> String {
    let mut b = Ulid::new().to_bytes();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13],
        b[14], b[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{AgentKind, LineageId, ToolCall};

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

    /// Every session id Claude Code writes for itself is a well-formed v4, so a
    /// merely UUID-shaped id sits outside the set the harness is known to accept.
    /// Rendering repeatedly because the ULID timestamp bits move between calls:
    /// a single sample would pass even if the nibbles were never stamped.
    #[test]
    fn minted_ids_are_well_formed_v4_uuids() {
        for _ in 0..64 {
            let id = mint_uuid();
            let fields: Vec<&str> = id.split('-').collect();

            assert_eq!(
                fields.iter().map(|f| f.len()).collect::<Vec<_>>(),
                [8, 4, 4, 4, 12]
            );
            assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
            assert_eq!(
                fields[2].chars().next(),
                Some('4'),
                "version nibble in {id}"
            );
            assert!(
                matches!(fields[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
                "variant nibble in {id}"
            );
        }
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
            turn(Role::System, "lineage internal note"),
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
