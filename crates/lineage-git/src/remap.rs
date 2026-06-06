use std::collections::{HashMap, HashSet};

use git2::{Oid, Repository};
use lineage_core::{Confidence, LineageError, LineageId};

use crate::line_resolve::materialize_line_objects;
use crate::notes::{read_note_for_commit, write_note_for_commit};
use crate::patch_id::{build_patch_id_index, patch_id_for_commit};
use crate::refs::{list_session_ids, read_conversation_stored, write_conversation};

pub struct RemapReport {
    pub remapped_commits: usize,
    pub rematerialized_sessions: usize,
    pub line_objects_updated: usize,
    pub patch_id_matches: usize,
}

pub fn remap_orphaned_commits(repo: &Repository) -> Result<RemapReport, LineageError> {
    let head_sha = repo
        .head()
        .map_err(|e| LineageError::Other(e.to_string()))?
        .peel_to_commit()
        .map_err(|e| LineageError::Other(e.to_string()))?
        .id()
        .to_string();

    let patch_index = build_patch_id_index(repo)?;
    let session_ids = list_session_ids(repo)?;

    let mut orphan_to_target: HashMap<String, String> = HashMap::new();
    let mut orphan_commits = HashSet::new();

    for session_id in &session_ids {
        let Some(conv) = read_conversation_stored(repo, session_id)? else {
            continue;
        };
        for sha in &conv.commit_shas {
            if commit_exists(repo, sha)? {
                continue;
            }
            orphan_commits.insert(sha.clone());

            if let Some(target) = orphan_to_target.get(sha) {
                let _ = target;
                continue;
            }

            let mapped = map_orphan_to_head(repo, sha, &head_sha, &patch_index)?;
            if let Some(target) = mapped {
                orphan_to_target.insert(sha.clone(), target);
            }
        }
    }

    let mut patch_id_matches = 0usize;
    for (orphan, target) in &orphan_to_target {
        if target != &head_sha || orphan != &head_sha {
            patch_id_matches += 1;
        }
    }

    let mut line_objects_updated = 0usize;
    let mut rematerialized = HashSet::new();

    for session_id in session_ids {
        let Some(mut conv) = read_conversation_stored(repo, &session_id)? else {
            continue;
        };

        let mut changed = false;
        conv.commit_shas = conv
            .commit_shas
            .iter()
            .map(|sha| {
                if let Some(target) = orphan_to_target.get(sha) {
                    changed = true;
                    target.clone()
                } else if !commit_exists(repo, sha).unwrap_or(false) {
                    changed = true;
                    head_sha.clone()
                } else {
                    sha.clone()
                }
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();

        if changed {
            write_conversation(repo, &conv)?;
        }

        let needs_materialize = changed
            || conv.commit_shas.iter().any(|sha| {
                sha == &head_sha
                    && read_note_for_commit(repo, sha)
                        .map(|n| n.is_none())
                        .unwrap_or(false)
            });

        if !needs_materialize {
            continue;
        }

        rematerialized.insert(session_id.clone());
        for target_sha in conv.commit_shas.clone() {
            if !commit_exists(repo, &target_sha)? {
                continue;
            }

            let objects =
                materialize_line_objects(repo, &conv, &target_sha, Confidence::Heuristic)?;
            let mut line_ids = Vec::new();
            for obj in &objects {
                crate::refs::write_line_object(repo, obj)?;
                line_ids.push(obj.id.clone());
                line_objects_updated += 1;
            }

            let mut sessions = vec![session_id.clone()];
            let patch_id = repo
                .find_commit(Oid::from_str(&target_sha).map_err(|e| LineageError::Other(e.to_string()))?)
                .ok()
                .and_then(|c| patch_id_for_commit(repo, &c).ok());

            if let Some(existing) = read_note_for_commit(repo, &target_sha)? {
                sessions = existing.session_ids.clone();
                if !sessions.iter().any(|s| s == &session_id) {
                    sessions.push(session_id.clone());
                }
                line_ids.extend(existing.line_object_ids);
            }
            merge_ids(&mut line_ids);

            write_note_for_commit(
                repo,
                &target_sha,
                &sessions,
                &line_ids,
                patch_id.as_deref(),
            )?;
        }
    }

    Ok(RemapReport {
        remapped_commits: orphan_commits.len(),
        rematerialized_sessions: rematerialized.len(),
        line_objects_updated,
        patch_id_matches,
    })
}

fn map_orphan_to_head(
    repo: &Repository,
    orphan_sha: &str,
    head_sha: &str,
    patch_index: &HashMap<String, String>,
) -> Result<Option<String>, LineageError> {
    if let Some(note) = read_note_for_commit(repo, orphan_sha)? {
        if let Some(patch_id) = note.patch_id {
            if let Some(mapped) = patch_index.get(&patch_id) {
                return Ok(Some(mapped.clone()));
            }
        }
    }

    if let Ok(oid) = Oid::from_str(orphan_sha) {
        if let Ok(commit) = repo.find_commit(oid) {
            let patch_id = patch_id_for_commit(repo, &commit)?;
            if let Some(mapped) = patch_index.get(&patch_id) {
                return Ok(Some(mapped.clone()));
            }
        }
    }

    Ok(Some(head_sha.to_string()))
}

fn commit_exists(repo: &Repository, sha: &str) -> Result<bool, LineageError> {
    let oid = Oid::from_str(sha).map_err(|e| LineageError::Other(e.to_string()))?;
    Ok(repo.find_commit(oid).is_ok())
}

fn merge_ids(ids: &mut Vec<LineageId>) {
    let mut seen = HashSet::new();
    ids.retain(|id| seen.insert(id.clone()));
}
