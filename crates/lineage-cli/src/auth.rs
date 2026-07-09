//! Client half of the Lineage server CLI login. The CLI only ever talks to the
//! Lineage server — the device flow's identity provider stays a server-side
//! concern, and the durable credential here is an opaque session handle, not a
//! provider refresh token. The handle is exchanged for a short-lived JWT at the
//! start of every authenticated command, so no access-token expiry bookkeeping
//! is needed on this side.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Overrides the credentials directory; primarily a test seam, but also lets
/// CI jobs isolate credentials from the machine account.
pub const CONFIG_DIR_ENV: &str = "LINEAGE_CONFIG_DIR";

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Credentials {
    /// Server used when a command passes no `--server`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_server: Option<String>,
    /// Keyed by server base URL; one login per server.
    #[serde(default)]
    pub servers: BTreeMap<String, ServerCredential>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ServerCredential {
    pub session_handle: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStartResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum DevicePollResponse {
    #[serde(rename = "pending", rename_all = "camelCase")]
    Pending { slow_down: bool },
    #[serde(rename = "complete", rename_all = "camelCase")]
    Complete {
        access_token: String,
        expires_in: u64,
        session_handle: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeResponse {
    pub access_token: String,
    pub expires_in: u64,
}

pub fn credentials_path() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var(CONFIG_DIR_ENV) {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir).join("credentials.json"));
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("lineage").join("credentials.json"));
        }
    }
    let home = std::env::var("HOME").map_err(|_| "cannot locate credentials: HOME is not set")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("lineage")
        .join("credentials.json"))
}

pub fn load_credentials() -> Result<Credentials> {
    let path = credentials_path()?;
    if !path.exists() {
        return Ok(Credentials::default());
    }
    let text = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&text)?)
}

pub fn save_credentials(credentials: &Credentials) -> Result<()> {
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(credentials)?)?;
    // The session handle is a bearer credential; other local users must not read it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn normalize_server(server: &str) -> String {
    server.trim_end_matches('/').to_string()
}

pub fn device_start(server: &str) -> Result<DeviceStartResponse> {
    let url = format!("{}/auth/cli/device", normalize_server(server));
    let response = ureq::post(&url)
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|e| format!("device authorization failed: {e}"))?;
    Ok(response.into_json()?)
}

pub fn device_poll(server: &str, device_code: &str) -> Result<DevicePollResponse> {
    let url = format!("{}/auth/cli/device/poll", normalize_server(server));
    let response = ureq::post(&url)
        .timeout(HTTP_TIMEOUT)
        .send_json(serde_json::json!({ "deviceCode": device_code }))
        .map_err(|e| match e {
            ureq::Error::Status(status, response) => {
                format!(
                    "sign-in failed (HTTP {status}): {}",
                    server_message(response)
                )
            }
            other => format!("sign-in poll failed: {other}"),
        })?;
    Ok(response.into_json()?)
}

pub fn exchange(server: &str, session_handle: &str) -> Result<ExchangeResponse> {
    let url = format!("{}/auth/cli/exchange", normalize_server(server));
    let response = ureq::post(&url)
        .timeout(HTTP_TIMEOUT)
        .send_json(serde_json::json!({ "sessionHandle": session_handle }))
        .map_err(|e| match e {
            // The server 401s exactly when the stored session is dead (expired,
            // revoked, or tampered) — the recovery is always a fresh login.
            ureq::Error::Status(401, _) => {
                "stored login is no longer valid: run `git lineage login`".to_string()
            }
            ureq::Error::Status(status, response) => {
                format!(
                    "token exchange failed (HTTP {status}): {}",
                    server_message(response)
                )
            }
            other => format!("token exchange failed: {other}"),
        })?;
    Ok(response.into_json()?)
}

/// Resolves the server to talk to: explicit flag, else the stored default.
pub fn resolve_server(flag: Option<&str>) -> Result<String> {
    if let Some(server) = flag {
        return Ok(normalize_server(server));
    }
    if let Some(server) = load_credentials()?.default_server {
        return Ok(server);
    }
    Err("no server: pass --server or run `git lineage login --server <url>`".into())
}

/// Exchanges the stored session handle for a fresh short-lived access token.
pub fn access_token_for(server: &str) -> Result<String> {
    let server = normalize_server(server);
    let credentials = load_credentials()?;
    let stored = credentials.servers.get(&server).ok_or_else(|| {
        format!("not logged in to {server}: run `git lineage login --server {server}`")
    })?;
    Ok(exchange(&server, &stored.session_handle)?.access_token)
}

pub fn store_login(server: &str, session_handle: String) -> Result<()> {
    let server = normalize_server(server);
    let mut credentials = load_credentials()?;
    credentials
        .servers
        .insert(server.clone(), ServerCredential { session_handle });
    credentials.default_server = Some(server);
    save_credentials(&credentials)
}

fn server_message(response: ureq::Response) -> String {
    // Nest error bodies carry the user-facing sentence in `message`.
    let text = response.into_string().unwrap_or_default();
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or(text)
}
