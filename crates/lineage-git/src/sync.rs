//! Client side of `specs/sync-protocol-v0.md`: assemble a `SyncBatch` from the
//! repo's git-native lineage storage and push it to a server's ingest endpoint.
//!
//! Transport mirrors the LFS HTTP path (`lfs_batch.rs`): `ureq`, bearer auth,
//! per-object results inspected from the response. Assembly is kept pure and
//! network-free so it can be unit-tested against a tempfile repo; only
//! `sync_push` touches the wire.
//!
//! Redaction and private-session exclusion happen in the CLI before assembly
//! (the same `prepare_for_export` path `export` uses) — this crate stays free of
//! the policy engine, exactly as the other git-write paths do.

use std::collections::BTreeSet;
use std::time::Duration;

use git2::Repository;
use lineage_core::{
    BlobManifestEntry, Conversation, GitNote, LineObject, LineageError, LineageId, RepoBinding,
    SessionCommitLink, SyncBatch, SyncResponse, SYNC_RESPONSE_SCHEMA,
};
use lineage_store::{normalize_oid, LfsStore};

use crate::lfs_refs::collect_blob_refs_from_conversation;
use crate::notes::read_note_for_commit;
use crate::refs::{read_line_object, LINEAGE_NOTES_REF};

const SYNC_TIMEOUT: Duration = Duration::from_secs(120);
const LINE_OBJECT_REF_PREFIX: &str = "refs/lineage/lines/";

/// Conversations per `POST /v0/sync`. One HTTP request / DB transaction per
/// chunk so a mid-run timeout leaves earlier chunks committed (sync never
/// deletes; retrying resends the whole set idempotently).
pub const SYNC_CONVERSATIONS_PER_CHUNK: usize = 10;

/// Local git-config key caching the server-issued repo id, so subsequent syncs
/// can send it back as `repo.server_repo_id`. Not committed — avoids a fork
/// inheriting the parent's binding (sync-protocol-v0 "Repo binding").
pub const SERVER_REPO_ID_KEY: &str = "lineage.serverRepoId";

#[derive(Debug, Default)]
pub struct SyncReport {
    pub repo_id: String,
    pub blobs_uploaded: usize,
    pub chunks: usize,
    pub accepted: usize,
    pub noop: usize,
    pub rejected: usize,
    pub pending: usize,
}

/// Tallied summary plus the untouched wire response, so callers can record the
/// server's per-object verdicts verbatim (diagnostics-v0 `sync` event) instead
/// of only the client-side counts.
#[derive(Debug)]
pub struct SyncOutcome {
    pub report: SyncReport,
    pub response: SyncResponse,
}

/// Assembles the up-sync batch from `conversations` (already redacted and
/// private-filtered by the caller) plus the repo's line-object refs and notes.
/// Line objects and commit links belonging to a non-synced conversation are
/// dropped, so a private session's provenance cannot leak through them.
pub fn assemble_batch(
    repo: &Repository,
    remote: &str,
    conversations: Vec<Conversation>,
) -> Result<SyncBatch, LineageError> {
    let binding = resolve_repo_binding(repo, remote)?;
    let conversations = drop_pulled(conversations);
    let synced_ids: BTreeSet<String> = conversations.iter().map(|c| c.id.to_string()).collect();

    let mut batch = SyncBatch::new(binding);
    batch.blobs = collect_blob_manifest(repo, &conversations)?;
    batch.line_objects = collect_line_objects(repo, &synced_ids)?;
    batch.session_commit_links = collect_session_commit_links(repo, &synced_ids)?;
    batch.conversations = conversations;
    Ok(batch)
}

/// Drops sessions this machine pulled from a server: that server already holds
/// them, so re-pushing spends bandwidth on a no-op and attributes the upload to
/// whoever pulled rather than whoever wrote it.
///
/// The test is each conversation's *own* `pull_origin` and nothing else. Do not
/// make this transitive by walking `parent_session_id` or `fork_origin` up to a
/// pulled ancestor: a fork of a pulled session is a new session that this
/// machine's user owns and is the only copy of, so excluding it would mean
/// their continuation of a teammate's work never reaches the server at all.
fn drop_pulled(conversations: Vec<Conversation>) -> Vec<Conversation> {
    conversations
        .into_iter()
        .filter(|conv| conv.pull_origin.is_none())
        .collect()
}

