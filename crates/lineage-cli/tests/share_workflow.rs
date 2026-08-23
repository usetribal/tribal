//! `tribal share` end to end against a stub server.
//!
//! The transport is a trait, so these tests exercise the real flow — resolve
//! which session, re-import it, apply the sync policy, assemble the batch, mint
//! the share — against a tempfile repo with no network. What is stubbed is only
//! the push and the create; everything the share decides is production code.

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lineage_cli::commands;
use lineage_cli::share_cmd::{
    self, ShareCreateRequest, ShareCreateResponse, ShareRequest, ShareTransport,
};
use lineage_core::{LineageRepoConfig, SyncBatch};
use lineage_git::{open_repo, write_repo_config, LineageRepo};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const REPO_URL: &str = "github.com/acme/widgets";

/// Records the batch pushed and the share requested, so a test can assert on
/// what the client decided to send rather than only on what it printed.
#[derive(Default)]
struct StubServer {
    batches: RefCell<Vec<SyncBatch>>,
    creates: RefCell<Vec<ShareCreateRequest>>,
    turn_count: usize,
}

impl StubServer {
    fn pinning(turn_count: usize) -> Self {
        Self {
            turn_count,
            ..Self::default()
        }
    }

    fn only_batch(&self) -> SyncBatch {
        let batches = self.batches.borrow();
        assert_eq!(batches.len(), 1, "share pushes exactly one batch");
        batches[0].clone()
    }

    fn only_create(&self) -> ShareCreateRequest {
        let creates = self.creates.borrow();
        assert_eq!(creates.len(), 1, "share creates exactly one share");
        creates[0].clone()
    }
}

impl ShareTransport for StubServer {
    fn push(&self, _repo: &LineageRepo, batch: &SyncBatch) -> Result<()> {
        self.batches.borrow_mut().push(batch.clone());
        Ok(())
    }

    fn create(&self, request: &ShareCreateRequest) -> Result<ShareCreateResponse> {
        self.creates.borrow_mut().push(request.clone());
        Ok(ShareCreateResponse {
            token: "sHaReToKeN0000000000t1".into(),
            url: "https://app.usetribal.io/s/sHaReToKeN0000000000t1".into(),
            turn_count: self.turn_count,
        })
    }
}

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.email", "alice@example.dev"],
        vec!["config", "user.name", "Alice"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(dir.path())
            .output()
            .unwrap();
    }
    fs::write(dir.path().join("src.txt"), "hello\n").unwrap();
    for args in [vec!["add", "."], vec!["commit", "-m", "init"]] {
        Command::new("git")
            .args(&args)
            .current_dir(dir.path())
            .output()
            .unwrap();
    }
    commands::init_config(dir.path()).unwrap();
    dir
}

/// The Claude adapter discovers `<workspace>/.claude/projects/*/*.jsonl`, so a
/// transcript dropped there is found exactly as a real session would be.
fn install_transcript(dir: &Path, name: &str, prompt: &str) -> PathBuf {
    let project = dir.join(".claude").join("projects").join("Fixture");
    fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{name}.jsonl"));
    let lines = [
        format!(
            r#"{{"parentUuid":null,"isSidechain":false,"cwd":".","sessionId":"{name}","type":"user","message":{{"role":"user","content":[{{"type":"text","text":"{prompt}"}}]}},"uuid":"{name}-1","timestamp":"2026-06-06T10:01:00Z"}}"#
        ),
        // An edit tool call, so a plain `import` treats the fixture as a code
        // session (`import_only_code_sessions` defaults on) and the `--session`
        // override has something stored to resolve against.
        format!(
            r#"{{"parentUuid":"{name}-1","isSidechain":false,"cwd":".","sessionId":"{name}","type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"Done."}},{{"type":"tool_use","id":"{name}-tu","name":"Write","input":{{"file_path":"src.txt","content":"hi\n"}}}}],"model":"claude-sonnet-4-20250514"}},"uuid":"{name}-2","timestamp":"2026-06-06T10:01:10Z"}}"#
        ),
    ];
    fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();
    path
}

fn touch(path: &Path) {
    // Rewriting the file moves its mtime, which is what the resolver ranks on.
    let contents = fs::read_to_string(path).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(path, contents).unwrap();
}

fn request() -> ShareRequest {
    ShareRequest {
        remote: "origin".into(),
        no_open: true,
        ..ShareRequest::default()
    }
}

