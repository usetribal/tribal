//! `git lineage pull` end to end against a stub server.
//!
//! The transport is a trait, so these tests exercise the real flow — digest
//! from local refs, negotiate, fetch, merge, write into
//! `refs/lineage/sessions/` — against a tempfile repo with no network. What is
//! stubbed is only the two HTTP calls; everything the pull actually decides
//! (what to ask for, what to merge, what to store) is the production code.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use lineage_cli::pull_cmd::{
    self, FetchRequest, FetchResponse, HaveEntry, NegotiateRequest, NegotiateResponse,
    PullTransport, PulledConversation, PulledTurn, WantEntry,
};
use lineage_core::{AgentKind, Conversation, LineageId, PullOrigin, Role, Turn};
use lineage_git::{open_repo, persist_conversation, read_conversation_stored, LineageRepo};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const REPO_URL: &str = "github.com/acme/widgets";
const SERVER: &str = "https://api.example.dev";

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init"],
        vec!["config", "user.email", "bob@example.dev"],
        vec!["config", "user.name", "Bob"],
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

/// Records what the client asked for and replays a canned answer, so a test can
/// assert on the digest the client computed as well as on what it stored.
struct StubServer {
    conversations: Vec<PulledConversation>,
    seen_have: RefCell<Vec<Vec<HaveEntry>>>,
    fetched_ids: RefCell<Vec<Vec<String>>>,
}

impl StubServer {
    fn holding(conversations: Vec<PulledConversation>) -> Self {
        Self {
            conversations,
            seen_have: RefCell::new(Vec::new()),
            fetched_ids: RefCell::new(Vec::new()),
        }
    }

    fn last_have(&self) -> Vec<HaveEntry> {
        self.seen_have.borrow().last().cloned().unwrap_or_default()
    }
}

impl PullTransport for StubServer {
    /// The real negotiation rule, so a stale digest genuinely produces a want:
    /// unknown id is `new`, fewer turns locally is `grown`, otherwise nothing.
    fn negotiate(&self, request: &NegotiateRequest) -> Result<NegotiateResponse> {
        self.seen_have.borrow_mut().push(request.have.clone());
        let have: BTreeMap<&str, &HaveEntry> = request
            .have
            .iter()
            .map(|entry| (entry.conversation_id.as_str(), entry))
            .collect();

        let want = self
            .conversations
            .iter()
            .filter_map(|conv| match have.get(conv.id.as_str()) {
                None => Some(WantEntry {
                    conversation_id: conv.id.clone(),
                    reason: "new".into(),
                }),
                Some(entry) if entry.turn_count < conv.turns.len() => Some(WantEntry {
                    conversation_id: conv.id.clone(),
                    reason: "grown".into(),
                }),
                Some(_) => None,
            })
            .collect();
        Ok(NegotiateResponse { want })
    }

    fn fetch(&self, request: &FetchRequest) -> Result<FetchResponse> {
        self.fetched_ids
            .borrow_mut()
            .push(request.conversation_ids.clone());
        Ok(FetchResponse {
            conversations: self
                .conversations
                .iter()
                .filter(|conv| request.conversation_ids.contains(&conv.id))
                .cloned()
                .collect(),
        })
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

fn alice_session(id: &str, turns: usize) -> PulledConversation {
    alice_session_touching(id, turns, vec![])
}

fn alice_session_touching(id: &str, turns: usize, commit_shas: Vec<String>) -> PulledConversation {
    PulledConversation {
        id: id.into(),
        agent: "claude".into(),
        started_at: "2026-07-20T09:00:00Z".parse().unwrap(),
        ended_at: Some("2026-07-20T10:00:00Z".parse().unwrap()),
        model: Some("claude-sonnet".into()),
        parent_session_id: None,
        prompted_by_name: Some("Alice".into()),
        commit_shas,
        metadata: BTreeMap::new(),
        turns: (0..turns)
            .map(|index| PulledTurn {
                id: format!("{id}-{index}"),
                role: if index % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("turn {index}"),
                content_truncated: false,
                model: Some("claude-sonnet".into()),
                timestamp: None,
            })
            .collect(),
    }
}

fn origin() -> PullOrigin {
    PullOrigin {
        server: SERVER.into(),
        tenant: None,
        pulled_at: "2026-07-26T12:00:00Z".parse().unwrap(),
        lineage_version: "0.0.0".into(),
    }
}

fn run(repo: &LineageRepo, server: &StubServer, dry_run: bool) -> pull_cmd::PullReport {
    pull_cmd::run_pull(repo, server, REPO_URL, &origin(), dry_run).unwrap()
}

fn seed_local(dir: &Path, id: &str, turns: usize) -> Conversation {
    let repo = open_repo(dir).unwrap();
    let mut conv = Conversation::new(AgentKind::Claude, dir.display().to_string());
    conv.id = LineageId::from(id.to_string());
    for index in 0..turns {
        conv.turns.push(Turn {
            id: LineageId::from(format!("{id}-{index}")),
            role: Role::User,
            content: format!("turn {index}"),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![],
        });
    }
    persist_conversation(repo.inner(), &conv).unwrap();
    conv
}

fn stored(dir: &Path, id: &str) -> Option<Conversation> {
    let repo = open_repo(dir).unwrap();
    read_conversation_stored(repo.inner(), &LineageId::from(id.to_string())).unwrap()
}

#[test]
fn the_digest_describes_every_session_this_repo_already_holds() {
    let dir = init_repo();
    seed_local(dir.path(), "s-local", 3);
    let repo = open_repo(dir.path()).unwrap();
    let server = StubServer::holding(vec![]);

    run(&repo, &server, false);

    let have = server.last_have();
    assert_eq!(have.len(), 1);
    assert_eq!(have[0].conversation_id, "s-local");
    assert_eq!(have[0].turn_count, 3);
}

#[test]
fn a_session_bob_never_imported_lands_in_the_refs_with_its_turns() {
    let dir = init_repo();
    let sha = head_sha(dir.path());
    let repo = open_repo(dir.path()).unwrap();
    let server = StubServer::holding(vec![alice_session_touching(
        "s-alice",
        4,
        vec![sha.clone()],
    )]);

    let report = run(&repo, &server, false);
    assert_eq!(report.written, vec!["s-alice".to_string()]);

    let pulled = stored(dir.path(), "s-alice").expect("session written to refs");
    assert_eq!(pulled.turns.len(), 4);
    assert_eq!(pulled.commit_shas, vec![sha]);
    assert_eq!(
        pulled.metadata[lineage_git::PROMPTED_BY_NAME],
        serde_json::Value::String("Alice".into())
    );
}

#[test]
fn a_pulled_session_is_stamped_with_where_it_came_from() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let server = StubServer::holding(vec![alice_session("s-alice", 2)]);

    run(&repo, &server, false);

    let marker = stored(dir.path(), "s-alice")
        .unwrap()
        .pull_origin
        .expect("pull_origin stamped");
    assert_eq!(marker.server, SERVER);
    assert_eq!(marker.pulled_at, origin().pulled_at);
}

#[test]
fn a_second_identical_pull_writes_nothing() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let server = StubServer::holding(vec![alice_session("s-alice", 3)]);

