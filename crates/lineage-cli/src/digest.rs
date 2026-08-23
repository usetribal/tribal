//! Presentation for the injected digest: which evidence is shown, how it is
//! rendered, and which traversal moves are named alongside it.
//!
//! Split from `context_cmd` (which keeps hook I/O and the settings installer)
//! because the seam is real — this side derives nothing from a repo and so is
//! testable without constructing hook payloads.
//!
//! Relation → command rendering lives here rather than in `lineage-retrieval`:
//! MCP consumes that crate and never shells out, so `tribal …` strings must
//! not be visible to it. The registry names relations; each surface spells them.

use lineage_retrieval::{verb_for_relation, Evidence, Retrieval, Strength, VERBS};

/// Selection defaults from context-injection-v0 "Digest format".
const MIN_STRENGTH: Strength = Strength::Low;

/// Which trigger a digest is being rendered for. The two want different amounts
/// of navigation and the spec used to treat them alike:
///
/// - `FileKeyed` fires constantly mid-task, appended into a `Read` result. The
///   agent is in flight and mostly does not want diverting, and a false positive
///   costs more because it repeats per read. Tight cap, one entry, no footer.
/// - `Intent` fires at a decision point, before the agent has committed to an
///   approach. Exploration has its highest value here and the budget is there
///   for it, so: full digest and the verb footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    FileKeyed,
    Intent,
}

impl Trigger {
    /// Roughly 4 bytes per token, matching how the spec's 1,024-token cap was
    /// already expressed as 4 KiB.
    const BYTES_PER_TOKEN: usize = 4;

    fn max_entries(self) -> usize {
        match self {
            // At most one pointer mid-task; the read is the primary task.
            Self::FileKeyed => 1,
            Self::Intent => 3,
        }
    }

    fn max_bytes(self) -> usize {
        match self {
            Self::FileKeyed => 200 * Self::BYTES_PER_TOKEN,
            Self::Intent => 1024 * Self::BYTES_PER_TOKEN,
        }
    }

    fn wants_footer(self) -> bool {
        matches!(self, Self::Intent)
    }
}

/// The selector: presentation policy over an already-final retrieval.
/// Evidence arrives strongest-first, so truncation keeps the best entries.
pub fn select(retrieval: &Retrieval, trigger: Trigger) -> Vec<&Evidence> {
    retrieval
        .evidence
        .iter()
        .filter(|e| e.strength >= MIN_STRENGTH)
        .take(trigger.max_entries())
        .collect()
}

/// The injected text. Each entry carries an addressable handle and the edges
/// that node actually has — nouns, not commands — and the verbs are named once
/// in a shared footer rather than repeated per entry, because three entries ×
/// three affordances is over 13% of the intent cap spent on navigation.
pub fn render_digest(file_path: &str, selected: &[&Evidence], trigger: Trigger) -> String {
    let mut digest = format!(
        "Lineage: {} past session(s) touched {file_path} — details below.\n",
        selected.len(),
    );
    for evidence in selected {
        digest.push_str(&render_entry(evidence));
    }
    if trigger.wants_footer() && !selected.is_empty() {
        digest.push_str(&verb_footer());
    }
    truncate_to_bytes(&digest, trigger.max_bytes())
}

/// One entry: its handle, its attribution, the edges it has, then its words.
fn render_entry(evidence: &Evidence) -> String {
    let mut entry = format!("- {} {}", turn_handle(evidence), evidence.attribution);
    if !evidence.line_ranges.is_empty() {
        let ranges: Vec<String> = evidence
            .line_ranges
            .iter()
            .map(|[start, end]| format!("{start}-{end}"))
            .collect();
        entry.push_str(&format!(" (lines {})", ranges.join(", ")));
    }
    entry.push('\n');
    for line in evidence.summary.lines() {
        entry.push_str(&format!("  {line}\n"));
    }
    entry
}

/// The vocabulary named once per digest. The `SessionStart` hook teaches it in
/// full; this is the reminder that the handles above are addressable, so it
/// stays one line.
fn verb_footer() -> String {
    let verbs: Vec<&str> = VERBS.iter().map(|verb| verb.cli).collect();
    format!(
        "Follow a handle with: tribal context <{}> <handle>\n",
        verbs.join("|"),
    )
}

