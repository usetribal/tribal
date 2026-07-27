//! `git lineage fork <session-id> --brief` — the context block for starting a
//! *subagent* on someone's session, instead of continuing it yourself.
//!
//! Three things shape this module.
//!
//! **Lineage emits text and nothing else.** Spawning a subagent is
//! model-initiated: only the calling agent's own tool can do it. So the whole
//! job here is to print a block that agent can hand on, and the block writes
//! nothing — no transcript, no fork edge. It is an initial context load, not a
//! fork.
//!
//! **Selection is fixed, and stated in the output.** Which turns appear is a
//! rule (all user prompts, every code-editing turn, the last assistant turn),
//! never a judgement about which turns are interesting. `docs/ARCHITECTURE.md`
//! invariant 3: rendering decides how to show, never *what* to show. A block
//! whose contents depended on a model's opinion could not be reproduced from
//! the session, and reproducibility is the only reason to trust a second-hand
//! account of someone else's work.
//!
//! **The traversal vocabulary travels with the block.** The `SessionStart` hook
//! fires for the session the user is in, not for a subagent it spawns, so a
//! subagent handed only the brief would be able to read this session and unable
//! to move through it. Embedding the vocabulary is what makes the brief a
//! starting point rather than a dead end.

use lineage_core::conversation_util::turn_modified_code;
use lineage_core::{ArtifactKind, Conversation, Role, Turn};

/// The most turns a brief will carry. A subagent's whole context is this block
/// plus its task, so the block must stay a briefing rather than become the
/// session.
pub const MAX_TURNS: usize = 100;

/// Byte backstop, independent of the turn cap. One whole-file edit turn can be
/// megabytes on its own, so a cap counted in turns bounds nothing — the same
/// reason `turn_indexable_text` caps edit snippets at 800 chars. 64 KiB is
/// roughly 16k tokens: large enough that a normal session survives the turn cap
/// alone, small enough that the worst case stays a fraction of a subagent's
/// window.
pub const MAX_BYTES: usize = 64 * 1024;

/// How much of one turn's prose is shown. Long enough to carry an argument,
/// short enough that no single turn can be most of the brief.
const TURN_MAX_CHARS: usize = 1_500;

/// Why a turn is in the brief. The kind is what the drop order is expressed in,
/// so it is carried rather than recomputed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A `Role::User` turn with content — the intent thread.
    Prompt,
    /// A turn carrying a file-edit or diff artifact.
    Edit,
    /// The final assistant turn, which is where the session left off.
    LastAssistant,
}

/// One selected turn, paired with why it was kept and where it sat.
#[derive(Debug, Clone)]
pub struct Selected<'a> {
    pub index: usize,
    pub reason: Reason,
    pub turn: &'a Turn,
}

/// What the rule found, before and after capping — the counts are what the
/// output reports, so a reader can tell a complete brief from a trimmed one.
#[derive(Debug, Clone)]
pub struct Selection<'a> {
    pub kept: Vec<Selected<'a>>,
    pub prompts_found: usize,
    pub edits_found: usize,
}

impl Selection<'_> {
    fn kept_of(&self, reason: Reason) -> usize {
        self.kept.iter().filter(|s| s.reason == reason).count()
    }

    fn dropped_anything(&self) -> bool {
        self.kept_of(Reason::Prompt) < self.prompts_found
            || self.kept_of(Reason::Edit) < self.edits_found
    }
}

