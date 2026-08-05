//! `git lineage share` — turn the session you are in into a link anyone can
//! open without an account (`specs/share-v0.md`).
//!
//! Capture is the sync path with a filter, not a second upload route: the same
//! redaction rules run, private sessions are refused rather than stripped, and
//! the batch that crosses the wire is an ordinary `sync-batch-v0` narrowed to
//! one conversation. Nothing new about a share reaches the server except the
//! create call at the end.
//!
//! Which session "the one you are in" means is a guess, and it is deliberately
//! confined to [`resolve_current_session`] so it can be replaced without
//! touching anything downstream of it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use chrono::Utc;
use lineage_adapters::all_adapters;
use lineage_agent::SessionRef;
use lineage_core::{Conversation, SyncBatch};
use lineage_git::{
    assemble_batch, open_repo, persist_import, read_conversation_stored, read_repo_config,
    resolve_session, stamp_prompted_by, sync_push_with_progress, LineageRepo,
};
use lineage_policy::{
    apply_policy, is_private_session, policy_from_repo_config, prepare_for_export, PolicyConfig,
};
use serde::{Deserialize, Serialize};

use crate::auth;
use crate::events::{EventLog, Outcome};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const SHARE_TIMEOUT: Duration = Duration::from_secs(60);

// --- Wire types (`packages/contracts/src/share.ts`) -----------------------------

/// The client names the repo and the conversation within it, nothing else.
/// Bearer tokens are identity-only, so the workspace is inferred from the
/// repo's owner namespace exactly as sync scopes a batch; the turn count is
/// read server-side, because a client that could supply the pin could widen it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareCreateRequest {
    /// Normalized remote URL (`github.com/<owner>/<name>`) — the same value the
    /// pushed batch carries, so the share cannot name a repo the push did not.
    pub repo: String,
    pub conversation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareCreateResponse {
    /// Returned exactly once; the server stores only its hash.
    pub token: String,
    /// Absolute, so the CLI opens a browser without knowing the web origin.
    pub url: String,
    /// The pin, echoed so the sharer sees what the link froze.
    pub turn_count: usize,
}

/// The two server effects a share has, behind a trait so the capture half —
/// which session, which policy, which batch — is testable without a network.
pub trait ShareTransport {
    fn push(&self, repo: &LineageRepo, batch: &SyncBatch) -> Result<()>;
    fn create(&self, request: &ShareCreateRequest) -> Result<ShareCreateResponse>;
}

/// The real transport: the sync push verbatim, then the share create.
pub struct HttpTransport {
    base: String,
    token: String,
}

impl HttpTransport {
    pub fn new(server: &str, token: &str) -> Self {
        Self {
            base: server.trim_end_matches('/').to_string(),
            token: token.to_string(),
        }
    }
}

impl ShareTransport for HttpTransport {
    fn push(&self, repo: &LineageRepo, batch: &SyncBatch) -> Result<()> {
        let outcome =
            sync_push_with_progress(repo.inner(), &self.base, &self.token, batch, |_, _| {})?;
        if outcome.report.rejected > 0 {
            return Err(format!(
                "the server rejected this session, so there is nothing to share ({} object(s) rejected)",
                outcome.report.rejected
            )
            .into());
        }
        Ok(())
    }

