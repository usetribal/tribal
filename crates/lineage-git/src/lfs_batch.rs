use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::time::Duration;

use git2::Repository;
use lineage_core::LineageError;
use lineage_store::{normalize_oid, LfsObject, LfsStore};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
struct BatchRequest<'a> {
    operation: &'a str,
    transfers: [&'a str; 1],
    objects: Vec<BatchObject>,
}

#[derive(Debug, Serialize)]
struct BatchObject {
    oid: String,
    size: usize,
}

#[derive(Debug, Deserialize)]
struct BatchResponse {
    objects: Vec<BatchObjectResult>,
}

#[derive(Debug, Deserialize)]
struct BatchObjectResult {
    oid: String,
    #[serde(default)]
    actions: Option<BatchActions>,
    #[serde(default)]
    error: Option<BatchError>,
}

#[derive(Debug, Deserialize)]
struct BatchActions {
    #[serde(default)]
    upload: Option<BatchAction>,
    #[serde(default)]
    download: Option<BatchAction>,
}

#[derive(Debug, Deserialize)]
struct BatchAction {
    href: String,
    #[serde(default)]
    header: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct BatchError {
    message: String,
}

pub fn discover_lfs_endpoint(repo: &Repository, remote: &str) -> Result<String, LineageError> {
    if let Some(url) = git_config_value(repo, &format!("lfs.{remote}.url")) {
        return Ok(url);
    }
    if let Some(url) = git_config_value(repo, "lfs.url") {
        return Ok(url);
    }

    let remote_url = git_remote_url(repo, remote)?;
    Ok(remote_url_to_lfs_endpoint(&remote_url))
}

pub fn push_via_http_batch(
    repo: &Repository,
    remote: &str,
    objects: &[LfsObject],
) -> Result<usize, LineageError> {
    if objects.is_empty() {
        return Ok(0);
    }
    let endpoint = discover_lfs_endpoint(repo, remote)?;
    let auth = git_credentials(repo, &endpoint)?;
    let mut uploaded = 0usize;

    for chunk in objects.chunks(50) {
        let response = batch_request(
            &endpoint,
            &auth,
            "upload",
            chunk.iter().map(|o| (format_oid(&o.oid), o.size)).collect(),
        )?;

        for item in response.objects {
            if let Some(err) = item.error {
                return Err(LineageError::Other(format!(
                    "LFS batch upload error for {}: {}",
                    item.oid, err.message
                )));
            }
            let Some(actions) = item.actions else {
                continue;
            };
            let Some(action) = actions.upload else {
                continue;
            };
            let oid = normalize_oid(&item.oid);
            let lfs = LfsStore::new(repo.path());
            let data = lfs.get(&oid)?;
            put_with_action(&action, &data)?;
            uploaded += 1;
        }
    }

    Ok(uploaded)
}

pub fn fetch_via_http_batch(
    repo: &Repository,
    remote: &str,
    objects: &[LfsObject],
) -> Result<usize, LineageError> {
    if objects.is_empty() {
        return Ok(0);
    }
    let endpoint = discover_lfs_endpoint(repo, remote)?;
    let auth = git_credentials(repo, &endpoint)?;
    let lfs = LfsStore::new(repo.path());
    let mut downloaded = 0usize;

    let missing: Vec<LfsObject> = objects
        .iter()
        .filter(|o| !lfs.exists(&o.oid))
        .cloned()
        .collect();

    for chunk in missing.chunks(50) {
        let response = batch_request(
            &endpoint,
            &auth,
            "download",
            chunk
                .iter()
                .map(|o| (format_oid(&o.oid), o.size))
                .collect(),
        )?;

        for item in response.objects {
            if let Some(err) = item.error {
                return Err(LineageError::Other(format!(
                    "LFS batch download error for {}: {}",
                    item.oid, err.message
                )));
            }
            let Some(actions) = item.actions else {
                continue;
            };
            let Some(action) = actions.download else {
                continue;
            };
            let data = get_with_action(&action)?;
            let oid = normalize_oid(&item.oid);
            lfs.put(&data)?;
            if normalize_oid(&oid) == normalize_oid(&sha256_hex(&data)) {
                downloaded += 1;
            }
        }
    }

    Ok(downloaded)
}

fn batch_request(
    endpoint: &str,
    auth: &GitAuth,
    operation: &str,
    objects: Vec<(String, usize)>,
) -> Result<BatchResponse, LineageError> {
    let url = format!("{}/objects/batch", endpoint.trim_end_matches('/'));
    let body = BatchRequest {
        operation,
        transfers: ["basic"],
        objects: objects
            .into_iter()
            .map(|(oid, size)| BatchObject { oid, size })
            .collect(),
    };

    let req = ureq::post(&url)
        .set("Accept", "application/vnd.git-lfs+json")
        .set("Content-Type", "application/vnd.git-lfs+json")
        .timeout(Duration::from_secs(120));

    let response = apply_auth(req, auth)
        .send_json(ureq::json!(body))
        .map_err(|e| LineageError::Other(format!("LFS batch request failed: {e}")))?;

    if !(200..300).contains(&response.status()) {
        let status = response.status();
        let text = response.into_string().unwrap_or_default();
        return Err(LineageError::Other(format!(
            "LFS batch HTTP {status}: {text}"
        )));
    }

    response
        .into_json::<BatchResponse>()
        .map_err(|e| LineageError::Other(format!("LFS batch parse failed: {e}")))
}

fn put_with_action(action: &BatchAction, data: &[u8]) -> Result<(), LineageError> {
    let mut req = ureq::put(&action.href).timeout(Duration::from_secs(120));
    for (k, v) in &action.header {
        req = req.set(k, v);
    }
    let response = req
        .send_bytes(data)
        .map_err(|e| LineageError::Other(format!("LFS upload failed: {e}")))?;
    if !(200..300).contains(&response.status()) {
        return Err(LineageError::Other(format!(
            "LFS upload HTTP {}",
            response.status()
        )));
    }
    Ok(())
}

fn get_with_action(action: &BatchAction) -> Result<Vec<u8>, LineageError> {
    let mut req = ureq::get(&action.href).timeout(Duration::from_secs(120));
    for (k, v) in &action.header {
        req = req.set(k, v);
    }
    let response = req
        .call()
        .map_err(|e| LineageError::Other(format!("LFS download failed: {e}")))?;
    if !(200..300).contains(&response.status()) {
        return Err(LineageError::Other(format!(
            "LFS download HTTP {}",
            response.status()
        )));
    }
    let mut reader = response.into_reader();
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf)
        .map_err(|e| LineageError::Other(format!("LFS download read failed: {e}")))?;
    Ok(buf)
}

