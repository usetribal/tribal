//! The materialization funnel behind doctor's `materialization` section
//! (diagnostics-v0): artifacts → resolvable → resolved → line objects, with a
//! per-stage loss reason for every artifact that produced nothing.

use std::collections::{BTreeMap, BTreeSet};

use git2::Repository;
use lineage_core::{workspace_root_for, ArtifactKind, LineageError, RepoPaths, ResolveStrategy};

use crate::line_resolve::files_changed_in_commit;
use crate::notes::list_notes;
use crate::refs::{list_line_objects, list_session_ids, read_conversation_stored};

#[derive(Debug, Default, Clone)]
pub struct MaterializationFunnel {
    pub total_artifacts: usize,
    pub resolvable: usize,
    pub resolved: usize,
    pub line_objects: usize,
    pub no_resolve_payload: usize,
    pub missing_old_string: usize,
    pub old_string_not_found: usize,
    pub commit_not_linked: usize,
}

/// Audits existing state only — it never materializes, so running doctor
/// cannot change what doctor reports.
pub fn audit_materialization(repo: &Repository) -> Result<MaterializationFunnel, LineageError> {
    let mut commits_by_session: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for note in list_notes(repo)? {
        for session_id in &note.session_ids {
            commits_by_session
                .entry(session_id.to_string())
                .or_default()
                .push(note.commit_sha.clone());
        }
    }

    let line_objects = list_line_objects(repo)?;
    // An artifact counts as resolved when any line object exists for its
    // (session, turn, file): line objects don't record which artifact
    // produced them, so turn+file is the finest attribution available.
    let resolved_keys: BTreeSet<(String, String, String)> = line_objects
        .iter()
        .map(|o| {
            (
                o.conversation_id.to_string(),
                o.turn_id.to_string(),
                o.file_path.clone(),
            )
        })
        .collect();

    let mut funnel = MaterializationFunnel {
        line_objects: line_objects.len(),
        ..Default::default()
    };
    // Commit diffs are the expensive step and sessions share commits, so each
    // sha is diffed at most once across the whole audit.
    let mut changed_by_commit: BTreeMap<String, Option<std::collections::HashSet<String>>> =
        BTreeMap::new();
    // The worktree registry is a property of the repository, so it is read once
    // and rebased onto each session's own workspace root below.
    let repo_paths = crate::repo::repo_paths(repo);

    for id in list_session_ids(repo)? {
        let Some(conv) = read_conversation_stored(repo, &id)? else {
            continue;
        };
        let workspace = workspace_root_for(&conv.workspace_root, repo.workdir());
        let paths = repo_paths.with_workspace_root(&workspace);
        let linked_commits = commits_by_session
            .get(&conv.id.to_string())
            .cloned()
            .unwrap_or_default();

        for turn in &conv.turns {
            for artifact in &turn.artifacts {
                if !matches!(artifact.kind, ArtifactKind::FileEdit | ArtifactKind::Diff) {
                    continue;
                }
                funnel.total_artifacts += 1;
                let file_path = paths.normalize(&artifact.path);

                let strategy = artifact.resolve.as_ref().map(|r| r.strategy);
                let resolvable = artifact.line_range.is_some()
                    || match artifact.resolve.as_ref() {
                        None => false,
                        Some(r) => match r.strategy {
                            ResolveStrategy::Citation => false,
                            ResolveStrategy::OldString => r.old_string.is_some(),
                            ResolveStrategy::FullFile => true,
                            ResolveStrategy::DiffHunk => r.patch.is_some(),
                        },
                    };
                if !resolvable {
                    if strategy == Some(ResolveStrategy::OldString) {
                        funnel.missing_old_string += 1;
                    } else {
                        funnel.no_resolve_payload += 1;
                    }
                    continue;
                }
                funnel.resolvable += 1;

                let key = (conv.id.to_string(), turn.id.to_string(), file_path.clone());
                if resolved_keys.contains(&key) {
                    funnel.resolved += 1;
                    continue;
                }

                if !file_reachable_from_links(
                    repo,
                    &linked_commits,
                    &artifact.path,
                    &paths,
                    &mut changed_by_commit,
                ) {
                    funnel.commit_not_linked += 1;
                    continue;
                }
                funnel.old_string_not_found += 1;
            }
        }
    }

    Ok(funnel)
}

/// Mirrors materialization's commit filter: an empty changed-set means the
/// commit imposes no file filter. Diff failures cache as `None` (unreachable).
///
/// The artifact path is resolved per commit rather than once by the caller,
/// because deleted-worktree recovery is decided against a specific commit's
/// file set — the same path may resolve under one linked commit and not another.
fn file_reachable_from_links(
    repo: &Repository,
    linked_commits: &[String],
    artifact_path: &str,
    paths: &RepoPaths,
    changed_by_commit: &mut BTreeMap<String, Option<std::collections::HashSet<String>>>,
) -> bool {
    linked_commits.iter().any(|sha| {
        let changed = changed_by_commit
            .entry(sha.clone())
            .or_insert_with(|| files_changed_in_commit(repo, sha).ok());
        changed.as_ref().is_some_and(|files| {
            if files.is_empty() {
                return true;
            }
            files.contains(&paths.resolve_against(artifact_path, files).0)
        })
    })
}
