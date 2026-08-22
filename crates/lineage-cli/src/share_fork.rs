//! `git lineage fork <share-url>` — the receiving half of a share
//! (`specs/share-v0.md`).
//!
//! The whole design goal is that a receiver never dead-ends and is never asked
//! a question. Someone who opened a link, read a session, and copied one
//! command has already decided everything there is to decide; every prompt
//! after that is the CLI asking them to decide it again. So where the session
//! lands is *resolved* rather than negotiated — current repository, then a
//! registry of this machine's checkouts, then a clone into a predictable place
//! — and each choice is printed as it is made so the outcome is never a
//! surprise, only unrequested.
//!
//! Nothing here requires a login or a `git lineage init`. The token in the URL
//! is the entire authorization (share-v0 "Fetch"), so this path is
//! unauthenticated by construction and must stay that way: an anonymous
//! receiver with only the binary is the case it exists for.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use chrono::{DateTime, Utc};
use lineage_agent::RenderedTranscript;
use lineage_core::{LineageId, PullOrigin};
use lineage_git::{open_repo, persist_conversation, read_conversation_stored};
use serde::{Deserialize, Serialize};

use crate::commands::index_persisted_sessions_best_effort;
use crate::pull_cmd::{merge_pulled, PulledConversation};
use crate::repo_registry;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const FETCH_TIMEOUT: Duration = Duration::from_secs(60);

/// The path a share link takes on the web app, and the prefix of the API route
/// that serves the same share to a client (`specs/share-v0.md` "Endpoints").
const SHARE_PATH_SEGMENT: &str = "s";
const SHARE_FETCH_PATH: &str = "/v0/shares";

/// Where a share-serving origin publishes its API origin (share-v0 "Finding the
/// API"). Short timeout: it is one small document on the way to the real fetch,
/// and falling back to the derivation beats making the receiver wait.
const DISCOVERY_PATH: &str = "/.well-known/lineage.json";
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Where a clone lands when the receiver ran the command from their home
/// directory — the deep-link landing, where `./<name>` would scatter checkouts
/// through `$HOME`.
const HOME_CLONE_ROOT: &str = "lineage";

// --- Wire types (`packages/contracts/src/share.ts`) -----------------------------

/// Where the fork lands, carried on the share because the receiver is
/// unauthenticated and cannot look a repository up.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRepo {
    /// Normalized remote URL (`github.com/<owner>/<name>`).
    pub normalized_remote_url: String,
    pub name: String,
}

/// The share envelope. The conversation inside it is the ordinary down-sync
/// shape from `pull_cmd`, not a share-specific encoding — which is what lets
/// the persist step below be the pull merge verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareFetchResponse {
    pub conversation: PulledConversation,
    pub turn_count: usize,
    pub repo: ShareRepo,
    pub created_at: DateTime<Utc>,
}

/// What an origin publishes about itself. Only the API base matters to a fork;
/// unknown fields are ignored so the document can grow (share-v0 "Evolution").
#[derive(Debug, Clone, Deserialize)]
struct DiscoveryDocument {
    api: String,
}

/// The one server call, behind a trait so every resolution branch below is
/// testable without a network.
pub trait ShareFetchTransport {
    fn fetch(&self, token: &str) -> Result<ShareFetchResponse>;
}

/// Unauthenticated on purpose — deliberately not the bearer transport `share`
/// uses. Sending a stored login here would make an anonymous fork behave
/// differently from an authenticated one, and the token in the URL is already
/// the whole authorization.
pub struct HttpTransport {
    base: String,
}

impl HttpTransport {
    pub fn new(server: &str) -> Self {
        Self {
            base: server.trim_end_matches('/').to_string(),
        }
    }
}

impl ShareFetchTransport for HttpTransport {
    fn fetch(&self, token: &str) -> Result<ShareFetchResponse> {
        let url = format!("{}{SHARE_FETCH_PATH}/{token}", self.base);
        let response = ureq::get(&url)
            .timeout(FETCH_TIMEOUT)
            .call()
            .map_err(|error| describe_fetch_failure(&url, error))?;
        Ok(response.into_json()?)
    }
}

