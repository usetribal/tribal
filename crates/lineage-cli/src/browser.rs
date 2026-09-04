//! Opens a URL in the system browser.
//!
//! Failure is never fatal: the printed URL is the product, and a headless box,
//! a missing opener, or a non-zero exit must not fail a command that already
//! printed the link.

use std::process::Command;

use crate::ui;

/// Try to open `url`. On failure, print `fallback` so the user still has a next
/// step — typically that the printed link is the one to open by hand.
pub fn open(url: &str, fallback: &str) {
    let (program, args) = opener();
    if launch(program, args, url) {
        return;
    }
    ui::action(fallback);
}

/// Split out so the failure path is testable without a PATH that has no opener:
/// tests call it with a program that cannot exist.
fn launch(program: &str, args: &[&str], url: &str) -> bool {
    Command::new(program)
        .args(args)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn opener() -> (&'static str, &'static [&'static str]) {
    if cfg!(target_os = "macos") {
        return ("open", &[]);
    }
    if cfg!(target_os = "windows") {
        // `start` is a shell builtin, so it needs cmd; the empty title argument
        // stops cmd reading the URL as the window title.
        return ("cmd", &["/C", "start", ""]);
    }
    ("xdg-open", &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_opener_is_reported_as_a_failed_launch_rather_than_panicking() {
        assert!(!launch(
            "lineage-no-such-browser-opener",
            &[],
            "https://app.usetribal.io/s/token"
        ));
    }

    #[test]
    fn the_windows_opener_goes_through_cmd_because_start_is_a_builtin() {
        let (program, args) = opener();
        assert!(
            matches!(program, "open" | "cmd" | "xdg-open"),
            "unexpected opener: {program}"
        );
        if program == "cmd" {
            assert_eq!(args, &["/C", "start", ""]);
        }
    }
}
