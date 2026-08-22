//! `git lineage fork` end to end, on the reopen half: a session this machine
//! holds is resolved from lineage refs, the adapter is asked for the invocation,
//! and it is printed — writing nothing.
//!
//! The fall-through half (no invocation available, so the session is written out
//! instead) is covered in `fork_workflow.rs`.
//!
//! Run through the built binary rather than the library, matching
//! `fork_workflow.rs`: the adapters read `$HOME`, and a child process is the only
//! way to redirect that without mutating the test process's environment, which
//! would be unsafe and racy under the parallel runner. It also means these tests
//! assert what a user actually sees. Nothing here launches an agent.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use lineage_cli::commands;
use lineage_core::{AgentKind, Conversation, LineageId, Role, Turn};
use lineage_git::{list_session_ids, open_repo, persist_conversation};

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

/// A session as the importer would have stored it: the vendor id under the key
/// its own adapter writes, which is what makes it reopenable on this machine.
fn seed_session(
    dir: &Path,
    agent: AgentKind,
    vendor_id_key: &str,
    vendor_id: &str,
) -> Conversation {
    let repo = open_repo(dir).unwrap();
    let mut conv = Conversation::new(agent, dir.display().to_string());
    conv.metadata.insert(
        vendor_id_key.into(),
        serde_json::Value::String(vendor_id.into()),
    );
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "the login endpoint accepts an empty password, fix it".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    persist_conversation(repo.inner(), &conv).unwrap();
    conv
}

fn run_resume(dir: &Path, home: &Path, session_id: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_git-lineage"))
        .args(["--repo", dir.to_str().unwrap(), "continue", session_id])
        .env("HOME", home)
        .output()
        .unwrap()
}

fn stdout_of(output: &Output) -> String {
    assert!(
        output.status.success(),
        "continue failed: {}",
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

/// The command is the adapter's verbatim, and it must name the id the harness
/// resolves by — the session's own vendor id, never lineage's id, which the
/// harness has never seen.
#[test]
fn resume_prints_the_adapter_command_for_the_stored_vendor_id() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let conv = seed_session(
        dir.path(),
        AgentKind::Claude,
        "claude_session_id",
        "019f9d91-3f94-48f4-8cbf-663330ac0cee",
    );

    let output = run_resume(dir.path(), home.path(), conv.id.as_str());
    let stdout = stdout_of(&output);

    assert!(
        stdout.contains("claude --resume 019f9d91-3f94-48f4-8cbf-663330ac0cee"),
        "{stdout}"
    );
    // The harness has never seen lineage's id, so passing it would resolve to
    // nothing with no error.
    assert!(
        !stdout.contains(&format!("--resume {}", conv.id.as_str())),
        "the command must name the vendor id, not lineage's: {stdout}"
    );
    // Claude derives its project key from the launch directory, so the output
    // has to say where to run it or the command silently finds nothing.
    assert!(
        stdout.contains(dir.path().to_str().unwrap()),
        "the output must say where to run it: {stdout}"
    );
}

/// The property that makes reopening worth preferring: it continues the session
/// that is already here. A transcript on disk or a new session in the refs would
/// mean it had written one out instead.
#[test]
fn resume_writes_no_transcript_and_records_no_new_session() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let conv = seed_session(
        dir.path(),
        AgentKind::Claude,
        "claude_session_id",
        "019f9d91-3f94-48f4-8cbf-663330ac0cee",
    );
    let before = list_session_ids(open_repo(dir.path()).unwrap().inner())
        .unwrap()
        .len();

    stdout_of(&run_resume(dir.path(), home.path(), conv.id.as_str()));

    assert!(
        claude_transcripts(home.path()).is_empty(),
        "resume must not materialize a transcript"
    );
    assert_eq!(
        list_session_ids(open_repo(dir.path()).unwrap().inner())
            .unwrap()
            .len(),
        before,
        "resume must not record a new session"
    );
}

/// Codex can be reopened even though no transcript can be written for it, so the
/// reopen path must not be gated on the transcript-writing capability.
#[test]
fn a_codex_session_resumes_even_though_it_cannot_be_forked() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let conv = seed_session(
        dir.path(),
        AgentKind::Codex,
        "codex_session_id",
        "0199aa11-2b3c-4d5e-8f90-112233445566",
    );

    let stdout = stdout_of(&run_resume(dir.path(), home.path(), conv.id.as_str()));
    assert!(
        stdout.contains("codex resume 0199aa11-2b3c-4d5e-8f90-112233445566"),
        "{stdout}"
    );
    // Codex keys rollouts by id under one state directory, so naming a
    // directory to run from would be a false instruction.
    assert!(
        !stdout.contains("run this from"),
        "codex resolves from anywhere: {stdout}"
    );
}

/// An agent that cannot reopen a session is not a dead end under `continue`: the
/// session is written out instead, which is the whole reason the two verbs merged.
#[test]
fn an_agent_that_cannot_be_reopened_falls_through_to_writing_one_out() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let conv = seed_session(
        dir.path(),
        AgentKind::Cursor,
        "cursor_session_id",
        "cursor-session-1",
    );

    let output = run_resume(dir.path(), home.path(), conv.id.as_str());
    let stderr = String::from_utf8(output.stderr.clone()).unwrap();
    // Cursor cannot reopen and cannot be written out either, so this still
    // fails — but by naming the write path it fell through to, not the reopen.
    assert!(!output.status.success());
    assert!(stderr.contains("cursor"), "{stderr}");
}

/// A teammate's session has no vendor id here, so there is nothing to reopen.
/// Under `continue` that is not an error the user has to act on: it is exactly
/// the case that gets written out as a new session carrying their context.
#[test]
fn a_session_with_no_vendor_id_is_written_out_instead() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();

    let repo = open_repo(dir.path()).unwrap();
    let mut conv = Conversation::new(AgentKind::Claude, dir.path().display().to_string());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "Alice's work, pulled from her machine".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    persist_conversation(repo.inner(), &conv).unwrap();

    let stdout = stdout_of(&run_resume(dir.path(), home.path(), conv.id.as_str()));

    // Written out, not reopened: a new session of the user's own, and a
    // transcript their harness can open.
    assert!(stdout.contains("Recorded fork"), "{stdout}");
    assert!(
        !claude_transcripts(home.path()).is_empty(),
        "the session should have been written out: {stdout}"
    );
}

#[test]
fn an_unknown_session_id_says_what_to_do_next() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();

    let output = run_resume(dir.path(), home.path(), "01NOTASESSION");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("01NOTASESSION"), "{stderr}");
    assert!(stderr.contains("git lineage list"), "{stderr}");
}
