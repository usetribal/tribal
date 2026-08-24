//! Reading one session's turns for the detail pane.

use std::path::Path;

use lineage_core::{
    invoked_command, without_plumbing, ArtifactKind, Conversation, LineageId, Role,
};
use lineage_git::{open_repo, read_conversation};
use lineage_select::{fold, Entry, Speaker, TranscriptTurn};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// A `tool_result` is the answer to a call, not a step of its own — counting it
/// would list the same action twice under two names.
const TOOL_RESULT: &str = "tool_result";

/// Load one session and fold it into what a reader follows.
///
/// Read on demand for the one session someone opened: the list holds no turns,
/// and hydrating every session up front would cost far more than it shows.
pub fn load_session_entries(repo_path: &Path, session_id: &str) -> Result<Vec<Entry>> {
    let repo = open_repo(repo_path)?;
    let id = LineageId::from(session_id);
    let Some(conv) = read_conversation(repo.inner(), &id)? else {
        return Ok(Vec::new());
    };
    Ok(fold(&transcript_turns(&conv)))
}

/// What a turn said, with the harness's markup resolved.
///
/// A turn that is only an invocation renders as the command itself (`/prime`),
/// not as nothing: it is why everything after it happened, and dropping it
/// starts the session mid-thought. How a session is *chosen* must never change
/// what the session *is* — the list summarises, this shows.
fn turn_content(content: &str) -> String {
    let stripped = without_plumbing(content);
    if !stripped.is_empty() {
        return stripped;
    }
    invoked_command(content).unwrap_or_default()
}

fn transcript_turns(conv: &Conversation) -> Vec<TranscriptTurn> {
    let start = conv.turns.iter().find_map(|turn| turn.timestamp);
    conv.turns
        .iter()
        .map(|turn| TranscriptTurn {
            speaker: match turn.role {
                Role::User => Speaker::User,
                // System and tool turns are the agent's side of the exchange as
                // far as a reader is concerned: what matters is whether the
                // person asked or the machine answered.
                _ => Speaker::Agent,
            },
            content: turn_content(&turn.content),
            tools: turn
                .tool_calls
                .iter()
                .filter(|call| call.name != TOOL_RESULT)
                .map(|call| call.name.clone())
                .collect(),
            wrote: turn
                .artifacts
                .iter()
                .filter(|artifact| artifact.kind != ArtifactKind::TerminalCommand)
                .map(|artifact| artifact.path.clone())
                .collect(),
            offset_seconds: turn
                .timestamp
                .zip(start)
                .map(|(at, first)| at.signed_duration_since(first).num_seconds()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{AgentKind, Artifact, ToolCall, Turn};

    fn turn(role: Role, content: &str) -> Turn {
        Turn {
            id: LineageId::new(),
            role,
            content: content.into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: "1".into(),
            name: name.into(),
            arguments: String::new(),
            result: None,
            target: None,
        }
    }

    #[test]
    fn a_tool_result_is_not_counted_as_a_step() {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/repo");
        let mut acted = turn(Role::Assistant, "");
        acted.tool_calls = vec![call("Read"), call(TOOL_RESULT)];
        conv.turns.push(acted);

        let turns = transcript_turns(&conv);
        assert_eq!(turns[0].tools, vec!["Read"]);
    }

    #[test]
    fn a_terminal_command_is_not_a_written_file() {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/repo");
        let mut acted = turn(Role::Assistant, "");
        acted.artifacts = vec![
            Artifact {
                kind: ArtifactKind::FileEdit,
                path: "src/auth.rs".into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: None,
                resolve: None,
            },
            Artifact {
                kind: ArtifactKind::TerminalCommand,
                path: "cargo test".into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: None,
                resolve: None,
            },
        ];
        conv.turns.push(acted);

        assert_eq!(transcript_turns(&conv)[0].wrote, vec!["src/auth.rs"]);
    }

    #[test]
    fn offsets_are_measured_from_the_sessions_first_stamp() {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/repo");
        let base = chrono::Utc::now();
        let mut first = turn(Role::User, "go");
        first.timestamp = Some(base);
        let mut later = turn(Role::Assistant, "done");
        later.timestamp = Some(base + chrono::Duration::seconds(90));
        conv.turns.push(first);
        conv.turns.push(later);

        let turns = transcript_turns(&conv);
        assert_eq!(turns[0].offset_seconds, Some(0));
        assert_eq!(turns[1].offset_seconds, Some(90));
    }

    #[test]
    fn an_invocation_survives_as_the_command_it_ran() {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/repo");
        conv.turns.push(turn(
            Role::User,
            "<command-message>prime</command-message> <command-name>/prime</command-name>",
        ));
        // The session started with `/prime`; showing nothing would start it
        // mid-thought, and how the session was picked must not change it.
        assert_eq!(transcript_turns(&conv)[0].content, "/prime");
    }

    #[test]
    fn plumbing_around_real_prose_is_still_stripped() {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/repo");
        conv.turns.push(turn(
            Role::User,
            "<command-name>/prime</command-name> then fix the guard",
        ));
        assert_eq!(transcript_turns(&conv)[0].content, "then fix the guard");
    }

    #[test]
    fn only_the_user_speaks_as_the_user() {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/repo");
        conv.turns.push(turn(Role::User, "go"));
        conv.turns.push(turn(Role::Assistant, "ok"));
        conv.turns.push(turn(Role::System, "note"));

        let speakers: Vec<Speaker> = transcript_turns(&conv)
            .iter()
            .map(|turn| turn.speaker)
            .collect();
        assert_eq!(
            speakers,
            vec![Speaker::User, Speaker::Agent, Speaker::Agent]
        );
    }
}
