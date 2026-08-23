//! `tribal fork <share-url>` — the receiving half of a share, end to end
//! against a stub server.
//!
//! Everything that leaves the process — the fetch, the registry lookup, the
//! clone, and starting the harness — is injected, so these drive the real flow
//! (parse the link, resolve where to land, merge into refs, hand off to the
//! fork machinery, open) against tempfile repositories with no network and
//! nothing launched. Every decision under test is production code.
//!
//! The registry tests are the exception: it is per-machine state, so they run
//! the built binary with `LINEAGE_CONFIG_DIR` pointed at a tempdir rather than
//! mutating this process's environment.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use chrono::Utc;
use lineage_cli::pull_cmd::{PulledConversation, PulledTurn};
use lineage_cli::repo_registry::{self, RepoEntry};
use lineage_cli::share_fork::{
    self, Landing, LandingContext, Opened, ShareFetchResponse, ShareFetchTransport, ShareLink,
    ShareRepo,
};
use lineage_git::{list_session_ids, open_repo, read_conversation};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const REPO_URL: &str = "github.com/acme/widgets";
const CONVERSATION_ID: &str = "01J8Z9QT7QK6X0SHAREDSESSN";

/// Serves one pinned share, or a dead link. Records the tokens asked for, so a
/// test can assert the client fetched the token out of the URL rather than the
/// whole URL.
struct StubServer {
    response: Option<ShareFetchResponse>,
    asked: RefCell<Vec<String>>,
}

impl StubServer {
    fn serving(turns: usize) -> Self {
        Self {
            response: Some(share_response(turns)),
            asked: RefCell::new(Vec::new()),
        }
    }

    fn dead_link() -> Self {
        Self {
            response: None,
            asked: RefCell::new(Vec::new()),
        }
    }
}

impl ShareFetchTransport for StubServer {
    fn fetch(&self, token: &str) -> Result<ShareFetchResponse> {
        self.asked.borrow_mut().push(token.to_string());
        self.response.clone().ok_or_else(|| {
            "this link is no longer available: it may have been revoked, or the link may be \
             mistyped. Ask whoever shared it for a new one"
                .into()
        })
    }
}

/// The share envelope as the server sends it: the pull wire conversation shape
/// plus the pin and the repository the receiver has to find or clone.
fn share_response(turns: usize) -> ShareFetchResponse {
    ShareFetchResponse {
        conversation: PulledConversation {
            id: CONVERSATION_ID.into(),
            agent: "claude".into(),
            started_at: "2026-07-01T00:00:00Z".parse().unwrap(),
            ended_at: None,
            model: Some("claude-sonnet".into()),
            parent_session_id: None,
            prompted_by_name: Some("Alice".into()),
            commit_shas: vec![],
            metadata: Default::default(),
            turns: (0..turns).map(shared_turn).collect(),
        },
        turn_count: turns,
        repo: ShareRepo {
            normalized_remote_url: REPO_URL.into(),
            name: "widgets".into(),
        },
        created_at: Utc::now(),
    }
}

fn shared_turn(index: usize) -> PulledTurn {
    let (role, content) = match index % 2 {
        0 => (
            "user",
            format!("the login endpoint accepts an empty password ({index})"),
        ),
        _ => ("assistant", format!("tightened the check ({index})")),
    };
    PulledTurn {
        id: format!("{CONVERSATION_ID}-{index}"),
        role: role.into(),
        content,
        content_truncated: false,
        model: None,
        timestamp: None,
    }
}

fn link() -> ShareLink {
    ShareLink {
        server: "https://api.usetribal.io".into(),
        token: "sHaReToKeN0000000000t1".into(),
    }
}