    fn create(&self, request: &ShareCreateRequest) -> Result<ShareCreateResponse> {
        let url = format!("{}/v0/shares", self.base);
        let response = ureq::post(&url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .timeout(SHARE_TIMEOUT)
            .send_json(serde_json::to_value(request)?)
            .map_err(|e| format!("share create failed: {e}"))?;
        Ok(response.into_json()?)
    }
}

// --- Current-session resolution (the seam) --------------------------------------

/// The harness transcript a share is about, before it has been imported.
pub struct CurrentSession {
    pub session: SessionRef,
    /// True when the caller named the session, so a failure downstream can say
    /// whether the guess or the user chose it.
    pub explicit: bool,
}

/// Decide which session `share` is about.
///
/// With `session_id` this is a lookup: the id forms `fork` accepts (lineage id,
/// id prefix, harness UUID) resolve against the stored refs, and the transcript
/// that produced that session is the one refreshed.
///
/// Without one it is a heuristic — the most recently modified harness transcript
/// discovered for this working directory's project. That guess is wrong for
/// anyone running `share` from a second terminal while another session writes,
/// which is exactly why it lives alone in this function: replacing it (a harness
/// that reports its own session, an environment variable, a hook) means
/// rewriting this body and nothing else.
pub fn resolve_current_session(
    repo: &LineageRepo,
    session_id: Option<&str>,
) -> Result<CurrentSession> {
    let discovered = discover_sessions(repo.workdir())?;

    let Some(hint) = session_id else {
        let session = most_recently_modified(discovered).ok_or_else(|| {
            "no agent session found for this directory — run `git lineage share --session <id>` \
             with a session `git lineage list` shows, or start an agent session here first"
                .to_string()
        })?;
        return Ok(CurrentSession {
            session,
            explicit: false,
        });
    };

    let id = resolve_session(repo.inner(), hint).map_err(|error| error.to_string())?;
    let stored = read_conversation_stored(repo.inner(), &id)?
        .ok_or_else(|| format!("session not found after resolve: {id}"))?;
    let session = discovered
        .into_iter()
        .find(|session| session.source_path == source_path_of(&stored))
        .ok_or_else(|| {
            format!(
                "session {id} is stored here but its transcript is not on this machine, \
                 so there is nothing to refresh before sharing"
            )
        })?;
    Ok(CurrentSession {
        session,
        explicit: true,
    })
}

fn discover_sessions(workdir: &Path) -> Result<Vec<SessionRef>> {
    let mut sessions = Vec::new();
    for (_, adapter) in all_adapters(workdir) {
        sessions.extend(adapter.discover()?);
    }
    Ok(sessions)
}

/// Modification time, not `started_at`: a session that began yesterday and is
/// still being written is the one the user is in, and its transcript file is
/// the only thing on this machine that knows that.
fn most_recently_modified(sessions: Vec<SessionRef>) -> Option<SessionRef> {
    sessions.into_iter().max_by_key(|session| {
        std::fs::metadata(&session.source_path)
            .and_then(|meta| meta.modified())
            .ok()
    })
}

fn source_path_of(conversation: &Conversation) -> PathBuf {
    conversation
        .metadata
        .get("source")
        .and_then(|value| value.as_str())
        .map(PathBuf::from)
        .unwrap_or_default()
}

// --- Capture --------------------------------------------------------------------

/// Re-import one transcript so the share pins the turns that exist now rather
/// than the ones the last `import` happened to catch.
fn refresh(repo: &LineageRepo, session: &SessionRef) -> Result<Conversation> {
    let inner = repo.inner();
    let repo_config = read_repo_config(inner)?;
    let policy = policy_from_repo_config(&repo_config);

    let mut conversation = adapter_for(repo.workdir(), session)?.read(session)?;
    let source = session.source_path.display().to_string();
    conversation
        .metadata
        .insert("source".into(), serde_json::Value::String(source.clone()));
    if is_private_session(&source, &repo_config) {
        conversation.private = true;
    }
    stamp_prompted_by(inner, &mut conversation)?;
    let imported = apply_policy(&policy, conversation).conversation;

    persist_import(inner, std::slice::from_ref(&imported))?;

    // Read back what was written rather than returning what went in. Persisting
    // rewrites a conversation — media artifacts are externalized to
    // `.lineage/media`, large content is compacted, ephemeral fields are
    // stripped — and it does that to its own copy, so `imported` is the
    // pre-persistence shape. Pushing that shape uploads an image inline as a
    // `data:` URL where the stored turn holds a media path, which is a different
    // document for the same turn id: the server hashes turns to keep them
    // write-once, so it rejects the share as a content mismatch.
    read_conversation_stored(inner, &imported.id)?.ok_or_else(|| {
        format!(
            "session {} was imported but could not be read back, so there is nothing to share",
            imported.id
        )
        .into()
    })
}

fn adapter_for(
    workdir: &Path,
    session: &SessionRef,
) -> Result<Box<dyn lineage_adapters::ErasedAdapter>> {
    all_adapters(workdir)
        .into_iter()
        .find(|(kind, _)| *kind == session.agent)
        .map(|(_, adapter)| adapter)
        .ok_or_else(|| {
            format!(
                "no adapter for {} is compiled into this build, so its sessions cannot be shared",
                session.agent.as_str()
            )
            .into()
        })
}

/// The same policy sync applies, so a share can only ever carry what a sync of
/// the same session would have carried.
fn prepare_for_share(repo: &LineageRepo, conversation: Conversation) -> Result<Conversation> {
    if conversation.private {
        return Err(format!(
            "session {} is marked private, so it cannot be shared. \
             Privacy is not something a link may unset: un-mark the session \
             (`refs/lineage/config` decides what counts as private) if you meant to share it",
            conversation.id
        )
        .into());
    }

    let repo_config = read_repo_config(repo.inner())?;
    let mut policy = policy_from_repo_config(&repo_config);
    policy.strip_private = true;
    policy.redaction_rules = PolicyConfig::default_safe().redaction_rules;
    Ok(prepare_for_export(&policy, conversation))
}

// --- Command --------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct ShareRequest {
    pub server: Option<String>,
    pub token: Option<String>,
    pub remote: String,
    pub session_id: Option<String>,
    /// Print the URL without opening a browser.
    pub no_open: bool,
}