fn truncate_to_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Whether a rendered vocabulary offers to continue a session, or only to read
/// one.
///
/// A brief is handed to a subagent that was spawned to explore one session
/// somebody already chose. Offering `fork` there invites it to fork again, and a
/// subagent has no way to tell it is already inside one — so the brief gets the
/// read-only vocabulary and only a top-level session sees the full set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Continuation {
    Offered,
    Withheld,
}

/// The vocabulary in full, for the once-per-session `SessionStart` injection.
/// A statement of capability, never an instruction to use it — an agent told to
/// use lineage would make the A/B harness measure the prompt rather than the
/// tool.
pub fn verb_vocabulary() -> String {
    render_vocabulary(Continuation::Offered)
}

/// The traversal half alone: what a session can be asked, with no offer to
/// continue one. This is what travels inside a brief.
pub fn traversal_vocabulary() -> String {
    render_vocabulary(Continuation::Withheld)
}

fn render_vocabulary(continuation: Continuation) -> String {
    let mut text = String::from(
        "Lineage indexes past agent sessions in this repo and can be traversed. \
         Injected evidence carries a `session#turn` handle; these commands take one:\n",
    );
    for verb in VERBS {
        text.push_str(&format!(
            "  tribal context {:<20} {}\n",
            verb.cli, verb.summary,
        ));
    }
    text.push_str("  tribal context query \"<question>\"  search every session by intent\n");
    if continuation == Continuation::Offered {
        text.push_str(FORK_CAPABILITY);
    }
    text
}

/// How to continue a session rather than only read one, including the shape of
/// the subagent invocation.
///
/// This states what the commands do and what their output is for. It does not
/// say when to reach for them, and must not: an agent told to use lineage would
/// make any measurement of injection a measurement of the prompt
/// (`specs/context-injection-v0.md`). "Here is the mechanism" is capability;
/// "use the mechanism" is instruction.
///
/// It lives in the hook rather than the bundled skill because a skill loads only
/// if it is installed and only if the harness looks for it, whereas this fires
/// every session. That is the same reasoning the spec gives for choosing a hook
/// in the first place.
const FORK_CAPABILITY: &str = "\
Sessions can also be continued rather than only read:
  tribal fork <session>          carry on a session in your agent
  tribal fork <session> --brief  print a context block instead of continuing it
A session this machine holds is reopened as itself; any other is written out as a
new session carrying its context. `--brief` writes nothing and prints a
self-contained block — whose session it was, what they asked for, the
turns that changed code, and the traversal commands above — for starting a subagent
on that session while leaving this session's context untouched. The block ends with
a marked slot for the task the subagent is being given.
";

/// Capped so the pointers stay a footer, not the payload.
pub const MAX_AFFORDANCES: usize = 3;

/// The addressable handle for an evidence entry: `session#turn`, or the bare
/// session id when the evidence is session-grained. This is what an agent quotes
/// back to a traversal verb, so it is the one string in the digest that has to
/// round-trip.
pub fn turn_handle(evidence: &Evidence) -> String {
    match &evidence.turn_id {
        Some(turn_id) => format!("{}#{}", evidence.session_id.as_str(), turn_id.as_str()),
        None => evidence.session_id.as_str().to_string(),
    }
}

/// Split a `session#turn` handle back into its parts. A bare id is a session
/// with no turn — the same shape `turn_handle` emits for session-grained
/// evidence.
pub fn parse_handle(handle: &str) -> (&str, Option<&str>) {
    match handle.split_once('#') {
        Some((session_id, turn_id)) => (session_id, Some(turn_id)),
        None => (handle, None),
    }
}

/// The affordance relations this evidence entry can honour, rendered as runnable
/// `tribal` commands (spec: Verbatim-turn digest — a selector MUST omit
/// relations it cannot honour).
///
/// `session` is always available and is not a traversal verb — the whole
/// conversation prints the turn uncapped, which is why the spec's `full-turn`
/// folds into it. The rest are looked up in the verb registry, so a verb the
/// installed CLI does not have cannot be named here.
pub fn affordances_for(evidence: &Evidence, anchor_file: Option<&str>) -> Vec<String> {
    let mut lines = vec![format!("tribal show {}", evidence.session_id.as_str())];
    if let (Some(file), Some([start, _])) = (anchor_file, evidence.line_ranges.first()) {
        lines.push(format!("tribal context chain {file}:{start}"));
    }
    if let (Some(turn_id), Some(verb)) = (&evidence.turn_id, verb_for_relation("produced-by")) {
        lines.push(format!("tribal context {} {}", verb.cli, turn_id.as_str()));
    }
    lines.truncate(MAX_AFFORDANCES);
    lines
}