fn git(args: &[&str], cwd: &Path) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A checkout whose `origin` is the shared repository — the cwd and registry
/// rungs both need one, and both need it to be a real repository because the
/// resolution re-reads git rather than trusting what it was told.
fn checkout_of(url: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(&["init"], dir.path());
    git(&["config", "user.email", "bob@example.dev"], dir.path());
    git(&["config", "user.name", "Bob"], dir.path());
    git(
        &["remote", "add", "origin", &format!("https://{url}.git")],
        dir.path(),
    );
    std::fs::write(dir.path().join("auth.rs"), "pub fn validate() {}\n").unwrap();
    git(&["add", "."], dir.path());
    git(&["commit", "-m", "init"], dir.path());
    dir
}

/// A bare repository on disk. `git clone` takes a path, so the clone rung is
/// exercised for real without the network.
fn bare_fixture() -> tempfile::TempDir {
    let source = checkout_of(REPO_URL);
    let bare = tempfile::tempdir().unwrap();
    let path = bare.path().join("widgets.git");
    let output = Command::new("git")
        .args(["clone", "--bare"])
        .arg(source.path())
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    bare
}

fn context<'a>(
    cwd: &'a Path,
    home: Option<PathBuf>,
    into: Option<PathBuf>,
    lookup: &'a dyn Fn(&str) -> Option<PathBuf>,
    clone: &'a dyn Fn(&str, &Path) -> Result<()>,
) -> LandingContext<'a> {
    LandingContext {
        cwd,
        home,
        into,
        lookup,
        clone,
    }
}

fn no_lookup(_: &str) -> Option<PathBuf> {
    None
}

fn refuse_clone(_: &str, _: &Path) -> Result<()> {
    Err("a clone was not expected here".into())
}

/// A harness that starts. Nothing is actually run: these tests care about which
/// branch the open step takes, and starting `claude` is not a thing a test may
/// do.
fn harness_starts(_: &str, _: &Path) -> bool {
    true
}

fn harness_missing(_: &str, _: &Path) -> bool {
    false
}

/// The default drive: fetch, land, fork, and pretend the harness started.
fn run(server: &StubServer, context: &LandingContext<'_>) -> Result<share_fork::ShareForkOutcome> {
    share_fork::run_share_fork(server, &link(), context, false, &harness_starts)
}

/// The forked session, found by the edge rather than by remembering its id.
fn fork_of(dir: &Path, source_id: &str) -> lineage_core::Conversation {
    let repo = open_repo(dir).unwrap();
    list_session_ids(repo.inner())
        .unwrap()
        .into_iter()
        .filter_map(|id| read_conversation(repo.inner(), &id).unwrap())
        .find(|conv| {
            conv.fork_origin
                .as_ref()
                .is_some_and(|origin| origin.source_session_id.as_str() == source_id)
        })
        .expect("a session recording the fork edge")
}

// --- Landing resolution ---------------------------------------------------------

#[test]
fn a_checkout_of_the_shared_repo_in_the_cwd_is_where_it_lands() {
    let home = tempfile::tempdir().unwrap();
    let cwd = checkout_of(REPO_URL);
    let server = StubServer::serving(2);

    let outcome = run(
        &server,
        &context(
            cwd.path(),
            Some(home.path().to_path_buf()),
            None,
            &no_lookup,
            &refuse_clone,
        ),
    )
    .unwrap();

    assert_eq!(outcome.landed.how, Landing::CurrentRepo);
    assert_eq!(
        outcome.landed.path.canonicalize().unwrap(),
        cwd.path().canonicalize().unwrap()
    );
    // The token, not the URL, is what the fetch names.
    assert_eq!(*server.asked.borrow(), vec![link().token]);
}

/// A cwd that is a repository for something else must not swallow the fork —
/// the registry, then a clone, are what answer for a repository this checkout
/// is not.
#[test]
fn an_unrelated_repository_in_the_cwd_falls_through_to_the_registry() {
    let home = tempfile::tempdir().unwrap();
    let cwd = checkout_of("github.com/acme/other");
    let recorded = checkout_of(REPO_URL);
    let recorded_path = recorded.path().to_path_buf();
    let lookup = move |url: &str| (url == REPO_URL).then(|| recorded_path.clone());
    let server = StubServer::serving(2);

    let outcome = run(
        &server,
        &context(
            cwd.path(),
            Some(home.path().to_path_buf()),
            None,
            &lookup,
            &refuse_clone,
        ),
    )
    .unwrap();

    assert_eq!(outcome.landed.how, Landing::Registry);
    assert_eq!(
        outcome.landed.path.canonicalize().unwrap(),
        recorded.path().canonicalize().unwrap()
    );
}