struct GitAuth {
    username: Option<String>,
    password: Option<String>,
}

fn git_credentials(repo: &Repository, endpoint: &str) -> Result<GitAuth, LineageError> {
    let host = endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("");

    let workdir = repo
        .workdir()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| repo.path().to_path_buf());

    let protocol = if endpoint.starts_with("http://") {
        "http"
    } else {
        "https"
    };

    let input = format!("protocol={protocol}\nhost={host}\n\n");
    let output = Command::new("git")
        .args(["credential", "fill"])
        .current_dir(&workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .map_err(|e| LineageError::Other(format!("git credential fill failed: {e}")))?;

    let mut username = None;
    let mut password = None;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(v) = line.strip_prefix("username=") {
            username = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("password=") {
            password = Some(v.to_string());
        }
    }

    Ok(GitAuth { username, password })
}

fn apply_auth(req: ureq::Request, auth: &GitAuth) -> ureq::Request {
    if let (Some(user), Some(pass)) = (&auth.username, &auth.password) {
        req.set("Authorization", &basic_auth(user, pass))
    } else {
        req
    }
}

fn basic_auth(user: &str, pass: &str) -> String {
    use base64::Engine;
    let token = format!("{user}:{pass}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(token.as_bytes())
    )
}

fn git_remote_url(repo: &Repository, remote: &str) -> Result<String, LineageError> {
    let workdir = repo
        .workdir()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| repo.path().to_path_buf());
    let output = Command::new("git")
        .args(["remote", "get-url", remote])
        .current_dir(&workdir)
        .output()
        .map_err(|e| LineageError::Other(e.to_string()))?;
    if !output.status.success() {
        return Err(LineageError::Other(format!(
            "remote {remote} not found"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_config_value(repo: &Repository, key: &str) -> Option<String> {
    let workdir = repo
        .workdir()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| repo.path().to_path_buf());
    let output = Command::new("git")
        .args(["config", "--get", key])
        .current_dir(&workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if v.is_empty() { None } else { Some(v) }
}

fn remote_url_to_lfs_endpoint(url: &str) -> String {
    let base = url.trim_end_matches('/').to_string();
    format!("{base}/info/lfs")
}

fn format_oid(oid: &str) -> String {
    let n = normalize_oid(oid);
    if n.starts_with("sha256:") {
        n
    } else {
        format!("sha256:{n}")
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

pub fn collect_lfs_objects(repo: &Repository) -> Result<Vec<LfsObject>, LineageError> {
    use crate::lfs_refs::{collect_all_blob_refs, read_lfs_pointer_ref};
    let lfs = LfsStore::new(repo.path());
    let mut objects = Vec::new();
    for blob_ref in collect_all_blob_refs(repo)? {
        let oid = normalize_oid(&blob_ref);
        let size = if lfs.exists(&oid) {
            lfs.get(&oid)?.len()
        } else if let Some(pointer) = read_lfs_pointer_ref(repo, &oid)? {
            lineage_store::LfsStore::parse_pointer(&pointer)
                .map(|(_, s)| s)
                .unwrap_or(0)
        } else {
            0
        };
        objects.push(LfsObject { oid, size });
    }
    Ok(objects)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_url_to_lfs_endpoint_appends_info_lfs() {
        assert_eq!(
            remote_url_to_lfs_endpoint("https://github.com/org/repo.git"),
            "https://github.com/org/repo.git/info/lfs"
        );
    }

    #[test]
    fn format_oid_adds_sha256_prefix() {
        assert_eq!(format_oid("deadbeef"), "sha256:deadbeef");
        assert_eq!(format_oid("sha256:abc"), "sha256:abc");
    }

    #[test]
    fn basic_auth_encodes_credentials() {
        let header = basic_auth("user", "pass");
        assert!(header.starts_with("Basic "));
    }
}
