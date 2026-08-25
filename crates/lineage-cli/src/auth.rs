//! Client half of the Tribal server CLI login. The CLI only ever talks to the
//! Tribal server — the device flow's identity provider stays a server-side
//! concern, and the durable credential here is an opaque session handle, not a
//! provider refresh token. The handle is exchanged for a short-lived JWT at the
//! start of every authenticated command, so no access-token expiry bookkeeping
//! is needed on this side.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::interactive::interactive;

use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Production Tribal API base URL when no `--server` and no stored default.
pub const DEFAULT_SERVER: &str = "https://api.usetribal.io/api";

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
    /// The identity is established but carries no organization membership yet,
    /// so the server cannot issue a usable token. Membership needs a GitHub read
    /// that the identity provider's device flow cannot request, which is what the
    /// trust grant below supplies.
    #[serde(rename = "trust_grant_required", rename_all = "camelCase")]
    TrustGrantRequired { trust_grant_handle: String },
    #[serde(rename = "complete", rename_all = "camelCase")]
    Complete {
        access_token: String,
        expires_in: u64,
        session_handle: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustGrantStartResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum TrustGrantPollResponse {
    #[serde(rename = "pending", rename_all = "camelCase")]
    Pending { slow_down: bool },
    #[serde(rename = "complete", rename_all = "camelCase")]
    Complete { tenant_count: u64 },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeResponse {
    pub access_token: String,
    pub expires_in: u64,
}

/// The CLI's per-machine config directory. Everything the CLI stores about the
/// machine rather than about a repository lives here — the login, and the repo
/// registry beside it.
pub fn config_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var(CONFIG_DIR_ENV) {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Ok(PathBuf::from(xdg).join("tribal"));
        }
    }
    let home = std::env::var("HOME").map_err(|_| "cannot locate config: HOME is not set")?;
    Ok(PathBuf::from(home).join(".config").join("tribal"))
}

pub fn credentials_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("credentials.json"))
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

pub fn trust_grant_start(server: &str) -> Result<TrustGrantStartResponse> {
    let url = format!("{}/auth/cli/trust-grant", normalize_server(server));
    let response = ureq::post(&url)
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(status, response) => format!(
                "organization access request failed (HTTP {status}): {}",
                server_message(response)
            ),
            other => format!("organization access request failed: {other}"),
        })?;
    Ok(response.into_json()?)
}

pub fn trust_grant_poll(
    server: &str,
    trust_grant_handle: &str,
    device_code: &str,
) -> Result<TrustGrantPollResponse> {
    let url = format!("{}/auth/cli/trust-grant/poll", normalize_server(server));
    let response = ureq::post(&url)
        .timeout(HTTP_TIMEOUT)
        .send_json(serde_json::json!({
            "trustGrantHandle": trust_grant_handle,
            "deviceCode": device_code,
        }))
        .map_err(|e| match e {
            ureq::Error::Status(status, response) => format!(
                "organization access failed (HTTP {status}): {}",
                server_message(response)
            ),
            other => format!("organization access poll failed: {other}"),
        })?;
    Ok(response.into_json()?)
}

pub fn exchange(server: &str, session_handle: &str) -> Result<ExchangeResponse> {
    let url = format!("{}/auth/cli/exchange", normalize_server(server));
    let response = ureq::post(&url)
        .timeout(HTTP_TIMEOUT)
        .send_json(serde_json::json!({ "sessionHandle": session_handle }))
        .map_err(|e| -> Box<dyn std::error::Error> {
            match e {
                // The server 401s exactly when the stored session is dead
                // (expired, revoked, or tampered) — the recovery is always a
                // fresh login, so this is typed for the resolver to act on.
                ureq::Error::Status(401, _) => Box::new(SessionRejected),
                ureq::Error::Status(status, response) => format!(
                    "token exchange failed (HTTP {status}): {}",
                    server_message(response)
                )
                .into(),
                other => format!("token exchange failed: {other}").into(),
            }
        })?;
    Ok(response.into_json()?)
}

