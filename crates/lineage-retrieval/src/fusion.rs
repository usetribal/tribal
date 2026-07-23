use std::collections::HashMap;

use crate::retriever::{IntentRetriever, Result};
use crate::types::{Evidence, IntentQuery, Retrieval};

/// RRF's rank-damping constant. `k=60` is the value from the original 2009
/// paper and holds up well across domains; a lower `k` lets one leg's top hit
/// dominate (precision), a higher `k` flattens rank differences. A tunable, not
/// a law — the eval stage calibrates it (gotcha F.1).
pub const DEFAULT_RRF_K: f64 = 60.0;

/// Rung 1.5 — fuse two intent legs (lexical + dense) by reciprocal-rank fusion.
/// RRF combines ranks, not scores, side-stepping the incompatible score scales
/// of BM25 and cosine (gotcha F.4). Both legs emit *turn*-ranked lists (the
/// dense leg rolls chunks up to anchor turns first), so fusion is over a common
/// key and a turn found by only one leg still survives — the failure mode where
/// a semantic miss loses an exact keyword hit, or vice versa, cannot happen.
pub struct FusedRetriever<A: IntentRetriever, B: IntentRetriever> {
    lexical: A,
    dense: B,
    k: f64,
}

impl<A: IntentRetriever, B: IntentRetriever> FusedRetriever<A, B> {
    pub fn new(lexical: A, dense: B) -> Self {
        Self {
            lexical,
            dense,
            k: DEFAULT_RRF_K,
        }
    }

    pub fn with_k(mut self, k: f64) -> Self {
        self.k = k;
        self
    }
}

/// One turn's fused state: its RRF score so far and the evidence to emit for
/// it (whichever leg saw it first — both carry the same verbatim turn text).
struct Fused {
    score: f64,
    evidence: Evidence,
}

/// The RRF key: the turn when the evidence is turn-grained, the session
/// otherwise — so a leg that cannot resolve turns (or session-grained
/// evidence in a mixed pipeline) still fuses instead of vanishing.
fn fusion_key(evidence: &Evidence) -> String {
    evidence
        .turn_id
        .as_ref()
        .unwrap_or(&evidence.session_id)
        .as_str()
        .to_string()
}

/// Accumulate one leg's ranked evidence into the fused map: each entry at rank
/// `r` (0-based) contributes `1/(k + r + 1)`. A turn present in both legs
/// sums both contributions, which is what lifts agreed-upon results.
fn accumulate(fused: &mut HashMap<String, Fused>, retrieval: Retrieval, k: f64) {
    for (rank, evidence) in retrieval.evidence.into_iter().enumerate() {
        let contribution = 1.0 / (k + (rank as f64) + 1.0);
        fused
            .entry(fusion_key(&evidence))
            .and_modify(|f| f.score += contribution)
            .or_insert(Fused {
                score: contribution,
                evidence,
            });
    }
}

