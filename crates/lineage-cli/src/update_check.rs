//! Whether a newer release exists, and telling the user about it once.
//!
//! The CLI is installed by a shell script rather than a package manager, so
//! nothing on the machine tells a user a release happened. Without this they
//! find out by hitting a bug that was already fixed. The check runs on its own
//! and reports; it never installs anything, because updating a running binary
//! in place is a different problem from knowing that you should.
//!
//! Best-effort throughout, in the same sense as [`crate::repo_registry`]: every
//! failure — offline, a rate limit, a body that is not what we expect — is
//! "no notice this time". A version check must never fail a command, delay one
//! noticeably, or turn a working repository into an error message.
//!
//! Latency is the reason for the cache rather than a nicety. The check is
//! reached from the command dispatcher, so a synchronous request would put the
//! network on the critical path of every invocation. Instead the answer is
//! stored with the time it was fetched, and a lookup inside the TTL is one
//! small file read.

use std::fs;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const CACHE_FILE: &str = "update-check.json";

/// How long a fetched answer is trusted.
///
/// Short because releases are currently frequent: a day-long TTL would mean a
/// user running the CLI hourly still hears about a release a day late. It costs
/// one small request per 15 minutes of active use, which is well inside an
/// unauthenticated rate limit.
const CACHE_TTL: chrono::Duration = chrono::Duration::minutes(15);

/// Kept well under a second: this is time a user waits with nothing on screen,
/// and the answer is never worth waiting for.
const FETCH_TIMEOUT: Duration = Duration::from_secs(2);

const RELEASES_URL: &str = "https://api.github.com/repos/usetribal/tribal/releases/latest";

/// The command that installs the newest release.
///
/// Deliberately the redirecting URL on our own domain rather than a GitHub
/// release asset: the asset URLs that the installer itself is built with are
/// pinned to the version that generated them, so one copied out of a script
/// installs that old version forever. This one resolves to whatever the newest
/// release is at the moment it is run.
pub const UPDATE_COMMAND: &str = "curl -fsSL https://usetribal.io/install.sh | bash";

