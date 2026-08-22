use lineage_core::LineageError;

use crate::config::{read_repo_config, LINEAGE_CONFIG_REF};
use crate::hydrate::hydrate_conversation;
use crate::lfs_ops::lfs_status;
use crate::refs::{
    list_session_ids, read_conversation_stored, LINEAGE_INDEX_REF, LINEAGE_NOTES_REF,
};
use crate::repo::LineageRepo;

#[derive(Debug, Default)]
pub struct DoctorReport {
    pub is_git_repo: bool,
    pub notes_ref_ok: bool,
    pub index_ref_ok: bool,
    pub config_ref_ok: bool,
    pub session_count: usize,
    pub broken_sessions: Vec<String>,
    pub missing_lfs_blobs: Vec<String>,
    pub warnings: Vec<String>,
}

impl DoctorReport {
    pub fn ok(&self) -> bool {
        self.is_git_repo && self.broken_sessions.is_empty() && self.missing_lfs_blobs.is_empty()
    }
}

/// Ref/config checks only — cheap (no session reads, no LFS scans), for
/// callers that don't need the per-session integrity pass in `run_doctor`.
pub fn run_doctor_refs(repo: &LineageRepo) -> Result<DoctorReport, LineageError> {
    let inner = repo.inner();
    let mut report = DoctorReport {
        is_git_repo: true,
        ..Default::default()
    };

    report.notes_ref_ok = inner.find_reference(LINEAGE_NOTES_REF).is_ok() || {
        report.warnings.push(format!(
            "notes ref {LINEAGE_NOTES_REF} not found (will be created on first import)"
        ));
        true
    };

    report.index_ref_ok = inner.find_reference(LINEAGE_INDEX_REF).is_ok() || {
        report.warnings.push(format!(
            "index ref {LINEAGE_INDEX_REF} not found (will be created on first import)"
        ));
        true
    };

    report.config_ref_ok = inner.find_reference(LINEAGE_CONFIG_REF).is_ok() || {
        report.warnings.push(format!(
            "config ref {LINEAGE_CONFIG_REF} not found (run: git lineage init --config)"
        ));
        true
    };

    if let Ok(config) = read_repo_config(inner) {
        report.warnings.push(format!(
            "large blob backend: {} (threshold {} bytes)",
            config.large_blob_backend.as_str(),
            config.large_blob_threshold_bytes
        ));
    }

    Ok(report)
}

pub fn run_doctor(repo: &LineageRepo) -> Result<DoctorReport, LineageError> {
    let mut report = run_doctor_refs(repo)?;
    let inner = repo.inner();

    let session_ids = list_session_ids(inner)?;
    report.session_count = session_ids.len();

    for id in session_ids {
        match read_conversation_stored(inner, &id)? {
            Some(mut conv) => {
                let hydrate = hydrate_conversation(inner, &mut conv)?;
                report.missing_lfs_blobs.extend(hydrate.missing_blobs);
            }
            None => report.broken_sessions.push(id.to_string()),
        }
    }

    report.missing_lfs_blobs.sort();
    report.missing_lfs_blobs.dedup();

    let lfs = lfs_status(inner)?;
    if lfs.missing_local.len() > report.missing_lfs_blobs.len() {
        report
            .missing_lfs_blobs
            .extend(lfs.missing_local.iter().cloned());
        report.missing_lfs_blobs.sort();
        report.missing_lfs_blobs.dedup();
    }
    if !lfs.missing_local.is_empty() {
        report.warnings.push(format!(
            "{} referenced LFS object(s) missing locally (run: git lineage lfs fetch)",
            lfs.missing_local.len()
        ));
    }

    Ok(report)
}