#[test]
fn nothing_local_means_the_repository_is_cloned_beside_the_cwd() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let bare = bare_fixture();
    let source = bare.path().join("widgets.git");
    let cloned = RefCell::new(Vec::new());
    // The clone is real git against a local bare repository; only the URL is
    // swapped, because a test may not reach github.
    let clone = |_: &str, destination: &Path| -> Result<()> {
        cloned.borrow_mut().push(destination.to_path_buf());
        share_fork::git_clone(source.to_str().unwrap(), destination)
    };
    let server = StubServer::serving(2);

    let outcome = run(
        &server,
        &context(
            cwd.path(),
            Some(home.path().to_path_buf()),
            None,
            &no_lookup,
            &clone,
        ),
    )
    .unwrap();

    assert_eq!(outcome.landed.how, Landing::Cloned);
    assert_eq!(outcome.landed.path, cwd.path().join("widgets"));
    assert_eq!(*cloned.borrow(), vec![cwd.path().join("widgets")]);
    assert!(cwd.path().join("widgets").join("auth.rs").exists());
}

/// The deep link drops the receiver in `$HOME`, where `./widgets` would leave
/// checkouts loose in the home directory.
#[test]
fn a_clone_from_the_home_directory_lands_under_a_lineage_owner_tree() {
    let home = tempfile::tempdir().unwrap();
    let bare = bare_fixture();
    let source = bare.path().join("widgets.git");
    let clone =
        |_: &str, destination: &Path| share_fork::git_clone(source.to_str().unwrap(), destination);
    let server = StubServer::serving(2);

    let outcome = run(
        &server,
        &context(
            home.path(),
            Some(home.path().to_path_buf()),
            None,
            &no_lookup,
            &clone,
        ),
    )
    .unwrap();

    assert_eq!(outcome.landed.how, Landing::Cloned);
    assert_eq!(
        outcome.landed.path,
        home.path().join("lineage").join("acme").join("widgets")
    );
}

#[test]
fn a_clone_failure_names_the_repository_and_says_what_to_do() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let clone = |_: &str, _: &Path| -> Result<()> { Err("authentication failed".into()) };
    let server = StubServer::serving(2);

    let error = run(
        &server,
        &context(
            cwd.path(),
            Some(home.path().to_path_buf()),
            None,
            &no_lookup,
            &clone,
        ),
    )
    .expect_err("a failed clone must not leave the fork half-done");

    let message = error.to_string();
    assert!(message.contains("widgets"), "{message}");
    assert!(message.contains("--into"), "{message}");
}

/// `--into` is the only rung that overrides a matching cwd, and it works in a
/// directory that is not a repository at all.
#[test]
fn into_wins_over_everything_and_initializes_an_empty_directory() {
    let home = tempfile::tempdir().unwrap();
    let cwd = checkout_of(REPO_URL);
    let elsewhere = tempfile::tempdir().unwrap();
    let into = elsewhere.path().join("scratch");
    let server = StubServer::serving(2);

    let outcome = run(
        &server,
        &context(
            cwd.path(),
            Some(home.path().to_path_buf()),
            Some(into.clone()),
            &no_lookup,
            &refuse_clone,
        ),
    )
    .unwrap();

    assert_eq!(outcome.landed.how, Landing::Into);
    assert_eq!(outcome.landed.path, into);
    assert!(into.join(".git").exists(), "--into git inits when it must");
}

// --- Fetch, persist, fork -------------------------------------------------------

