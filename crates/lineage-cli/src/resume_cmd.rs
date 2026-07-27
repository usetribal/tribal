//! `git lineage resume <session-id>` — reopen a session that is already on this
//! machine, in the harness that produced it.
//!
//! Sibling of `fork`, and the difference between them is the whole reason this
//! is a separate verb. Fork *writes* a new session out of stored turns, so it
//! works for a teammate's session and produces a new identity. Resume writes
//! nothing: it names a session the harness already holds, so it only works for a
//! session this machine imported, and the session it reopens is the same one.
//!
//! Like fork, it prints the command rather than running it. A printed command is
//! actionable by a human reading a terminal and by an agent that invoked the
//! CLI; spawning a terminal is actionable by neither.
//!
//! Nothing here knows a vendor verb, a flag, or an id convention — the adapter
//! returns the whole invocation (ARCHITECTURE.md invariant 4).

use std::path::Path;

use lineage_adapters::all_adapters;
use lineage_agent::ResumeInvocation;
use lineage_core::Conversation;
use lineage_git::{open_repo, read_conversation, resolve_session};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn resume(repo_path: &Path, session_hint: &str) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let id = resolve_session(repo.inner(), session_hint).map_err(|error| error.to_string())?;
    let source =
        read_conversation(repo.inner(), &id)?.ok_or_else(|| {
            format!(
                "no session {session_hint} in this repository's lineage refs. \
             `git lineage list` shows what is here"
            )
        })?;

    // Refusals — the agent cannot be reopened at all, or this session carries no
    // vendor id — arrive from the adapter naming the agent, rather than as a
    // vendor branch here.
    let invocation = resume_invocation_for(repo.workdir(), &source)?;

    println!("{} session {}", source.agent.as_str(), source.id);
    println!();
    print_invocation(&invocation);
    Ok(())
}

fn resume_invocation_for(workdir: &Path, source: &Conversation) -> Result<ResumeInvocation> {
    let adapter = all_adapters(workdir)
        .into_iter()
        .find(|(kind, _)| *kind == source.agent)
        .map(|(_, adapter)| adapter)
        .ok_or_else(|| {
            format!(
                "no adapter for {} is compiled into this build, so its sessions cannot be resumed",
                source.agent.as_str()
            )
        })?;
    Ok(adapter.resume_invocation(source)?)
}

/// The command is adapter-supplied verbatim, and so is whether a directory
/// matters: a harness that resolves a session globally would make "run this
/// from …" a false instruction.
fn print_invocation(invocation: &ResumeInvocation) {
    match &invocation.cwd {
        Some(cwd) => println!("To reopen it, run this from {}:", cwd.display()),
        None => println!("To reopen it, run:"),
    }
    println!();
    println!("    {}", invocation.command);
    println!();
    println!("This is the original session, not a copy: continuing it adds to its history.");
    println!("To continue someone else's work as your own instead, use `git lineage fork`.");
}
