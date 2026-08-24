//! Pick a session for fork when the id is not supplied up front.

use std::io::{self, IsTerminal};
use std::path::Path;

use lineage_core::{display_title, LineageId};
use lineage_git::{open_repo, resolve_session, ResolveError, SessionCandidate};
use lineage_search::{LineageIndex, SearchHit};
use lineage_select::{Outcome, Purpose};

use crate::flush::flush_sessions;
use crate::session_rows::collect_session_rows;
use crate::session_search::RepoSessionSearch;
use crate::session_transcript::load_session_entries;
use crate::ui;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone)]
pub struct ForkPickOptions {
    pub session_id: Option<String>,
    pub query: Option<String>,
    pub pick: Option<usize>,
    /// What the chosen session is for. Travels to the selector, which decides
    /// what that makes unavailable and how to say so.
    pub purpose: Purpose,
}

impl Default for ForkPickOptions {
    fn default() -> Self {
        Self {
            session_id: None,
            query: None,
            pick: None,
            purpose: Purpose::Browse,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ForkCandidateView {
    pub index: usize,
    pub id: String,
    pub title: String,
    pub agent: String,
    pub turns: usize,
    pub started_at: String,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ForkPickResult {
    pub session_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<ForkCandidateView>,
}

/// Resolve which session to fork: explicit id, search query, or interactive list.
pub fn pick_fork_session(repo_path: &Path, options: &ForkPickOptions) -> Result<ForkPickResult> {
    if let Some(session_id) = options.session_id.as_deref() {
        let repo = open_repo(repo_path)?;
        let id = resolve_session(repo.inner(), session_id).map_err(format_resolve_error)?;
        let conv = lineage_git::read_conversation(repo.inner(), &id)?
            .ok_or_else(|| format!("session not found after resolve: {id}"))?;
        return Ok(ForkPickResult {
            session_id: id.to_string(),
            title: display_title(&conv),
            candidates: vec![],
        });
    }

    if let Some(query) = options.query.as_deref() {
        let (views, candidates) = search_candidates(repo_path, query)?;
        if candidates.is_empty() {
            return Err(format!("no sessions matched '{query}'").into());
        }
        let index = choose_index(&views, options.pick, query)?;
        let chosen = &candidates[index];
        return Ok(ForkPickResult {
            session_id: chosen.id.to_string(),
            title: chosen.title.clone(),
            candidates: views,
        });
    }

    if stdin_is_tty() {
        return pick_interactively(repo_path, options.purpose);
    }

    Err(
        "session id required — pass one, use --query to search, or run from a terminal to pick"
            .into(),
    )
}

/// Open the selector over this repository's sessions.
///
/// Flushes first so a session the user has just finished is on the list: the
/// transcript is on disk continuously but only reaches refs at import, and a
/// picker that cannot offer the session someone just left is the whole problem
/// this replaced.
pub fn pick_interactively(repo_path: &Path, purpose: Purpose) -> Result<ForkPickResult> {
    pick_interactively_with(repo_path, purpose, |_| Ok(()))
}

/// As [`pick_interactively`], with `share` performing the work the confirmation
/// commits to, so it runs under the animation rather than after it.
pub fn pick_interactively_with<W>(
    repo_path: &Path,
    purpose: Purpose,
    share: W,
) -> Result<ForkPickResult>
where
    W: Fn(&str) -> std::result::Result<(), String> + Clone + Send + 'static,
{
    let _ = flush_sessions(repo_path, &mut |_, _| {})?;

    let rows = collect_session_rows(repo_path)?;
    if rows.is_empty() {
        return Err("no sessions in this repository — import or pull one first".into());
    }

    let search = RepoSessionSearch::open(repo_path)?;
    let leg = if search.is_fused() { "fused" } else { "lex" };
    let outcome = lineage_select::select_with(
        rows.clone(),
        purpose,
        search,
        leg,
        |session_id| {
            // A session that cannot be read shows as empty rather than failing
            // the pick: the list is still usable, and the pane says so.
            load_session_entries(repo_path, session_id).unwrap_or_default()
        },
        share,
    )?;
    let Outcome::Chose(session_id) = outcome else {
        return Err("no session chosen".into());
    };
    let title = rows
        .iter()
        .find(|row| row.id == session_id)
        .map(|row| row.title.clone())
        .unwrap_or_default();
    Ok(ForkPickResult {
        session_id,
        title,
        candidates: vec![],
    })
}

fn search_candidates(
    repo_path: &Path,
    query: &str,
) -> Result<(Vec<ForkCandidateView>, Vec<SessionCandidate>)> {
    let repo = open_repo(repo_path)?;
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    let mut hits = index.search(query, 40)?;
    if hits.is_empty() {
        let _ = index.rebuild(repo.inner());
        hits = index.search(query, 40)?;
    }
    Ok(fold_search_hits(repo.inner(), &hits))
}

fn fold_search_hits(
    inner: &git2::Repository,
    hits: &[SearchHit],
) -> (Vec<ForkCandidateView>, Vec<SessionCandidate>) {
    let mut seen = Vec::new();
    let mut candidates = Vec::new();
    let mut views = Vec::new();
    for hit in hits {
        if seen.iter().any(|id: &String| id == &hit.session_id) {
            continue;
        }
        seen.push(hit.session_id.clone());
        let id = LineageId::from(hit.session_id.as_str());
        if let Ok(Some(conv)) = lineage_git::read_conversation(inner, &id) {
            let candidate = SessionCandidate {
                id: id.clone(),
                title: display_title(&conv),
            };
            views.push(ForkCandidateView {
                index: views.len() + 1,
                id: candidate.id.to_string(),
                title: candidate.title.clone(),
                agent: conv.agent.as_str().to_string(),
                turns: conv.turns.len(),
                started_at: conv.started_at.to_rfc3339(),
                score: Some(hit.score),
            });
            candidates.push(candidate);
        }
    }
    (views, candidates)
}

fn choose_index(views: &[ForkCandidateView], pick: Option<usize>, query: &str) -> Result<usize> {
    match pick {
        Some(one_based) if one_based >= 1 && one_based <= views.len() => Ok(one_based - 1),
        Some(one_based) => Err(format!(
            "--pick {one_based} is out of range ({} candidate(s))",
            views.len()
        )
        .into()),
        None if views.len() == 1 => Ok(0),
        None => {
            print_candidates(views);
            Err(format!(
                "'{query}' matched {} sessions — re-run with --pick N",
                views.len()
            )
            .into())
        }
    }
}

pub fn print_candidates(views: &[ForkCandidateView]) {
    for view in views {
        let score = view
            .score
            .map(|value| format!("  score={value:.2}"))
            .unwrap_or_default();
        ui::indent(format!(
            "{} {}  {}  {} turns  {}{}",
            ui::rank_label(view.index),
            ui::accent(&view.title),
            ui::dim(&view.id),
            view.turns,
            ui::day(&view.started_at),
            ui::dim(&score)
        ));
    }
}

fn format_resolve_error(error: ResolveError) -> Box<dyn std::error::Error> {
    error.into()
}

fn stdin_is_tty() -> bool {
    io::stdin().is_terminal()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_index_requires_pick_when_multiple() {
        let views = vec![
            ForkCandidateView {
                index: 1,
                id: "a".into(),
                title: "One".into(),
                agent: "claude".into(),
                turns: 1,
                started_at: "2026-07-26".into(),
                score: None,
            },
            ForkCandidateView {
                index: 2,
                id: "b".into(),
                title: "Two".into(),
                agent: "claude".into(),
                turns: 2,
                started_at: "2026-07-26".into(),
                score: None,
            },
        ];
        assert!(choose_index(&views, None, "auth").is_err());
        assert_eq!(choose_index(&views, Some(2), "auth").unwrap(), 1);
    }
}
