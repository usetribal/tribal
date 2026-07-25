//! The thin plan runner and the two canned plans. A plan is ordinary Rust that
//! composes primitives and retrievers; the runner only threads a deadline
//! through the stages, records per-stage elapsed, and lets a stage exit early to
//! honest-nothing (spec: the miss path answers "nothing" well inside budget).
//! This is deliberately not a DAG engine — the plan's control flow is the code.

use std::time::{Duration, Instant};

use git2::Repository;
use lineage_search::LineageIndex;

use crate::primitives::{
    materialize_turns, time_search, turns_from_line_objects, AnchoredTurn, LineRef,
    MaterializeAnchor, TurnRef, MIN_ADMITTED_STRENGTH,
};
use crate::retriever::{IntentRetriever, Result};
use crate::types::{IntentQuery, Retrieval};

/// One stage's cost, for the routing log and the measurement pass. Printable so
/// `--timing` can dump the trace without the plan owning a formatter.
#[derive(Debug, Clone)]
pub struct StageTiming {
    pub name: &'static str,
    pub elapsed_ms: u128,
}

/// Threads a single deadline through a plan's stages and accumulates their
/// timings. Boring by design: `stage(name, closure)` times the closure; a stage
/// may check `over_budget()` and return early. The budget guard is advisory —
/// stages are synchronous and not cancellable, so a plan honours the deadline by
/// *checking* it between stages, never by interrupting one (the hook is a
/// one-shot process; spec: return what you have rather than overrun).
pub struct PlanRun {
    started: Instant,
    budget: Option<Duration>,
    timings: Vec<StageTiming>,
}

impl PlanRun {
    pub fn new(budget_ms: Option<u64>) -> Self {
        Self {
            started: Instant::now(),
            budget: budget_ms.map(Duration::from_millis),
            timings: Vec::new(),
        }
    }

    /// True once the elapsed time has reached the budget — the signal a plan
    /// checks before starting the next stage.
    pub fn over_budget(&self) -> bool {
        self.budget
            .is_some_and(|budget| self.started.elapsed() >= budget)
    }

    /// Run `body` as a named stage, recording its wall time. The result is
    /// passed straight through so stages compose without ceremony.
    pub fn stage<T>(&mut self, name: &'static str, body: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let out = body();
        self.timings.push(StageTiming {
            name,
            elapsed_ms: started.elapsed().as_millis(),
        });
        out
    }

    pub fn timings(&self) -> &[StageTiming] {
        &self.timings
    }
}

/// How many verbatim turns a plan materializes. Over-materializing past the
/// selector's entry cap wastes privacy/text reads on entries selection will
/// drop, so plans stop at the cap.
const MATERIALIZE_LIMIT: usize = 3;

/// The output of a plan: the wire `Retrieval` (unchanged — cache, selection, and
/// the event log stay untouched) plus the per-stage timing trace for the routing
/// log. The optional anchor file lets the CLI render the `earlier-edits`
/// affordance for a line-anchored plan.
pub struct PlanResult {
    pub retrieval: Retrieval,
    pub timings: Vec<StageTiming>,
    pub anchor_file: Option<String>,
}

/// The fused-salient-turn plan: FTS ∥ dense → RRF → materialize ≤3 verbatim
/// turns with `session` affordances. Query expansion is out of scope, so there
/// is no expand stage yet. The producer legs run inside `retriever` (already
/// fused); this plan owns the budget guard, the turn→evidence materialization,
/// and the timing trace.
pub fn fused_salient_turn_plan<R: IntentRetriever>(
    retriever: &R,
    query: &IntentQuery,
) -> Result<PlanResult> {
    let mut run = PlanRun::new(query.budget_ms);

    let fused = run.stage("retrieve_fused", || retriever.retrieve_intent(query))?;

    // The fused retriever already materialized verbatim evidence with privacy
    // applied; re-materializing here would double the reads for no gain. The
    // plan's job past retrieval is only to honour the budget and cap entries.
    let mut retrieval = fused;
    if run.over_budget() {
        retrieval.truncated = true;
    }
    retrieval.evidence.truncate(MATERIALIZE_LIMIT);

    Ok(PlanResult {
        retrieval,
        timings: run.timings().to_vec(),
        anchor_file: None,
    })
}