/// One manifest entry per LFS object referenced by a synced conversation. The
/// referencing objects already passed the privacy filter, so a blob reachable
/// only from a private/unsynced session is never declared (sync-protocol-v0
/// "Blob transfer").
fn collect_blob_manifest(
    repo: &Repository,
    conversations: &[Conversation],
) -> Result<Vec<BlobManifestEntry>, LineageError> {
    let lfs = LfsStore::new(repo.path());
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();

    for conv in conversations {
        for blob_ref in collect_blob_refs_from_conversation(conv) {
            let sha256 = normalize_oid(&blob_ref);
            if !seen.insert(sha256.clone()) {
                continue;
            }
            // Skip a reference whose content is absent locally — there is nothing
            // to upload, and a manifest entry without bytes would only ever
            // report `pending`.
            let Ok(data) = lfs.get(&sha256) else {
                continue;
            };
            entries.push(BlobManifestEntry {
                sha256,
                byte_size: data.len() as u64,
                content_type: None,
            });
        }
    }
    Ok(entries)
}

/// Line objects sync as first-class objects keyed by their own ULID; restrict to
/// those belonging to a synced conversation.
fn collect_line_objects(
    repo: &Repository,
    synced_ids: &BTreeSet<String>,
) -> Result<Vec<LineObject>, LineageError> {
    let mut out = Vec::new();
    for reference in repo
        .references_glob(&format!("{LINE_OBJECT_REF_PREFIX}*"))
        .map_err(|e| LineageError::Other(e.to_string()))?
        .filter_map(|r| r.ok())
    {
        let Some(suffix) = reference
            .name()
            .and_then(|n| n.strip_prefix(LINE_OBJECT_REF_PREFIX))
        else {
            continue;
        };
        let Some(obj) = read_line_object(repo, &LineageId::from(suffix))? else {
            continue;
        };
        if synced_ids.contains(&obj.conversation_id.to_string()) {
            out.push(obj);
        }
    }
    Ok(out)
}

/// Decomposes each git note into one `(session, commit)` link per session id,
/// dropping links to non-synced sessions. The note's `line_object_ids` are not
/// part of the link — line objects sync independently and carry their own
/// `commit_sha` (sync-protocol-v0 "Object mapping").
fn collect_session_commit_links(
    repo: &Repository,
    synced_ids: &BTreeSet<String>,
) -> Result<Vec<SessionCommitLink>, LineageError> {
    let Ok(notes) = repo.notes(Some(LINEAGE_NOTES_REF)) else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for entry in notes {
        let (_note_oid, annotated) = entry.map_err(|e| LineageError::Other(e.to_string()))?;
        let Some(note) = read_note(repo, annotated)? else {
            continue;
        };
        for session_id in &note.session_ids {
            if !synced_ids.contains(&session_id.to_string()) {
                continue;
            }
            out.push(SessionCommitLink {
                conversation_id: session_id.clone(),
                commit_sha: note.commit_sha.clone(),
                patch_id: note.patch_id.clone(),
            });
        }
    }
    Ok(out)
}

fn read_note(repo: &Repository, annotated: git2::Oid) -> Result<Option<GitNote>, LineageError> {
    read_note_for_commit(repo, &annotated.to_string())
}

/// Builds the repo identity hints the server resolves against, caching any
/// previously-issued `server_repo_id` from local git config. Public because
/// the context-injection hook builds the same identity for its queries.
pub fn resolve_repo_binding(repo: &Repository, remote: &str) -> Result<RepoBinding, LineageError> {
    Ok(RepoBinding {
        normalized_remote_url: normalized_remote_url(repo, remote)?,
        root_commit_sha: root_commit_sha(repo)?,
        server_repo_id: read_git_config(repo, SERVER_REPO_ID_KEY),
    })
}

/// `host/owner/name`, lowercase, no scheme/login, no `.git` suffix
/// (sync-protocol-v0 "Repo binding").
pub fn normalize_remote_url(url: &str) -> String {
    let trimmed = url.trim();
    // scp-like form (git@host:owner/name) has no scheme; rewrite its single
    // colon to a slash so it joins the host/path grammar below.
    let without_scheme = if let Some(rest) = trimmed.strip_prefix("git@") {
        rest.replacen(':', "/", 1)
    } else {
        trimmed.split("://").nth(1).unwrap_or(trimmed).to_string()
    };
    // Drop any embedded login (user[:pass]@host...).
    let host_and_path = without_scheme
        .rsplit_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(&without_scheme);
    host_and_path
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_lowercase()
}