#[test]
fn a_dead_link_says_the_link_is_no_longer_available_and_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let cwd = checkout_of(REPO_URL);
    let server = StubServer::dead_link();

    let error = run(
        &server,
        &context(
            cwd.path(),
            Some(home.path().to_path_buf()),
            None,
            &no_lookup,
            &refuse_clone,
        ),
    )
    .expect_err("a revoked or unknown token must fail");

    assert!(
        error.to_string().contains("no longer available"),
        "got: {error}"
    );
    let repo = open_repo(cwd.path()).unwrap();
    assert!(list_session_ids(repo.inner()).unwrap().is_empty());
}

#[test]
fn the_pinned_conversation_is_persisted_and_the_fork_edge_recorded() {
    let home = tempfile::tempdir().unwrap();
    let cwd = checkout_of(REPO_URL);
    let server = StubServer::serving(4);

    let outcome = run(
        &server,
        &context(
            cwd.path(),
            Some(home.path().to_path_buf()),
            None,
            &no_lookup,
            &refuse_clone,
        ),
    )
    .unwrap();

    assert_eq!(outcome.session_id, CONVERSATION_ID);
    assert_eq!(outcome.turn_count, 4);

    let repo = open_repo(cwd.path()).unwrap();
    let stored = read_conversation(
        repo.inner(),
        &lineage_core::LineageId::from(CONVERSATION_ID),
    )
    .unwrap()
    .expect("the shared session is in this repository's refs");
    // Exactly the pin: the client stores what it was served, never more.
    assert_eq!(stored.turns.len(), 4);
    assert!(stored.turns[0].content.contains("empty password"));
    // A shared session arrived from a server, like a pulled one.
    assert_eq!(
        stored.pull_origin.as_ref().unwrap().server,
        "https://api.usetribal.io"
    );

    let fork = fork_of(cwd.path(), CONVERSATION_ID);
    assert!(fork.is_fork());
    // Alice's turns stay on Alice's ref; the fork is a new identity.
    assert!(fork.turns.is_empty());
    assert!(outcome
        .resume_command
        .as_deref()
        .unwrap()
        .starts_with("claude --resume "));
}

/// Forking the same link twice must be safe: turns are immutable and the merge
/// is grow-only, so the second fork adds no turns to the shared session.
#[test]
fn forking_the_same_link_twice_does_not_duplicate_its_turns() {
    let home = tempfile::tempdir().unwrap();
    let cwd = checkout_of(REPO_URL);

    for _ in 0..2 {
        let server = StubServer::serving(2);
        run(
            &server,
            &context(
                cwd.path(),
                Some(home.path().to_path_buf()),
                None,
                &no_lookup,
                &refuse_clone,
            ),
        )
        .unwrap();
    }

    let repo = open_repo(cwd.path()).unwrap();
    let stored = read_conversation(
        repo.inner(),
        &lineage_core::LineageId::from(CONVERSATION_ID),
    )
    .unwrap()
    .unwrap();
    assert_eq!(stored.turns.len(), 2);
}

// --- Opening the forked session -------------------------------------------------

/// The default is to open, because the receiver's one command already meant
/// "put me in this session" — asking again would be asking them to decide
/// something they decided by running it.
#[test]
fn the_forked_session_is_opened_without_being_asked() {
    let home = tempfile::tempdir().unwrap();
    let cwd = checkout_of(REPO_URL);
    let launched = RefCell::new(Vec::new());
    let launch = |command: &str, at: &Path| {
        launched
            .borrow_mut()
            .push((command.to_string(), at.to_path_buf()));
        true
    };
    let server = StubServer::serving(2);

    let outcome = share_fork::run_share_fork(
        &server,
        &link(),
        &context(
            cwd.path(),
            Some(home.path().to_path_buf()),
            None,
            &no_lookup,
            &refuse_clone,
        ),
        false,
        &launch,
    )
    .unwrap();

    assert_eq!(outcome.opened, Opened::Launched);
    let (command, at) = launched.borrow()[0].clone();
    // Adapter-supplied verbatim, run from the directory the harness resolves the
    // session against — not merely the directory the fork landed in.
    assert!(command.starts_with("claude --resume "), "{command}");
    assert!(at.exists(), "{}", at.display());
}

