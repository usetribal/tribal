use git2::Repository;
use lineage_core::{Conversation, LineageError, LineageId};

use crate::hooks::{link_session_at_commit, LinkAttempt, LinkBasis, LinkReport, LinkedSession};
use crate::refs::{list_session_ids, read_conversation_stored};

/// Totals from a derived-layer rebuild, for the caller's event-log entry and
/// user-facing summary.
#[derive(Debug, Clone, Default)]
pub struct RebuildReport {
    pub commits_scanned: usize,
    pub links_written: usize,
    pub line_objects: usize,
    pub notes_deleted: usize,
    pub line_object_refs_deleted: usize,
    /// Per-commit outcomes for commits that linked at least one session, so
    /// the caller can record basis in the event log — otherwise rebuilt
    /// links would all read `established_by: unknown` in doctor.
    pub linked_commits: Vec<(String, LinkReport)>,
}

/// Recompute all *automatic* derived state — commit notes and line objects —
/// from stored conversations × git history under the current code. Wipes
/// first so pre-gate links cannot survive; callers replay manual links from
/// the event log afterwards (they are user intent, not derivable).
pub fn rebuild_links(repo: &Repository) -> Result<RebuildReport, LineageError> {
    rebuild_links_with_progress(repo, &mut |_| {})
}

/// Like [`rebuild_links`] but calls `progress(commits_scanned)` after each
/// commit is processed, so the CLI can drive a progress bar for the commit-scan
/// phase (the slow part of a full rebuild) without this crate depending on a
/// rendering library. The revwalk length is not known up front, so the callback
/// receives a running count rather than a `(done, total)` pair.
pub fn rebuild_links_with_progress(
    repo: &Repository,
    progress: &mut dyn FnMut(usize),
) -> Result<RebuildReport, LineageError> {
    let mut report = RebuildReport::default();

    // Wipe: line-object refs, then every note. Both are derived; blobs left
    // behind are the existing `gc` command's concern.
    let refs = repo
        .references_glob("refs/lineage/lines/*")
        .map_err(|e| LineageError::Other(e.to_string()))?;
    for reference in refs {
        let mut reference = reference.map_err(|e| LineageError::Other(e.to_string()))?;
        reference
            .delete()
            .map_err(|e| LineageError::Other(e.to_string()))?;
        report.line_object_refs_deleted += 1;
    }
    for note in crate::notes::list_notes(repo)? {
        crate::notes::delete_note_for_commit(repo, &note.commit_sha)?;
        report.notes_deleted += 1;
    }

    // Read every conversation once, up front. A full-history rebuild tests each
    // session against every commit; re-reading the (large) conversation blob per
    // commit was the dominant cost. The blob is constant across commits.
    let ids: Vec<LineageId> = list_session_ids(repo)?;
    let mut conversations: Vec<(LineageId, Conversation)> = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(conversation) = read_conversation_stored(repo, &id)? {
            conversations.push((id, conversation));
        }
    }

    let mut walk = repo
        .revwalk()
        .map_err(|e| LineageError::Other(e.to_string()))?;
    walk.push_head()
        .map_err(|e| LineageError::Other(e.to_string()))?;
    for oid in walk {
        let oid = oid.map_err(|e| LineageError::Other(e.to_string()))?;
        report.commits_scanned += 1;
        let sha = oid.to_string();

        // Diff the commit once, then gate every preloaded session against it.
        let changed = crate::line_resolve::files_changed_in_commit(repo, &sha)?;
        let mut link = LinkReport::default();
        for (id, conversation) in &conversations {
            match link_session_at_commit(repo, &sha, id, conversation, &changed)? {
                LinkAttempt::Linked {
                    line_objects,
                    basis,
                } => link.linked.push(LinkedSession {
                    session_id: id.clone(),
                    line_objects,
                    basis,
                }),
                LinkAttempt::SkippedNoOverlap => link.skipped_no_overlap.push(id.clone()),
            }
        }

        for session in &link.linked {
            report.links_written += 1;
            if session.basis == LinkBasis::LineObjects {
                report.line_objects += session.line_objects;
            }
        }
        if !link.linked.is_empty() {
            report.linked_commits.push((sha, link));
        }
        progress(report.commits_scanned);
    }

    Ok(report)
}
