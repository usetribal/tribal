use std::process::Command;

use git2::Repository;
use lineage_core::{LfsTransport, LineageError};
use lineage_store::{normalize_oid, LfsStore};

use crate::config::read_repo_config;
use crate::lfs_batch::{collect_lfs_objects, fetch_via_http_batch, push_via_http_batch};
use crate::lfs_refs::{
    collect_all_blob_refs, list_lfs_data_refs, read_lfs_data_from_ref, LFS_DATA_REF_PREFIX,
    LFS_POINTER_REF_PREFIX,
};

#[derive(Debug, Default)]
pub struct LfsStatusReport {
    pub referenced: usize,
    pub present_local: usize,
    pub missing_local: Vec<String>,
    pub transport_refs: usize,
    pub git_lfs_available: bool,
}

#[derive(Debug, Default)]
pub struct LfsTransferReport {
    pub uploaded: usize,
    pub downloaded: usize,
    pub skipped: usize,
    pub method: String,
}

pub fn lfs_status(repo: &Repository) -> Result<LfsStatusReport, LineageError> {
    let lfs = LfsStore::new(repo.path());
    let refs = collect_all_blob_refs(repo)?;
    let mut report = LfsStatusReport {
        referenced: refs.len(),
        git_lfs_available: git_lfs_available(),
        ..Default::default()
    };

    for blob_ref in &refs {
        let oid = normalize_oid(blob_ref);
        if lfs.exists(&oid) {
            report.present_local += 1;
        } else {
            report.missing_local.push(blob_ref.clone());
        }
    }

    report.transport_refs = list_lfs_data_refs(repo)?.len();
    Ok(report)
}

pub fn lfs_push(repo: &Repository, remote: &str) -> Result<LfsTransferReport, LineageError> {
    let config = read_repo_config(repo)?;
    ensure_transport_refs(repo)?;

    let mut report = LfsTransferReport::default();
    let use_cli = matches!(
        config.lfs_transport,
        LfsTransport::Auto | LfsTransport::GitCli
    ) && git_lfs_available();

    if use_cli {
        match push_via_git_lfs(repo, remote) {
            Ok(n) => {
                report.uploaded = n;
                report.method = "git-lfs".into();
            }
            Err(e) if config.lfs_transport == LfsTransport::GitCli => return Err(e),
            Err(_) if config.lfs_transport == LfsTransport::Auto => {}
            Err(e) => return Err(e),
        }
    }

    let use_http = matches!(
        config.lfs_transport,
        LfsTransport::Auto | LfsTransport::Http
    ) && report.method.is_empty();

    if use_http {
        let objects = collect_lfs_objects(repo)?;
        match push_via_http_batch(repo, remote, &objects) {
            Ok(n) => {
                report.uploaded = n;
                report.method = "http-batch".into();
            }
            Err(e) if config.lfs_transport == LfsTransport::Http => return Err(e),
            Err(_) => {}
        }
    }

    if matches!(
        config.lfs_transport,
        LfsTransport::Refs | LfsTransport::Auto
    ) {
        push_refs(repo, remote, LFS_POINTER_REF_PREFIX)?;
        push_refs(repo, remote, LFS_DATA_REF_PREFIX)?;
        if report.uploaded == 0 {
            report.uploaded = collect_all_blob_refs(repo)?.len();
        }
        report.method = if report.method.is_empty() {
            "refs".into()
        } else {
            format!("{}, refs", report.method)
        };
    }

    Ok(report)
}