#[test]
fn no_open_prints_the_command_instead_of_running_it() {
    let home = tempfile::tempdir().unwrap();
    let cwd = checkout_of(REPO_URL);
    let launched = RefCell::new(false);
    let launch = |_: &str, _: &Path| {
        *launched.borrow_mut() = true;
        true
    };
    let server = StubServer::serving(2);

    let outcome = share_fork::run_share_fork(
        &server,
        &link(),
        &context(
            cwd.path(),
            Some(home.path().to_path_buf()),
            None,
            &no_lookup,
            &refuse_clone,
        ),
        true,
        &launch,
    )
    .unwrap();

    assert_eq!(outcome.opened, Opened::Printed);
    assert!(!*launched.borrow(), "--no-open must launch nothing");
    // The session is still forked and still openable — --no-open changes who
    // runs the command, not whether there is one.
    assert!(outcome.resume_command.is_some());
    assert!(fork_of(cwd.path(), CONVERSATION_ID).is_fork());
}

/// A missing harness is a thing to install, never a dead end: the fork already
/// succeeded, so the command that opens it is printed and the session waits.
#[test]
fn a_harness_that_will_not_start_leaves_the_fork_done_and_the_command_printed() {
    let home = tempfile::tempdir().unwrap();
    let cwd = checkout_of(REPO_URL);
    let server = StubServer::serving(2);

    let outcome = share_fork::run_share_fork(
        &server,
        &link(),
        &context(
            cwd.path(),
            Some(home.path().to_path_buf()),
            None,
            &no_lookup,
            &refuse_clone,
        ),
        false,
        &harness_missing,
    )
    .expect("a harness that will not start is not a failed fork");

    assert_eq!(outcome.opened, Opened::LaunchFailed);
    assert!(outcome.resume_command.is_some());
    assert!(fork_of(cwd.path(), CONVERSATION_ID).is_fork());
}

// --- Registry -------------------------------------------------------------------

/// The registry is per-machine state, so these drive the binary with
/// `LINEAGE_CONFIG_DIR` pointed at a tempdir rather than calling the library in
/// this process, whose environment is shared with every other test.
fn run_cli(args: &[&str], cwd: &Path, config_dir: &Path, home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tribal"))
        .args(args)
        .current_dir(cwd)
        .env("LINEAGE_CONFIG_DIR", config_dir)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .unwrap()
}

fn registry_json(config_dir: &Path) -> serde_json::Value {
    let text =
        std::fs::read_to_string(config_dir.join("repos.json")).expect("a registry was written");
    serde_json::from_str(&text).unwrap()
}

/// One call site in the dispatcher records the registry, so *any* command run
/// inside a checkout keeps it fresh — which is what makes a fork months later
/// able to find it.
#[test]
fn running_any_command_in_a_repository_records_it_in_the_registry() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let checkout = checkout_of(REPO_URL);

    let output = run_cli(&["list"], checkout.path(), config.path(), home.path());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let entry = &registry_json(config.path())["repos"][REPO_URL];
    assert_eq!(
        PathBuf::from(entry["path"].as_str().unwrap())
            .canonicalize()
            .unwrap(),
        checkout.path().canonicalize().unwrap()
    );
    assert!(entry["last_used"].as_str().is_some());
}

/// A directory with no repository, or a repository with no `origin`, is not a
/// checkout of anything the registry can answer for — recording it would put an
/// entry there that a fork would then have to reject.
#[test]
fn a_directory_with_no_resolvable_remote_is_not_recorded() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let plain = tempfile::tempdir().unwrap();
    git(&["init"], plain.path());

    run_cli(&["list"], plain.path(), config.path(), home.path());

    assert!(!config.path().join("repos.json").exists());
}