/// Resolves the server to talk to: explicit flag, stored default, else production.
pub fn resolve_server(flag: Option<&str>) -> Result<String> {
    if let Some(server) = flag {
        return Ok(normalize_server(server));
    }
    if let Some(server) = load_credentials()?.default_server {
        return Ok(server);
    }
    Ok(normalize_server(DEFAULT_SERVER))
}

/// Exchanges the stored session handle for a fresh short-lived access token.
///
/// [`NotAuthenticated`] rather than a plain error for the two states a sign-in
/// resolves — no stored login, and a stored login the server no longer accepts —
/// so [`resolve_token`] can act on them and every other failure still surfaces
/// as itself.
pub fn access_token_for(server: &str) -> Result<String> {
    let server = normalize_server(server);
    let credentials = load_credentials()?;
    let Some(stored) = credentials.servers.get(&server) else {
        return Err(NotAuthenticated::new(&server).into());
    };
    match exchange(&server, &stored.session_handle) {
        Ok(response) => Ok(response.access_token),
        Err(error) if error.is::<SessionRejected>() => Err(NotAuthenticated::new(&server).into()),
        Err(error) => Err(error),
    }
}

/// The server rejected the stored session handle: expired, revoked, or tampered.
/// A distinct type because it is the one exchange failure a fresh login fixes.
#[derive(Debug)]
pub struct SessionRejected;

impl std::fmt::Display for SessionRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stored login is no longer valid")
    }
}

impl std::error::Error for SessionRejected {}

/// No usable credential for this server. Carries the server so a caller that
/// cannot sign in — no TTY — can still name it in the error a user reads.
#[derive(Debug)]
pub struct NotAuthenticated {
    pub server: String,
}

impl NotAuthenticated {
    fn new(server: &str) -> Self {
        Self {
            server: server.to_string(),
        }
    }
}

impl std::fmt::Display for NotAuthenticated {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not logged in to {}: run `tribal login`", self.server)
    }
}

impl std::error::Error for NotAuthenticated {}

/// The token every command that talks to a Lineage server uses.
///
/// One resolver rather than one per command: the precedence is a contract with
/// scripts (an explicit `--token` and `LINEAGE_TOKEN` bypass the stored login
/// entirely, for CI), and a second copy of it is a second thing to get wrong —
/// which had already happened once, between `sync` and `pull`.
///
/// It is also the only place a sign-in can start, and that is deliberate: a new
/// command needs a token, the only way to get one is here, so it inherits the
/// sign-in without anyone maintaining a list of which commands need auth.
///
/// `sign_in` runs only when nothing is stored or the server rejected what was,
/// and only on a terminal — see [`resolve_token_with`].
pub fn resolve_token(server: &str, token: Option<&str>) -> Result<String> {
    resolve_token_with(server, token, interactive(), |server| {
        crate::commands::login(Some(server))
    })
}

/// [`resolve_token`] with its two ambient inputs injected: whether a sign-in can
/// be run interactively, and what running one does.
pub fn resolve_token_with(
    server: &str,
    token: Option<&str>,
    interactive: bool,
    sign_in: impl FnOnce(&str) -> Result<()>,
) -> Result<String> {
    let explicit = token
        .map(str::to_string)
        .filter(|t| !t.is_empty())
        .or_else(|| {
            std::env::var("LINEAGE_TOKEN")
                .ok()
                .filter(|t| !t.is_empty())
        });
    if let Some(token) = explicit {
        return Ok(token);
    }

    let error = match access_token_for(server) {
        Ok(token) => return Ok(token),
        Err(error) => error,
    };
    if !error.is::<NotAuthenticated>() {
        return Err(error);
    }
    // Off a terminal there is nobody to approve the browser step, so the device
    // flow would block until its code expired. Failing with the message that
    // names the fix is the better answer for a script.
    if !interactive {
        return Err(error);
    }

    sign_in(server)?;
    access_token_for(server)
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
