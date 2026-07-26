//! `git lineage fork` end to end: resolve from lineage refs, materialize a
//! vendor transcript, record the edge, print what to run.
//!
//! Run through the built binary rather than the library. The adapter reads
//! `$HOME` to locate the harness state directory, and a child process is the
//! only way to redirect that without mutating the test process's environment —
//! which would be both unsafe and racy under the parallel test runner. It also
//! means these tests assert what a user actually sees. Nothing here launches an
//! agent or touches the network.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use lineage_cli::commands;
use lineage_core::{
    AgentKind, Artifact, ArtifactKind, ArtifactResolve, Conversation, LineageId, ResolveStrategy,
    Role, Turn,
};
use lineage_git::{
    list_session_ids, open_repo, persist_conversation, read_conversation, read_line_object,
    read_note_for_commit, PROMPTED_BY_NAME,
};

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

/// A stored session as if Alice had imported it: named author, a real ask, an
/// edit artifact, and a linked commit — the data the fork output reads back.
fn seed_alice_session(dir: &Path, agent: AgentKind) -> Conversation {
    let repo = open_repo(dir).unwrap();
    let sha = head_sha(dir);

    let mut conv = Conversation::new(agent, dir.display().to_string());
    conv.commit_shas.push(sha);
    conv.metadata.insert(
        PROMPTED_BY_NAME.into(),
        serde_json::Value::String("Alice".into()),
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
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "Tightened the check in validate".into(),
        tool_calls: vec![],
        model: Some("claude-sonnet".into()),
        timestamp: None,
        artifacts: vec![edit_of("auth.rs")],
    });
    persist_conversation(repo.inner(), &conv).unwrap();
    conv
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

fn head_sha(dir: &Path) -> String {
    open_repo(dir)
        .unwrap()
        .inner()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string()
}

fn run_fork(dir: &Path, home: &Path, session_id: &str, extra: &[&str]) -> Output {
    let mut args = vec!["--repo", dir.to_str().unwrap(), "fork", session_id];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_git-lineage"))
        .args(&args)
        .env("HOME", home)
        .output()
        .unwrap()
}