/// A dead link is the one failure a receiver will actually hit, and the server
/// answers revoked and never-issued with the same status on purpose (share-v0
/// "Fetch") — so the message says the link does not work and never claims to
/// know which of the two it was.
///
/// A 404 from the *route* rather than the share means we asked the wrong server,
/// which is a different problem with a different fix: reporting it as a dead
/// link sends the receiver to ask for a new one that will fail identically.
fn describe_fetch_failure(url: &str, error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(404, response) => {
            if is_share_miss(response) {
                return "this link is no longer available: it may have been revoked, or the link \
                        may be mistyped. Ask whoever shared it for a new one"
                    .to_string();
            }
            format!(
                "no share endpoint at {url} — the server answered, but nothing serves shares \
                 there. If this deployment publishes its API elsewhere, name it with --server"
            )
        }
        other => format!("could not fetch the shared session: {other}"),
    }
}

/// Whether a 404 came from the share lookup or from there being no such route.
///
/// The share miss carries the server's own JSON error; a routing miss is
/// whatever the framework emits for an unmapped path. Only the former names the
/// share, so an unreadable body is treated as a routing miss — the message that
/// suggests checking the server is the safer of the two to be wrong with.
fn is_share_miss(response: ureq::Response) -> bool {
    let body = response.into_string().unwrap_or_default();
    body.contains("share link not found")
}

// --- URL parsing ----------------------------------------------------------------

/// A share link split into the parts the fork needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareLink {
    /// The API origin to fetch from, derived from the link's own origin.
    pub server: String,
    pub token: String,
}

/// True when the fork argument is a share link rather than a session id.
///
/// Session ids are ULIDs, prefixes of them, or harness UUIDs — none of which
/// can contain a `/` or a scheme, so the two argument forms cannot collide.
pub fn is_share_url(argument: &str) -> bool {
    argument.starts_with("http://")
        || argument.starts_with("https://")
        || argument.starts_with(&format!("/{SHARE_PATH_SEGMENT}/"))
}

/// Split a share link into the API origin and the token.
///
/// A bare `/s/<token>` is accepted so a receiver can paste the path out of a
/// page they are already on, but it carries no origin — that form needs
/// `--server`.
pub fn parse_share_url(url: &str, server_override: Option<&str>) -> Result<ShareLink> {
    resolve_share_url(url, server_override, &discover_api_origin)
}

/// [`parse_share_url`] with the discovery fetch injected, so every resolution
/// branch is testable without a network.
pub fn resolve_share_url(
    url: &str,
    server_override: Option<&str>,
    discover: &dyn Fn(&str) -> Option<String>,
) -> Result<ShareLink> {
    let token = share_token(url)?;

    if let Some(server) = server_override {
        return Ok(ShareLink {
            server: server.trim_end_matches('/').to_string(),
            token,
        });
    }

    let origin = web_origin(url).ok_or_else(|| {
        format!(
            "'{url}' has no host to fetch from — pass the full share link, \
             or name the server with --server"
        )
    })?;
    Ok(ShareLink {
        server: api_origin(&origin, discover),
        token,
    })
}

fn share_token(url: &str) -> Result<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let position = segments
        .iter()
        .rposition(|segment| *segment == SHARE_PATH_SEGMENT)
        .ok_or_else(|| {
            format!(
                "'{url}' is not a share link — a share link looks like https://<host>/s/<token>"
            )
        })?;
    let token = segments.get(position + 1).ok_or_else(|| {
        format!("'{url}' names no share token — a share link looks like https://<host>/s/<token>")
    })?;
    Ok((*token).to_string())
}

fn web_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split('/').next()?;
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}"))
}

