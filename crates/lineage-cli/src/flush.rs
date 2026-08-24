//! Bring stored sessions up to date before something reads them.
//!
//! A selector over "the sessions this repo holds" is only honest if the session
//! the user just finished is among them. Transcripts land on disk continuously
//! but only reach refs at import, so anything that offers a choice over stored
//! sessions flushes first.

use std::path::Path;

use chrono::{DateTime, Utc};
use lineage_adapters::all_adapters;
use lineage_core::{derive_session_id, generate_architecture_summary, AgentKind, SOURCE_MTIME_KEY};
use lineage_git::{
    list_session_ids, open_repo, persist_import, read_conversation_stored, read_repo_config,
    stamp_prompted_by,
};
use lineage_policy::{apply_policy, is_private_session, policy_from_repo_config};

use crate::commands::index_persisted_sessions_best_effort;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// What a flush did, for a caller that wants to say so.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FlushReport {
    pub imported: usize,
    pub skipped: usize,
    /// Sessions whose transcript could not be read. Counted, never fatal.
    pub failed: usize,
}

/// Import any agent transcript newer than what is stored, then index it.
///
/// Prints nothing. Progress goes to `progress(done, total)` so a caller about to
/// take the alternate screen can render it, the same arrangement
/// `LineageIndex::rebuild_with_progress` uses to keep rendering out of a library.
///
/// Head is never linked: attaching sessions to whatever HEAD happens to be is a
/// commit-time act, and this runs before a read.
///
/// A transcript that fails to parse is counted and skipped rather than raised —
/// one malformed file must not stop the caller opening.
pub fn flush_sessions(
    repo_path: &Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<FlushReport> {
    let repo = open_repo(repo_path)?;
    let inner = repo.inner();
    let repo_config = read_repo_config(inner)?;
    let policy = policy_from_repo_config(&repo_config);
    let stored = stored_mtimes(&repo)?;

    // Discovery is per-adapter and cheap; the count is needed up front so
    // progress can be reported against a total rather than an unknown.
    let mut discovered = Vec::new();
    for (kind, adapter) in all_adapters(repo.workdir()) {
        discovered.push((kind, adapter.discover()?, adapter));
    }
    let total: usize = discovered
        .iter()
        .map(|(_, sessions, _)| sessions.len())
        .sum();
    progress(0, total);

    let mut report = FlushReport::default();
    let mut conversations = Vec::new();
    let mut done = 0usize;
    for (kind, sessions, adapter) in discovered {
        for session in sessions {
            done += 1;
            progress(done, total);

            let source_mtime = std::fs::metadata(&session.source_path)
                .and_then(|meta| meta.modified())
                .map(DateTime::<Utc>::from)
                .ok();
            if is_unchanged(kind, &session, source_mtime, &stored) {
                report.skipped += 1;
                continue;
            }

            match adapter.read(&session) {
                Ok(conv) => {
                    conversations.push(prepare(conv, &session, source_mtime, &repo_config, &policy))
                }
                Err(_) => report.failed += 1,
            }
        }
    }

    if conversations.is_empty() {
        return Ok(report);
    }

    for conv in &mut conversations {
        stamp_prompted_by(inner, conv)?;
    }
    let results = persist_import(inner, &conversations)?;
    report.imported = results.len();

    let ids: Vec<_> = conversations.iter().map(|c| c.id.clone()).collect();
    index_persisted_sessions_best_effort(&repo, &ids);
    Ok(report)
}

/// The transcript mtime each stored session was last read at. A file untouched
/// since then holds nothing new, so it is never parsed again.
fn stored_mtimes(
    repo: &lineage_git::LineageRepo,
) -> Result<std::collections::HashMap<lineage_core::LineageId, DateTime<Utc>>> {
    let mut mtimes = std::collections::HashMap::new();
    for id in list_session_ids(repo.inner())? {
        let Some(conv) = read_conversation_stored(repo.inner(), &id)? else {
            continue;
        };
        let stamped = conv
            .metadata
            .get(SOURCE_MTIME_KEY)
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        if let Some(mtime) = stamped {
            mtimes.insert(id, mtime);
        }
    }
    Ok(mtimes)
}

fn is_unchanged(
    kind: AgentKind,
    session: &lineage_agent::SessionRef,
    source_mtime: Option<DateTime<Utc>>,
    stored: &std::collections::HashMap<lineage_core::LineageId, DateTime<Utc>>,
) -> bool {
    let id = derive_session_id(kind, session.session_token());
    match (source_mtime, stored.get(&id)) {
        (Some(modified), Some(read_at)) => modified <= *read_at,
        _ => false,
    }
}

/// Stamp the source and its mtime, mark privacy, and run policy — the same
/// preparation `import` does, minus commit linking.
///
/// The mtime stamped is the one read *before* parsing, never a fresh stat: a
/// vendor appending mid-flush would otherwise record a time newer than the
/// content actually stored, and the next flush would skip turns it never saw.
fn prepare(
    mut conv: lineage_core::Conversation,
    session: &lineage_agent::SessionRef,
    source_mtime: Option<DateTime<Utc>>,
    repo_config: &lineage_core::LineageRepoConfig,
    policy: &lineage_policy::PolicyConfig,
) -> lineage_core::Conversation {
    let source = session.source_path.display().to_string();
    conv.metadata
        .insert("source".into(), serde_json::Value::String(source.clone()));
    if let Some(modified) = source_mtime {
        conv.metadata.insert(
            SOURCE_MTIME_KEY.into(),
            serde_json::Value::String(modified.to_rfc3339()),
        );
    }
    if is_private_session(&source, repo_config) {
        conv.private = true;
    }
    let summary = generate_architecture_summary(&conv);
    conv.metadata.insert(
        "architecture_summary".into(),
        serde_json::Value::String(summary),
    );
    apply_policy(policy, conv).conversation
}
