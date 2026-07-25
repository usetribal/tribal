//! The typed primitive layer: small, independently testable operations the plan
//! runner composes by ordinary Rust — not a DAG engine. Composition is type
//! compatibility (a primitive that yields `TurnRef`s feeds one that consumes
//! them), and every primitive that yields turn *text* returns it as
//! [`Gated`], which only `SessionGate` can construct — so no plan and no
//! agent-facing verb can emit evidence without the privacy filter having run
//! (spec: Privacy — filtering at the source).
//!
//! The producer primitives (FTS / dense / fusion) already exist as
//! `IntentRetriever`s and are not re-wrapped here; this module adds the
//! line-anchored and materialization vocabulary the temporal plan needs.

use git2::Repository;
use lineage_core::Confidence;
use lineage_search::{Hop, LineObjectRow, LineageIndex};

use crate::retriever::{Result, RetrievalError};
use crate::session::{verbatim_summary, Gated, SessionGate};
use crate::types::{strength_for, Evidence, EvidenceTier, Strength};

/// A conversation turn, identified with its session (privacy and attribution are
/// session-keyed, so a turn never travels without the session it belongs to).
/// Internal composition type — not the wire `Evidence`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRef {
    pub session_id: String,
    pub turn_id: String,
}

/// A line position: a file, a line, and the commit the position is at. The
/// anchor a temporal walk starts from; also what an `earlier-edits` affordance
/// points back to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRef {
    pub file_path: String,
    pub line: u32,
    pub commit_sha: String,
}

/// A turn resolved from a line object, carrying the metadata the temporal plan
/// ranks and materializes on: which line object attributed it, its commit time
/// (for time ordering), and the match confidence (for strength).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchoredTurn {
    pub turn: TurnRef,
    pub file_path: String,
    pub line_range: [u32; 2],
    pub committed_at: i64,
    pub confidence: Confidence,
}

/// A session surfaced from its member turns, keeping the best (lowest) rank any
/// of its turns achieved — the bridge from turn-grained ranking to a
/// session-level surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedSession {
    pub session_id: String,
    pub best_rank: usize,
}

/// `time_search` — the ancestry walk as a primitive. A thin wrapper over the
/// index's indexed-only `line_history`; the one live blame that anchors HEAD is
/// the caller's job (`resolve_anchor`), so this primitive never touches a repo
/// and cannot blame. Returns the hops oldest-visited-last, as the walk found
/// them.
pub fn time_search(index: &LineageIndex, anchor: &LineRef) -> Result<Vec<Hop>> {
    index
        .line_history(&anchor.file_path, anchor.line, &anchor.commit_sha)
        .map_err(|e| RetrievalError::Retrieval(e.to_string()))
}

/// `turns_from_line_objects` — resolve a file (optionally a specific line) to
/// the turns that wrote it, via the `line_objects` mirror, most recent first.
/// When `line` is given, only line objects whose range covers it survive, so a
/// line-anchored query narrows to the turns that touched that line. No walking:
/// a single indexed scan (the "why so bloated" aggregation query).
pub fn turns_from_line_objects(
    index: &LineageIndex,
    file_path: &str,
    line: Option<u32>,
) -> Result<Vec<AnchoredTurn>> {
    let rows = index
        .line_objects_for_file(file_path)
        .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
    Ok(rows
        .into_iter()
        .filter(|row| line.is_none_or(|l| row.start_line <= l && l <= row.end_line))
        .map(anchored_turn_from_row)
        .collect())
}

fn anchored_turn_from_row(row: LineObjectRow) -> AnchoredTurn {
    AnchoredTurn {
        turn: TurnRef {
            session_id: row.session_id,
            turn_id: row.turn_id,
        },
        file_path: row.file_path,
        line_range: [row.start_line, row.end_line],
        committed_at: row.committed_at,
        // A row whose stored confidence is unrecognized is treated as heuristic
        // (the weakest real match) rather than dropped — the mirror is derived
        // data and an unknown value should degrade, not vanish.
        confidence: parse_confidence(&row.confidence),
    }
}

pub(crate) fn parse_confidence(raw: &str) -> Confidence {
    match raw {
        "exact" => Confidence::Exact,
        "manual" => Confidence::Manual,
        _ => Confidence::Heuristic,
    }
}

