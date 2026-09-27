//! What every transcript writer shares: turning turns into narrated records, and
//! minting the vendor session id a written transcript is filed under.
//!
//! What a written transcript deliberately does *not* do is reproduce tool state.
//! A tool call names a handle the resuming model may act as though it still holds,
//! and a result paired to it asserts an outcome that is no longer inspectable — a
//! model reading either can conclude it already made edits it did not make. Tool
//! activity is therefore flattened into prose the model reads as history. Honest
//! narrative beats structure that lies.

use lineage_core::{Role, ToolCall, Turn};
use serde_json::Value;
use ulid::Ulid;

/// The name the Claude adapter gives the entry that answers a call. Such an entry
/// is a result, not a call, so it is never narrated as something the agent did.
const ANSWER_CALL_NAME: &str = "tool_result";

const TOOLS_USED_NOTE: &str =
    "[tribal: this turn used tools, recorded here as history rather than replayable calls]";
const TOOL_OUTPUT_NOTE: &str = "[tribal: tool output from the original session]";

/// Who a narrated record is attributed to. Harnesses accept only a user and an
/// assistant in a resumable transcript, so tribal's four roles collapse onto two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Speaker {
    User,
    Assistant,
}

impl Speaker {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Speaker::User => "user",
            Speaker::Assistant => "assistant",
        }
    }
}

/// The records one turn becomes, in order. Empty when it carries nothing worth
/// replaying.
pub(crate) fn narrate(turn: &Turn) -> Vec<(Speaker, String)> {
    let records = match turn.role {
        // System turns are tribal's own; attributing them to the user would be a
        // lie, and no harness has a system record in a resumable transcript.
        Role::System => Vec::new(),
        Role::User => vec![(Speaker::User, turn.content.trim().to_string())],
        Role::Assistant => vec![(Speaker::Assistant, assistant_prose(turn))],
        Role::Tool => tool_turn_prose(turn),
    };
    records
        .into_iter()
        .map(|(speaker, body)| (speaker, body.trim().to_string()))
        .filter(|(_, body)| !body.is_empty())
        .collect()
}

/// A Tool turn either answers calls made on an earlier turn, or holds the calls it
/// answers. Adapters differ: Claude's carries only answer entries, while Codex's
/// carries the call itself with its result, and a content line the adapter built
/// from the call rather than anything the harness said.
fn tool_turn_prose(turn: &Turn) -> Vec<(Speaker, String)> {
    let calls: Vec<&ToolCall> = made_calls(turn).collect();
    if calls.is_empty() {
        // Replaying an answer as a user turn verbatim would read as the person
        // having typed the tool's output, so it is labelled as the recap it is.
        return vec![(
            Speaker::User,
            output_prose(Some(&turn.content), &turn.tool_calls),
        )];
    }
    vec![
        (Speaker::Assistant, tools_used_prose(&calls)),
        (Speaker::User, output_prose(None, &turn.tool_calls)),
    ]
}

/// Assistant text plus a plain-language note of what it did, so the resumed model
/// knows work happened without being handed a handle to it.
fn assistant_prose(turn: &Turn) -> String {
    let mut parts = Vec::new();
    let text = turn.content.trim();
    if !text.is_empty() {
        parts.push(text.to_string());
    }
    let calls: Vec<&ToolCall> = made_calls(turn).collect();
    if !calls.is_empty() {
        parts.push(tools_used_prose(&calls));
    }
    parts.join("\n\n")
}

fn made_calls(turn: &Turn) -> impl Iterator<Item = &ToolCall> {
    turn.tool_calls
        .iter()
        .filter(|call| call.name != ANSWER_CALL_NAME)
}

fn tools_used_prose(calls: &[&ToolCall]) -> String {
    let actions: Vec<String> = calls
        .iter()
        .map(|call| format!("- {}{}", call.name, summarize(call)))
        .collect();
    format!("{TOOLS_USED_NOTE}\n{}", actions.join("\n"))
}

fn output_prose(text: Option<&str>, calls: &[ToolCall]) -> String {
    let mut parts = Vec::new();
    if let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) {
        parts.push(text.to_string());
    }
    for call in calls {
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
    format!("{TOOL_OUTPUT_NOTE}\n{}", parts.join("\n"))
}

/// The one detail worth surfacing beside a tool's name: the file or command it
/// acted on. Anything more would bloat the narrative without helping the model
/// orient. Read from the raw arguments first, then from the target the adapter
/// resolved, which is the only place a Codex command survives — Codex stores its
/// arguments as JSON text inside the JSON string, so no key is found there.
fn summarize(call: &ToolCall) -> String {
    let from_arguments = serde_json::from_str::<Value>(&call.arguments)
        .ok()
        .and_then(|value| {
            ["file_path", "path", "file", "command"]
                .iter()
                .find_map(|key| value.get(*key).and_then(|v| v.as_str()).map(String::from))
        });
    let detail = from_arguments.or_else(|| call.target.as_ref().map(|t| t.value.clone()));
    match detail {
        Some(detail) => format!(" {detail}"),
        None => String::new(),
    }
}

