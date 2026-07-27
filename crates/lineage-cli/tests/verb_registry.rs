//! The anti-drift guarantee: the CLI subcommand set equals the verb registry.
//!
//! `lineage-retrieval::VERBS` is the single definition of the traversal
//! vocabulary, and the whole "one vocabulary, two consumers" thesis rests on no
//! capability existing for one consumer and not the other. Wiring is explicit
//! match arms by design, so this test — not a generator — is what stops the
//! surfaces drifting apart. The MCP half of the equality lives in
//! `lineage-mcp/tests/server.rs`.
//!
//! `CONTINUE_SESSION` is registered beside the verbs and covered here too, but
//! it lands in a different place on this surface: `git lineage fork`, a
//! top-level command, not a `context` subcommand. That difference is precisely
//! why it is not in `VERBS` — everything in that list is reachable as
//! `git lineage context <cli>`.

use std::process::Command;

use lineage_retrieval::{CONTINUE_SESSION, VERBS};

/// The subcommand names `git lineage context --help` advertises. Read from the
/// built binary rather than from the source enum: what an agent can actually
/// invoke is what clap accepts, not what a table says.
fn context_subcommands() -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_git-lineage"))
        .args(["context", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success(), "context --help failed: {out:?}");
    let help = String::from_utf8(out.stdout).unwrap();

    // clap lists subcommands as indented `name  description` lines under
    // "Commands:", ending at the first blank line.
    help.lines()
        .skip_while(|line| !line.starts_with("Commands:"))
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

#[test]
fn every_registry_verb_is_a_context_subcommand() {
    let subcommands = context_subcommands();
    assert!(
        subcommands.contains(&"query".to_string()),
        "sanity: help parsing found no subcommands ({subcommands:?})"
    );
    for verb in VERBS {
        assert!(
            subcommands.contains(&verb.cli.to_string()),
            "verb {} is in the registry but not on the CLI (have: {subcommands:?})",
            verb.relation,
        );
    }
}

/// The CLI half of the continuation pairing. Read from the built binary for the
/// same reason the verbs are: what an agent can invoke is what clap accepts.
///
/// `--brief` is asserted alongside the command because the capability the
/// `SessionStart` vocabulary names is the pair — a vocabulary offering a flag
/// the binary rejects is worse than one that never mentioned it.
#[test]
fn the_continuation_capability_is_a_top_level_command_with_brief() {
    let out = Command::new(env!("CARGO_BIN_EXE_git-lineage"))
        .args([CONTINUE_SESSION.cli, "--help"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "`{} --help` failed: {out:?}",
        CONTINUE_SESSION.cli,
    );
    let help = String::from_utf8(out.stdout).unwrap();
    assert!(
        help.contains("--brief"),
        "`{}` does not offer --brief: {help}",
        CONTINUE_SESSION.cli,
    );
}

/// The other direction. `context` also carries non-traversal plumbing (hook,
/// install, log, chain, query, salience) that is deliberately not a verb, so
/// the check is that no *traversal-shaped* subcommand exists outside the
/// registry — enumerated here so adding one without registering it fails.
#[test]
fn no_traversal_subcommand_exists_outside_the_registry() {
    const NON_VERB_SUBCOMMANDS: &[&str] = &[
        "hook",
        "log",
        "install",
        "uninstall",
        "salience",
        "chain",
        "query",
        "help",
    ];

    for name in context_subcommands() {
        let known = NON_VERB_SUBCOMMANDS.contains(&name.as_str())
            || VERBS.iter().any(|verb| verb.cli == name);
        assert!(
            known,
            "`context {name}` is neither registered plumbing nor a registry verb",
        );
    }
}
