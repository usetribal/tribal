//! Presentation for the injected digest: which evidence is shown, how it is
//! rendered, and which traversal moves are named alongside it.
//!
//! Split from `context_cmd` (which keeps hook I/O and the settings installer)
//! because the seam is real — this side derives nothing from a repo and so is
//! testable without constructing hook payloads.
//!
//! Relation → command rendering lives here rather than in `lineage-retrieval`:
//! MCP consumes that crate and never shells out, so `git lineage …` strings must
//! not be visible to it. The registry names relations; each surface spells them.

use lineage_retrieval::{verb_for_relation, Evidence};

/// Capped so the pointers stay a footer, not the payload.
pub const MAX_AFFORDANCES: usize = 3;

/// The affordance relations this evidence entry can honour, rendered as runnable
/// `git lineage` commands (spec: Verbatim-turn digest — a selector MUST omit
/// relations it cannot honour).
///
/// `session` is always available and is not a traversal verb — the whole
/// conversation prints the turn uncapped, which is why the spec's `full-turn`
/// folds into it. The rest are looked up in the verb registry, so a verb the
/// installed CLI does not have cannot be named here.
pub fn affordances_for(evidence: &Evidence, anchor_file: Option<&str>) -> Vec<String> {
    let mut lines = vec![format!("git lineage show {}", evidence.session_id.as_str())];
    if let (Some(file), Some([start, _])) = (anchor_file, evidence.line_ranges.first()) {
        lines.push(format!("git lineage context chain {file}:{start}"));
    }
    if let (Some(turn_id), Some(verb)) = (&evidence.turn_id, verb_for_relation("produced-by")) {
        lines.push(format!(
            "git lineage context {} {}",
            verb.cli,
            turn_id.as_str()
        ));
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
    fn affordances_omit_relations_the_entry_cannot_honour() {
        let cmds = affordances_for(&evidence(None, Vec::new()), None);
        assert_eq!(cmds, vec!["git lineage show s1"]);
    }

    #[test]
    fn a_line_anchored_turn_offers_the_chain_and_the_produced_by_verb() {
        let cmds = affordances_for(&evidence(Some("t1"), vec![[10, 12]]), Some("src/lib.rs"));
        assert!(cmds.contains(&"git lineage context chain src/lib.rs:10".to_string()));
        assert!(cmds.contains(&"git lineage context produced-by t1".to_string()));
        assert!(cmds.len() <= MAX_AFFORDANCES);
    }
}
