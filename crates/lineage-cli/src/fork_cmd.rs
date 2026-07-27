//! `git lineage fork <session-id>` — pick up someone else's agent session and
//! continue it in your own harness.
//!
//! Two rules shape this file.
//!
//! **Resolve from lineage's own refs, never from `metadata["source"]`.** That
//! field is an absolute path on the machine that did the import, so gating on it
//! is exactly what makes today's extension fork un-shareable: on a teammate's
//! machine the path dangles even though the mechanism would have worked.
//!
//! **Print the command, do not run it.** A printed command is actionable by
//! both a human reading a terminal and an agent that invoked the CLI; spawning a
//! terminal is actionable by neither. It also keeps the harness in the user's
//! hands, which matters when the thing being opened is a colleague's work.

use std::fs;
use std::path::Path;

use chrono::Utc;
use lineage_adapters::all_adapters;
use lineage_agent::RenderedTranscript;
use lineage_core::{files_written, Conversation, LineageId, Role};
use lineage_git::{open_repo, persist_conversation, read_conversation};

use crate::brief;
use crate::digest::traversal_vocabulary;
use crate::events::{EventLog, Outcome};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// How much of the opening ask to show. Long enough to recognise the piece of
/// work, short enough that the command to run stays on screen.
const TOPIC_MAX_CHARS: usize = 160;
/// Files listed before the tail is summarised as a count.
const FILES_SHOWN: usize = 5;

pub fn fork(repo_path: &Path, session_id: &str, dry_run: bool, as_brief: bool) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let source = read_conversation(repo.inner(), &LineageId::from(session_id))?.ok_or_else(|| {
        format!(
            "no session {session_id} in this repository's lineage refs. \
             `git lineage list` shows what is here; if the session is a teammate's, \
             fetch their lineage refs first (`git lineage lfs fetch`, then `git fetch origin 'refs/lineage/*:refs/lineage/*'`)"
        )
    })?;

    // `--brief` returns before any adapter is consulted. It is not a preview of
    // the write and must not inherit fork's constraints: a session with no
    // renderable transcript — a pulled one, or one from an agent this build
    // cannot write — can still be briefed, because reading a session and
    // continuing it are different capabilities.
    if as_brief {
        return print_brief(&repo, &source, dry_run);
    }

    // Rendering is the adapter's job whether or not it can do it, so the refusal
    // for codex/cursor arrives here by name rather than as a silent no-op.
    let rendered = render_for(repo.workdir(), &source)?;

    // An empty transcript writes a file the harness will refuse to open, and the
    // refusal arrives later as "session not found" — pointing at the harness for
    // something lineage knew here. Sessions can render to nothing legitimately:
    // one whose turns are all system notes, or one imported with its content
    // redacted away.
    if rendered.contents.trim().is_empty() {
        return Err(format!(
            "session {} has no turns that can be replayed, so there is nothing to continue. \
             `git lineage show {}` shows what was stored",
            source.id, source.id
        )
        .into());
    }

    print_provenance(&source);

    if dry_run {
        println!(
            "Would write {} ({}).",
            rendered.path.display(),
            human_size(rendered.contents.len())
        );
        println!("Nothing was written and no fork was recorded (--dry-run).");
        println!();
        print_next_step(&rendered);
        return Ok(());
    }

    write_transcript(&rendered)?;

    // The edge is persisted only after the transcript exists: a recorded fork
    // whose session cannot be opened is a lie about the graph.
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
    print_next_step(&rendered);
    Ok(())
}

/// Prints the block and nothing else. Nothing is written and no fork edge is
/// recorded — this is an initial context load for a subagent the *calling agent*
/// spawns, not a fork, so there is no new session for the graph to record.
fn print_brief(
    repo: &lineage_git::LineageRepo,
    source: &Conversation,
    dry_run: bool,
) -> Result<()> {
    // `--dry-run` answers "what would be written"; `--brief` writes nothing by
    // design, so the pair has no coherent meaning and combining them is a
    // mistake worth naming rather than silently resolving in either direction.
    if dry_run {
        return Err(
            "--brief and --dry-run cannot be combined: --brief already writes nothing, \
             so there is no write for --dry-run to preview. Drop --dry-run"
                .into(),
        );
    }

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

/// Whose session, when, and what it was about — before the command, because a
/// user deciding whether to continue someone's work needs to recognise the work
/// first. A bare id tells them nothing they can act on.
fn print_provenance(source: &Conversation) {
    println!("{}", describe_author(source));
    println!("  {}", describe_shape(source));

    if let Some(topic) = opening_ask(source) {
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

/// The first thing the human asked for. This is the session's own data, not a
/// summary: the surface renders what it is given (ARCHITECTURE.md invariant 3).
fn opening_ask(source: &Conversation) -> Option<String> {
    let first = source
        .turns
        .iter()
        .find(|turn| turn.role == Role::User && !turn.content.trim().is_empty())?;
    Some(truncate(first.content.trim(), TOPIC_MAX_CHARS))
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

/// The command is adapter-supplied verbatim: the verb, the flags, and the
/// directory it must run from are all vendor knowledge (invariant 4).
fn print_next_step(rendered: &RenderedTranscript) {
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

/// A raw byte count is a number the reader has to convert before it means
/// anything. The only question it answers here is "is this transcript small or
/// enormous", which one significant figure settles.
fn human_size(bytes: usize) -> String {
    const KIB: usize = 1024;
    const MIB: usize = KIB * 1024;
    if bytes >= MIB {
        return format!("{:.1} MB", bytes as f64 / MIB as f64);
    }
    if bytes >= KIB {
        return format!("{} KB", bytes / KIB);
    }
    format!("{bytes} bytes")
}

/// Cuts on a char boundary and marks the cut, so a truncated topic is visibly
/// truncated rather than silently misleading.
fn truncate(text: &str, max_chars: usize) -> String {
    let flattened = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.chars().count() <= max_chars {
        return flattened;
    }
    let mut out: String = flattened.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{AgentKind, Turn};

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
    fn opening_ask_is_the_first_non_empty_user_turn_flattened() {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/proj");
        conv.turns.push(user_turn("   "));
        conv.turns.push(user_turn("fix the auth\n  bug"));
        assert_eq!(opening_ask(&conv).as_deref(), Some("fix the auth bug"));
    }

    #[test]
    fn opening_ask_is_absent_when_nobody_asked_anything() {
        let conv = Conversation::new(AgentKind::Claude, "/tmp/proj");
        assert!(opening_ask(&conv).is_none());
    }

    #[test]
    fn a_long_ask_is_visibly_cut() {
        let long = "word ".repeat(200);
        let cut = truncate(&long, TOPIC_MAX_CHARS);
        assert!(cut.ends_with('…'));
        assert_eq!(cut.chars().count(), TOPIC_MAX_CHARS + 1);
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
    fn a_transcript_size_reads_as_a_magnitude_not_a_byte_count() {
        assert_eq!(human_size(512), "512 bytes");
        assert_eq!(human_size(2048), "2 KB");
        assert_eq!(human_size(1_541_947), "1.5 MB");
    }

    #[test]
    fn turn_count_reads_as_prose_for_a_single_turn() {
        let mut conv = Conversation::new(AgentKind::Claude, "/tmp/proj");
        conv.turns.push(user_turn("hello"));
        assert_eq!(describe_shape(&conv), "1 turn");
    }
}
