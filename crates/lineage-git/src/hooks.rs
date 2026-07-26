use git2::Repository;
use lineage_core::{Confidence, LineageError, LineageId};

use crate::line_resolve::materialize_line_objects_with_paths;
use crate::notes::write_note_for_commit;
use crate::patch_id::patch_id_for_commit;
use crate::refs::{list_session_ids, read_conversation_stored};

/// How a session↔commit link was established. Ordered by evidential weight;
/// batch recency is deliberately absent — being in the import window when
/// someone committed is not evidence (context-oracle-followups gap 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkBasis {
    LineObjects,
    FileOverlap,
}

impl LinkBasis {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkBasis::LineObjects => "line_objects",
            LinkBasis::FileOverlap => "file_overlap",
        }
    }
}

/// Per-session outcome of a link-to-HEAD pass, carried back so callers (the
/// post-commit hook's event-log entry, diagnostics-v0 `link`) can report which
/// sessions were linked, on what basis, and how many line objects each
/// materialized.
#[derive(Debug, Clone)]
pub struct LinkedSession {
    pub session_id: LineageId,
    pub line_objects: usize,
    pub basis: LinkBasis,
}

/// Linked sessions plus the ones the gate refused — skips are reported, never
/// silent, so doctor can show why a session is absent from a commit's note.
#[derive(Debug, Clone, Default)]
pub struct LinkReport {
    pub linked: Vec<LinkedSession>,
    pub skipped_no_overlap: Vec<LineageId>,
}

pub fn link_all_sessions_to_head(repo: &Repository) -> Result<LinkReport, LineageError> {
    let ids = list_session_ids(repo)?;
    link_sessions_to_head(repo, &ids)
}

pub fn link_recent_sessions_to_head(repo: &Repository) -> Result<LinkReport, LineageError> {
    let state = crate::import_state::read_last_import(repo)?;
    if state.session_ids.is_empty() {
        return link_all_sessions_to_head(repo);
    }
    link_sessions_to_head(repo, &state.session_ids)
}

fn link_sessions_to_head(repo: &Repository, ids: &[LineageId]) -> Result<LinkReport, LineageError> {
    let head = repo
        .head()
        .map_err(|e| LineageError::Other(e.to_string()))?
        .peel_to_commit()
        .map_err(|e| LineageError::Other(e.to_string()))?;

    link_sessions_to_commit(repo, ids, &head.id().to_string())
}

/// Evidence-gated linking of a session set against one commit — the unit both
/// the post-commit hook (HEAD) and `rebuild` (every commit) are built on.
pub fn link_sessions_to_commit(
    repo: &Repository,
    ids: &[LineageId],
    commit_sha: &str,
) -> Result<LinkReport, LineageError> {
    let mut report = LinkReport::default();

    // Diffing the commit once, then testing every session's written-file set
    // against it, keeps a full-history rebuild from re-diffing the same commit
    // per session. The worktree layout is resolved once here for the same
    // reason — reading git's registry costs more than the comparison it feeds.
    let changed = crate::line_resolve::files_changed_in_commit(repo, commit_sha)?;
    let paths = crate::repo::repo_paths(repo);
    for id in ids {
        let Some(conversation) = read_conversation_stored(repo, id)? else {
            continue;
        };
        match link_session_at_commit(repo, commit_sha, id, &conversation, &changed, &paths)? {
            LinkAttempt::Linked {
                line_objects,
                basis,
            } => report.linked.push(LinkedSession {
                session_id: id.clone(),
                line_objects,
                basis,
            }),
            LinkAttempt::SkippedNoOverlap => report.skipped_no_overlap.push(id.clone()),
        }
    }
    Ok(report)
}

pub(crate) enum LinkAttempt {
    Linked {
        line_objects: usize,
        basis: LinkBasis,
    },
    SkippedNoOverlap,
}

/// Evidence-gate one session against one commit, given the session's already
/// read conversation, the commit's already computed changed-file set, and the
/// repository's already resolved path context. The caller owns all three so a
/// rebuild can read each conversation once, diff each commit once, and read the
/// worktree registry once, rather than per (commit, session) pair.
pub(crate) fn link_session_at_commit(
    repo: &Repository,
    commit_sha: &str,
    session_id: &LineageId,
    conversation: &lineage_core::Conversation,
    changed: &std::collections::HashSet<String>,
    repo_paths: &lineage_core::RepoPaths,
) -> Result<LinkAttempt, LineageError> {
    // The gate (gap 8): a link needs evidence — the session must have
    // *written* a file this commit changed. Reads don't count, so a session
    // that only consulted a changed file never becomes commit provenance.
    // Overlap is checked before materialization because materialization is
    // itself filtered by the commit's changed files: no overlap ⇒ no line
    // objects, so skipping early is equivalent and cheap enough to run across
    // whole histories (rebuild). Manual `git lineage link` bypasses this path
    // entirely and stays authoritative.
    let workspace = lineage_core::workspace_root_for(&conversation.workspace_root, repo.workdir());
    let paths = repo_paths.with_workspace_root(&workspace);
    // Resolved against the commit rather than merely normalized, so a session
    // whose only evidence is a deleted-worktree path still clears the gate —
    // otherwise materialization below would never get the chance to recover it.
    let overlaps = lineage_core::files_written(conversation)
        .iter()
        .map(|p| paths.resolve_against(p, changed).0)
        .any(|p| changed.contains(&p));
    if !overlaps {
        return Ok(LinkAttempt::SkippedNoOverlap);
    }

    let line_objects = materialize_line_objects_with_paths(
        repo,
        conversation,
        commit_sha,
        Confidence::Heuristic,
        repo_paths,
    )?;
    let basis = if line_objects.is_empty() {
        LinkBasis::FileOverlap
    } else {
        LinkBasis::LineObjects
    };

    let mut line_ids: Vec<LineageId> = line_objects.iter().map(|o| o.id.clone()).collect();

    for obj in &line_objects {
        crate::refs::write_line_object(repo, obj)?;
    }

    let mut session_ids = vec![session_id.clone()];
    if let Some(existing) = crate::notes::read_note_for_commit(repo, commit_sha)? {
        session_ids = existing.session_ids.clone();
        if !session_ids.iter().any(|id| id == session_id) {
            session_ids.push(session_id.clone());
        }
        merge_line_ids(&mut line_ids, existing.line_object_ids);
    }

    let patch_id = repo
        .find_commit(
            git2::Oid::from_str(commit_sha).map_err(|e| LineageError::Other(e.to_string()))?,
        )
        .ok()
        .and_then(|c| patch_id_for_commit(repo, &c).ok());

    write_note_for_commit(
        repo,
        commit_sha,
        &session_ids,
        &line_ids,
        patch_id.as_deref(),
    )?;
    Ok(LinkAttempt::Linked {
        line_objects: line_objects.len(),
        basis,
    })
}

fn merge_line_ids(target: &mut Vec<LineageId>, more: Vec<LineageId>) {
    for id in more {
        if !target.iter().any(|existing| existing == &id) {
            target.push(id);
        }
    }
}
