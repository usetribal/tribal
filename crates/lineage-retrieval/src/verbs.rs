//! The closed traversal vocabulary: every move a receiving agent can make over
//! the provenance graph, named once.
//!
//! Canned plans are fixed compositions of primitives; an agent traversing is a
//! dynamic composition of the same ones. Both speak this vocabulary, so no
//! capability exists for one consumer and not the other — the property that lets
//! observed traversal sequences later be derived into new canned plans.
//!
//! Relations are named **abstractly** here and rendered per surface: the CLI
//! turns `search-within` into a `git lineage context …` command, MCP turns it
//! into a tool name. An MCP-connected agent never shells out, so command strings
//! must not live in a crate MCP consumes.

/// One agent-exposed traversal move. `relation` is the abstract edge name that
/// appears in a digest's per-entry edge statements; `cli` and `mcp` are the two
/// surfaces' spellings, kept here so the equality test can assert both surfaces
/// carry exactly this set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verb {
    pub relation: &'static str,
    pub cli: &'static str,
    pub mcp: &'static str,
    pub summary: &'static str,
}

/// The v1 vocabulary, hand-written. Each entry is derived from a way the
/// injected set can be *wrong* and the repair for it: wrong turns inside the
/// right sessions, a turn missing its argument, a turn whose outcome is
/// unknown, and a commit whose reasoning is unknown.
///
/// Deliberately not generated. A generator would have to model clap arg shapes,
/// JSON Schema, and relation names — three targets with different requirements —
/// making it larger than what it generates, with no second batch of verbs to
/// amortise it. The anti-drift guarantee the codegen would have bought comes
/// from `VERBS` plus one equality test over the two surfaces.
pub const VERBS: &[Verb] = &[
    Verb {
        relation: "search-within",
        cli: "search-within",
        mcp: "lineage_search_within",
        summary: "search the text of specific sessions (one call, not N greps)",
    },
    Verb {
        relation: "around",
        cli: "around",
        mcp: "lineage_turns_around",
        summary: "read the turns immediately before and after a turn",
    },
    Verb {
        relation: "produced-by",
        cli: "produced-by",
        mcp: "lineage_produced_by",
        summary: "list the code a turn produced (file:line ranges)",
    },
    Verb {
        relation: "sessions-for-commit",
        cli: "sessions-for-commit",
        mcp: "lineage_sessions_for_commit",
        summary: "find the sessions behind a commit",
    },
];

/// The verb whose relation this is, or `None` for a name outside the
/// vocabulary. Surfaces look verbs up rather than re-spelling them.
pub fn verb_for_relation(relation: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|v| v.relation == relation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relations_are_unique_and_resolvable() {
        let mut relations: Vec<&str> = VERBS.iter().map(|v| v.relation).collect();
        let count = relations.len();
        relations.sort_unstable();
        relations.dedup();
        assert_eq!(relations.len(), count, "relation names must be unique");

        for verb in VERBS {
            assert_eq!(verb_for_relation(verb.relation), Some(verb));
        }
        assert!(verb_for_relation("no-such-relation").is_none());
    }

    /// Surface spellings are what the CLI and MCP registry tests compare
    /// against, so a blank one would make those tests vacuous.
    #[test]
    fn every_verb_names_both_surfaces() {
        for verb in VERBS {
            assert!(!verb.cli.is_empty(), "{} has no CLI name", verb.relation);
            assert!(
                verb.mcp.starts_with("lineage_"),
                "{} must be a lineage_ tool",
                verb.relation
            );
            assert!(!verb.summary.is_empty());
        }
    }
}
