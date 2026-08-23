//! `tribal fork` — carry on an agent session, whether or not it started
//! here.
//!
//! One verb, because which of the two ways applies is a property of the session
//! rather than a decision worth putting to the user: a session the harness still
//! holds is reopened in place, and any other is written out as a new session
//! carrying the original's context. The adapter is the oracle — it either
//! returns an invocation for the session or it does not — so nothing here
//! inspects agents or ids to choose.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use lineage_adapters::all_adapters;
use lineage_agent::{RenderedTranscript, ResumeInvocation};
use lineage_core::{display_title, files_written, opening_ask, Conversation};
use lineage_git::{open_repo, persist_conversation, read_conversation, resolve_session};

use crate::brief;
use crate::digest::traversal_vocabulary;
use crate::events::{EventLog, Outcome};
use crate::session_pick::{self, ForkPickOptions, ForkPickResult};
use crate::share_fork;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const TOPIC_MAX_CHARS: usize = 160;
const FILES_SHOWN: usize = 5;

#[derive(Debug, Clone, Default)]
pub struct ForkRequest {
    pub pick: ForkPickOptions,
    /// Write a new session out even when the harness could reopen this one.
    pub force_fork: bool,
    pub brief: bool,
    pub json: bool,
    /// Share-link options, meaningful only when the argument is a share URL.
    pub share: ShareOptions,
}

#[derive(Debug, Clone, Default)]
pub struct ShareOptions {
    pub server: Option<String>,
    pub into: Option<PathBuf>,
    pub no_open: bool,
}

pub fn fork_session(repo_path: &Path, request: ForkRequest) -> Result<()> {
    // A share link and a session id cannot be confused: ids are ULIDs, id
    // prefixes, or harness UUIDs, none of which carry a scheme or a slash. So
    // the argument decides which path this takes, with no flag to remember.
    if let Some(url) = request
        .pick
        .session_id
        .as_deref()
        .filter(|argument| share_fork::is_share_url(argument))
    {
        share_fork::fork_share(&share_fork::ShareForkRequest {
            url: url.to_string(),
            server: request.share.server.clone(),
            into: request.share.into.clone(),
            no_open: request.share.no_open,
        })?;
        return Ok(());
    }

    let picked = session_pick::pick_fork_session(repo_path, &request.pick)?;
    if request.json {
        println!("{}", serde_json::to_string_pretty(&picked)?);
        if should_stop_after_json(&request, &picked) {
            return Ok(());
        }
    }

    // Reopening is tried first and only for a plain continue: --brief writes
    // nothing by definition, and --fork is the explicit request for a new
    // session. Any adapter refusal — the harness cannot resume, or this session
    // carries no vendor id because it came from a teammate — means there is
    // nothing here to reopen, which is exactly when writing one out is right.
    if !request.brief && !request.force_fork {
        if let Some(invocation) = resume_invocation(repo_path, &picked.session_id)? {
            print_resume(&invocation);
            return Ok(());
        }
    }

    if let Some(rendered) = fork_resolved(repo_path, &picked.session_id, request.brief)? {
        print_next_step(&rendered);
    }
    Ok(())
}

/// The invocation that reopens this session in the harness that produced it, or
/// `None` when nothing on this machine holds it.
fn resume_invocation(repo_path: &Path, session_id: &str) -> Result<Option<ResumeInvocation>> {
    let repo = open_repo(repo_path)?;
    let id = resolve_session(repo.inner(), session_id).map_err(|error| error.to_string())?;
    let Some(source) = read_conversation(repo.inner(), &id)? else {
        return Ok(None);
    };
    let adapter = all_adapters(repo.workdir())
        .into_iter()
        .find(|(kind, _)| *kind == source.agent)
        .map(|(_, adapter)| adapter);
    let Some(adapter) = adapter else {
        return Ok(None);
    };
    Ok(adapter.resume_invocation(&source).ok())
}

/// The command is adapter-supplied verbatim, and so is whether a directory
/// matters: a harness that resolves a session globally would make "run this
/// from …" a false instruction.
fn print_resume(invocation: &ResumeInvocation) {
    match &invocation.cwd {
        Some(cwd) => println!("To reopen it, run this from {}:", cwd.display()),
        None => println!("To reopen it, run:"),
    }
    println!();
    println!("    {}", invocation.command);
    println!();
    println!("This is the original session, not a copy: continuing it adds to its history.");
    println!("To write it out as a new session of your own instead, add --new.");
}

fn should_stop_after_json(request: &ForkRequest, picked: &ForkPickResult) -> bool {
    request.pick.query.is_some()
        && request.pick.session_id.is_none()
        && picked.candidates.len() > 1
        && request.pick.pick.is_none()
}

