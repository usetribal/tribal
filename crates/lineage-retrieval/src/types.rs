use lineage_core::{Confidence, LineageId, RepoBinding};
use serde::{Deserialize, Serialize};

pub const CONTEXT_QUERY_SCHEMA: &str = "context-query-v0";
pub const RETRIEVAL_SCHEMA: &str = "retrieval-v0";

/// One retrieval request, regardless of where the retriever runs. These types
/// are the wire contract for the future server-side retriever
/// (specs/context-injection-v0.md), so field names are frozen by the spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextQuery {
    pub file_path: String,
    pub file_blob_sha: String,
    pub repo: RepoBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_ms: Option<u64>,
}

/// An intent (prompt-keyed) retrieval request: free-text intent, no file or
/// line anchor. Distinct from `ContextQuery` because that shape is file-keyed
/// and frozen by the injection spec — intent matching is a different surface
/// (the `UserPromptSubmit` trigger) with a different query shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentQuery {
    /// The user's message / intent text to match against the corpus.
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_ms: Option<u64>,
}

/// Ordered relevance scale for selection floors and cache heuristics.
/// Variant order is the total order (`None < Low < Medium < High`) — `Ord`
/// derives from it, so do not reorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strength {
    None,
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTier {
    LineObjects,
    FilesTouched,
    /// The session's content matched an intent query (lexical or dense). Not
    /// anchored to a file or line — the match is against what the session was
    /// about. Ranking within a retrieval preserves the retriever's relevance
    /// order; strength is a coarse floor (see `strength_for`).
    IntentMatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub session_id: LineageId,
    pub tier: EvidenceTier,
    pub strength: Strength,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_confidence: Option<Confidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub line_ranges: Vec<[u32; 2]>,
    pub summary: String,
    pub attribution: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retrieval {
    pub evidence: Vec<Evidence>,
    pub strength: Strength,
    /// True when retrieval stopped early on `budget_ms`, so an empty result
    /// means "ran out of time", not "nothing known" (diagnostics-v0
    /// `over_budget`). Defaults false for cache entries written before the
    /// field existed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl Retrieval {
    /// The honest "nothing known" answer — cached like any other (spec: Cache).
    pub fn empty() -> Self {
        Self {
            evidence: Vec::new(),
            strength: Strength::None,
            truncated: false,
        }
    }

    /// Orders evidence strongest-first and derives the overall grade, keeping
    /// the two spec invariants (ordering, `strength = max`) in one place.
    pub fn from_evidence(mut evidence: Vec<Evidence>) -> Self {
        evidence.sort_by_key(|e| std::cmp::Reverse(e.strength));
        let strength = evidence
            .iter()
            .map(|e| e.strength)
            .max()
            .unwrap_or(Strength::None);
        Self {
            evidence,
            strength,
            truncated: false,
        }
    }
}

/// The spec's tier → strength mapping. Strength is always derived from its
/// inputs, never asserted independently — `Confidence` is match quality
/// (`manual` is not "less than" `exact`), so it cannot serve as the order.
pub fn strength_for(tier: EvidenceTier, match_confidence: Option<Confidence>) -> Strength {
    match (tier, match_confidence) {
        (EvidenceTier::LineObjects, Some(Confidence::Exact | Confidence::Manual)) => Strength::High,
        (EvidenceTier::LineObjects, _) => Strength::Medium,
        // A content match is real evidence — stronger than a bare files-touched
        // link, below an exact line-object. The retriever's own relevance order
        // (preserved by evidence ranking) carries the finer signal; strength is
        // only the coarse floor selection acts on.
        (EvidenceTier::IntentMatch, _) => Strength::Medium,
        (EvidenceTier::FilesTouched, _) => Strength::Low,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(strength: Strength) -> Evidence {
        Evidence {
            session_id: LineageId::new(),
            tier: EvidenceTier::FilesTouched,
            strength,
            match_confidence: None,
            line_ranges: Vec::new(),
            summary: "touched src/lib.rs".into(),
            attribution: "claude session, 2026-07-01".into(),
        }
    }

    #[test]
    fn strength_order_is_total() {
        assert!(Strength::None < Strength::Low);
        assert!(Strength::Low < Strength::Medium);
        assert!(Strength::Medium < Strength::High);
    }

    #[test]
    fn strength_mapping_follows_spec() {
        use EvidenceTier::*;
        assert_eq!(
            strength_for(LineObjects, Some(Confidence::Exact)),
            Strength::High
        );
        assert_eq!(
            strength_for(LineObjects, Some(Confidence::Manual)),
            Strength::High
        );
        assert_eq!(
            strength_for(LineObjects, Some(Confidence::Heuristic)),
            Strength::Medium
        );
        assert_eq!(strength_for(FilesTouched, None), Strength::Low);
    }

    #[test]
    fn from_evidence_orders_strongest_first_and_takes_max() {
        let retrieval = Retrieval::from_evidence(vec![
            evidence(Strength::Low),
            evidence(Strength::High),
            evidence(Strength::Medium),
        ]);
        assert_eq!(retrieval.strength, Strength::High);
        let strengths: Vec<Strength> = retrieval.evidence.iter().map(|e| e.strength).collect();
        assert_eq!(
            strengths,
            vec![Strength::High, Strength::Medium, Strength::Low]
        );
    }

    #[test]
    fn empty_retrieval_has_strength_none() {
        let retrieval = Retrieval::empty();
        assert_eq!(retrieval.strength, Strength::None);
        assert_eq!(Retrieval::from_evidence(Vec::new()), retrieval);
    }

    #[test]
    fn wire_shapes_round_trip_with_spec_field_names() {
        let query = ContextQuery {
            file_path: "src/auth.rs".into(),
            file_blob_sha: "ab".repeat(32),
            repo: RepoBinding {
                normalized_remote_url: "github.com/acme/widgets".into(),
                root_commit_sha: "cd".repeat(20),
                server_repo_id: None,
            },
            budget_ms: Some(150),
        };
        let json = serde_json::to_value(&query).unwrap();
        assert_eq!(json["file_blob_sha"], query.file_blob_sha);
        assert_eq!(json["budget_ms"], 150);
        let back: ContextQuery = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), json);

        let retrieval = Retrieval::from_evidence(vec![evidence(Strength::Low)]);
        let json = serde_json::to_value(&retrieval).unwrap();
        assert_eq!(json["strength"], "low");
        assert_eq!(json["evidence"][0]["tier"], "files_touched");
        // Empty/absent optionals stay off the wire, matching how lineage-core
        // serializes documents (sync-protocol-v0 "Content hash" canonical form).
        assert!(json["evidence"][0].get("match_confidence").is_none());
        assert!(json["evidence"][0].get("line_ranges").is_none());
        let back: Retrieval = serde_json::from_value(json).unwrap();
        assert_eq!(back, retrieval);
    }
}