#[test]
fn the_most_recently_active_session_is_the_one_shared() {
    let dir = init_repo();
    install_transcript(
        dir.path(),
        "aaaaaaaa-0000-0000-0000-000000000001",
        "older work",
    );
    let newer = install_transcript(
        dir.path(),
        "bbbbbbbb-0000-0000-0000-000000000002",
        "what I am doing now",
    );
    touch(&newer);
    let server = StubServer::pinning(2);

    share_cmd::run_share(dir.path(), &server, &request()).unwrap();

    let batch = server.only_batch();
    assert_eq!(
        batch.conversations.len(),
        1,
        "one conversation, not the repo"
    );
    assert!(
        batch.conversations[0].turns[0]
            .content
            .contains("what I am doing now"),
        "shared the wrong session: {:?}",
        batch.conversations[0].turns[0].content
    );
}

#[test]
fn naming_a_session_overrides_the_guess() {
    let dir = init_repo();
    install_transcript(
        dir.path(),
        "aaaaaaaa-0000-0000-0000-000000000001",
        "the one I want",
    );
    let newer = install_transcript(
        dir.path(),
        "bbbbbbbb-0000-0000-0000-000000000002",
        "a session I am not sharing",
    );
    touch(&newer);
    // The harness UUID is what a user copies out of their terminal, so the
    // override must accept it and not only the lineage id.
    let named = ShareRequest {
        session_id: Some("aaaaaaaa-0000-0000-0000-000000000001".into()),
        ..request()
    };
    commands::import(dir.path(), &["claude".into()], None, true, false).unwrap();
    let server = StubServer::pinning(2);

    share_cmd::run_share(dir.path(), &server, &named).unwrap();

    let batch = server.only_batch();
    assert!(
        batch.conversations[0].turns[0]
            .content
            .contains("the one I want"),
        "the --session override did not win: {:?}",
        batch.conversations[0].turns[0].content
    );
}

#[test]
fn a_private_session_is_refused_and_nothing_is_pushed() {
    let dir = init_repo();
    install_transcript(
        dir.path(),
        "cccccccc-0000-0000-0000-000000000003",
        "secrets in here",
    );
    // Marking the transcript private through repo config is how a real session
    // becomes private, so the refusal is exercised the way a user would hit it.
    let mut config = LineageRepoConfig::default();
    config.private_session_patterns.push("*.jsonl".into());
    let repo = open_repo(dir.path()).unwrap();
    write_repo_config(repo.inner(), &config).unwrap();
    let server = StubServer::pinning(2);

    let error = share_cmd::run_share(dir.path(), &server, &request())
        .expect_err("a private session must not be shareable");

    assert!(error.to_string().contains("private"), "got: {error}");
    assert!(
        server.batches.borrow().is_empty(),
        "a refused share must not reach the wire"
    );
    assert!(server.creates.borrow().is_empty());
}

#[test]
fn the_create_names_the_repo_and_the_conversation_that_was_pushed() {
    let dir = init_repo();
    install_transcript(
        dir.path(),
        "dddddddd-0000-0000-0000-000000000004",
        "share me",
    );
    let server = StubServer::pinning(2);

    share_cmd::run_share(dir.path(), &server, &request()).unwrap();

    let batch = server.only_batch();
    let create = server.only_create();
    assert_eq!(create.repo, REPO_URL);
    assert_eq!(batch.repo.normalized_remote_url, REPO_URL);
    assert_eq!(
        create.conversation_id,
        batch.conversations[0].id.to_string()
    );
}

#[test]
fn the_pinned_turn_count_comes_back_from_the_server_not_the_client() {
    let dir = init_repo();
    install_transcript(dir.path(), "eeeeeeee-0000-0000-0000-000000000005", "pin me");
    // The server's count deliberately differs from the local one: the client
    // reports what it was told, never what it counted.
    let server = StubServer::pinning(7);

    let response = share_cmd::run_share(dir.path(), &server, &request()).unwrap();

    assert_eq!(response.turn_count, 7);
    assert_eq!(
        response.url,
        "https://app.usetribal.io/s/sHaReToKeN0000000000t1"
    );
}

#[test]
fn a_directory_with_no_agent_session_says_so_rather_than_sharing_nothing() {
    let dir = init_repo();
    let server = StubServer::pinning(0);

    let error = share_cmd::run_share(dir.path(), &server, &request())
        .expect_err("nothing to share should fail");

    assert!(error.to_string().contains("--session"), "got: {error}");
    assert!(server.batches.borrow().is_empty());
}

#[test]
fn the_url_survives_a_browser_that_will_not_open() {
    let dir = init_repo();
    install_transcript(
        dir.path(),
        "ffffffff-0000-0000-0000-000000000006",
        "open me",
    );
    let server = StubServer::pinning(2);

    // The share is minted and returned before the browser is touched at all, so
    // a machine with no opener (CI, a headless box) still ends the command
    // holding the link. `run_share` opening nothing is what makes that true.
    let response = share_cmd::run_share(dir.path(), &server, &request()).unwrap();

    assert!(response.url.ends_with("sHaReToKeN0000000000t1"));
    assert_eq!(server.only_create().conversation_id.len(), 26);
}