/// The line-anchored temporal plan: a file[:line] anchor →
/// `line_objects`/ancestry → turns (time-ordered) → salience-admitted →
/// materialize with `session` + `earlier-edits` affordances. When `text` is
/// given it re-ranks the anchored turns by FTS score of their bodies (a cheap
/// filter, not a second retrieval). One live blame anchors HEAD; everything
/// after is indexed reads.
pub fn line_anchored_temporal_plan(
    repo: &Repository,
    index: &LineageIndex,
    anchor: &LineRef,
    line: Option<u32>,
    text: Option<&str>,
    budget_ms: Option<u64>,
) -> Result<PlanResult> {
    let mut run = PlanRun::new(budget_ms);

    // The direct authors of the anchored file/line, most recent first.
    let mut anchored = run.stage("turns_from_line_objects", || {
        turns_from_line_objects(index, &anchor.file_path, line)
    })?;

    // The ancestry walk extends the anchor's authorship back through the turns
    // that touched the region across commits — turns the current-commit line
    // objects alone miss (a line rewritten since still has predecessors).
    let walk_authors = run.stage("time_search", || time_search(index, anchor))?;
    merge_walk_authors(&mut anchored, walk_authors);

    if let Some(text) = text {
        run.stage("rerank_by_text", || {
            rerank_by_text(index, &mut anchored, text)
        });
    }

    let turns: Vec<TurnRef> = anchored
        .iter()
        .take(MATERIALIZE_LIMIT)
        .map(|a| a.turn.clone())
        .collect();
    let anchors: Vec<Option<MaterializeAnchor>> = anchored
        .iter()
        .take(MATERIALIZE_LIMIT)
        .map(|a| {
            Some(MaterializeAnchor {
                file_path: a.file_path.clone(),
                line_range: a.line_range,
                confidence: a.confidence,
            })
        })
        .collect();

    let evidence = run.stage("materialize_turns", || {
        materialize_turns(repo, index, &turns, &anchors)
    })?;
    let admitted: Vec<_> = evidence
        .into_inner()
        .into_iter()
        .filter(|e| e.strength >= MIN_ADMITTED_STRENGTH)
        .collect();

    let mut retrieval = Retrieval::from_evidence(admitted);
    retrieval.truncated = run.over_budget();
    Ok(PlanResult {
        retrieval,
        timings: run.timings().to_vec(),
        anchor_file: Some(anchor.file_path.clone()),
    })
}

/// Fold ancestry-walk authors into the line-object authors, de-duplicating by
/// turn and preserving recency order (line-object authors, then older walk
/// authors the current commit's objects did not already cover). A dark hop has
/// no turn, so it contributes nothing here.
fn merge_walk_authors(anchored: &mut Vec<AnchoredTurn>, hops: Vec<lineage_search::Hop>) {
    let mut seen: std::collections::HashSet<String> =
        anchored.iter().map(|a| a.turn.turn_id.clone()).collect();
    for hop in hops {
        let (Some(session_id), Some(turn_id)) = (hop.session_id, hop.turn_id) else {
            continue;
        };
        if !seen.insert(turn_id.clone()) {
            continue;
        }
        anchored.push(AnchoredTurn {
            turn: TurnRef {
                session_id,
                turn_id,
            },
            file_path: hop.file_path,
            line_range: [hop.start_line, hop.end_line],
            committed_at: hop.committed_at,
            confidence: hop
                .confidence
                .as_deref()
                .map(crate::primitives::parse_confidence)
                .unwrap_or(lineage_core::Confidence::Heuristic),
        });
    }
}