fn normalized_remote_url(repo: &Repository, remote: &str) -> Result<String, LineageError> {
    let url = repo
        .find_remote(remote)
        .map_err(|e| LineageError::Other(format!("remote {remote} not found: {e}")))?
        .url()
        .map(str::to_string)
        .ok_or_else(|| LineageError::Other(format!("remote {remote} has no url")))?;
    Ok(normalize_remote_url(&url))
}

/// First parentless commit reachable from HEAD via first-parent traversal — the
/// repository's root commit (sync-protocol-v0 "Repo binding").
fn root_commit_sha(repo: &Repository) -> Result<String, LineageError> {
    let mut commit = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| LineageError::Other(format!("cannot resolve HEAD: {e}")))?;
    while let Some(parent) = commit.parents().next() {
        commit = parent;
    }
    Ok(commit.id().to_string())
}

/// Splits an assembled batch into conversation-scoped chunks. Each chunk carries
/// that subset's line objects and commit links; blob manifests stay on the first
/// chunk only (bytes are uploaded once before any POST).
pub fn chunk_batch(batch: &SyncBatch, max_conversations: usize) -> Vec<SyncBatch> {
    let max = max_conversations.max(1);
    if batch.conversations.is_empty() {
        return vec![batch.clone()];
    }

    let mut chunks: Vec<SyncBatch> = batch
        .conversations
        .chunks(max)
        .map(|convs| {
            let ids: BTreeSet<String> = convs.iter().map(|c| c.id.to_string()).collect();
            let mut chunk = SyncBatch::new(batch.repo.clone());
            chunk.conversations = convs.to_vec();
            chunk.line_objects = batch
                .line_objects
                .iter()
                .filter(|line| ids.contains(&line.conversation_id.to_string()))
                .cloned()
                .collect();
            chunk.session_commit_links = batch
                .session_commit_links
                .iter()
                .filter(|link| ids.contains(&link.conversation_id.to_string()))
                .cloned()
                .collect();
            chunk
        })
        .collect();

    if let Some(first) = chunks.first_mut() {
        first.blobs = batch.blobs.clone();
    }
    chunks
}

/// Pushes an assembled batch: uploads referenced blobs first (idempotent), POSTs
/// conversation-sized chunks, caches the returned `repo_id`, and tallies results.
pub fn sync_push(
    repo: &Repository,
    server_url: &str,
    token: &str,
    batch: &SyncBatch,
) -> Result<SyncOutcome, LineageError> {
    sync_push_with_progress(repo, server_url, token, batch, |_, _| {})
}

/// Like [`sync_push`], calling `on_chunk(done, total)` after each successful POST
/// (`done` is 1-based).
pub fn sync_push_with_progress(
    repo: &Repository,
    server_url: &str,
    token: &str,
    batch: &SyncBatch,
    mut on_chunk: impl FnMut(usize, usize),
) -> Result<SyncOutcome, LineageError> {
    let base = server_url.trim_end_matches('/');
    let lfs = LfsStore::new(repo.path());
    let chunks = chunk_batch(batch, SYNC_CONVERSATIONS_PER_CHUNK);

    let mut report = SyncReport {
        chunks: chunks.len(),
        ..SyncReport::default()
    };
    for entry in &batch.blobs {
        let data = lfs.get(&entry.sha256)?;
        put_blob(base, token, &entry.sha256, &data)?;
        report.blobs_uploaded += 1;
    }

    let mut merged_results = Vec::new();
    let mut repo_id = String::new();
    for (index, chunk) in chunks.iter().enumerate() {
        let response = post_batch(base, token, chunk)?;
        tally(&response, &mut report);
        repo_id = response.repo_id.clone();
        merged_results.extend(response.results);
        on_chunk(index + 1, chunks.len());
    }
    report.repo_id = repo_id.clone();

    write_git_config(repo, SERVER_REPO_ID_KEY, &repo_id)?;
    Ok(SyncOutcome {
        report,
        response: SyncResponse {
            schema_version: SYNC_RESPONSE_SCHEMA.into(),
            repo_id,
            results: merged_results,
            metadata: Default::default(),
        },
    })
}

fn tally(response: &SyncResponse, report: &mut SyncReport) {
    use lineage_core::SyncObjectStatus::*;
    for result in &response.results {
        match result.status {
            Accepted => report.accepted += 1,
            Noop => report.noop += 1,
            Rejected => report.rejected += 1,
            Pending => report.pending += 1,
        }
    }
}