/// Set to any non-empty value to silence the notice.
pub const OPT_OUT_ENV: &str = "TRIBAL_NO_UPDATE_CHECK";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cache {
    /// The newest version seen on the last successful fetch.
    pub latest: String,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

pub fn cache_path() -> Result<PathBuf> {
    Ok(crate::auth::config_dir()?.join(CACHE_FILE))
}

/// The cached answer, or `None` for anything that is not a readable cache.
///
/// Corruption is treated as absence, matching the repo registry: the only
/// recovery worth having is to fetch again, which the caller does anyway.
fn load_cache() -> Option<Cache> {
    let path = cache_path().ok()?;
    let text = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_cache(cache: &Cache) -> Result<()> {
    let path = cache_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, serde_json::to_string_pretty(cache)?)?;
    Ok(())
}

fn is_fresh(cache: &Cache, now: DateTime<Utc>) -> bool {
    // A cache stamped in the future is a clock that moved backwards, not a
    // valid entry to trust for longer than the TTL.
    let age = now.signed_duration_since(cache.fetched_at);
    age >= chrono::Duration::zero() && age < CACHE_TTL
}

/// Ask GitHub for the newest release tag.
///
/// A tag is `v0.5.0`; the leading `v` is stripped so it compares against the
/// crate version as written in `Cargo.toml`.
fn fetch_latest() -> Option<String> {
    let response = ureq::get(RELEASES_URL)
        .set("User-Agent", "tribal-cli")
        .timeout(FETCH_TIMEOUT)
        .call()
        .ok()?;
    let release: Release = response.into_json().ok()?;
    let tag = release.tag_name.trim();
    let version = tag.strip_prefix('v').unwrap_or(tag);
    if version.is_empty() {
        return None;
    }
    Some(version.to_string())
}

/// The newest known version, fetching only when the cache has expired.
fn latest_version(now: DateTime<Utc>) -> Option<String> {
    if let Some(cache) = load_cache() {
        if is_fresh(&cache, now) {
            return Some(cache.latest);
        }
    }

    let latest = fetch_latest()?;
    let cache = Cache {
        latest: latest.clone(),
        fetched_at: now,
    };
    if let Err(error) = save_cache(&cache) {
        // A cache we cannot write means the next run fetches again, which is
        // slower but still correct — not a reason to withhold the notice.
        tracing::warn!("update check cache write failed: {error}");
    }
    Some(latest)
}

/// Compare two dotted version strings numerically.
///
/// A string comparison would rank `0.10.0` below `0.9.0`, which is exactly the
/// case this has to get right.
///
/// Only the release part is compared: anything from the first `-` or `+` is
/// dropped, so `0.5.0-rc.1` reads as `0.5.0` and does not announce itself as an
/// upgrade from the release it is a candidate for. A component that is not a
/// number counts as zero, which keeps an unrecognised tag from ever looking
/// newer than a real version.
pub(crate) fn is_newer(latest: &str, current: &str) -> bool {
    let parts = |version: &str| -> Vec<u64> {
        version
            .split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (latest, current) = (parts(latest), parts(current));

    for index in 0..latest.len().max(current.len()) {
        let l = latest.get(index).copied().unwrap_or(0);
        let c = current.get(index).copied().unwrap_or(0);
        if l != c {
            return l > c;
        }
    }
    false
}

/// Whether a notice may be printed at all, before any work is done to find one.
///
/// `--json` is excluded by the caller rather than here: this answers the
/// questions that are about the environment, not the invocation.
fn notices_allowed() -> bool {
    if std::env::var(OPT_OUT_ENV).is_ok_and(|value| !value.is_empty()) {
        return false;
    }
    // Off a terminal the reader is a script, a pipe, or CI, none of which can
    // act on the notice and all of which would have it land in their output.
    std::io::stdout().is_terminal()
}

/// The notice to print after a command's own output, if there is one.
///
/// Separate from printing it so the decision is testable without capturing
/// stdout, and so the caller controls placement.
pub fn notice(current: &str, now: DateTime<Utc>) -> Option<String> {
    if !notices_allowed() {
        return None;
    }
    let latest = latest_version(now)?;
    if !is_newer(&latest, current) {
        return None;
    }
    Some(format!(
        "\nA new version of tribal is available: {current} -> {latest}\n  {UPDATE_COMMAND}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_versions_numerically_not_lexically() {
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("0.6.0", "0.5.0"));
        assert!(is_newer("1.0.0", "0.99.9"));
    }

    #[test]
    fn same_or_older_is_not_newer() {
        assert!(!is_newer("0.5.0", "0.5.0"));
        assert!(!is_newer("0.4.0", "0.5.0"));
    }

    #[test]
    fn missing_components_count_as_zero() {
        assert!(is_newer("0.5.1", "0.5"));
        assert!(!is_newer("0.5", "0.5.0"));
    }

    #[test]
    fn unparsable_versions_never_announce_themselves() {
        assert!(!is_newer("garbage", "0.5.0"));
        assert!(!is_newer("0.5.0-rc.1", "0.5.0"));
    }

    #[test]
    fn cache_is_fresh_inside_the_ttl_only() {
        let now = Utc::now();
        let cache = |fetched_at| Cache {
            latest: "0.6.0".into(),
            fetched_at,
        };

        assert!(is_fresh(&cache(now), now));
        assert!(is_fresh(&cache(now - chrono::Duration::minutes(14)), now));
        assert!(!is_fresh(&cache(now - chrono::Duration::minutes(16)), now));
    }

    #[test]
    fn a_cache_stamped_in_the_future_is_not_fresh() {
        let now = Utc::now();
        let cache = Cache {
            latest: "0.6.0".into(),
            fetched_at: now + chrono::Duration::hours(1),
        };
        assert!(!is_fresh(&cache, now));
    }
}