#[test]
fn a_corrupt_registry_is_overwritten_rather_than_failing_the_command() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let checkout = checkout_of(REPO_URL);
    std::fs::write(config.path().join("repos.json"), "{ this is not json").unwrap();

    let output = run_cli(&["list"], checkout.path(), config.path(), home.path());

    assert!(
        output.status.success(),
        "a corrupt registry must not fail a command: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(registry_json(config.path())["repos"][REPO_URL].is_object());
}

/// Most-recently-used wins, and the entry is re-verified against git before it
/// is trusted: a recorded path can have been deleted or re-pointed since.
#[test]
fn the_registry_returns_the_most_recent_checkout_and_re_verifies_it() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let older = checkout_of(REPO_URL);
    let newer = checkout_of(REPO_URL);

    run_cli(&["list"], older.path(), config.path(), home.path());
    run_cli(&["list"], newer.path(), config.path(), home.path());

    let recorded = PathBuf::from(
        registry_json(config.path())["repos"][REPO_URL]["path"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(
        recorded.canonicalize().unwrap(),
        newer.path().canonicalize().unwrap(),
        "the later invocation replaced the earlier one"
    );
}

/// The registry is a hint about where to look, not an authority on what is
/// there. A path that has been deleted or re-pointed since it was recorded must
/// fall through to a clone rather than land someone else's session in the wrong
/// tree.
#[test]
fn a_stale_registry_entry_is_rejected_rather_than_trusted() {
    let live = checkout_of(REPO_URL);
    let repointed = checkout_of("github.com/acme/other");
    let deleted = tempfile::tempdir().unwrap();
    let deleted_path = deleted.path().to_path_buf();
    drop(deleted);

    let mut registry = repo_registry::Registry::default();
    for (url, path) in [
        (REPO_URL, live.path().to_path_buf()),
        ("github.com/acme/gone", deleted_path),
        ("github.com/acme/moved", repointed.path().to_path_buf()),
    ] {
        registry.repos.insert(
            url.into(),
            RepoEntry {
                path,
                last_used: Utc::now(),
            },
        );
    }

    assert_eq!(
        repo_registry::lookup_in(&registry, REPO_URL)
            .unwrap()
            .canonicalize()
            .unwrap(),
        live.path().canonicalize().unwrap()
    );
    assert_eq!(
        repo_registry::lookup_in(&registry, "github.com/acme/gone"),
        None
    );
    assert_eq!(
        repo_registry::lookup_in(&registry, "github.com/acme/moved"),
        None,
        "the path is now a checkout of something else"
    );
    assert_eq!(
        repo_registry::lookup_in(&registry, "github.com/acme/never-recorded"),
        None
    );
}

// --- Argument routing and --no-open ---------------------------------------------

/// The argument decides which fork this is. A URL that cannot be fetched must
/// fail as a *share* — reaching a network error proves it never went looking
/// for a session id by that name.
#[test]
fn a_url_argument_takes_the_share_path_rather_than_the_session_lookup() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let checkout = checkout_of(REPO_URL);

    let output = run_cli(
        &[
            "fork",
            "https://app.lineage-does-not-resolve.invalid/s/tok123",
        ],
        checkout.path(),
        config.path(),
        home.path(),
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("could not fetch the shared session"),
        "took the session-id path instead of the share path: {stderr}"
    );
    assert!(
        !stderr.contains("tribal list"),
        "the session-id error must not appear for a URL: {stderr}"
    );
}

#[test]
fn a_session_id_argument_is_unaffected_by_the_share_path() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let checkout = checkout_of(REPO_URL);

    let output = run_cli(
        &["fork", "01NOTASESSION"],
        checkout.path(),
        config.path(),
        home.path(),
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("01NOTASESSION"), "{stderr}");
}

#[test]
fn a_link_that_is_not_a_share_link_is_refused_before_any_request() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let checkout = checkout_of(REPO_URL);

    let output = run_cli(
        &["fork", "https://app.usetribal.io/sessions/abc"],
        checkout.path(),
        config.path(),
        home.path(),
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("/s/<token>"), "{stderr}");
}