fn stdout_of(output: &Output) -> String {
    assert!(
        output.status.success(),
        "fork failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// The forked session, found by the edge rather than by remembering its id.
fn fork_of(dir: &Path, source: &Conversation) -> Conversation {
    let repo = open_repo(dir).unwrap();
    list_session_ids(repo.inner())
        .unwrap()
        .into_iter()
        .filter_map(|id| read_conversation(repo.inner(), &id).unwrap())
        .find(|conv| {
            conv.fork_origin
                .as_ref()
                .is_some_and(|origin| origin.source_session_id == source.id)
        })
        .expect("a session recording the fork edge")
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

#[test]
fn fork_writes_a_transcript_and_records_the_edge() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let source = seed_alice_session(dir.path(), AgentKind::Claude);

    let output = run_fork(dir.path(), home.path(), source.id.as_str(), &[]);
    stdout_of(&output);

    let written = claude_transcripts(home.path());
    assert_eq!(written.len(), 1, "exactly one transcript materialized");

    let fork = fork_of(dir.path(), &source);
    let origin = fork.fork_origin.as_ref().unwrap();
    assert!(fork.is_fork());
    assert_eq!(origin.source_session_id, source.id);
    // The transcript filename is the vendor handle Claude resolves the session
    // by, so the recorded handle must be the one that was actually written.
    assert_eq!(
        written[0].file_name().unwrap().to_str().unwrap(),
        format!("{}.jsonl", origin.forked_session_handle)
    );
    // Alice's id is never reused for Bob's copy.
    assert_ne!(origin.forked_session_handle, source.id.as_str());

    // Alice's turns stay on Alice's ref. The fork is a new identity carrying her
    // context in the transcript, not her words in the graph.
    assert!(fork.turns.is_empty());
    let stored_source = read_conversation(open_repo(dir.path()).unwrap().inner(), &source.id)
        .unwrap()
        .unwrap();
    assert_eq!(stored_source.turns.len(), source.turns.len());
    assert!(stored_source.fork_origin.is_none());
}

/// The point of the command is that a human reads the output and knows whose
/// work this is and whether to continue it, before running anything. A bare id
/// fails that even with a working mechanism.
#[test]
fn the_output_names_the_author_the_topic_and_the_command() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let source = seed_alice_session(dir.path(), AgentKind::Claude);

    let output = run_fork(dir.path(), home.path(), source.id.as_str(), &[]);
    let stdout = stdout_of(&output);

    assert!(stdout.contains("Alice's claude session"), "{stdout}");
    assert!(stdout.contains("2 turns"), "{stdout}");
    assert!(stdout.contains("accepts an empty password"), "{stdout}");
    assert!(stdout.contains("auth.rs"), "{stdout}");

    let fork = fork_of(dir.path(), &source);
    let handle = &fork.fork_origin.as_ref().unwrap().forked_session_handle;
    // Adapter-supplied verbatim; the CLI never assembles a vendor command.
    assert!(
        stdout.contains(&format!("claude --resume {handle}")),
        "{stdout}"
    );
    // The command only resolves from the workspace the key was derived from.
    assert!(
        stdout.contains(dir.path().to_str().unwrap()),
        "the output must say where to run it: {stdout}"
    );
}

/// Post-fork lines are the forker's. Once Bob's continuation records its own
/// edits, the line objects materialized from it name *his* conversation — Alice
/// is reachable as an ancestor and is never an author of them.
#[test]
fn line_objects_from_a_forked_session_bind_to_the_forker() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let source = seed_alice_session(dir.path(), AgentKind::Claude);

    let output = run_fork(dir.path(), home.path(), source.id.as_str(), &[]);
    stdout_of(&output);

    let repo = open_repo(dir.path()).unwrap();
    let sha = head_sha(dir.path());

    // Bob continues the work: his turn, on his session.
    let mut fork = fork_of(dir.path(), &source);
    fork.commit_shas.push(sha.clone());
    fork.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "also rejected whitespace-only passwords".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![edit_of("auth.rs")],
    });
    persist_conversation(repo.inner(), &fork).unwrap();

    commands::materialize(dir.path(), None, Some(fork.id.as_str())).unwrap();

    let note = read_note_for_commit(repo.inner(), &sha).unwrap().unwrap();
    let objects: Vec<_> = note
        .line_object_ids
        .iter()
        .filter_map(|id| read_line_object(repo.inner(), id).unwrap())
        .collect();
    assert!(
        objects.iter().any(|obj| obj.conversation_id == fork.id),
        "the fork's edit materialized against its own session: {:?}",
        objects
            .iter()
            .map(|o| o.conversation_id.as_str())
            .collect::<Vec<_>>()
    );
    // Every object the fork produced binds to the fork's own turns; none is
    // attributed to Alice, whose session is reachable only as an ancestor.
    let fork_turn_ids: Vec<&str> = fork.turns.iter().map(|t| t.id.as_str()).collect();
    for object in objects.iter().filter(|o| o.conversation_id == fork.id) {
        assert!(fork_turn_ids.contains(&object.turn_id.as_str()));
        assert_ne!(object.conversation_id, source.id);
    }
}

#[test]
fn dry_run_writes_nothing_and_records_nothing() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();
    let source = seed_alice_session(dir.path(), AgentKind::Claude);
    let before = list_session_ids(open_repo(dir.path()).unwrap().inner())
        .unwrap()
        .len();

    let output = run_fork(dir.path(), home.path(), source.id.as_str(), &["--dry-run"]);
    let stdout = stdout_of(&output);

    assert!(claude_transcripts(home.path()).is_empty());
    assert_eq!(
        list_session_ids(open_repo(dir.path()).unwrap().inner())
            .unwrap()
            .len(),
        before,
        "--dry-run must not persist a fork edge"
    );
    // A dry run still has to show the path and the command, or it cannot answer
    // the question it exists for.
    assert!(stdout.contains(".claude/projects/"), "{stdout}");
    assert!(stdout.contains("claude --resume "), "{stdout}");
    assert!(stdout.contains("--dry-run"), "{stdout}");
}

#[test]
fn an_unknown_session_id_says_what_to_do_next() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();

    let output = run_fork(dir.path(), home.path(), "01NOTASESSION", &[]);
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("01NOTASESSION"), "{stderr}");
    assert!(stderr.contains("git lineage list"), "{stderr}");
}

/// Only Claude can be continued in its harness today. The refusal has to name
/// the agent, or a user cannot tell whether to try a different session or a
/// different tool.
#[test]
fn forking_a_session_from_an_unsupported_agent_refuses_by_name() {
    let dir = init_repo();
    let home = tempfile::tempdir().unwrap();
    commands::init_config(dir.path()).unwrap();

    for agent in [AgentKind::Codex, AgentKind::Cursor] {
        let source = seed_alice_session(dir.path(), agent);
        let output = run_fork(dir.path(), home.path(), source.id.as_str(), &[]);
        assert!(!output.status.success(), "{agent:?} fork should refuse");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(agent.as_str()), "{stderr}");
        assert!(stderr.contains("unsupported"), "{stderr}");
    }
}