pub fn share(repo_path: &Path, request: &ShareRequest) -> Result<()> {
    let server = auth::resolve_server(request.server.as_deref())?;
    let token = crate::commands::resolve_sync_token(&server, request.token.as_deref())?;
    let transport = HttpTransport::new(&server, &token);
    let response = run_share(repo_path, &transport, request)?;

    println!();
    println!("    {}", response.url);
    println!();
    println!(
        "Pinned at {} turn(s): continuing this session does not change what the link shows.",
        response.turn_count
    );

    if !request.no_open {
        open_in_browser(&response.url);
    }
    Ok(())
}

/// The whole flow with the transport injected: resolve, refresh, prepare, push
/// one conversation, create the share. Separate from [`share`] so tests drive it
/// against a real tempfile repo with no network — everything it decides (which
/// session, whether the policy refuses it, what the batch contains) is the part
/// worth testing.
pub fn run_share(
    repo_path: &Path,
    transport: &dyn ShareTransport,
    request: &ShareRequest,
) -> Result<ShareCreateResponse> {
    let repo = open_repo(repo_path)?;
    let current = resolve_current_session(&repo, request.session_id.as_deref())?;
    let imported = refresh(&repo, &current.session)?;
    let conversation_id = imported.id.clone();
    let prepared = prepare_for_share(&repo, imported)?;

    let batch = assemble_batch(repo.inner(), &request.remote, vec![prepared])?;
    if batch.conversations.is_empty() {
        return Err(format!(
            "session {conversation_id} came from a Lineage server, so the server that holds it \
             is the one to share it from"
        )
        .into());
    }
    println!(
        "sharing session {conversation_id} ({} turn(s))",
        batch.conversations[0].turns.len()
    );
    transport.push(&repo, &batch)?;

    let response = transport.create(&ShareCreateRequest {
        repo: batch.repo.normalized_remote_url.clone(),
        conversation_id: conversation_id.to_string(),
    })?;

    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "share",
        Outcome::Ok,
        serde_json::json!({
            "remote": request.remote,
            "repo": batch.repo.normalized_remote_url,
            "conversation_id": conversation_id.as_str(),
            "turn_count": response.turn_count,
            "session_explicit": current.explicit,
        }),
    );
    Ok(response)
}

/// Opening the browser is a convenience on top of the product, which is the
/// printed URL — a headless box, a missing opener, or a non-zero exit must not
/// fail a share that the server already minted.
fn open_in_browser(url: &str) {
    let (program, args) = browser_opener();
    if launch(program, args, url) {
        return;
    }
    println!("Could not open a browser here — the link above is the share.");
}