/// The API origin that serves the share the web origin displayed.
///
/// The origin is asked rather than guessed: where the API sits relative to the
/// web app is a property of the deployment (`app.<domain>` beside
/// `api.<domain>/api` here, split ports in a dev stack, one origin for a
/// self-hoster), and none of it is recoverable from the link. share-v0 "Finding
/// the API" makes every origin that serves share pages publish it.
///
/// The derivation below is only the fallback for an origin that publishes no
/// document — a server predating discovery, which would otherwise be bricked.
/// `--server` overrides both.
fn api_origin(web_origin: &str, discover: &dyn Fn(&str) -> Option<String>) -> String {
    discover(web_origin).unwrap_or_else(|| derived_api_origin(web_origin))
}

/// The pre-discovery guess: this deployment publishes the web app at
/// `app.<domain>` and the API at `api.<domain>`, so swapping the label is the
/// derivation. Any other host is used as-is, which is right for a single-origin
/// deployment and wrong for a path-prefixed one — it cannot see a prefix, which
/// is precisely why discovery exists.
fn derived_api_origin(web_origin: &str) -> String {
    match web_origin.split_once("://") {
        Some((scheme, host)) if host.starts_with("app.") => {
            format!("{scheme}://api.{}", &host["app.".len()..])
        }
        _ => web_origin.to_string(),
    }
}

/// Reads the discovery document a share-serving origin publishes.
///
/// Every failure is `None` — a missing document, a timeout, HTML from a
/// single-page app answering an unknown path with its shell, a body that is not
/// the expected shape. Discovery is an improvement on guessing, never a new way
/// for a fork to fail, so the caller falls back rather than reporting.
fn discover_api_origin(web_origin: &str) -> Option<String> {
    let url = format!("{}{DISCOVERY_PATH}", web_origin.trim_end_matches('/'));
    let response = ureq::get(&url).timeout(DISCOVERY_TIMEOUT).call().ok()?;
    let document: DiscoveryDocument = response.into_json().ok()?;
    let api = document.api.trim_end_matches('/').to_string();
    if !api.starts_with("http://") && !api.starts_with("https://") {
        return None;
    }
    Some(api)
}

// --- Landing resolution ---------------------------------------------------------

/// How the fork's directory was chosen. Printed, and asserted on in tests,
/// because "which branch resolved" is the first question when a fork lands
/// somewhere unexpected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Landing {
    /// The command was run inside a checkout of the shared repository.
    CurrentRepo,
    /// A checkout recorded in this machine's registry.
    Registry,
    /// Nothing local matched, so the repository was cloned.
    Cloned,
    /// `--into` was given; the caller chose, and nothing was matched.
    Into,
}

#[derive(Debug, Clone)]
pub struct Landed {
    pub path: PathBuf,
    pub how: Landing,
}

/// Everything the resolution reads from the outside world, so tests drive each
/// branch without a `$HOME`, a registry file, or a network clone.
pub struct LandingContext<'a> {
    pub cwd: &'a Path,
    pub home: Option<PathBuf>,
    /// `--into`, which overrides every other rung.
    pub into: Option<PathBuf>,
    /// The registry lookup, injected so a test does not write to the real one.
    pub lookup: &'a dyn Fn(&str) -> Option<PathBuf>,
    /// Clones `(url, destination)`. Injected so tests clone from a local
    /// fixture repository instead of the network.
    pub clone: &'a dyn Fn(&str, &Path) -> Result<()>,
}

