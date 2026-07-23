use crate::conversation_util::turn_modified_code;
use crate::types::{Role, Turn};

/// Substrings identifying read/inspection tools for salience purposes. Kept in
/// core (rather than reusing the adapters' read-tool predicate) because
/// adapters depend on core, and their predicate answers a different question —
/// "should this call mint an artifact" — whose rules must stay free to drift.
const READ_TOOL_MARKERS: &[&str] = &[
    "read", "grep", "glob", "search", "list", "cat", "find", "fetch", "view", "open",
];

/// An assistant turn whose tool calls are all reads is exploration only when
/// its prose is also thin; a long explanation alongside reads may carry real
/// reasoning, so it stays at narration weight instead.
const EXPLORE_MAX_CONTENT_CHARS: usize = 200;

/// Weight applied to assistant narration: kept in the corpus (real decisions
/// hide in prose), but never allowed to outrank full-weight evidence.
const NARRATION_WEIGHT: f32 = 0.3;

/// The v0 salience rule set's verdict for one turn. Classes mirror the
/// categories the rules were measured with (docs/plans/xifong/
/// enhanced-semantic-retrieval — 65-session breakdown), so any corpus's
/// distribution is comparable to those numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SalienceClass {
    /// A user turn: the intent stream itself.
    User,
    /// A turn that wrote code (edit/diff artifact or edit-style tool call).
    Edit,
    /// An explicit decision point (e.g. an AskUserQuestion tool call).
    Decision,
    /// Assistant prose that is neither an edit nor pure exploration.
    Narration,
    /// An all-read, low-prose assistant turn: orientation, not intent.
    Explore,
    /// A tool-result turn: mechanical output.
    ToolResult,
}

impl SalienceClass {
    /// Index/retrieval weight. Zero-weight turns leave the corpus entirely;
    /// narration stays but is down-weighted (the "never surface noise" rule
    /// acts through selection floors, not deletion).
    pub fn weight(self) -> f32 {
        match self {
            Self::User | Self::Edit | Self::Decision => 1.0,
            Self::Narration => NARRATION_WEIGHT,
            Self::Explore | Self::ToolResult => 0.0,
        }
    }

    /// Stable lowercase label for storage and reporting.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Edit => "edit",
            Self::Decision => "decision",
            Self::Narration => "narration",
            Self::Explore => "explore",
            Self::ToolResult => "tool_result",
        }
    }
}

fn is_read_tool_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    // "ls" is matched exactly: as a substring it would false-positive on
    // unrelated tool names.
    lower == "ls" || READ_TOOL_MARKERS.iter().any(|m| lower.contains(m))
}

fn is_decision_tool_name(name: &str) -> bool {
    name.to_lowercase().contains("askuserquestion")
}

/// Classify one turn under the v0 salience rules. Order matters: authorship
/// (edits) outranks the explore test, so a turn that read widely *and* then
/// wrote code is Edit, not Explore.
pub fn turn_salience(turn: &Turn) -> SalienceClass {
    match turn.role {
        Role::Tool => return SalienceClass::ToolResult,
        Role::User => return SalienceClass::User,
        Role::Assistant | Role::System => {}
    }
    if turn_modified_code(turn) {
        return SalienceClass::Edit;
    }
    if turn
        .tool_calls
        .iter()
        .any(|tc| is_decision_tool_name(&tc.name))
    {
        return SalienceClass::Decision;
    }
    let all_reads =
        !turn.tool_calls.is_empty() && turn.tool_calls.iter().all(|tc| is_read_tool_name(&tc.name));
    if all_reads && turn.content.chars().count() < EXPLORE_MAX_CONTENT_CHARS {
        return SalienceClass::Explore;
    }
    SalienceClass::Narration
}

/// Convenience for callers that only need the weight.
pub fn turn_salience_weight(turn: &Turn) -> f32 {
    turn_salience(turn).weight()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::LineageId;
    use crate::types::{AgentKind, Artifact, ArtifactKind, Conversation, ToolCall};

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

    fn tool_call(name: &str) -> ToolCall {
        ToolCall {
            id: "t".into(),
            name: name.into(),
            arguments: "{}".into(),
            result: None,
        }
    }

    #[test]
    fn user_turns_are_full_weight() {
        let t = turn(Role::User, "make the import idempotent");
        assert_eq!(turn_salience(&t), SalienceClass::User);
        assert_eq!(turn_salience_weight(&t), 1.0);
    }

    #[test]
    fn tool_result_turns_are_dropped() {
        let t = turn(Role::Tool, "1234 lines of build output");
        assert_eq!(turn_salience(&t), SalienceClass::ToolResult);
        assert_eq!(turn_salience_weight(&t), 0.0);
    }

    #[test]
    fn edit_turns_are_full_weight_even_when_read_heavy() {
        let mut t = turn(Role::Assistant, "ok");
        t.tool_calls = vec![tool_call("Read"), tool_call("Grep")];
        t.artifacts.push(Artifact {
            kind: ArtifactKind::FileEdit,
            path: "src/lib.rs".into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: None,
            resolve: None,
        });
        assert_eq!(turn_salience(&t), SalienceClass::Edit);
    }

    #[test]
    fn ask_user_question_is_a_decision_point() {
        let mut t = turn(Role::Assistant, "");
        t.tool_calls = vec![tool_call("AskUserQuestion")];
        assert_eq!(turn_salience(&t), SalienceClass::Decision);
        assert_eq!(turn_salience_weight(&t), 1.0);
    }

    #[test]
    fn all_read_short_assistant_turns_are_explore() {
        let mut t = turn(Role::Assistant, "Let me look at the config.");
        t.tool_calls = vec![tool_call("Read"), tool_call("Glob")];
        assert_eq!(turn_salience(&t), SalienceClass::Explore);
        assert_eq!(turn_salience_weight(&t), 0.0);
    }

    #[test]
    fn read_heavy_turn_with_long_prose_is_narration_not_explore() {
        let mut t = turn(Role::Assistant, &"x".repeat(EXPLORE_MAX_CONTENT_CHARS + 1));
        t.tool_calls = vec![tool_call("Read")];
        assert_eq!(turn_salience(&t), SalienceClass::Narration);
    }

    #[test]
    fn bash_only_turns_are_narration() {
        // Bash is ambiguous (build/test/read); recovered shell writes surface
        // as FileEdit artifacts and classify as Edit before this rule runs.
        let mut t = turn(Role::Assistant, "running the tests");
        t.tool_calls = vec![tool_call("Bash")];
        assert_eq!(turn_salience(&t), SalienceClass::Narration);
    }

    #[test]
    fn plain_prose_assistant_turns_are_narration() {
        let t = turn(Role::Assistant, "The tradeoff is X because Y.");
        assert_eq!(turn_salience(&t), SalienceClass::Narration);
        assert_eq!(turn_salience_weight(&t), NARRATION_WEIGHT);
    }

    #[test]
    fn classes_have_stable_labels() {
        let mut conv = Conversation::new(AgentKind::Claude, "/repo");
        conv.turns.push(turn(Role::User, "hi"));
        assert_eq!(turn_salience(&conv.turns[0]).as_str(), "user");
    }
}