#[cfg(test)]
mod tests {
    use lineage_core::LineageId;
    use lineage_retrieval::{EvidenceTier, Strength};

    use super::*;

    fn evidence(turn_id: Option<&str>, line_ranges: Vec<[u32; 2]>) -> Evidence {
        Evidence {
            session_id: LineageId::from("s1".to_string()),
            turn_id: turn_id.map(|t| LineageId::from(t.to_string())),
            tier: EvidenceTier::IntentMatch,
            strength: Strength::Medium,
            match_confidence: None,
            line_ranges,
            summary: "body".into(),
            attribution: "claude".into(),
        }
    }

    #[test]
    fn handles_round_trip_through_the_verbs() {
        let entry = evidence(Some("t1"), Vec::new());
        assert_eq!(turn_handle(&entry), "s1#t1");
        assert_eq!(parse_handle("s1#t1"), ("s1", Some("t1")));

        // Session-grained evidence has no turn, and the bare form round-trips.
        let session_only = evidence(None, Vec::new());
        assert_eq!(turn_handle(&session_only), "s1");
        assert_eq!(parse_handle("s1"), ("s1", None));
    }

    fn retrieval_of(entries: usize) -> Retrieval {
        Retrieval::from_evidence(
            (0..entries)
                .map(|i| {
                    let mut e = evidence(Some(&format!("t{i}")), Vec::new());
                    e.summary = format!("the words of turn {i}");
                    e
                })
                .collect(),
        )
    }

    #[test]
    fn the_file_keyed_digest_is_tight_and_carries_no_footer() {
        let retrieval = retrieval_of(3);
        let selected = select(&retrieval, Trigger::FileKeyed);
        assert_eq!(selected.len(), 1, "at most one pointer mid-task");

        let rendered = render_digest("src/lib.rs", &selected, Trigger::FileKeyed);
        assert!(!rendered.contains("Follow a handle with"));
        assert!(rendered.len() <= 200 * 4);
    }

    #[test]
    fn the_intent_digest_carries_handles_and_one_shared_footer() {
        let retrieval = retrieval_of(3);
        let selected = select(&retrieval, Trigger::Intent);
        assert_eq!(selected.len(), 3);

        let rendered = render_digest("src/lib.rs", &selected, Trigger::Intent);
        for i in 0..3 {
            assert!(
                rendered.contains(&format!("s1#t{i}")),
                "entry {i} is addressable: {rendered}"
            );
        }
        assert_eq!(
            rendered.matches("Follow a handle with").count(),
            1,
            "the vocabulary is named once, not per entry",
        );
    }

    /// The budget claim the reshape exists to make: navigation must cost under
    /// 5% of the intent cap. The footer is the whole navigation cost now — the
    /// handles ride along with attribution lines that would exist anyway.
    #[test]
    fn navigation_costs_under_five_percent_of_the_intent_budget() {
        let footer = verb_footer();
        let budget = 1024 * 4;
        assert!(
            footer.len() * 20 < budget,
            "footer is {} bytes of a {budget}-byte budget",
            footer.len(),
        );
    }

    #[test]
    fn an_empty_selection_renders_no_footer() {
        let rendered = render_digest("src/lib.rs", &[], Trigger::Intent);
        assert!(!rendered.contains("Follow a handle with"));
    }

    #[test]
    fn affordances_omit_relations_the_entry_cannot_honour() {
        let cmds = affordances_for(&evidence(None, Vec::new()), None);
        assert_eq!(cmds, vec!["tribal show s1"]);
    }

    #[test]
    fn a_line_anchored_turn_offers_the_chain_and_the_produced_by_verb() {
        let cmds = affordances_for(&evidence(Some("t1"), vec![[10, 12]]), Some("src/lib.rs"));
        assert!(cmds.contains(&"tribal context chain src/lib.rs:10".to_string()));
        assert!(cmds.contains(&"tribal context produced-by t1".to_string()));
        assert!(cmds.len() <= MAX_AFFORDANCES);
    }
}