/// The selection rule, in full:
///
/// - every `Role::User` turn with non-empty content (not just the first — the
///   prompts are the intent thread, and one prompt makes a session look like a
///   single question when it was a negotiation),
/// - every turn that modified code,
/// - the last assistant turn, which is where the work actually stopped.
///
/// When either cap is exceeded, turns are dropped lowest-priority first —
/// edits, then prompts, oldest first within each — and the last assistant turn
/// is never dropped. Edits go first because a prompt is irreplaceable context
/// and an edit is recoverable: the subagent has the traversal verbs and can ask
/// the session for the rest.
pub fn select(conversation: &Conversation, max_turns: usize, max_bytes: usize) -> Selection<'_> {
    let mut candidates: Vec<Selected<'_>> = Vec::new();
    for (index, turn) in conversation.turns.iter().enumerate() {
        let Some(reason) = classify(conversation, index, turn) else {
            continue;
        };
        candidates.push(Selected {
            index,
            reason,
            turn,
        });
    }

    let prompts_found = candidates
        .iter()
        .filter(|s| s.reason == Reason::Prompt)
        .count();
    let edits_found = candidates
        .iter()
        .filter(|s| s.reason == Reason::Edit)
        .count();

    let kept = apply_caps(candidates, max_turns, max_bytes);
    Selection {
        kept,
        prompts_found,
        edits_found,
    }
}

/// The last assistant turn wins over the edit reason when a turn is both, so
/// that turn is never in the droppable set.
fn classify(conversation: &Conversation, index: usize, turn: &Turn) -> Option<Reason> {
    if Some(index) == last_assistant_index(conversation) {
        return Some(Reason::LastAssistant);
    }
    if turn.role == Role::User && !turn.content.trim().is_empty() {
        return Some(Reason::Prompt);
    }
    if turn_modified_code(turn) {
        return Some(Reason::Edit);
    }
    None
}

fn last_assistant_index(conversation: &Conversation) -> Option<usize> {
    conversation
        .turns
        .iter()
        .rposition(|turn| turn.role == Role::Assistant)
}

/// Drops until both caps hold: edits oldest-first, then prompts oldest-first.
/// Dropping one turn at a time rather than computing a count keeps the two caps
/// under one rule — a byte overrun and a turn overrun retire the same turn next.
fn apply_caps<'a>(
    candidates: Vec<Selected<'a>>,
    max_turns: usize,
    max_bytes: usize,
) -> Vec<Selected<'a>> {
    let mut kept = candidates;
    while over_cap(&kept, max_turns, max_bytes) {
        let Some(victim) = next_to_drop(&kept) else {
            break;
        };
        kept.remove(victim);
    }
    kept
}

