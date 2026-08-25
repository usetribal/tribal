//! Whether this run may open a TUI or ask a question.
//!
//! One resolved decision consulted everywhere, rather than an `is_terminal`
//! call at each site. The checks used to be split between stdin and stdout, so
//! `tribal list | less` and `tribal list < /dev/null` disagreed about whether a
//! selector could open — one of them wrong in either direction.

use std::io::{self, IsTerminal};
use std::sync::OnceLock;

/// Set once from the parsed `--no-interactive` flag, before any command runs.
///
/// A global rather than an argument threaded through every handler: the flag is
/// global on the parser, and the alternative is a parameter on every function
/// that might one day prompt, which is the drift this replaces.
static NO_INTERACTIVE: OnceLock<bool> = OnceLock::new();

/// Record `--no-interactive` for the rest of the process.
pub fn set_no_interactive(no_interactive: bool) {
    let _ = NO_INTERACTIVE.set(no_interactive);
}

/// Whether a TUI may open or a question may be asked.
///
/// Both streams must be a terminal: a selector has to draw to stdout *and* read
/// keys from stdin, so either one being redirected makes it undriveable.
pub fn interactive() -> bool {
    if NO_INTERACTIVE.get().copied().unwrap_or(false) {
        return false;
    }
    resolve(io::stdin().is_terminal(), io::stdout().is_terminal())
}

/// The rule itself, separated from the process state so it can be tested
/// against inputs a test cannot otherwise produce.
fn resolve(stdin_tty: bool, stdout_tty: bool) -> bool {
    stdin_tty && stdout_tty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interactive_requires_both_streams() {
        assert!(resolve(true, true));
        assert!(!resolve(true, false));
        assert!(!resolve(false, true));
        assert!(!resolve(false, false));
    }
}
