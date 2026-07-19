//! The materialization funnel behind doctor's `materialization` section
//! (diagnostics-v0): artifacts → resolvable → resolved → line objects, with a
//! per-stage loss reason for every artifact that produced nothing.

use std::collections::{BTreeMap, BTreeSet};

use git2::Repository;
use lineage_core::{
    normalize_repo_path, workspace_root_for, ArtifactKind, LineageError, ResolveStrategy,
};

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

    for id in list_session_ids(repo)? {
        let Some(conv) = read_conversation_stored(repo, &id)? else {
            continue;
        };
        let workspace = workspace_root_for(&conv.workspace_root, repo.workdir());
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
                let file_path = normalize_repo_path(&artifact.path, Some(&workspace));

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

                if !file_reachable_from_links(repo, &linked_commits, &file_path) {
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
/// commit imposes no file filter.
fn file_reachable_from_links(
    repo: &Repository,
    linked_commits: &[String],
    file_path: &str,
) -> bool {
    linked_commits.iter().any(|sha| {
        files_changed_in_commit(repo, sha)
            .map(|changed| changed.is_empty() || changed.contains(file_path))
            .unwrap_or(false)
    })
}