fn over_cap(kept: &[Selected<'_>], max_turns: usize, max_bytes: usize) -> bool {
    kept.len() > max_turns || selection_bytes(kept) > max_bytes
}

fn selection_bytes(kept: &[Selected<'_>]) -> usize {
    kept.iter().map(|s| turn_text(s.turn).len()).sum()
}

/// The position of the lowest-priority turn: oldest edit, else oldest prompt.
/// `None` means only the last assistant turn is left, which is never dropped —
/// a brief with no turns at all would be worse than one over its cap.
fn next_to_drop(kept: &[Selected<'_>]) -> Option<usize> {
    kept.iter()
        .position(|s| s.reason == Reason::Edit)
        .or_else(|| kept.iter().position(|s| s.reason == Reason::Prompt))
}

/// One turn's prose plus the paths it changed. The paths matter more than the
/// diff body for a brief: the subagent can fetch the code by traversal, but it
/// cannot guess which files the session was working in.
fn turn_text(turn: &Turn) -> String {
    let mut text = truncate_chars(turn.content.trim(), TURN_MAX_CHARS);
    let paths = edited_paths(turn);
    if paths.is_empty() {
        return text;
    }
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(&format!("(edited {})", paths.join(", ")));
    text
}

fn edited_paths(turn: &Turn) -> Vec<String> {
    let mut paths: Vec<String> = turn
        .artifacts
        .iter()
        .filter(|a| {
            matches!(a.kind, ArtifactKind::FileEdit | ArtifactKind::Diff) && !a.path.is_empty()
        })
        .map(|a| a.path.clone())
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars).collect();
    out.push('…');
    out
}

/// The line a calling agent looks for to know where its own text goes. It is a
/// delimiter rather than a prose instruction because the consumer is an agent
/// assembling a prompt, and "append below this line" has to be unambiguous
/// without being read as part of the brief.
pub const TASK_SLOT_MARKER: &str = "--- TASK (append the subagent's task below this line) ---";

/// The whole block: A the brief, B the traversal vocabulary, C the empty task
/// slot. C is deliberately left empty — the task belongs to the agent spawning
/// the subagent, and lineage has no idea what it is.
pub fn render_brief(
    conversation: &Conversation,
    selection: &Selection<'_>,
    author: &str,
    vocabulary: &str,
) -> String {
    let mut block = String::new();
    block.push_str(&render_header(conversation, selection, author));
    block.push_str(&render_turns(conversation, selection));
    block.push('\n');
    block.push_str(vocabulary);
    block.push('\n');
    block.push_str(TASK_SLOT_MARKER);
    block.push('\n');
    block
}

fn render_header(conversation: &Conversation, selection: &Selection<'_>, author: &str) -> String {
    let mut header = format!(
        "You are being briefed on an agent session recorded in this repository.\n\
         Session: {}\n\
         {author}\n",
        conversation.id.as_str(),
    );
    if let Some(notice) = truncation_notice(selection) {
        header.push_str(&notice);
    }
    header.push('\n');
    header
}

/// Said out loud whenever anything was dropped. A brief that silently omits half
/// a session is worse than no brief: the reader has no way to know the account
/// is partial, and the traversal verbs are exactly how they would fill the gap.
fn truncation_notice(selection: &Selection<'_>) -> Option<String> {
    if !selection.dropped_anything() {
        return None;
    }
    let mut notice = String::from(
        "This brief is partial — some turns were dropped to fit. \
         Use the traversal commands below to read the rest.\n",
    );
    let prompts_kept = selection.kept_of(Reason::Prompt);
    if prompts_kept < selection.prompts_found {
        notice.push_str(&format!(
            "  {prompts_kept} of {} user prompts shown\n",
            selection.prompts_found
        ));
    }
    let edits_kept = selection.kept_of(Reason::Edit);
    if edits_kept < selection.edits_found {
        notice.push_str(&format!(
            "  {edits_kept} of {} edit turns shown\n",
            selection.edits_found
        ));
    }
    Some(notice)
}

fn render_turns(conversation: &Conversation, selection: &Selection<'_>) -> String {
    if selection.kept.is_empty() {
        return "This session has no user prompts, no code edits, and no assistant reply.\n"
            .to_string();
    }
    let mut body = String::from(
        "Selected turns, in order (all user prompts, every turn that changed code, and the last \
         assistant turn). Each is headed by the `session#turn` handle the commands below take:\n\n",
    );
    for entry in &selection.kept {
        // The handle is spelled in full rather than as a bare turn id: the
        // traversal verbs take `session#turn`, and an agent should not have to
        // reassemble one from two places in the block.
        body.push_str(&format!(
            "[turn {} · {}] {}#{}\n",
            entry.index + 1,
            label(entry.reason),
            conversation.id.as_str(),
            entry.turn.id.as_str(),
        ));
        body.push_str(&turn_text(entry.turn));
        body.push_str("\n\n");
    }
    body
}

fn label(reason: Reason) -> &'static str {
    match reason {
        Reason::Prompt => "user asked",
        Reason::Edit => "changed code",
        Reason::LastAssistant => "last assistant turn",
    }
}

#[cfg(test)]
mod tests {
    use lineage_core::{AgentKind, Artifact, ArtifactResolve, LineageId, ResolveStrategy};

    use super::*;

    fn turn(role: Role, content: &str, artifacts: Vec<Artifact>) -> Turn {
        Turn {
            id: LineageId::new(),
            role,
            content: content.into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts,
        }
    }

    fn edit(path: &str) -> Artifact {
        Artifact {
            kind: ArtifactKind::FileEdit,
            path: path.into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: None,
            resolve: Some(ArtifactResolve {
                strategy: ResolveStrategy::OldString,
                old_string: None,
                new_string: Some("fn ok() {}".into()),
                patch: None,
            }),
        }
    }

    fn session(turns: Vec<Turn>) -> Conversation {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/proj");
        conv.turns = turns;
        conv
    }

    #[test]
    fn every_user_prompt_is_kept_not_only_the_first() {
        let conv = session(vec![
            turn(Role::User, "first ask", vec![]),
            turn(Role::Assistant, "sure", vec![]),
            turn(Role::User, "actually, do it this way", vec![]),
            turn(Role::Assistant, "done", vec![]),
        ]);
        let selection = select(&conv, MAX_TURNS, MAX_BYTES);
        assert_eq!(selection.prompts_found, 2);
        assert_eq!(selection.kept_of(Reason::Prompt), 2);
    }

    #[test]
    fn empty_user_turns_are_not_prompts() {
        let conv = session(vec![
            turn(Role::User, "   \n ", vec![]),
            turn(Role::User, "the real ask", vec![]),
        ]);
        let selection = select(&conv, MAX_TURNS, MAX_BYTES);
        assert_eq!(selection.prompts_found, 1);
    }

    #[test]
    fn edit_turns_and_the_last_assistant_turn_are_kept() {
        let conv = session(vec![
            turn(Role::Assistant, "thinking out loud", vec![]),
            turn(Role::Assistant, "patched it", vec![edit("auth.rs")]),
            turn(Role::Assistant, "here is what I did", vec![]),
        ]);
        let selection = select(&conv, MAX_TURNS, MAX_BYTES);
        // The bare narration turn is not selected; the edit and the final turn are.
        assert_eq!(selection.kept.len(), 2);
        assert_eq!(selection.kept[0].reason, Reason::Edit);
        assert_eq!(selection.kept[1].reason, Reason::LastAssistant);
    }

    #[test]
    fn the_last_assistant_turn_is_kept_even_when_it_is_also_an_edit() {
        let conv = session(vec![turn(Role::Assistant, "patched", vec![edit("a.rs")])]);
        let selection = select(&conv, MAX_TURNS, MAX_BYTES);
        assert_eq!(selection.kept.len(), 1);
        assert_eq!(selection.kept[0].reason, Reason::LastAssistant);
        assert_eq!(selection.edits_found, 0);
    }

    #[test]
    fn edits_are_dropped_before_prompts_oldest_first() {
        let conv = session(vec![
            turn(Role::User, "ask one", vec![]),
            turn(Role::Assistant, "edit one", vec![edit("one.rs")]),
            turn(Role::User, "ask two", vec![]),
            turn(Role::Assistant, "edit two", vec![edit("two.rs")]),
            turn(Role::Assistant, "summary", vec![]),
        ]);
        // Three slots: the last assistant turn plus two of the four droppables.
        let selection = select(&conv, 3, MAX_BYTES);
        assert_eq!(selection.kept.len(), 3);
        assert_eq!(selection.kept_of(Reason::Edit), 0, "edits go first");
        assert_eq!(selection.kept_of(Reason::Prompt), 2);
        assert_eq!(selection.edits_found, 2);
    }

    #[test]
    fn prompts_are_dropped_oldest_first_once_the_edits_are_gone() {
        let conv = session(vec![
            turn(Role::User, "oldest ask", vec![]),
            turn(Role::User, "newest ask", vec![]),
            turn(Role::Assistant, "summary", vec![]),
        ]);
        let selection = select(&conv, 2, MAX_BYTES);
        assert_eq!(selection.kept.len(), 2);
        assert_eq!(selection.kept[0].turn.content, "newest ask");
        assert_eq!(selection.kept[1].reason, Reason::LastAssistant);
    }

    #[test]
    fn the_last_assistant_turn_survives_a_cap_of_zero() {
        let conv = session(vec![
            turn(Role::User, "ask", vec![]),
            turn(Role::Assistant, "reply", vec![]),
        ]);
        let selection = select(&conv, 0, 0);
        assert_eq!(selection.kept.len(), 1);
        assert_eq!(selection.kept[0].reason, Reason::LastAssistant);
    }

    /// The byte cap binds independently of the turn cap: turns well under the
    /// 100-turn limit can still be far too much text, so the aggregate is
    /// bounded on its own terms.
    #[test]
    fn the_byte_cap_binds_even_when_the_turn_cap_does_not() {
        let long = "x".repeat(TURN_MAX_CHARS);
        let mut turns: Vec<Turn> = (0..80)
            .map(|_| turn(Role::Assistant, &long, vec![edit("big.rs")]))
            .collect();
        turns.push(turn(Role::Assistant, "summary", vec![]));
        let conv = session(turns);

        let selection = select(&conv, MAX_TURNS, MAX_BYTES);
        assert!(
            selection.kept.len() < 81,
            "81 turns is under the turn cap, so only the byte cap can bind here"
        );
        assert!(selection_bytes(&selection.kept) <= MAX_BYTES);
        assert!(selection.dropped_anything());
    }

    #[test]
    fn a_dropped_turn_is_reported_in_the_block() {
        let conv = session(vec![
            turn(Role::User, "ask", vec![]),
            turn(Role::Assistant, "edit", vec![edit("a.rs")]),
            turn(Role::Assistant, "summary", vec![]),
        ]);
        let selection = select(&conv, 2, MAX_BYTES);
        let block = render_brief(&conv, &selection, "Alice's claude session", "VOCAB\n");
        assert!(block.contains("0 of 1 edit turns shown"), "{block}");
        assert!(block.contains("partial"), "{block}");
    }

    #[test]
    fn a_complete_brief_claims_no_truncation() {
        let conv = session(vec![
            turn(Role::User, "ask", vec![]),
            turn(Role::Assistant, "summary", vec![]),
        ]);
        let selection = select(&conv, MAX_TURNS, MAX_BYTES);
        let block = render_brief(&conv, &selection, "Alice's claude session", "VOCAB\n");
        assert!(!block.contains("partial"), "{block}");
    }

    /// Everything a subagent needs to start: whose work, the session id it
    /// quotes to the verbs, the vocabulary, and where its own task goes.
    #[test]
    fn the_block_carries_the_id_the_vocabulary_and_the_task_slot() {
        let conv = session(vec![
            turn(
                Role::User,
                "the login endpoint accepts an empty password",
                vec![],
            ),
            turn(Role::Assistant, "tightened validate", vec![edit("auth.rs")]),
        ]);
        let selection = select(&conv, MAX_TURNS, MAX_BYTES);
        let block = render_brief(&conv, &selection, "Alice's claude session", "VOCAB LINE\n");
        assert!(block.contains(conv.id.as_str()), "{block}");
        assert!(block.contains("Alice's claude session"), "{block}");
        assert!(block.contains("empty password"), "{block}");
        assert!(block.contains("auth.rs"), "{block}");
        assert!(block.contains("VOCAB LINE"), "{block}");
        assert!(block.trim_end().ends_with(TASK_SLOT_MARKER), "{block}");
        // Every selected turn is addressable as the verbs expect it, with no
        // reassembly from the session line at the top.
        for entry in &selection.kept {
            assert!(
                block.contains(&format!("{}#{}", conv.id.as_str(), entry.turn.id.as_str())),
                "{block}"
            );
        }
    }

    #[test]
    fn a_session_with_nothing_selectable_says_so_rather_than_printing_an_empty_brief() {
        let conv = session(vec![turn(Role::System, "imported from a hook", vec![])]);
        let selection = select(&conv, MAX_TURNS, MAX_BYTES);
        assert!(selection.kept.is_empty());
        let block = render_brief(&conv, &selection, "Someone's claude session", "VOCAB\n");
        assert!(block.contains("no user prompts"), "{block}");
    }
}