    run(&repo, &server, false);
    let second = run(&repo, &server, false);

    // Nothing is wanted the second time: the digest now matches the server's copy.
    assert_eq!(second.wanted, 0);
    assert!(second.written.is_empty());
    assert_eq!(stored(dir.path(), "s-alice").unwrap().turns.len(), 3);
}

#[test]
fn the_marker_is_not_restamped_when_the_session_is_pulled_again() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let server = StubServer::holding(vec![alice_session("s-alice", 2)]);
    run(&repo, &server, false);
    let first_pulled_at = stored(dir.path(), "s-alice")
        .unwrap()
        .pull_origin
        .unwrap()
        .pulled_at;

    // The session grows on the server, so the next pull genuinely rewrites it.
    let grown = StubServer::holding(vec![alice_session("s-alice", 5)]);
    let mut later = origin();
    later.pulled_at = "2026-07-27T12:00:00Z".parse().unwrap();
    pull_cmd::run_pull(&repo, &grown, REPO_URL, &later, false).unwrap();

    let marker = stored(dir.path(), "s-alice").unwrap().pull_origin.unwrap();
    assert_eq!(marker.pulled_at, first_pulled_at);
}

#[test]
fn merging_grows_the_turn_set_and_never_drops_local_turns() {
    let dir = init_repo();
    let sha = head_sha(dir.path());
    // Bob holds a longer copy than the server does — his own import ran later.
    seed_local(dir.path(), "s-alice", 6);

    // Force the fetch regardless of the digest, so the merge is what is tested.
    let server = StubServer::holding(vec![alice_session_touching(
        "s-alice",
        3,
        vec![sha.clone()],
    )]);
    let forced = FetchRequest {
        repo: REPO_URL.into(),
        conversation_ids: vec!["s-alice".into()],
    };
    let fetched = server.fetch(&forced).unwrap();
    let merged = pull_cmd::merge_pulled(
        stored(dir.path(), "s-alice"),
        &fetched.conversations[0],
        &origin(),
    );

    assert_eq!(merged.turns.len(), 6);
    assert_eq!(
        merged.ended_at,
        Some("2026-07-20T10:00:00Z".parse().unwrap())
    );
    assert_eq!(merged.commit_shas, vec![sha]);
}

#[test]
fn a_commit_this_checkout_has_not_fetched_is_dropped_rather_than_failing_the_pull() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let unfetched = "a".repeat(40);
    let server = StubServer::holding(vec![alice_session_touching(
        "s-alice",
        2,
        vec![unfetched, head_sha(dir.path())],
    )]);

    run(&repo, &server, false);

    let pulled = stored(dir.path(), "s-alice").unwrap();
    assert_eq!(pulled.commit_shas, vec![head_sha(dir.path())]);
    assert_eq!(pulled.turns.len(), 2);
}

#[test]
fn a_session_the_server_never_mentions_is_left_untouched() {
    let dir = init_repo();
    seed_local(dir.path(), "s-bob-only", 2);
    let repo = open_repo(dir.path()).unwrap();
    let server = StubServer::holding(vec![alice_session("s-alice", 1)]);

    run(&repo, &server, false);

    let untouched = stored(dir.path(), "s-bob-only").expect("local session survives the pull");
    assert_eq!(untouched.turns.len(), 2);
    assert!(untouched.pull_origin.is_none());
}

#[test]
fn dry_run_reports_what_would_arrive_and_writes_nothing() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let server = StubServer::holding(vec![alice_session("s-alice", 2)]);

    let report = run(&repo, &server, true);

    assert_eq!(report.wanted, 1);
    assert_eq!(report.written, vec!["s-alice".to_string()]);
    assert!(stored(dir.path(), "s-alice").is_none());
}

#[test]
fn a_stale_local_copy_is_asked_for_again_and_grows() {
    let dir = init_repo();
    seed_local(dir.path(), "s-alice", 2);
    let repo = open_repo(dir.path()).unwrap();
    let server = StubServer::holding(vec![alice_session("s-alice", 5)]);

    let report = run(&repo, &server, false);

    assert_eq!(report.reasons.get("grown"), Some(&1));
    assert_eq!(stored(dir.path(), "s-alice").unwrap().turns.len(), 5);
}