/// `turns_to_sessions` — dedupe a ranked turn list upward to sessions, keeping
/// the best rank each session's turns achieved. Preserves the incoming order:
/// the first time a session is seen fixes its position, so the strongest turn's
/// rank is the session's rank.
pub fn turns_to_sessions(ranked_turns: &[TurnRef]) -> Vec<RankedSession> {
    let mut seen = std::collections::HashSet::new();
    let mut sessions = Vec::new();
    for (rank, turn) in ranked_turns.iter().enumerate() {
        if seen.insert(turn.session_id.clone()) {
            sessions.push(RankedSession {
                session_id: turn.session_id.clone(),
                best_rank: rank,
            });
        }
    }
    sessions
}

/// `materialize_turns` — turn refs → wire `Evidence`. The privacy filter and
/// attribution run here through `SessionGate`, before any evidence exists
/// (spec: Privacy enforced at the source). A turn whose session is private (or
/// whose fork chain reaches a private one) or whose text is missing yields no
/// evidence. `anchors[i]` optionally line-anchors turn `i` so the evidence
/// carries a file:line for the `earlier-edits` affordance.
///
/// The result is [`Gated`]: turn text can only be constructed on the far side
/// of the gate, so this stays the only shape a plan can emit even though it is
/// no longer the only exit.
pub fn materialize_turns(
    repo: &Repository,
    index: &LineageIndex,
    turns: &[TurnRef],
    anchors: &[Option<MaterializeAnchor>],
) -> Result<Gated<Vec<Evidence>>> {
    let mut gate = SessionGate::new(repo);
    let mut evidence = Vec::new();
    for (i, turn) in turns.iter().enumerate() {
        let Some(attribution) = gate.attribution(&turn.session_id)? else {
            continue;
        };
        let Some(row) = index
            .get_turn(&turn.turn_id)
            .map_err(|e| RetrievalError::Retrieval(e.to_string()))?
        else {
            continue;
        };
        let anchor = anchors.get(i).and_then(|a| a.as_ref());
        evidence.push(evidence_from_turn(turn, &row.body, &attribution, anchor));
    }
    Ok(gate.seal(evidence))
}

/// The line anchor a materialized turn carries: its confidence sets strength and
/// its range/file drive the `earlier-edits` affordance.
#[derive(Debug, Clone)]
pub struct MaterializeAnchor {
    pub file_path: String,
    pub line_range: [u32; 2],
    pub confidence: Confidence,
}

fn evidence_from_turn(
    turn: &TurnRef,
    body: &str,
    attribution: &str,
    anchor: Option<&MaterializeAnchor>,
) -> Evidence {
    // A line-anchored turn is line-object evidence (its confidence grades
    // strength); an intent match is not anchored and floors at medium.
    let (tier, match_confidence, line_ranges, strength) = match anchor {
        Some(a) => (
            EvidenceTier::LineObjects,
            Some(a.confidence),
            vec![a.line_range],
            strength_for(EvidenceTier::LineObjects, Some(a.confidence)),
        ),
        None => (
            EvidenceTier::IntentMatch,
            None,
            Vec::new(),
            strength_for(EvidenceTier::IntentMatch, None),
        ),
    };
    Evidence {
        session_id: turn.session_id.clone().into(),
        turn_id: Some(turn.turn_id.clone().into()),
        tier,
        strength,
        match_confidence,
        line_ranges,
        summary: verbatim_summary(body),
        attribution: attribution.to_string(),
    }
}

/// `search_within_sessions` — scoped FTS over a given session set: the repair
/// for "right sessions, wrong turns". One indexed query rather than N greps over
/// materialized transcripts, which is the whole point of having it as a verb.
/// Bounded by `limit`; an empty session set matches nothing.
pub fn search_within_sessions(
    repo: &Repository,
    index: &LineageIndex,
    session_ids: &[String],
    query: &str,
    limit: usize,
) -> Result<Gated<Vec<Evidence>>> {
    let hits = index
        .search_turns_in_sessions(session_ids, query, limit)
        .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
    let mut gate = SessionGate::new(repo);
    gate.admit_all(
        hits,
        |hit| hit.session_id.as_str(),
        |hit, attribution| {
            evidence_from_turn(
                &TurnRef {
                    session_id: hit.session_id,
                    turn_id: hit.turn_id,
                },
                &hit.body,
                attribution,
                None,
            )
        },
    )
}

/// `turn_neighbourhood` — the turns within `radius` positions of `turn_id` in
/// its own session, in conversation order: the repair for "right turn, missing
/// its argument". Bounded by `limit` as well as by `radius`, so a wide radius on
/// a long session cannot blow the budget.
pub fn turn_neighbourhood(
    repo: &Repository,
    index: &LineageIndex,
    turn_id: &str,
    radius: u32,
    limit: usize,
) -> Result<Gated<Vec<Evidence>>> {
    let rows = index
        .turns_around(turn_id, radius, limit)
        .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
    let mut gate = SessionGate::new(repo);
    gate.admit_all(
        rows,
        |row| row.session_id.as_str(),
        |row, attribution| {
            evidence_from_turn(
                &TurnRef {
                    session_id: row.session_id,
                    turn_id: row.turn_id,
                },
                &row.body,
                attribution,
                None,
            )
        },
    )
}