impl<A: IntentRetriever, B: IntentRetriever> IntentRetriever for FusedRetriever<A, B> {
    fn retrieve_intent(&self, query: &IntentQuery) -> Result<Retrieval> {
        let lexical = self.lexical.retrieve_intent(query)?;
        let dense = self.dense.retrieve_intent(query)?;

        // A leg that truncated on its budget produces a partial ranking; the
        // fused result inherits that so the caller still knows it may be short.
        let truncated = lexical.truncated || dense.truncated;

        let mut fused: HashMap<String, Fused> = HashMap::new();
        accumulate(&mut fused, lexical, self.k);
        accumulate(&mut fused, dense, self.k);

        let mut entries: Vec<Fused> = fused.into_values().collect();
        entries.sort_by(|a, b| b.score.total_cmp(&a.score));

        // Evidence carries only the coarse `strength`; the fine RRF order is
        // preserved by emitting entries in fused-score order. `from_evidence`'s
        // strength-sort is stable, so ties keep this order.
        let evidence: Vec<Evidence> = entries.into_iter().map(|f| f.evidence).collect();
        let mut retrieval = Retrieval::from_evidence(evidence);
        retrieval.truncated = truncated;
        Ok(retrieval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{strength_for, EvidenceTier, Strength};
    use lineage_core::LineageId;

    /// A canned leg: returns a fixed session ranking, so fusion is tested in
    /// isolation from any index or embedder.
    struct CannedLeg(Vec<String>);

    fn evidence_for(session_id: &str) -> Evidence {
        Evidence {
            session_id: LineageId::from(session_id.to_string()),
            turn_id: None,
            tier: EvidenceTier::IntentMatch,
            strength: strength_for(EvidenceTier::IntentMatch, None),
            match_confidence: None,
            line_ranges: Vec::new(),
            summary: format!("summary for {session_id}"),
            attribution: format!("claude session {session_id}, 2026-07-01"),
        }
    }

    impl IntentRetriever for CannedLeg {
        fn retrieve_intent(&self, _query: &IntentQuery) -> Result<Retrieval> {
            let evidence = self.0.iter().map(|s| evidence_for(s)).collect();
            Ok(Retrieval::from_evidence(evidence))
        }
    }

    fn query() -> IntentQuery {
        IntentQuery {
            text: "anything".into(),
            budget_ms: None,
        }
    }

    fn ranked_ids(retrieval: &Retrieval) -> Vec<String> {
        retrieval
            .evidence
            .iter()
            .map(|e| e.session_id.as_str().to_string())
            .collect()
    }

    #[test]
    fn agreement_between_legs_outranks_a_single_leg_top_hit() {
        // "b" is #2 in both legs; "a" is #1 in lexical only, "x" #1 in dense
        // only. Agreement should lift "b" above the single-leg leaders.
        let lexical = CannedLeg(vec!["a".into(), "b".into()]);
        let dense = CannedLeg(vec!["x".into(), "b".into()]);
        let fused = FusedRetriever::new(lexical, dense);

        let out = fused.retrieve_intent(&query()).unwrap();
        assert_eq!(ranked_ids(&out)[0], "b");
    }

    #[test]
    fn a_single_leg_hit_still_survives_fusion() {
        // The either-or failure mode we exist to prevent: dense misses the
        // exact keyword hit "a", but fusion must still include it.
        let lexical = CannedLeg(vec!["a".into()]);
        let dense = CannedLeg(vec!["z".into()]);
        let fused = FusedRetriever::new(lexical, dense);

        let ids = ranked_ids(&fused.retrieve_intent(&query()).unwrap());
        assert!(ids.contains(&"a".to_string()));
        assert!(ids.contains(&"z".to_string()));
    }

    #[test]
    fn empty_legs_are_honest_nothing() {
        let fused = FusedRetriever::new(CannedLeg(vec![]), CannedLeg(vec![]));
        let out = fused.retrieve_intent(&query()).unwrap();
        assert!(out.evidence.is_empty());
        assert_eq!(out.strength, Strength::None);
    }

    fn rank_of(retrieval: &Retrieval, id: &str) -> usize {
        ranked_ids(retrieval).iter().position(|s| s == id).unwrap()
    }

    #[test]
    fn k_tunes_top_rank_vs_agreement() {
        // "solo" is #1 in lexical only; "both" is #2 in each leg. Low k rewards
        // the single top hit (solo ahead); high k flattens ranks so the agreed
        // "both" (two contributions) overtakes it. The relative order flips with
        // k — which is why k is a tunable, not a constant. Distinct per-leg
        // filler avoids score ties that would make the order nondeterministic.
        // "both" sits at rank 2 (0-based) in each leg — deep enough that at low
        // k its two contributions still lose to solo's single rank-0 hit, but at
        // high k (flat ranks) the two contributions win.
        let legs = || {
            (
                CannedLeg(vec!["solo".into(), "l1".into(), "both".into()]),
                CannedLeg(vec!["d1".into(), "d2".into(), "both".into()]),
            )
        };

        let (lex, den) = legs();
        let low = low_k_result(lex, den);
        assert!(rank_of(&low, "solo") < rank_of(&low, "both"));

        let (lex, den) = legs();
        let high = high_k_result(lex, den);
        assert!(rank_of(&high, "both") < rank_of(&high, "solo"));
    }

    fn low_k_result(lex: CannedLeg, den: CannedLeg) -> Retrieval {
        FusedRetriever::new(lex, den)
            .with_k(0.1)
            .retrieve_intent(&query())
            .unwrap()
    }

    fn high_k_result(lex: CannedLeg, den: CannedLeg) -> Retrieval {
        FusedRetriever::new(lex, den)
            .with_k(1000.0)
            .retrieve_intent(&query())
            .unwrap()
    }
}