fn put_blob(base: &str, token: &str, sha256: &str, data: &[u8]) -> Result<(), LineageError> {
    let url = format!("{base}/v0/blobs/{sha256}");
    let response = ureq::put(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/octet-stream")
        .timeout(SYNC_TIMEOUT)
        .send_bytes(data)
        .map_err(|e| LineageError::Other(format!("blob upload failed: {e}")))?;
    if !(200..300).contains(&response.status()) {
        return Err(LineageError::Other(format!(
            "blob upload HTTP {}",
            response.status()
        )));
    }
    Ok(())
}

fn post_batch(base: &str, token: &str, batch: &SyncBatch) -> Result<SyncResponse, LineageError> {
    let url = format!("{base}/v0/sync");
    let response = ureq::post(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .timeout(SYNC_TIMEOUT)
        .send_json(serde_json::to_value(batch)?)
        .map_err(|e| LineageError::Other(format!("sync request failed: {e}")))?;
    if !(200..300).contains(&response.status()) {
        let status = response.status();
        let text = response.into_string().unwrap_or_default();
        return Err(LineageError::Other(format!("sync HTTP {status}: {text}")));
    }
    response
        .into_json::<SyncResponse>()
        .map_err(|e| LineageError::Other(format!("sync response parse failed: {e}")))
}

fn read_git_config(repo: &Repository, key: &str) -> Option<String> {
    let value = repo.config().ok()?.get_string(key).ok()?;
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn write_git_config(repo: &Repository, key: &str, value: &str) -> Result<(), LineageError> {
    repo.config()
        .and_then(|mut cfg| cfg.set_str(key, value))
        .map_err(|e| LineageError::Other(format!("failed to cache {key}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lineage_core::{AgentKind, Conversation, LineObject, SessionCommitLink};

    #[test]
    fn normalize_remote_url_strips_scheme_login_and_suffix() {
        assert_eq!(
            normalize_remote_url("https://github.com/Acme/Widgets.git"),
            "github.com/acme/widgets"
        );
        assert_eq!(
            normalize_remote_url("https://user:tok@github.com/Acme/Widgets.git/"),
            "github.com/acme/widgets"
        );
        assert_eq!(
            normalize_remote_url("git@github.com:Acme/Widgets.git"),
            "github.com/acme/widgets"
        );
    }

    #[test]
    fn chunk_batch_splits_by_conversation_and_keeps_dependents() {
        use lineage_core::{Confidence, LineageId};

        let binding = RepoBinding {
            normalized_remote_url: "github.com/acme/widgets".into(),
            root_commit_sha: "a".repeat(40),
            server_repo_id: None,
        };
        let mut batch = SyncBatch::new(binding);
        let c0 = Conversation::new(AgentKind::Claude, "/tmp/a");
        let c1 = Conversation::new(AgentKind::Claude, "/tmp/b");
        let c2 = Conversation::new(AgentKind::Claude, "/tmp/c");
        let id0 = c0.id.clone();
        let id1 = c1.id.clone();
        batch.conversations = vec![c0, c1, c2];
        batch.line_objects = vec![
            LineObject::new(
                "f.rs",
                [1, 2],
                "c".repeat(40),
                id0.clone(),
                LineageId::from("t0"),
                Confidence::Exact,
            ),
            LineObject::new(
                "g.rs",
                [3, 4],
                "d".repeat(40),
                id1.clone(),
                LineageId::from("t1"),
                Confidence::Exact,
            ),
        ];
        batch.session_commit_links = vec![SessionCommitLink {
            conversation_id: id0.clone(),
            commit_sha: "e".repeat(40),
            patch_id: None,
        }];
        batch.blobs = vec![BlobManifestEntry {
            sha256: "f".repeat(64),
            byte_size: 1,
            content_type: None,
        }];

        let chunks = chunk_batch(&batch, 2);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].conversations.len(), 2);
        assert_eq!(chunks[1].conversations.len(), 1);
        assert_eq!(chunks[0].line_objects.len(), 2);
        assert_eq!(chunks[1].line_objects.len(), 0);
        assert_eq!(chunks[0].session_commit_links.len(), 1);
        assert_eq!(chunks[1].session_commit_links.len(), 0);
        assert_eq!(chunks[0].blobs.len(), 1);
        assert!(chunks[1].blobs.is_empty());
    }
}
