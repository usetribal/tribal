//! `tribal fork <id> --brief` end to end: print a self-contained context
//! block for a subagent, write nothing.
//!
//! Run through the built binary for the same reason `fork_workflow` does — the
//! adapter reads `$HOME`, and a scoped child process is the only way to prove
//! nothing was written to the harness state directory without mutating this
//! process's environment. Nothing here launches an agent or touches the network.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use lineage_cli::commands;
use lineage_core::{
    AgentKind, Artifact, ArtifactKind, ArtifactResolve, Conversation, LineageId, PullOrigin,
    ResolveStrategy, Role, Turn,
};
use lineage_git::{list_session_ids, open_repo, persist_conversation, PROMPTED_BY_NAME};

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.email", "alice@example.dev"],
        vec!["config", "user.name", "Alice"],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(dir.path())
            .output()
            .unwrap();
    }
    std::fs::write(dir.path().join("auth.rs"), "pub fn validate() {}\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

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

fn assistant_turn(content: &str, artifacts: Vec<Artifact>) -> Turn {
    Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: content.into(),
        tool_calls: vec![],
        model: Some("claude-sonnet".into()),
        timestamp: None,
        artifacts,
    }
}

fn edit_of(path: &str) -> Artifact {
    Artifact {
        kind: ArtifactKind::FileEdit,
        path: path.into(),
        blob_ref: None,
        content_hash: None,
        mime_type: None,
        preview_data_url: None,
        line_range: None,
        resolve: Some(ArtifactResolve {
            strategy: ResolveStrategy::OldString,
            old_string: None,
            new_string: Some("pub fn validate() {}".into()),
            patch: None,
        }),
    }
}

fn store(dir: &Path, conv: &Conversation) {
    let repo = open_repo(dir).unwrap();
    persist_conversation(repo.inner(), conv).unwrap();
}

/// A session with a negotiated intent thread: two asks, not one, plus an edit
/// and a closing reply.
fn seed_alice_session(dir: &Path) -> Conversation {
    let mut conv = Conversation::new(AgentKind::Claude, dir.display().to_string());
    conv.metadata.insert(
        PROMPTED_BY_NAME.into(),
        serde_json::Value::String("Alice".into()),
    );
    conv.turns.push(user_turn(
        "the login endpoint accepts an empty password, fix it",
    ));
    conv.turns.push(assistant_turn(
        "Tightened the check",
        vec![edit_of("auth.rs")],
    ));
    conv.turns
        .push(user_turn("also reject whitespace-only passwords"));
    conv.turns
        .push(assistant_turn("Trimmed before the length check", vec![]));
    store(dir, &conv);
    conv
}

fn run_brief(dir: &Path, home: &Path, session_id: &str, extra: &[&str]) -> Output {
    let mut args = vec![
        "--repo",
        dir.to_str().unwrap(),
        "fork",
        session_id,
        "--brief",
    ];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_tribal"))
        .args(&args)
        .env("HOME", home)
        .output()
        .unwrap()
}

fn stdout_of(output: &Output) -> String {
    assert!(
        output.status.success(),
        "brief failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn claude_transcripts(home: &Path) -> Vec<PathBuf> {
    let projects = home.join(".claude").join("projects");
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .flat_map(|entry| std::fs::read_dir(entry.path()).into_iter().flatten())
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect()
}

/// The intent thread is every ask, not the opening one. A brief that showed
/// only the first prompt would describe a session that never happened: the
/// second ask is what the closing turn is actually answering.
#[test]
fn the_brief_carries_every_user_prompt_the_edits_and_the_last_reply() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let source = seed_alice_session(dir.path());

    let stdout = stdout_of(&run_brief(dir.path(), home.path(), source.id.as_str(), &[]));

    assert!(stdout.contains("Alice's claude session"), "{stdout}");
    assert!(stdout.contains(source.id.as_str()), "{stdout}");
    assert!(stdout.contains("accepts an empty password"), "{stdout}");
    assert!(stdout.contains("whitespace-only passwords"), "{stdout}");
    assert!(stdout.contains("auth.rs"), "{stdout}");
    assert!(
        stdout.contains("Trimmed before the length check"),
        "{stdout}"
    );
    assert!(
        !stdout.contains("partial"),
        "a complete brief must not claim truncation: {stdout}"
    );
}

/// The `SessionStart` hook fires for the parent session and never for a spawned
/// subagent, so a subagent handed only this block would be able to read the
/// session and unable to move through it. The vocabulary has to travel inside.
#[test]
fn the_brief_embeds_the_traversal_vocabulary_and_does_not_offer_to_fork() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let source = seed_alice_session(dir.path());

    let stdout = stdout_of(&run_brief(dir.path(), home.path(), source.id.as_str(), &[]));

    for verb in lineage_retrieval::VERBS {
        assert!(
            stdout.contains(&format!("tribal context {}", verb.cli)),
            "verb {} must be reachable from inside the brief: {stdout}",
            verb.cli
        );
    }
    // A subagent cannot tell it is already inside a fork, so the block must not
    // invite it to fork again.
    assert!(
        !stdout.contains("tribal fork"),
        "the brief must not advertise fork: {stdout}"
    );
    // The vocabulary is only usable if the block also carries handles in the
    // shape it takes.
    for turn in &source.turns {
        assert!(
            stdout.contains(&format!("{}#{}", source.id.as_str(), turn.id.as_str())),
            "every selected turn must be addressable: {stdout}"
        );
    }
}