/// One region of code a turn produced: where it landed and how confidently it
/// is attributed. Refs only — no turn text — so this is ungated; the gate
/// applies at materialization, as it always has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducedLines {
    pub anchor: LineRef,
    pub line_range: [u32; 2],
    pub confidence: Confidence,
}

/// `line_objects_of_turn` — the code a turn produced: the repair for "right
/// turn, want its outcome", and the direction that makes the graph two-way.
/// Bounded by `limit`.
pub fn line_objects_of_turn(
    index: &LineageIndex,
    turn_id: &str,
    limit: usize,
) -> Result<Vec<ProducedLines>> {
    let rows = index
        .line_objects_for_turn(turn_id, limit)
        .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
    Ok(rows.into_iter().map(produced_lines_from_row).collect())
}

fn produced_lines_from_row(row: LineObjectRow) -> ProducedLines {
    ProducedLines {
        confidence: parse_confidence(&row.confidence),
        line_range: [row.start_line, row.end_line],
        anchor: LineRef {
            file_path: row.file_path,
            line: row.start_line,
            commit_sha: row.commit_sha,
        },
    }
}

/// A session reached by traversal, with the display label the gate produced for
/// it. The gate is what makes this shape safe to emit: a private session (or a
/// fork of one) never has an attribution, so it never becomes a `SessionRef`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub session_id: String,
    pub attribution: String,
}

/// `sessions_for_commit` — the sessions behind a commit: the one v1 verb whose
/// entry point is ordinary git work rather than an injected digest. Reads the
/// `session_commits` mirror, so it is one indexed lookup. Bounded by `limit`.
///
/// Gated even though it emits no turn text: a private session must not be named
/// as evidence at all (spec: Privacy).
pub fn sessions_for_commit(
    repo: &Repository,
    index: &LineageIndex,
    commit_sha: &str,
    limit: usize,
) -> Result<Gated<Vec<SessionRef>>> {
    let ids = index
        .sessions_for_commit(commit_sha, limit)
        .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
    let mut gate = SessionGate::new(repo);
    gate.admit_all(
        ids,
        |id| id.as_str(),
        |session_id, attribution| SessionRef {
            session_id,
            attribution: attribution.to_string(),
        },
    )
}

