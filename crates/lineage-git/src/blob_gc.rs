use std::collections::{HashMap, HashSet};

use git2::Repository;
use lineage_core::{LineageError, LineageId};
use lineage_store::{normalize_oid, LfsStore};

use crate::lfs_refs::collect_all_blob_refs;
use crate::refs::{
    list_session_ids, read_conversation_stored, read_line_object, session_ref, LINEAGE_NOTES_REF,
};

#[derive(Debug, Default)]
pub struct PurgeReport {
    pub blobs_purged: usize,
    pub line_objects_purged: usize,
    pub transport_refs_purged: usize,
}

pub fn count_blob_refs(repo: &Repository) -> Result<HashMap<String, usize>, LineageError> {
    let mut counts = HashMap::new();
    for id in list_session_ids(repo)? {
        if let Some(conv) = read_conversation_stored(repo, &id)? {
            for blob_ref in crate::lfs_refs::collect_blob_refs_from_conversation(&conv) {
                *counts.entry(blob_ref).or_insert(0) += 1;
            }
        }
    }
    Ok(counts)
}

pub fn count_blob_refs_excluding(
    repo: &Repository,
    exclude_session: &LineageId,
) -> Result<HashMap<String, usize>, LineageError> {
    let mut counts = HashMap::new();
    for id in list_session_ids(repo)? {
        if &id == exclude_session {
            continue;
        }
        if let Some(conv) = read_conversation_stored(repo, &id)? {
            for blob_ref in crate::lfs_refs::collect_blob_refs_from_conversation(&conv) {
                *counts.entry(blob_ref).or_insert(0) += 1;
            }
        }
    }
    Ok(counts)
}

pub fn purge_session_blobs(
    repo: &Repository,
    session_id: &LineageId,
    blob_refs: &[String],
) -> Result<usize, LineageError> {
    let remaining = count_blob_refs_excluding(repo, session_id)?;
    let lfs = LfsStore::new(repo.path());
    let mut purged = 0usize;

    for blob_ref in blob_refs {
        if remaining.get(blob_ref).copied().unwrap_or(0) > 0 {
            continue;
        }
        let oid = normalize_oid(blob_ref);
        let path = lfs.object_path(&oid);
        if path.exists() && std::fs::remove_file(&path).is_ok() {
            purged += 1;
        }
        for prefix in ["refs/lineage/lfs/", "refs/lineage/lfs-data/"] {
            let r = format!("{prefix}{oid}");
            if let Ok(mut reference) = repo.find_reference(&r) {
                let _ = reference.delete();
            }
        }
    }
    Ok(purged)
}

pub fn referenced_line_object_ids(repo: &Repository) -> Result<HashSet<LineageId>, LineageError> {
    let mut ids = HashSet::new();
    let Ok(notes) = repo.notes(Some(LINEAGE_NOTES_REF)) else {
        return Ok(ids);
    };
    for entry in notes {
        let (_note_commit, annotated_commit) =
            entry.map_err(|e| LineageError::Other(e.to_string()))?;
        let Ok(note) = repo.find_note(Some(LINEAGE_NOTES_REF), annotated_commit) else {
            continue;
        };
        let Some(msg) = note.message() else {
            continue;
        };
        if let Ok(git_note) = lineage_core::GitNote::from_json(msg) {
            ids.extend(git_note.line_object_ids);
        }
    }
    Ok(ids)
}

pub fn list_line_objects_for_session(
    repo: &Repository,
    session_id: &LineageId,
) -> Result<Vec<LineageId>, LineageError> {
    let prefix = "refs/lineage/lines/";
    let mut ids = Vec::new();
    for reference in repo
        .references_glob(&format!("{prefix}*"))
        .map_err(|e| LineageError::Other(e.to_string()))?
        .filter_map(|r| r.ok())
    {
        let name = reference.name().unwrap_or("");
        let Some(suffix) = name.strip_prefix(prefix) else {
            continue;
        };
        let object_id = LineageId::from(suffix);
        if let Some(obj) = read_line_object(repo, &object_id)? {
            if &obj.conversation_id == session_id {
                ids.push(object_id);
            }
        }
    }
    Ok(ids)
}

pub fn purge_orphans(repo: &Repository) -> Result<PurgeReport, LineageError> {
    let mut report = PurgeReport::default();
    let referenced_lo = referenced_line_object_ids(repo)?;
    let prefix = "refs/lineage/lines/";

    for reference in repo
        .references_glob(&format!("{prefix}*"))
        .map_err(|e| LineageError::Other(e.to_string()))?
        .filter_map(|r| r.ok())
    {
        let name = reference.name().unwrap_or("");
        let Some(suffix) = name.strip_prefix(prefix) else {
            continue;
        };
        let object_id = LineageId::from(suffix);
        if referenced_lo.contains(&object_id) {
            continue;
        }
        if let Ok(mut reference) = repo.find_reference(name) {
            if reference.delete().is_ok() {
                report.line_objects_purged += 1;
            }
        }
    }

    let counts = count_blob_refs(repo)?;
    let lfs = LfsStore::new(repo.path());
    let mut candidate_oids: HashSet<String> = HashSet::new();

    for blob_ref in collect_all_blob_refs(repo)? {
        candidate_oids.insert(normalize_oid(&blob_ref));
    }
    for oid in crate::lfs_refs::list_lfs_data_refs(repo)? {
        candidate_oids.insert(normalize_oid(&oid));
    }

    for oid in candidate_oids {
        let blob_ref = format!("lfs:sha256:{oid}");
        if counts.get(&blob_ref).copied().unwrap_or(0) > 0 {
            continue;
        }
        let path = lfs.object_path(&oid);
        if path.exists() && std::fs::remove_file(&path).is_ok() {
            report.blobs_purged += 1;
        }
        for prefix in ["refs/lineage/lfs/", "refs/lineage/lfs-data/"] {
            let r = format!("{prefix}{oid}");
            if let Ok(mut reference) = repo.find_reference(&r) {
                if reference.delete().is_ok() {
                    report.transport_refs_purged += 1;
                }
            }
        }
    }

    Ok(report)
}

pub fn delete_session_ref(repo: &Repository, session_id: &LineageId) -> Result<(), LineageError> {
    let ref_name = session_ref(session_id);
    if let Ok(mut reference) = repo.find_reference(&ref_name) {
        reference
            .delete()
            .map_err(|e| LineageError::Other(e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn count_blob_refs_empty_repo() {
        let dir = init_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let counts = count_blob_refs(&repo).unwrap();
        assert!(counts.is_empty());
    }

    #[test]
    fn purge_orphans_on_empty_repo() {
        let dir = init_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let report = purge_orphans(&repo).unwrap();
        assert_eq!(report.line_objects_purged, 0);
    }

    #[test]
    fn referenced_line_objects_empty_without_notes() {
        let dir = init_repo();
        let repo = git2::Repository::open(dir.path()).unwrap();
        let ids = referenced_line_object_ids(&repo).unwrap();
        assert!(ids.is_empty());
    }
}
