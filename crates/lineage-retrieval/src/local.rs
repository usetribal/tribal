use std::collections::BTreeMap;
use std::time::Instant;

use git2::Repository;
use lineage_core::{
    generate_architecture_summary, normalize_repo_path_unscoped, turn_indexable_text, Confidence,
    Conversation, LineageId,
};
use lineage_git::{read_conversation, read_line_object};
use lineage_search::LineageIndex;

use crate::retriever::{Result, RetrievalError, Retriever};
use crate::session::{attribution_for, is_private_or_private_ancestor, verbatim_summary};
use crate::types::{strength_for, ContextQuery, Evidence, EvidenceTier, Retrieval};

const LINE_OBJECT_REF_GLOB: &str = "refs/lineage/lines/*";

/// Cache-key component: bump on any change to what this retriever would
/// answer for an unchanged repo (tiers, grouping, summary source).
pub const LOCAL_RETRIEVER_VERSION: &str = "5";

/// Solo-mode retriever: answers from the repo's own lineage refs and search
/// index, in-process. Team mode swaps in a server-backed implementation
/// behind the same `Retriever` trait.
pub struct LocalRetriever<'a> {
    repo: &'a Repository,
    index: &'a LineageIndex,
}

/// Line-object evidence for one session before it becomes wire `Evidence`.
/// `turn_id` is the turn the strongest line object attributes the lines to —
/// carried through so the digest can quote the turn that actually wrote the
/// code rather than describing the session as a whole.
struct LineMatches {
    ranges: Vec<[u32; 2]>,
    confidence: Confidence,
    turn_id: Option<String>,
}

impl<'a> LocalRetriever<'a> {
    pub fn new(repo: &'a Repository, index: &'a LineageIndex) -> Self {
        Self { repo, index }
    }

    /// All line objects for the queried file, grouped per session. Enumerates
    /// every line-object ref; per-query cost is acceptable because the cache
    /// in front of retrieval absorbs repeats (spec: Cache).
    fn line_matches_for_file(&self, file_path: &str) -> Result<BTreeMap<String, LineMatches>> {
        let refs = self
            .repo
            .references_glob(LINE_OBJECT_REF_GLOB)
            .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;

        let mut by_session: BTreeMap<String, LineMatches> = BTreeMap::new();
        for reference in refs {
            let reference = reference.map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
            let Some(name) = reference.name() else {
                continue;
            };
            let Some(id) = name.rsplit('/').next() else {
                continue;
            };
            let object = read_line_object(self.repo, &LineageId::from(id))
                .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
            let Some(object) = object else { continue };
            if normalize_repo_path_unscoped(&object.file_path, None) != file_path {
                continue;
            }

            let entry = by_session
                .entry(object.conversation_id.as_str().to_string())
                .or_insert(LineMatches {
                    ranges: Vec::new(),
                    confidence: object.confidence,
                    turn_id: Some(object.turn_id.as_str().to_string()),
                });
            entry.ranges.push(object.line_range);
            // A session's match confidence is its best one: any exact/manual
            // line object outranks heuristic ones for the strength mapping.
            // The quoted turn follows the same winner, so the digest quotes the
            // turn behind the strongest attribution rather than whichever ref
            // the enumeration reached first.
            if matches!(object.confidence, Confidence::Exact | Confidence::Manual) {
                entry.confidence = object.confidence;
                entry.turn_id = Some(object.turn_id.as_str().to_string());
            }
        }

        for matches in by_session.values_mut() {
            matches.ranges.sort_unstable();
            matches.ranges.dedup();
        }
        Ok(by_session)
    }

    fn evidence_for_session(
        &self,
        session_id: &str,
        line_matches: Option<LineMatches>,
    ) -> Result<Option<Evidence>> {
        let id = LineageId::from(session_id.to_string());
        let conversation = read_conversation(self.repo, &id)
            .map_err(|e| RetrievalError::Retrieval(e.to_string()))?;
        let Some(conversation) = conversation else {
            return Ok(None);
        };
        if is_private_or_private_ancestor(self.repo, &conversation)? {
            return Ok(None);
        }

        let attribution = attribution_for(&conversation);

        let (tier, match_confidence, line_ranges, turn_id) = match line_matches {
            Some(matches) => (
                EvidenceTier::LineObjects,
                Some(matches.confidence),
                matches.ranges,
                matches.turn_id,
            ),
            None => (EvidenceTier::FilesTouched, None, Vec::new(), None),
        };

        // Quote the attributing turn when there is one. The session-level
        // summary keys off the FIRST user turn, which is a near-constant for
        // anyone who opens sessions with a standing preamble — it made a
        // 1,219-turn build session read as a generic repo tour, so a reader
        // concluded there was nothing to pursue. Fall back to it only for
        // files_touched evidence, where no turn is attributed.
        let summary = turn_id
            .as_deref()
            .and_then(|id| turn_summary(&conversation, id))
            .unwrap_or_else(|| generate_architecture_summary(&conversation));

        Ok(Some(Evidence {
            session_id: conversation.id,
            turn_id: turn_id.map(LineageId::from),
            tier,
            strength: strength_for(tier, match_confidence),
            match_confidence,
            line_ranges,
            summary,
            attribution,
        }))
    }
}

/// The attributed turn's own words, capped like any other verbatim evidence.
/// Quotes exactly the turn `turn_id` names — never a neighbour — so the
/// rendered handle and the rendered text always describe the same node.
/// Returns `None` when the turn is absent from the stored conversation (a line
/// object can outlive an amended session) or carries no text, so the caller
/// falls back rather than emitting an empty summary.
fn turn_summary(conversation: &Conversation, turn_id: &str) -> Option<String> {
    let turn = conversation
        .turns
        .iter()
        .find(|t| t.id.as_str() == turn_id)?;
    let text = turn_indexable_text(turn);
    if text.trim().is_empty() {
        return None;
    }
    Some(verbatim_summary(&text))
}

impl Retriever for LocalRetriever<'_> {
    fn retrieve(&self, query: &ContextQuery) -> Result<Retrieval> {
        let started = Instant::now();
        let file_path = normalize_repo_path_unscoped(&query.file_path, None);

        let mut line_matches = self.line_matches_for_file(&file_path)?;

        // Candidate order matters under a tight budget: line-object sessions
        // first so the strongest evidence survives an early stop.
        let mut candidates: Vec<String> = line_matches.keys().cloned().collect();
        for session_id in self
            .index
            .sessions_that_wrote_file(&file_path)
            .map_err(|e| RetrievalError::Retrieval(e.to_string()))?
        {
            if !line_matches.contains_key(&session_id) {
                candidates.push(session_id);
            }
        }

        let mut evidence = Vec::new();
        let mut truncated = false;
        for session_id in candidates {
            if let Some(budget_ms) = query.budget_ms {
                // Spec: return what we have rather than overrun the caller's
                // budget — partial evidence beats a blown deadline.
                if started.elapsed().as_millis() >= u128::from(budget_ms) {
                    truncated = true;
                    break;
                }
            }
            let matches = line_matches.remove(&session_id);
            if let Some(entry) = self.evidence_for_session(&session_id, matches)? {
                evidence.push(entry);
            }
        }

        let mut retrieval = Retrieval::from_evidence(evidence);
        retrieval.truncated = truncated;
        Ok(retrieval)
    }
}