pub fn lfs_fetch(repo: &Repository, remote: &str) -> Result<LfsTransferReport, LineageError> {
    let config = read_repo_config(repo)?;
    let mut report = LfsTransferReport::default();

    let use_cli = matches!(
        config.lfs_transport,
        LfsTransport::Auto | LfsTransport::GitCli
    ) && git_lfs_available();

    if use_cli {
        if let Ok(n) = fetch_via_git_lfs(repo, remote) {
            report.downloaded += n;
            report.method = "git-lfs".into();
        } else if config.lfs_transport == LfsTransport::GitCli {
            return Err(LineageError::Other("git-lfs fetch failed".into()));
        }
    }

    let use_http = matches!(
        config.lfs_transport,
        LfsTransport::Auto | LfsTransport::Http
    );

    if use_http {
        let objects = collect_lfs_objects(repo)?;
        let missing_before = objects
            .iter()
            .filter(|o| !LfsStore::new(repo.path()).exists(&o.oid))
            .count();
        if missing_before > 0 || config.lfs_transport == LfsTransport::Http {
            match fetch_via_http_batch(repo, remote, &objects) {
                Ok(n) => {
                    report.downloaded += n;
                    report.method = if report.method.is_empty() {
                        "http-batch".into()
                    } else {
                        format!("{}, http-batch", report.method)
                    };
                }
                Err(e) if config.lfs_transport == LfsTransport::Http => return Err(e),
                Err(_) => {}
            }
        }
    }

    if matches!(
        config.lfs_transport,
        LfsTransport::Auto | LfsTransport::Refs
    ) {
        fetch_refs(repo, remote, LFS_POINTER_REF_PREFIX)?;
        fetch_refs(repo, remote, LFS_DATA_REF_PREFIX)?;

        let lfs = LfsStore::new(repo.path());
        for oid in list_lfs_data_refs(repo)? {
            if lfs.exists(&oid) {
                report.skipped += 1;
                continue;
            }
            if let Some(data) = read_lfs_data_from_ref(repo, &oid)? {
                lfs.put(&data)?;
                report.downloaded += 1;
            }
        }
        if report.method.is_empty() {
            report.method = "refs".into();
        } else if !report.method.contains("refs") {
            report.method = format!("{}, refs", report.method);
        }
    }

    Ok(report)
}

fn push_via_git_lfs(repo: &Repository, remote: &str) -> Result<usize, LineageError> {
    run_git(repo, &["lfs", "install", "--local"])?;
    run_git(repo, &["lfs", "push", remote, "--all"])?;
    Ok(collect_all_blob_refs(repo)?.len())
}

fn fetch_via_git_lfs(repo: &Repository, remote: &str) -> Result<usize, LineageError> {
    run_git(repo, &["lfs", "install", "--local"])?;
    run_git(repo, &["lfs", "fetch", remote])?;
    let lfs = LfsStore::new(repo.path());
    let mut count = 0usize;
    for blob_ref in collect_all_blob_refs(repo)? {
        let oid = normalize_oid(&blob_ref);
        if !lfs.exists(&oid) {
            continue;
        }
        count += 1;
    }
    Ok(count)
}

fn git_lfs_available() -> bool {
    Command::new("git")
        .args(["lfs", "version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ensure_transport_refs(repo: &Repository) -> Result<(), LineageError> {
    let lfs = LfsStore::new(repo.path());
    for blob_ref in collect_all_blob_refs(repo)? {
        let oid = normalize_oid(&blob_ref);
        if !lfs.exists(&oid) {
            continue;
        }
        if crate::lfs_refs::read_lfs_data_from_ref(repo, &oid)?.is_some() {
            continue;
        }
        let data = lfs.get(&oid)?;
        crate::lfs_refs::write_lfs_data_ref(repo, &oid, &data)?;
        if crate::lfs_refs::read_lfs_pointer_ref(repo, &oid)?.is_none() {
            crate::lfs_refs::write_lfs_pointer_ref(repo, &oid, data.len())?;
        }
    }
    Ok(())
}

fn push_refs(repo: &Repository, remote: &str, prefix: &str) -> Result<(), LineageError> {
    let refs: Vec<String> = repo
        .references_glob(&format!("{prefix}*"))
        .map_err(|e| LineageError::Other(e.to_string()))?
        .filter_map(|r| r.ok())
        .filter_map(|r| r.name().map(str::to_string))
        .collect();
    if refs.is_empty() {
        return Ok(());
    }
    let mut args = vec!["push", remote];
    for r in &refs {
        args.push(r);
    }
    run_git(repo, &args)
}

fn fetch_refs(repo: &Repository, remote: &str, prefix: &str) -> Result<(), LineageError> {
    let spec = format!("+{prefix}*:{prefix}*");
    run_git(repo, &["fetch", remote, &spec])
}

fn run_git(repo: &Repository, args: &[&str]) -> Result<(), LineageError> {
    let workdir = repo
        .workdir()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| repo.path().to_path_buf());
    let output = Command::new("git")
        .args(args)
        .current_dir(&workdir)
        .output()
        .map_err(|e| LineageError::Other(e.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(LineageError::Other(format!(
            "git {} failed: {stderr}",
            args.join(" ")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lfs_status_on_fresh_repo() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let repo = Repository::open(dir.path()).unwrap();
        let report = lfs_status(&repo).unwrap();
        assert_eq!(report.referenced, 0);
    }
}