/// Decide where the session lands, printing each choice and asking nothing.
pub fn resolve_landing(repo: &ShareRepo, context: &LandingContext<'_>) -> Result<Landed> {
    if let Some(into) = &context.into {
        return land_into(into);
    }

    if repo_registry::origin_url(context.cwd).as_deref() == Some(&repo.normalized_remote_url) {
        let workdir = open_repo(context.cwd)?.workdir().to_path_buf();
        println!("Using this repository: {}", workdir.display());
        return Ok(Landed {
            path: workdir,
            how: Landing::CurrentRepo,
        });
    }

    if let Some(path) = (context.lookup)(&repo.normalized_remote_url) {
        println!("Using your checkout of {}: {}", repo.name, path.display());
        return Ok(Landed {
            path,
            how: Landing::Registry,
        });
    }

    let destination = clone_destination(repo, context);
    let url = clone_url(&repo.normalized_remote_url);
    println!("Cloning {url} into {}", destination.display());
    (context.clone)(&url, &destination).map_err(|error| {
        format!(
            "could not clone {} into {}: {error}. \
             Clone it yourself and run this command again from inside it, \
             or pass --into <dir> to fork somewhere else",
            repo.name,
            destination.display()
        )
    })?;
    Ok(Landed {
        path: destination,
        how: Landing::Cloned,
    })
}

/// `--into` is the escape hatch for a receiver who wants the session somewhere
/// that is not a checkout of the shared repository at all. It is honoured
/// literally — including `git init`-ing an empty directory — with the caveat
/// printed, because the session's file references were written against a
/// different tree and mostly will not resolve here.
fn land_into(into: &Path) -> Result<Landed> {
    std::fs::create_dir_all(into)?;
    let initialized = if open_repo(into).is_err() {
        git(&["init"], into)?;
        " (new repository)"
    } else {
        ""
    };
    println!("Forking into {}{initialized}", into.display());
    println!(
        "Note: --into was given, so this may not be a checkout of the shared repository — \
         file paths the session mentions may not exist here."
    );
    Ok(Landed {
        path: into.to_path_buf(),
        how: Landing::Into,
    })
}

/// `./<name>`, except from the home directory — the deep-link landing, where a
/// bare `./<name>` would scatter checkouts across `$HOME`.
pub fn clone_destination(repo: &ShareRepo, context: &LandingContext<'_>) -> PathBuf {
    let Some(home) = context
        .home
        .as_deref()
        .filter(|home| same_dir(home, context.cwd))
    else {
        return context.cwd.join(&repo.name);
    };
    let root = home.join(HOME_CLONE_ROOT);
    match owner_of(&repo.normalized_remote_url) {
        Some(owner) => root.join(owner).join(&repo.name),
        None => root.join(&repo.name),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canonical(a) == canonical(b)
}

fn owner_of(normalized_remote_url: &str) -> Option<&str> {
    let segments: Vec<&str> = normalized_remote_url
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if segments.len() < 3 {
        return None;
    }
    segments.get(segments.len() - 2).copied()
}

/// The normalized URL has no scheme and no `.git` suffix (sync-protocol-v0
/// "Repo binding"); https is the form that works for a stranger, and git's own
/// credential machinery handles a private one from here.
fn clone_url(normalized_remote_url: &str) -> String {
    format!("https://{normalized_remote_url}.git")
}

pub fn git_clone(url: &str, destination: &Path) -> Result<()> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let status = Command::new("git")
        .arg("clone")
        .arg(url)
        .arg(destination)
        .status()?;
    if !status.success() {
        return Err(format!("git clone exited with {status}").into());
    }
    Ok(())
}

fn git(args: &[&str], cwd: &Path) -> Result<()> {
    let status = Command::new("git").args(args).current_dir(cwd).status()?;
    if !status.success() {
        return Err(format!("git {} exited with {status}", args.join(" ")).into());
    }
    Ok(())
}

// --- Persistence ----------------------------------------------------------------

/// Write the shared conversation into the target repository's lineage refs.
///
/// This is the pull merge unchanged, because the share fetch returns the
/// down-sync shape (share-v0 "Wire shapes"): a receiver who already had the
/// session keeps every turn they hold, and a second fork of the same link
/// writes nothing new.
pub fn persist_shared(
    repo_path: &Path,
    share: &ShareFetchResponse,
    server: &str,
) -> Result<String> {
    let repo = open_repo(repo_path)?;
    let id = LineageId::from(share.conversation.id.clone());
    let existing = read_conversation_stored(repo.inner(), &id)?;
    let merged = merge_pulled(existing, &share.conversation, &share_origin(server));
    persist_conversation(repo.inner(), &merged)?;
    // Persisting writes refs; searching reads an index. Without this the session
    // a receiver just forked is the one thing `search` and `context query` cannot
    // find — exactly backwards for a session someone picked up to work from.
    index_persisted_sessions_best_effort(&repo, std::slice::from_ref(&merged.id));
    Ok(merged.id.to_string())
}