/// Split out so the failure path is testable without a PATH that has no opener:
/// tests call it with a program that cannot exist.
fn launch(program: &str, args: &[&str], url: &str) -> bool {
    Command::new(program)
        .args(args)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn browser_opener() -> (&'static str, &'static [&'static str]) {
    if cfg!(target_os = "macos") {
        return ("open", &[]);
    }
    if cfg!(target_os = "windows") {
        // `start` is a shell builtin, so it needs cmd; the empty title argument
        // stops cmd reading the URL as the window title.
        return ("cmd", &["/C", "start", ""]);
    }
    ("xdg-open", &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::AgentKind;

    #[test]
    fn a_private_session_is_refused_by_name_rather_than_emptied() {
        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let repo = open_repo(dir.path()).unwrap();
        let mut conversation =
            Conversation::new(AgentKind::Claude, dir.path().display().to_string());
        conversation.private = true;

        let error = prepare_for_share(&repo, conversation).expect_err("private must refuse");
        assert!(error.to_string().contains("private"), "got: {error}");
    }

    /// A share must push the *stored* conversation, not the one handed to
    /// `persist_import`. Persisting rewrites its own copy — externalizing media,
    /// compacting content — so the in-memory value keeps the adapter's inline
    /// `data:` URL while the stored turn holds a media path. Pushing the former
    /// is a different document for the same turn id, which the server rejects as
    /// a content-hash mismatch.
    #[test]
    fn persisting_leaves_the_callers_copy_un_externalized() {
        use lineage_core::{Artifact, ArtifactKind, LineageId, Role, Turn};

        let dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let repo = open_repo(dir.path()).unwrap();

        const INLINE_PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGNgYGAAAAAEAAH2FzhVAAAAAElFTkSuQmCC";

        let mut conversation =
            Conversation::new(AgentKind::Claude, dir.path().display().to_string());
        conversation.turns.push(Turn {
            id: LineageId::from(format!("{}-0", conversation.id)),
            role: Role::User,
            content: "look at this".into(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![Artifact {
                kind: ArtifactKind::Image,
                path: INLINE_PNG.into(),
                blob_ref: None,
                content_hash: None,
                mime_type: Some("image/png".into()),
                preview_data_url: None,
                line_range: None,
                resolve: None,
            }],
        });

        persist_import(repo.inner(), std::slice::from_ref(&conversation)).unwrap();

        // The value the caller still holds is the pre-persistence one.
        assert_eq!(conversation.turns[0].artifacts[0].path, INLINE_PNG);

        let stored = read_conversation_stored(repo.inner(), &conversation.id)
            .unwrap()
            .expect("persisted session must read back");
        let artifact = &stored.turns[0].artifacts[0];
        assert!(
            artifact.path.starts_with(".lineage/media/"),
            "stored artifact should be externalized, got: {}",
            artifact.path
        );
        assert!(
            artifact.content_hash.is_some(),
            "stored artifact needs a hash"
        );
    }

    #[test]
    fn a_missing_opener_is_reported_as_a_failed_launch_rather_than_panicking() {
        assert!(!launch(
            "lineage-no-such-browser-opener",
            &[],
            "https://uselineage.io/s/token"
        ));
    }

    #[test]
    fn the_windows_opener_goes_through_cmd_because_start_is_a_builtin() {
        let (program, args) = browser_opener();
        assert!(
            matches!(program, "open" | "cmd" | "xdg-open"),
            "unexpected opener: {program}"
        );
        if program == "cmd" {
            assert_eq!(args, &["/C", "start", ""]);
        }
    }

    #[test]
    fn a_conversation_without_a_source_path_matches_nothing() {
        let conversation = Conversation::new(AgentKind::Claude, "/tmp/proj");
        assert_eq!(source_path_of(&conversation), PathBuf::new());
    }

    #[test]
    fn the_most_recently_modified_transcript_wins() {
        let dir = tempfile::tempdir().unwrap();
        let older = dir.path().join("older.jsonl");
        let newer = dir.path().join("newer.jsonl");
        std::fs::write(&older, "{}").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(&newer, "{}").unwrap();

        let picked = most_recently_modified(vec![session_ref(&older), session_ref(&newer)])
            .expect("a transcript is picked");
        assert_eq!(picked.source_path, newer);
    }

    fn session_ref(path: &Path) -> SessionRef {
        SessionRef {
            id_hint: path.display().to_string(),
            agent: AgentKind::Claude,
            source_path: path.to_path_buf(),
            started_at: None,
        }
    }
}
