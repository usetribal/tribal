//! `git lineage fork` — pick up someone else's agent session and continue it in
//! your own harness.

use std::fs;
use std::path::Path;

use chrono::Utc;
use lineage_adapters::all_adapters;
use lineage_agent::RenderedTranscript;
use lineage_core::{display_title, files_written, opening_ask, Conversation};
use lineage_git::{open_repo, persist_conversation, read_conversation, resolve_session};

use crate::brief;
use crate::digest::traversal_vocabulary;
use crate::events::{EventLog, Outcome};
use crate::session_pick::{self, ForkPickOptions, ForkPickResult};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const TOPIC_MAX_CHARS: usize = 160;
const FILES_SHOWN: usize = 5;

#[derive(Debug, Clone, Default)]
pub struct ForkRequest {
    pub pick: ForkPickOptions,
    pub dry_run: bool,
    pub brief: bool,
    pub json: bool,
}

pub fn fork(repo_path: &Path, request: ForkRequest) -> Result<()> {
    let picked = session_pick::pick_fork_session(repo_path, &request.pick)?;
    if request.json {
        println!("{}", serde_json::to_string_pretty(&picked)?);
        if should_stop_after_json(&request, &picked) {
            return Ok(());
        }
    }
    fork_resolved(repo_path, &picked.session_id, request.dry_run, request.brief)
}

fn should_stop_after_json(request: &ForkRequest, picked: &ForkPickResult) -> bool {
    request.pick.query.is_some()
        && request.pick.session_id.is_none()
        && picked.candidates.len() > 1
        && request.pick.pick.is_none()
}

fn fork_resolved(
    repo_path: &Path,
    session_id: &str,
    dry_run: bool,
    as_brief: bool,
) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let id = resolve_session(repo.inner(), session_id).map_err(|error| error.to_string())?;
    let source = read_conversation(repo.inner(), &id)?.ok_or_else(|| {
        format!(
            "no session {session_id} in this repository's lineage refs. \
             `git lineage list` shows what is here; if the session is a teammate's, \
             fetch their lineage refs first (`git lineage lfs fetch`, then `git fetch origin 'refs/lineage/*:refs/lineage/*'`)"
        )
    })?;

    if as_brief {
        return print_brief(&repo, &source, dry_run);
    }

    let rendered = render_for(repo.workdir(), &source)?;

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

fn print_brief(
    repo: &lineage_git::LineageRepo,
    source: &Conversation,
    dry_run: bool,
) -> Result<()> {
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
