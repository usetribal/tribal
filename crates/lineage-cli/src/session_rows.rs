//! Assembling the rows the session selector renders.

use std::path::Path;

use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use lineage_core::{display_title, opening_ask, Conversation, LineageId, SOURCE_MTIME_KEY};
use lineage_git::{
    list_session_ids, open_repo, read_conversation_stored, LineageRepo, PROMPTED_BY_NAME,
};
use lineage_select::{Origin, SessionRow};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

type StoredSnapshot = (HashMap<LineageId, DateTime<Utc>>, Vec<SessionRow>);

/// How much of a session's opening to keep for the row's context line. Wider
/// than any terminal, because the renderer is what knows the real budget.
const CONTEXT_CHARS: usize = 200;

/// Every session this repository stores, newest first.
///
/// Reads the stored shape rather than a hydrated one: the selector shows titles
/// and counts, and hydrating media for a list the user will mostly scroll past
/// costs far more than it shows.
pub fn collect_session_rows(repo_path: &Path) -> Result<Vec<SessionRow>> {
    let repo = open_repo(repo_path)?;
    Ok(collect_session_rows_from_repo(&repo))
}

/// One git walk: stored mtimes for flush skip logic and selector rows together.
pub fn stored_snapshot(repo: &LineageRepo) -> Result<StoredSnapshot> {
    let mut mtimes = HashMap::new();
    let mut rows = Vec::new();
    for id in list_session_ids(repo.inner())? {
        let Some(conv) = read_conversation_stored(repo.inner(), &id)? else {
            continue;
        };
        if let Some(mtime) = source_mtime(&conv) {
            mtimes.insert(id.clone(), mtime);
        }
        rows.push(row_from_conversation(&conv));
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.started_at));
    Ok((mtimes, rows))
}

pub fn collect_session_rows_from_repo(repo: &LineageRepo) -> Vec<SessionRow> {
    let mut rows = Vec::new();
    for id in list_session_ids(repo.inner()).unwrap_or_default() {
        let Ok(Some(conv)) = read_conversation_stored(repo.inner(), &id) else {
            continue;
        };
        rows.push(row_from_conversation(&conv));
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.started_at));
    rows
}

fn source_mtime(conv: &Conversation) -> Option<DateTime<Utc>> {
    conv.metadata
        .get(SOURCE_MTIME_KEY)
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

fn row_from_conversation(conv: &Conversation) -> SessionRow {
    SessionRow {
        id: conv.id.to_string(),
        title: display_title(conv),
        agent: conv.agent.as_str().to_string(),
        turns: conv.turns.len(),
        started_at: conv.started_at,
        duration: session_duration(conv),
        project: project_name(&conv.workspace_root),
        context: opening_ask(conv, CONTEXT_CHARS),
        origin: match conv.pull_origin {
            Some(_) => Origin::Received,
            None => Origin::Local,
        },
        prompted_by: conv
            .metadata
            .get(PROMPTED_BY_NAME)
            .and_then(|v| v.as_str())
            .map(String::from),
    }
}

/// How long the session ran, from its own turn timestamps.
///
/// Not `ended_at - started_at`: `started_at` is when the session was imported,
/// which for a session imported after the fact is later than its last turn and
/// yields a negative span. The turns carry the only times that describe the
/// conversation itself, and a session whose turns are unstamped has no
/// knowable duration rather than a zero one.
fn session_duration(conv: &lineage_core::Conversation) -> Option<Duration> {
    let first = conv.turns.iter().find_map(|turn| turn.timestamp)?;
    let last = conv
        .turns
        .iter()
        .rev()
        .find_map(|turn| turn.timestamp)
        .unwrap_or(first);
    let span = last.signed_duration_since(first);
    (span > Duration::zero()).then_some(span)
}

/// The last path segment of the workspace, which is what people call a project.
/// A root that ends in a separator or is empty names nothing.
fn project_name(workspace_root: &str) -> Option<String> {
    Path::new(workspace_root)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}