/// Fork a session that is already in this repository's refs, returning what was
/// written so a caller can open it. `None` for `--brief`, which writes nothing.
///
/// Public because forking a share is this exact step with the session put there
/// first: the share path resolves where to land and persists the conversation,
/// then hands off here rather than growing a second copy of the fork rules.
pub fn fork_resolved(
    repo_path: &Path,
    session_id: &str,
    as_brief: bool,
) -> Result<Option<RenderedTranscript>> {
    let repo = open_repo(repo_path)?;
    let id = resolve_session(repo.inner(), session_id).map_err(|error| error.to_string())?;
    let source = read_conversation(repo.inner(), &id)?.ok_or_else(|| {
        format!(
            "no session {session_id} in this repository's lineage refs. \
             `tribal list` shows what is here; if the session is a teammate's, \
             fetch their lineage refs first (`tribal lfs fetch`, then `git fetch origin 'refs/lineage/*:refs/lineage/*'`)"
        )
    })?;

    if as_brief {
        print_brief(&repo, &source)?;
        return Ok(None);
    }

    let rendered = render_for(repo.workdir(), &source)?;

    if rendered.contents.trim().is_empty() {
        return Err(format!(
            "session {} has no turns that can be replayed, so there is nothing to continue. \
             `tribal show {}` shows what was stored",
            source.id, source.id
        )
        .into());
    }

    print_provenance(&source);

    write_transcript(&rendered)?;

    let fork = Conversation::fork_from(&source, rendered.session_handle.clone());
    persist_conversation(repo.inner(), &fork)?;

    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "fork",
        Outcome::Ok,
        serde_json::json!({
            "source_session_id": source.id.as_str(),
            "fork_session_id": fork.id.as_str(),
            "vendor_session_handle": rendered.session_handle,
        }),
    );

    println!("Wrote {}", rendered.path.display());
    println!("Recorded fork {} (continues {})", fork.id, source.id);
    println!();
    Ok(Some(rendered))
}

/// What to run, for a caller that is not going to run it. Separate from
/// [`fork_resolved`] because forking a share runs it instead, and printing a
/// command beside a session that is already open would read as an instruction.
pub fn print_next_step(rendered: &RenderedTranscript) {
    println!(
        "To continue it, run this from {}:",
        rendered.resume_cwd.display()
    );
    println!();
    println!("    {}", rendered.resume_command);
    println!();
    println!("You get their context, not their transcript: tool calls are replayed as prose,");
    println!(
        "so the session reads as history rather than handing you handles that no longer exist."
    );
}

fn print_brief(repo: &lineage_git::LineageRepo, source: &Conversation) -> Result<()> {
    let selection = brief::select(source, brief::MAX_TURNS, brief::MAX_BYTES);
    let block = brief::render_brief(
        source,
        &selection,
        &describe_author(source),
        &traversal_vocabulary(),
    );
    print!("{block}");

    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "fork_brief",
        Outcome::Ok,
        serde_json::json!({
            "source_session_id": source.id.as_str(),
            "turns_total": source.turns.len(),
            "turns_briefed": selection.kept.len(),
        }),
    );
    Ok(())
}

fn render_for(workdir: &Path, source: &Conversation) -> Result<RenderedTranscript> {
    let adapter = all_adapters(workdir)
        .into_iter()
        .find(|(kind, _)| *kind == source.agent)
        .map(|(_, adapter)| adapter)
        .ok_or_else(|| {
            format!(
                "no adapter for {} is compiled into this build, so its sessions cannot be forked",
                source.agent.as_str()
            )
        })?;
    Ok(adapter.render_transcript(source)?)
}

fn write_transcript(rendered: &RenderedTranscript) -> Result<()> {
    if let Some(parent) = rendered.path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&rendered.path, &rendered.contents)?;
    Ok(())
}

fn print_provenance(source: &Conversation) {
    println!("{}", describe_author(source));
    println!("  {}", describe_shape(source));
    println!("  Title:   {}", display_title(source));

    if let Some(topic) = opening_ask(source, TOPIC_MAX_CHARS) {
        println!("  Asked for: {topic}");
    }
    if let Some(files) = describe_files(source) {
        println!("  Changed:   {files}");
    }
    if !source.commit_shas.is_empty() {
        println!("  Commits:   {}", short_shas(&source.commit_shas));
    }
    println!();
}

fn describe_author(source: &Conversation) -> String {
    let who = source
        .metadata
        .get(lineage_git::PROMPTED_BY_NAME)
        .or_else(|| source.metadata.get(lineage_git::PROMPTED_BY_EMAIL))
        .and_then(|v| v.as_str())
        .unwrap_or("Someone");
    format!(
        "{who}'s {} session, {}",
        source.agent.as_str(),
        source.started_at.format("%-d %B %Y")
    )
}

fn describe_shape(source: &Conversation) -> String {
    let turns = source.turns.len();
    let plural = if turns == 1 { "turn" } else { "turns" };
    match source.primary_model() {
        Some(model) => format!("{turns} {plural}, {model}"),
        None => format!("{turns} {plural}"),
    }
}

fn describe_files(source: &Conversation) -> Option<String> {
    let files = files_written(source);
    if files.is_empty() {
        return None;
    }
    if files.len() <= FILES_SHOWN {
        return Some(files.join(", "));
    }
    Some(format!(
        "{}, +{} more",
        files[..FILES_SHOWN].join(", "),
        files.len() - FILES_SHOWN
    ))
}

fn short_shas(shas: &[String]) -> String {
    shas.iter()
        .map(|sha| sha.chars().take(8).collect::<String>())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{AgentKind, LineageId, Role, Turn};

    fn user_turn(content: &str) -> Turn {
        Turn {
            id: LineageId::new(),
            role: Role::User,
            content: content.into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        }
    }

    #[test]
    fn author_falls_back_to_someone_when_the_session_names_nobody() {
        let conv = Conversation::new(AgentKind::Claude, "/tmp/proj");
        assert!(describe_author(&conv).starts_with("Someone's claude session"));
    }

    #[test]
    fn author_prefers_the_recorded_name() {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/proj");
        conv.metadata.insert(
            lineage_git::PROMPTED_BY_NAME.into(),
            serde_json::Value::String("Alice".into()),
        );
        assert!(describe_author(&conv).starts_with("Alice's claude session"));
    }

    #[test]
    fn turn_count_reads_as_prose_for_a_single_turn() {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/proj");
        conv.turns.push(user_turn("hello"));
        assert_eq!(describe_shape(&conv), "1 turn");
    }
}