/// The strength floor a plan admits (spec: minimum strength `low`). Shared so
/// the temporal plan and the fused plan agree on what "admitted" means.
pub const MIN_ADMITTED_STRENGTH: Strength = Strength::Low;

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{
        AgentKind, Artifact, ArtifactKind, ArtifactResolve, Conversation, LineObject, LineageId,
        ResolveStrategy, Role, Turn,
    };
    use lineage_git::{open_repo, persist_conversation, write_line_object};

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    fn session_writing(dir: &std::path::Path, prompt: &str, path: &str) -> Conversation {
        let mut conv = Conversation::new(AgentKind::Claude, dir.display().to_string());
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::User,
            content: prompt.into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![lineage_core::ToolCall {
                id: "t".into(),
                name: "Edit".into(),
                arguments: format!(r#"{{"file_path": "{path}"}}"#),
                result: None,
            }],
            model: None,
            timestamp: None,
            artifacts: vec![Artifact {
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
                    new_string: Some("fn added() {}".into()),
                    patch: None,
                }),
            }],
        });
        conv
    }

    #[test]
    fn turns_to_sessions_keeps_best_rank_and_dedupes() {
        let turns = vec![
            TurnRef {
                session_id: "s1".into(),
                turn_id: "t1".into(),
            },
            TurnRef {
                session_id: "s2".into(),
                turn_id: "t2".into(),
            },
            // s1's second turn is lower-ranked; the session keeps rank 0.
            TurnRef {
                session_id: "s1".into(),
                turn_id: "t3".into(),
            },
        ];
        let sessions = turns_to_sessions(&turns);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, "s1");
        assert_eq!(sessions[0].best_rank, 0);
        assert_eq!(sessions[1].session_id, "s2");
        assert_eq!(sessions[1].best_rank, 1);
    }

    /// Commit a file so a line object can point at a real, blame-able commit,
    /// then return the commit sha.
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
    fn turns_from_line_objects_filters_by_line_and_orders_recent_first() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let conv = session_writing(dir.path(), "add the fn", "src/lib.rs");
        persist_conversation(repo.inner(), &conv).unwrap();
        let commit = commit_file(dir.path(), "lib.rs", "fn added() {}\n");

        // A line object at lines [1,3] attributed to the deciding turn.
        let obj = LineObject::new(
            "lib.rs",
            [1, 3],
            commit,
            conv.id.clone(),
            conv.turns[0].id.clone(),
            Confidence::Exact,
        );
        write_line_object(repo.inner(), &obj).unwrap();

        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
        index
            .populate_line_tables(repo.inner(), &mut |_, _| {})
            .unwrap();

        let all = turns_from_line_objects(&index, "lib.rs", None).unwrap();
        assert_eq!(all.len(), 1, "the written line object is mirrored");
        assert_eq!(all[0].turn.turn_id, conv.turns[0].id.as_str());
        assert_eq!(all[0].confidence, Confidence::Exact);

        // A line inside the range keeps it; one outside drops it.
        assert_eq!(
            turns_from_line_objects(&index, "lib.rs", Some(2))
                .unwrap()
                .len(),
            1
        );
        assert!(turns_from_line_objects(&index, "lib.rs", Some(100))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn materialize_filters_private_sessions_structurally() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let mut conv = session_writing(dir.path(), "add the fn", "src/lib.rs");
        conv.private = true;
        persist_conversation(repo.inner(), &conv).unwrap();
        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
        index.index_conversation(&conv).unwrap();

        let turns = vec![TurnRef {
            session_id: conv.id.as_str().to_string(),
            turn_id: conv.turns[0].id.as_str().to_string(),
        }];
        let evidence = materialize_turns(repo.inner(), &index, &turns, &[None])
            .unwrap()
            .into_inner();
        assert!(
            evidence.is_empty(),
            "private session must never materialize"
        );
    }

    #[test]
    fn materialize_carries_verbatim_text_and_line_anchor() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let conv = session_writing(dir.path(), "the deciding words are here", "src/lib.rs");
        persist_conversation(repo.inner(), &conv).unwrap();
        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
        index.index_conversation(&conv).unwrap();

        let turns = vec![TurnRef {
            session_id: conv.id.as_str().to_string(),
            turn_id: conv.turns[0].id.as_str().to_string(),
        }];
        let anchors = vec![Some(MaterializeAnchor {
            file_path: "src/lib.rs".into(),
            line_range: [10, 12],
            confidence: Confidence::Exact,
        })];
        let evidence = materialize_turns(repo.inner(), &index, &turns, &anchors)
            .unwrap()
            .into_inner();
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].tier, EvidenceTier::LineObjects);
        assert_eq!(evidence[0].strength, Strength::High);
        assert_eq!(evidence[0].line_ranges, vec![[10, 12]]);
        assert!(evidence[0].summary.contains("deciding words"));
    }

    #[test]
    fn parse_confidence_degrades_unknown_to_heuristic() {
        assert_eq!(parse_confidence("exact"), Confidence::Exact);
        assert_eq!(parse_confidence("manual"), Confidence::Manual);
        assert_eq!(parse_confidence("nonsense"), Confidence::Heuristic);
    }

    /// A session of plain user prompts, so every turn is salient and reaches the
    /// index — the corpus the traversal verbs walk.
    fn session_saying(dir: &std::path::Path, prompts: &[&str]) -> Conversation {
        let mut conv = Conversation::new(AgentKind::Claude, dir.display().to_string());
        for prompt in prompts {
            conv.turns.push(Turn {
                id: LineageId::new(),
                role: Role::User,
                content: (*prompt).into(),
                tool_calls: vec![],
                model: None,
                timestamp: None,
                artifacts: vec![],
            });
        }
        conv
    }

    /// Persist and index a session, returning the index it was written to.
    fn indexed(dir: &std::path::Path, convs: &[&Conversation]) -> LineageIndex {
        let repo = open_repo(dir).unwrap();
        let index = LineageIndex::open(dir.join(".git/lineage/index.db")).unwrap();
        for conv in convs {
            persist_conversation(repo.inner(), conv).unwrap();
            index.index_conversation(conv).unwrap();
        }
        index
    }

    #[test]
    fn search_within_sessions_stays_scoped_and_is_bounded() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let inside = session_saying(dir.path(), &["redis for the cache", "redis again"]);
        let outside = session_saying(dir.path(), &["redis but out of scope"]);
        let index = indexed(dir.path(), &[&inside, &outside]);

        let scoped = search_within_sessions(
            repo.inner(),
            &index,
            &[inside.id.as_str().to_string()],
            "redis",
            10,
        )
        .unwrap()
        .into_inner();
        assert_eq!(scoped.len(), 2);
        assert!(scoped
            .iter()
            .all(|e| e.session_id.as_str() == inside.id.as_str()));

        let bounded = search_within_sessions(
            repo.inner(),
            &index,
            &[inside.id.as_str().to_string()],
            "redis",
            1,
        )
        .unwrap()
        .into_inner();
        assert_eq!(bounded.len(), 1, "the bound is honoured");
    }

    #[test]
    fn search_within_sessions_never_emits_a_private_session() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let mut private = session_saying(dir.path(), &["the private redis decision"]);
        private.private = true;
        let index = indexed(dir.path(), &[&private]);

        // The index holds the turn; only the gate keeps it out of evidence.
        assert_eq!(
            index
                .search_turns_in_sessions(&[private.id.as_str().to_string()], "redis", 10)
                .unwrap()
                .len(),
            1
        );
        let evidence = search_within_sessions(
            repo.inner(),
            &index,
            &[private.id.as_str().to_string()],
            "redis",
            10,
        )
        .unwrap()
        .into_inner();
        assert!(evidence.is_empty());
    }

    #[test]
    fn turn_neighbourhood_returns_the_surrounding_turns_in_order() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let conv = session_saying(dir.path(), &["first", "second", "third", "fourth"]);
        let index = indexed(dir.path(), &[&conv]);

        let around = turn_neighbourhood(repo.inner(), &index, conv.turns[1].id.as_str(), 1, 10)
            .unwrap()
            .into_inner();
        let bodies: Vec<&str> = around.iter().map(|e| e.summary.as_str()).collect();
        assert_eq!(bodies, vec!["first", "second", "third"]);

        // An unknown turn is silence, not an error — a stale handle must not
        // break the agent's traversal.
        assert!(
            turn_neighbourhood(repo.inner(), &index, "no-such-turn", 1, 10)
                .unwrap()
                .into_inner()
                .is_empty()
        );
    }

    #[test]
    fn turn_neighbourhood_never_emits_a_private_session() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let mut private = session_saying(dir.path(), &["one", "two"]);
        private.private = true;
        let index = indexed(dir.path(), &[&private]);

        let around = turn_neighbourhood(repo.inner(), &index, private.turns[0].id.as_str(), 1, 10)
            .unwrap()
            .into_inner();
        assert!(around.is_empty());
    }

    #[test]
    fn line_objects_of_turn_resolves_the_code_a_turn_produced() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let conv = session_writing(dir.path(), "add the fn", "src/lib.rs");
        persist_conversation(repo.inner(), &conv).unwrap();
        let commit = commit_file(dir.path(), "lib.rs", "fn added() {}\n");

        let obj = LineObject::new(
            "lib.rs",
            [1, 3],
            commit.clone(),
            conv.id.clone(),
            conv.turns[0].id.clone(),
            Confidence::Exact,
        );
        write_line_object(repo.inner(), &obj).unwrap();

        let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
        index
            .populate_line_tables(repo.inner(), &mut |_, _| {})
            .unwrap();

        let produced = line_objects_of_turn(&index, conv.turns[0].id.as_str(), 10).unwrap();
        assert_eq!(produced.len(), 1);
        assert_eq!(produced[0].anchor.file_path, "lib.rs");
        assert_eq!(produced[0].anchor.commit_sha, commit);
        assert_eq!(produced[0].line_range, [1, 3]);
        assert_eq!(produced[0].confidence, Confidence::Exact);

        assert!(line_objects_of_turn(&index, "no-such-turn", 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn sessions_for_commit_resolves_and_gates() {
        let dir = init_repo();
        let repo = open_repo(dir.path()).unwrap();
        let sha = commit_file(dir.path(), "lib.rs", "fn linked() {}\n");
        let mut public = session_saying(dir.path(), &["the public decision"]);
        public.commit_shas.push(sha.clone());
        let mut private = session_saying(dir.path(), &["the private decision"]);
        private.private = true;
        private.commit_shas.push(sha.clone());
        let index = indexed(dir.path(), &[&public, &private]);

        // Both sessions are mirrored; only the public one survives the gate.
        assert_eq!(index.sessions_for_commit(&sha, 10).unwrap().len(), 2);
        let found = sessions_for_commit(repo.inner(), &index, &sha, 10)
            .unwrap()
            .into_inner();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].session_id, public.id.as_str());
        assert!(found[0].attribution.contains("claude session"));

        assert!(
            sessions_for_commit(repo.inner(), &index, &"b".repeat(40), 10)
                .unwrap()
                .into_inner()
                .is_empty()
        );
    }
}