/// A shared session came from a Tribal server the same way a pulled one did,
/// so it is marked with the same origin — `fork` and `list` then describe it
/// identically whether it arrived by link or by `pull`.
fn share_origin(server: &str) -> PullOrigin {
    PullOrigin {
        server: server.to_string(),
        tenant: None,
        pulled_at: Utc::now(),
        lineage_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

// --- Command --------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ShareForkRequest {
    /// The share link, or a bare `/s/<token>` path.
    pub url: String,
    /// Overrides the API origin derived from the link.
    pub server: Option<String>,
    /// Fork here instead of resolving where to land.
    pub into: Option<PathBuf>,
    /// Print the resume command instead of running it.
    pub no_open: bool,
}

/// What happened to the forked session once it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    /// The harness was started on it.
    Launched,
    /// The receiver asked for the command instead (`--no-open`).
    Printed,
    /// The harness could not be started, so the command was printed.
    LaunchFailed,
    /// There was nothing to open — `--brief`, which the share path never asks for.
    Nothing,
}

/// What the fork did, so the caller can report it and tests can assert on the
/// branch that resolved rather than on printed prose.
#[derive(Debug, Clone)]
pub struct ShareForkOutcome {
    pub landed: Landed,
    pub session_id: String,
    pub turn_count: usize,
    /// The command that opens the forked session, whether or not it was run.
    pub resume_command: Option<String>,
    pub opened: Opened,
}

pub fn fork_share(request: &ShareForkRequest) -> Result<ShareForkOutcome> {
    let link = parse_share_url(&request.url, request.server.as_deref())?;
    let transport = HttpTransport::new(&link.server);
    let cwd = std::env::current_dir()?;
    let context = LandingContext {
        cwd: &cwd,
        home: std::env::var_os("HOME").map(PathBuf::from),
        into: request.into.clone(),
        lookup: &repo_registry::lookup,
        clone: &git_clone,
    };

    run_share_fork(&transport, &link, &context, request.no_open, &launch)
}

/// The whole receive flow with everything that leaves the process injected:
/// fetch, resolve where to land, persist, hand off to the existing fork
/// machinery, open. Separate from [`fork_share`] so tests drive every branch —
/// including the harness failing to start — against tempfile repositories with
/// no network and nothing launched.
pub fn run_share_fork(
    transport: &dyn ShareFetchTransport,
    link: &ShareLink,
    context: &LandingContext<'_>,
    no_open: bool,
    launch: &dyn Fn(&str, &Path) -> bool,
) -> Result<ShareForkOutcome> {
    let share = transport.fetch(&link.token)?;
    println!(
        "Shared session from {} ({} turn(s))",
        share.repo.name, share.turn_count
    );

    let landed = resolve_landing(&share.repo, context)?;
    let session_id = persist_shared(&landed.path, &share, &link.server)?;
    let rendered = crate::fork_cmd::fork_resolved(&landed.path, &session_id, false)?;

    let opened = match &rendered {
        Some(rendered) => open_session(rendered, no_open, launch),
        None => Opened::Nothing,
    };
    Ok(ShareForkOutcome {
        landed,
        session_id,
        turn_count: share.turn_count,
        resume_command: rendered.map(|rendered| rendered.resume_command),
        opened,
    })
}

