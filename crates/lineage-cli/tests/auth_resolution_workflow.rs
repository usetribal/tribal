//! How a Lineage-server command gets its token, and when that starts a sign-in.
//!
//! `resolve_token_with` is exercised directly rather than through a command:
//! its two ambient inputs — whether a terminal is attached, and what signing in
//! does — are injected, so the decision can be asserted without a browser, a
//! server, or a TTY. The commands are covered by the source guard at the bottom,
//! which is what ties them to this behaviour.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use lineage_cli::auth;

/// Credentials live under a per-test directory, so a developer's real login is
/// neither read nor written by the suite.
fn isolated_config() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    // SAFETY: single-threaded test body; the variable is read by auth on demand.
    unsafe { std::env::set_var(auth::CONFIG_DIR_ENV, dir.path()) };
    dir
}

fn never_called(_: &str) -> Result<(), Box<dyn std::error::Error>> {
    panic!("sign-in must not be attempted here");
}

/// An explicit token is a script's deliberate choice; resolving it must not
/// consult, or start, a login.
#[test]
fn an_explicit_token_short_circuits_everything() {
    let _config = isolated_config();
    let token = auth::resolve_token_with(
        "https://example.invalid/api",
        Some("explicit-token"),
        true,
        never_called,
    )
    .unwrap();
    assert_eq!(token, "explicit-token");
}

/// The CI path: no stored login and no terminal, so there is nobody to approve a
/// browser step. Failing with the message that names the fix beats blocking
/// until the device code expires.
#[test]
fn without_a_terminal_a_missing_login_fails_rather_than_blocking() {
    let _config = isolated_config();
    let error = auth::resolve_token_with("https://example.invalid/api", None, false, never_called)
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("not logged in"), "{message}");
    assert!(message.contains("git lineage login"), "{message}");
}

/// The behaviour asked for: at a terminal, a command that needs a token signs
/// in rather than telling the user to run a second command.
#[test]
fn at_a_terminal_a_missing_login_starts_one() {
    let _config = isolated_config();
    static CALLS: AtomicUsize = AtomicUsize::new(0);

    let error = auth::resolve_token_with("https://example.invalid/api", None, true, |server| {
        CALLS.fetch_add(1, Ordering::SeqCst);
        assert_eq!(server, "https://example.invalid/api");
        Ok(())
    })
    .unwrap_err();

    assert_eq!(CALLS.load(Ordering::SeqCst), 1, "sign-in should have run");
    // The stub stores nothing, so the retry still finds no credential. That the
    // retry happens at all is the point: a real login would have stored one.
    assert!(error.to_string().contains("not logged in"));
}

/// A sign-in the user abandoned must surface as itself. Reporting "not logged
/// in" would describe the symptom and hide what actually went wrong.
#[test]
fn a_failed_sign_in_reports_its_own_error() {
    let _config = isolated_config();
    let error = auth::resolve_token_with("https://example.invalid/api", None, true, |_| {
        Err("device code expired".into())
    })
    .unwrap_err();
    assert!(error.to_string().contains("device code expired"));
}

/// The guard that keeps this from drifting as Team grows: a command that
/// resolves credentials its own way would silently opt out of the sign-in, and
/// nothing about adding one would prompt an author to notice.
#[test]
fn no_command_resolves_credentials_outside_auth() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();

    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        // auth.rs is where the one resolver lives.
        if path.file_name().is_some_and(|name| name == "auth.rs") {
            continue;
        }
        // Comments naming the precedence are documentation, not a second
        // resolver — the guard is about code that reads credentials.
        let text = std::fs::read_to_string(&path).unwrap();
        let resolves = text.lines().any(|line| {
            let code = line.trim();
            !code.starts_with("//")
                && (code.contains("access_token_for") || code.contains("LINEAGE_TOKEN"))
        });
        if resolves {
            offenders.push(path.file_name().unwrap().to_string_lossy().to_string());
        }
    }

    assert!(
        offenders.is_empty(),
        "these resolve credentials directly instead of through auth::resolve_token, \
         so they would not sign in when a user is logged out: {offenders:?}"
    );
}