/// Re-rank anchored turns by how well their bodies match the free text, keeping
/// only matched turns ahead of unmatched ones (stable within each group so the
/// time order survives ties). Cheap: one FTS query, then a membership lookup —
/// not a second full retrieval.
fn rerank_by_text(index: &LineageIndex, anchored: &mut [AnchoredTurn], text: &str) {
    let Ok(hits) = index.search_turns(text, 200) else {
        return;
    };
    let rank_of: std::collections::HashMap<String, usize> = hits
        .into_iter()
        .enumerate()
        .map(|(i, h)| (h.turn_id, i))
        .collect();
    // Turns the text matched sort by FTS rank; unmatched turns keep their time
    // order after them. `usize::MAX` is the "unmatched" sentinel.
    anchored.sort_by_key(|a| *rank_of.get(&a.turn.turn_id).unwrap_or(&usize::MAX));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{strength_for, Evidence, EvidenceTier};
    use lineage_core::LineageId;

    struct CannedLeg(Vec<String>);

    impl IntentRetriever for CannedLeg {
        fn retrieve_intent(&self, _query: &IntentQuery) -> Result<Retrieval> {
            let evidence = self
                .0
                .iter()
                .map(|s| Evidence {
                    session_id: LineageId::from(s.clone()),
                    turn_id: Some(LineageId::from(format!("turn-{s}"))),
                    tier: EvidenceTier::IntentMatch,
                    strength: strength_for(EvidenceTier::IntentMatch, None),
                    match_confidence: None,
                    line_ranges: Vec::new(),
                    summary: format!("summary {s}"),
                    attribution: format!("claude {s}"),
                })
                .collect();
            Ok(Retrieval::from_evidence(evidence))
        }
    }

    #[test]
    fn plan_run_records_stage_timings_in_order() {
        let mut run = PlanRun::new(None);
        run.stage("a", || {});
        run.stage("b", || {});
        let names: Vec<_> = run.timings().iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn plan_run_flags_over_budget_after_deadline() {
        let run = PlanRun::new(Some(0));
        // A zero budget is already spent by the time the first stage would run.
        assert!(run.over_budget());
        let generous = PlanRun::new(Some(60_000));
        assert!(!generous.over_budget());
    }

    #[test]
    fn fused_plan_caps_entries_and_traces_a_stage() {
        let leg = CannedLeg(vec!["a".into(), "b".into(), "c".into(), "d".into()]);
        let query = IntentQuery {
            text: "x".into(),
            budget_ms: None,
        };
        let result = fused_salient_turn_plan(&leg, &query).unwrap();
        assert_eq!(
            result.retrieval.evidence.len(),
            3,
            "capped to the materialize limit"
        );
        assert!(result.timings.iter().any(|t| t.name == "retrieve_fused"));
        assert!(result.anchor_file.is_none());
    }

    fn commit_file(dir: &std::path::Path, path: &str, contents: &str) -> String {
        std::fs::write(dir.join(path), contents).unwrap();
        for args in [
            vec!["add", path],
            vec![
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "add",
            ],
        ] {
            std::process::Command::new("git")
                .args(&args)
                .current_dir(dir)
                .output()
                .unwrap();
        }
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn temporal_plan_returns_line_anchored_evidence() {
        use lineage_core::{AgentKind, Conversation, LineObject, Role, Turn};

        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let repo = lineage_git::open_repo(dir.path()).unwrap();

        let mut conv = Conversation::new(AgentKind::Claude, dir.path().display().to_string());
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: "why is this line here — the caching decision".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        lineage_git::persist_conversation(repo.inner(), &conv).unwrap();
        let commit = commit_file(dir.path(), "lib.rs", "fn cached() {}\n");

        let obj = LineObject::new(
            "lib.rs",
            [1, 1],
            commit.clone(),
            conv.id.clone(),
            conv.turns[0].id.clone(),
            lineage_core::Confidence::Exact,
        );
        lineage_git::write_line_object(repo.inner(), &obj).unwrap();

        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
        index.index_conversation(&conv).unwrap();
        index
            .populate_line_tables(repo.inner(), &mut |_, _| {})
            .unwrap();

        let anchor = LineRef {
            file_path: "lib.rs".into(),
            line: 1,
            commit_sha: commit,
        };
        let result =
            line_anchored_temporal_plan(repo.inner(), &index, &anchor, Some(1), None, None)
                .unwrap();

        assert_eq!(result.retrieval.evidence.len(), 1);
        let ev = &result.retrieval.evidence[0];
        assert_eq!(ev.tier, EvidenceTier::LineObjects);
        assert!(ev.summary.contains("caching decision"));
        assert!(result.timings.iter().any(|t| t.name == "time_search"));
        // The anchor file is what lets the CLI render an `earlier-edits`
        // pointer; the rendering itself is the CLI's concern now.
        assert_eq!(result.anchor_file.as_deref(), Some("lib.rs"));
    }

    #[test]
    fn temporal_plan_no_lineage_is_honest_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let repo = lineage_git::open_repo(dir.path()).unwrap();
        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();

        let anchor = LineRef {
            file_path: "nope.rs".into(),
            line: 1,
            commit_sha: "0".repeat(40),
        };
        let result =
            line_anchored_temporal_plan(repo.inner(), &index, &anchor, Some(1), None, None)
                .unwrap();
        assert!(result.retrieval.evidence.is_empty());
    }
}