/// Open the forked session, or say how to.
///
/// The receiver ran one command meaning "put me in this session", so opening it
/// is that command finishing rather than a second decision — which is why there
/// is no prompt here, only `--no-open` for someone who wanted the command. A
/// harness that will not start falls back to the same printed command: the fork
/// already succeeded and every byte of it is on disk, so a missing `claude` is
/// something to install, never a lost session.
fn open_session(
    rendered: &RenderedTranscript,
    no_open: bool,
    launch: &dyn Fn(&str, &Path) -> bool,
) -> Opened {
    if no_open {
        print_how_to_continue(rendered);
        return Opened::Printed;
    }
    if launch(&rendered.resume_command, &rendered.resume_cwd) {
        return Opened::Launched;
    }
    println!("Could not start the agent here — the session is forked and waiting.");
    print_how_to_continue(rendered);
    Opened::LaunchFailed
}

fn print_how_to_continue(rendered: &RenderedTranscript) {
    println!(
        "To continue it, run this from {}:",
        rendered.resume_cwd.display()
    );
    println!();
    println!("    {}", rendered.resume_command);
}

/// True when the harness ran at all. Its own exit status is the receiver's
/// session ending, not a failure of the fork.
fn launch(command: &str, cwd: &Path) -> bool {
    let mut parts = command.split_whitespace();
    let Some(program) = parts.next() else {
        return false;
    };
    Command::new(program)
        .args(parts)
        .current_dir(cwd)
        .status()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> ShareRepo {
        ShareRepo {
            normalized_remote_url: "github.com/acme/widgets".into(),
            name: "widgets".into(),
        }
    }

    #[test]
    fn a_url_is_a_share_link_and_a_session_id_is_not() {
        assert!(is_share_url("https://app.usetribal.io/s/tok"));
        assert!(is_share_url("http://localhost:4200/s/tok"));
        assert!(is_share_url("/s/tok"));
        assert!(!is_share_url("01J8Z9QT7QK6X0000000000000"));
        assert!(!is_share_url("aaaaaaaa-0000-0000-0000-000000000001"));
    }

    /// The server's own share miss (`apps/api` SharesService) versus what a
    /// framework emits for a path it does not route. Both are 404s, and telling
    /// a receiver their link died when we asked the wrong server sends them to
    /// ask for a replacement that fails identically.
    #[test]
    fn a_missing_route_is_not_reported_as_a_dead_link() {
        let routing_miss = ureq::Error::Status(
            404,
            ureq::Response::new(
                404,
                "Not Found",
                r#"{"message":"Cannot GET /v0/shares/tok"}"#,
            )
            .unwrap(),
        );
        let message = describe_fetch_failure("https://api.example.dev/v0/shares/tok", routing_miss);
        assert!(message.contains("no share endpoint"), "got: {message}");
        assert!(!message.contains("no longer available"), "got: {message}");
    }

    #[test]
    fn a_revoked_or_unknown_token_still_reads_as_a_dead_link() {
        let share_miss = ureq::Error::Status(
            404,
            ureq::Response::new(404, "Not Found", r#"{"message":"share link not found"}"#).unwrap(),
        );
        let message = describe_fetch_failure("https://api.example.dev/v0/shares/tok", share_miss);
        assert!(message.contains("no longer available"), "got: {message}");
    }

    fn publishes(api: &'static str) -> impl Fn(&str) -> Option<String> {
        move |_| Some(api.to_string())
    }

    fn publishes_nothing(_: &str) -> Option<String> {
        None
    }

    /// The bug this whole path exists for: the API lives under a path prefix,
    /// which no host rewrite can infer, so the fetch went to a route that does
    /// not exist. The origin says where it is instead.
    #[test]
    fn the_api_origin_comes_from_the_document_the_link_origin_publishes() {
        let link = resolve_share_url(
            "https://app.usetribal.io/s/tok123",
            None,
            &publishes("https://api.usetribal.io/api"),
        )
        .unwrap();
        assert_eq!(link.server, "https://api.usetribal.io/api");
        assert_eq!(link.token, "tok123");
    }

    /// A dev stack splits the web app and API across ports — the shape that
    /// nothing derivable from the link can reach, and that a stored login used
    /// to cover for locally.
    #[test]
    fn split_ports_resolve_from_the_document_rather_than_the_host() {
        let link = resolve_share_url(
            "http://localhost:4200/s/tok",
            None,
            &publishes("http://localhost:3000/api"),
        )
        .unwrap();
        assert_eq!(link.server, "http://localhost:3000/api");
    }

    /// A server predating discovery must still resolve rather than dead-end.
    #[test]
    fn an_origin_publishing_nothing_falls_back_to_the_host_rewrite() {
        let link =
            resolve_share_url("https://app.usetribal.io/s/tok", None, &publishes_nothing).unwrap();
        assert_eq!(link.server, "https://api.usetribal.io");
    }

    #[test]
    fn a_single_origin_deployment_is_left_alone() {
        assert_eq!(
            derived_api_origin("http://localhost:4200"),
            "http://localhost:4200"
        );
    }

    #[test]
    fn an_explicit_server_wins_over_the_document() {
        let link = resolve_share_url(
            "https://app.usetribal.io/s/tok",
            Some("http://127.0.0.1:3000/"),
            &publishes("https://api.usetribal.io/api"),
        )
        .unwrap();
        assert_eq!(link.server, "http://127.0.0.1:3000");
        assert_eq!(link.token, "tok");
    }

    #[test]
    fn query_and_fragment_are_not_part_of_the_token() {
        let link = resolve_share_url(
            "https://app.example.dev/s/tok?utm=mail#top",
            None,
            &publishes_nothing,
        )
        .unwrap();
        assert_eq!(link.token, "tok");
    }

    #[test]
    fn a_path_only_link_needs_a_server_and_says_so() {
        let error = resolve_share_url("/s/tok", None, &publishes_nothing)
            .expect_err("no origin to fetch from");
        assert!(error.to_string().contains("--server"), "got: {error}");
    }

    #[test]
    fn a_url_that_is_not_a_share_link_is_refused_by_shape() {
        let error = resolve_share_url("https://example.com/sessions/abc", None, &publishes_nothing)
            .expect_err("not a share link");
        assert!(error.to_string().contains("/s/<token>"), "got: {error}");
    }

    #[test]
    fn the_clone_url_is_the_https_form_of_the_normalized_remote() {
        assert_eq!(
            clone_url("github.com/acme/widgets"),
            "https://github.com/acme/widgets.git"
        );
    }

    #[test]
    fn a_clone_lands_beside_the_cwd_when_that_is_not_the_home_directory() {
        let cwd = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let context = LandingContext {
            cwd: cwd.path(),
            home: Some(home.path().to_path_buf()),
            into: None,
            lookup: &|_| None,
            clone: &|_, _| Ok(()),
        };
        assert_eq!(
            clone_destination(&repo(), &context),
            cwd.path().join("widgets")
        );
    }

    /// The deep link lands the receiver in their home directory, where a bare
    /// `./widgets` would leave repositories loose in `$HOME`.
    #[test]
    fn a_clone_from_the_home_directory_lands_under_a_lineage_owner_tree() {
        let home = tempfile::tempdir().unwrap();
        let context = LandingContext {
            cwd: home.path(),
            home: Some(home.path().to_path_buf()),
            into: None,
            lookup: &|_| None,
            clone: &|_, _| Ok(()),
        };
        assert_eq!(
            clone_destination(&repo(), &context),
            home.path().join("lineage").join("acme").join("widgets")
        );
    }

    #[test]
    fn a_remote_without_an_owner_segment_still_has_a_home_destination() {
        let home = tempfile::tempdir().unwrap();
        let context = LandingContext {
            cwd: home.path(),
            home: Some(home.path().to_path_buf()),
            into: None,
            lookup: &|_| None,
            clone: &|_, _| Ok(()),
        };
        let flat = ShareRepo {
            normalized_remote_url: "widgets".into(),
            name: "widgets".into(),
        };
        assert_eq!(
            clone_destination(&flat, &context),
            home.path().join("lineage").join("widgets")
        );
    }
}