/// The seam a calling agent appends its own task text below. It has to be a
/// delimiter the agent can find without guessing, and it has to be last.
#[test]
fn the_brief_ends_with_a_marked_task_slot() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let source = seed_alice_session(dir.path());

    let stdout = stdout_of(&run_brief(dir.path(), home.path(), source.id.as_str(), &[]));

    assert!(
        stdout
            .trim_end()
            .ends_with(lineage_cli::brief::TASK_SLOT_MARKER),
        "{stdout}"
    );
}

#[test]
fn the_brief_writes_nothing_and_records_no_fork() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let source = seed_alice_session(dir.path());
    let before = list_session_ids(open_repo(dir.path()).unwrap().inner())
        .unwrap()
        .len();

    stdout_of(&run_brief(dir.path(), home.path(), source.id.as_str(), &[]));

    assert!(
        claude_transcripts(home.path()).is_empty(),
        "--brief must not materialize a transcript"
    );
    assert_eq!(
        list_session_ids(open_repo(dir.path()).unwrap().inner())
            .unwrap()
            .len(),
        before,
        "--brief must not persist a fork edge"
    );
}

/// A pulled session has no vendor transcript on this machine and, for an agent
/// this build cannot write, no renderable one either. Reading a session and
/// continuing it are different capabilities, so briefing must not inherit
/// fork's constraint.
#[test]
fn a_session_with_no_renderable_transcript_still_briefs() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();

    let mut conv = Conversation::new(AgentKind::Codex, dir.path().display().to_string());
    conv.pull_origin = Some(PullOrigin {
        server: "https://lineage.example".into(),
        tenant: None,
        pulled_at: chrono::Utc::now(),
        lineage_version: "0.1.0".into(),
    });
    conv.turns.push(user_turn("port the rate limiter to tokio"));
    conv.turns
        .push(assistant_turn("Swapped the sleep for an interval", vec![]));
    store(dir.path(), &conv);

    // fork itself refuses: codex has no transcript writer in this build.
    let forked = Command::new(env!("CARGO_BIN_EXE_tribal"))
        .args([
            "--repo",
            dir.path().to_str().unwrap(),
            "fork",
            conv.id.as_str(),
        ])
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert!(!forked.status.success());

    let stdout = stdout_of(&run_brief(dir.path(), home.path(), conv.id.as_str(), &[]));
    assert!(
        stdout.contains("port the rate limiter to tokio"),
        "{stdout}"
    );
    assert!(stdout.contains(conv.id.as_str()), "{stdout}");
}

/// The drop order made visible: with a corpus larger than the cap, the edit
/// turns go before the prompts and the output says how many were withheld.
/// Silently trimming someone else's session is what makes a second-hand account
/// untrustworthy.
#[test]
fn an_oversized_session_drops_edits_first_and_says_what_it_withheld() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();

    let mut conv = Conversation::new(AgentKind::Claude, dir.path().display().to_string());
    conv.metadata.insert(
        PROMPTED_BY_NAME.into(),
        serde_json::Value::String("Alice".into()),
    );
    // Past the 100-turn cap on selectable turns alone: 60 prompts + 60 edits.
    for i in 0..60 {
        conv.turns.push(user_turn(&format!("ask number {i}")));
        conv.turns.push(assistant_turn(
            &format!("edit number {i}"),
            vec![edit_of(&format!("file{i}.rs"))],
        ));
    }
    conv.turns
        .push(assistant_turn("that is everything I changed", vec![]));
    store(dir.path(), &conv);

    let stdout = stdout_of(&run_brief(dir.path(), home.path(), conv.id.as_str(), &[]));

    assert!(stdout.contains("partial"), "{stdout}");
    assert!(stdout.contains("of 60 edit turns shown"), "{stdout}");
    // Prompts outrank edits, so every ask survives while edits are still going.
    assert!(
        !stdout.contains("of 60 user prompts shown"),
        "prompts must not be dropped while droppable edits remain: {stdout}"
    );
    assert!(stdout.contains("ask number 0"), "{stdout}");
    assert!(stdout.contains("ask number 59"), "{stdout}");
    // The last assistant turn is never dropped.
    assert!(stdout.contains("that is everything I changed"), "{stdout}");
    // Oldest edits go first, so the newest surviving edit is the latest one.
    assert!(!stdout.contains("file0.rs"), "{stdout}");
}

#[test]
fn an_unknown_session_id_says_what_to_do_next() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();

    let output = run_brief(dir.path(), home.path(), "01NOTASESSION", &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("01NOTASESSION"), "{stderr}");
    assert!(stderr.contains("tribal list"), "{stderr}");
}