/// A fresh UUID of `version`, for a session id a harness will resolve by name.
///
/// A ULID is 128 bits from the dependency tribal already uses for ids, formatted
/// in UUID layout — adding a `uuid` crate to mint a name would buy nothing. The
/// version and variant nibbles are stamped rather than left as ULID bytes, because
/// a ULID's leading 48 bits are a timestamp and the version nibble would otherwise
/// be whatever the clock produced. Each writer passes the version its harness
/// writes for itself, keeping the minted id inside the set it has been observed to
/// accept.
pub(crate) fn mint_uuid(version: u8) -> String {
    let mut b = Ulid::new().to_bytes();
    b[6] = (b[6] & 0x0f) | (version << 4);
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
    use lineage_core::{LineageId, ToolTarget, ToolTargetKind};

    fn turn(role: Role, content: &str) -> Turn {
        Turn {
            id: LineageId::new(),
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            model: None,
            timestamp: None,
            artifacts: Vec::new(),
        }
    }

    fn call(id: &str, name: &str, arguments: &str, result: Option<&str>) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: arguments.into(),
            result: result.map(String::from),
            target: None,
        }
    }

    /// Codex's adapter puts the call and its result on one Tool turn, and fills
    /// the content with a line it built from the call. That line is not something
    /// the harness said, so it must not be replayed as output.
    #[test]
    fn a_tool_turn_holding_its_own_call_becomes_a_note_and_its_output() {
        let mut tool = turn(Role::Tool, r#"exec_command("{\"cmd\":\"pwd\"}")"#);
        let mut exec = call(
            "call-1",
            "exec_command",
            r#""{\"cmd\":\"pwd\"}""#,
            Some("/repo"),
        );
        exec.target = Some(ToolTarget {
            kind: ToolTargetKind::Command,
            value: "pwd".into(),
        });
        tool.tool_calls.push(exec);

        let records = narrate(&tool);

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].0, Speaker::Assistant);
        assert!(records[0].1.contains(TOOLS_USED_NOTE));
        assert!(
            records[0].1.contains("- exec_command pwd"),
            "{}",
            records[0].1
        );
        assert_eq!(records[1].0, Speaker::User);
        assert_eq!(records[1].1, format!("{TOOL_OUTPUT_NOTE}\n/repo"));
        assert!(!records
            .iter()
            .any(|(_, body)| body.contains("exec_command(")));
    }

    #[test]
    fn a_tool_turn_of_answers_alone_is_a_labelled_recap() {
        let mut tool = turn(Role::Tool, "");
        tool.tool_calls
            .push(call("tu-1", ANSWER_CALL_NAME, "", Some("pub mod auth;")));

        let records = narrate(&tool);

        assert_eq!(
            records,
            vec![(Speaker::User, format!("{TOOL_OUTPUT_NOTE}\npub mod auth;"))]
        );
    }

    #[test]
    fn a_call_with_no_result_narrates_only_the_note() {
        let mut tool = turn(Role::Tool, "shell(ls)");
        tool.tool_calls.push(call("call-1", "shell", "{}", None));

        let records = narrate(&tool);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].0, Speaker::Assistant);
    }

    #[test]
    fn a_path_in_the_arguments_is_preferred_over_the_target() {
        let mut read = call("tu-1", "Read", r#"{"file_path":"/abs/src/auth.rs"}"#, None);
        read.target = Some(ToolTarget {
            kind: ToolTargetKind::Path,
            value: "src/auth.rs".into(),
        });

        assert_eq!(summarize(&read), " /abs/src/auth.rs");
    }

    /// Rendering repeatedly because the ULID timestamp bits move between calls: a
    /// single sample would pass even if the nibbles were never stamped.
    #[test]
    fn minted_ids_carry_the_requested_version_and_the_rfc_variant() {
        for version in [4u8, 7] {
            for _ in 0..64 {
                let id = mint_uuid(version);
                let fields: Vec<&str> = id.split('-').collect();

                assert_eq!(
                    fields.iter().map(|f| f.len()).collect::<Vec<_>>(),
                    [8, 4, 4, 4, 12]
                );
                assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
                assert_eq!(
                    fields[2].chars().next(),
                    char::from_digit(u32::from(version), 16),
                    "version nibble in {id}"
                );
                assert!(
                    matches!(fields[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
                    "variant nibble in {id}"
                );
            }
        }
    }
}
