//! Pick a session for fork when the id is not supplied up front.

use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use lineage_core::{display_title, LineageId};
use lineage_git::{open_repo, resolve_session, ResolveError, SessionCandidate};
use lineage_search::{LineageIndex, SearchHit};
use lineage_select::{Outcome, Purpose};

use crate::flush::flush_and_collect_rows;
use crate::interactive::interactive;
use crate::session_search::RepoSessionSearch;
use crate::session_transcript::load_session_entries;
use crate::ui;
#[cfg(feature = "profile")]
use lineage_profiler::{emit_prep, profiling_enabled, CliPhase, PrepReport};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

type PrepResult =
    std::result::Result<(Vec<lineage_select::SessionRow>, Arc<RepoSessionSearch>), String>;

/// Backing out of the selector. An error for a caller that needed a session,
/// and an ordinary exit for one that was only browsing — so the two agree on
/// the wording rather than each inventing it.
const NO_SESSION_CHOSEN: &str = "no session chosen";

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
pub fn pick_interactively(repo_path: &Path, purpose: Purpose) -> Result<ForkPickResult> {
    pick_interactively_with(repo_path, purpose, |_| Ok(()))
}

pub fn browse(repo_path: &Path, open_on: Option<&str>) -> Result<()> {
    match browse_with(repo_path, open_on) {
        Ok(()) => Ok(()),
        Err(error) if error.to_string() == NO_SESSION_CHOSEN => Ok(()),
        Err(error) => Err(error),
    }
}

fn browse_with(repo_path: &Path, open_on: Option<&str>) -> Result<()> {
    open_selector(repo_path, Purpose::Browse, |_| Ok(()), open_on).map(|_| ())
}

pub fn pick_interactively_with<W>(
    repo_path: &Path,
    purpose: Purpose,
    share: W,
) -> Result<ForkPickResult>
where
    W: Fn(&str) -> std::result::Result<(), String> + Clone + Send + 'static,
{
    open_selector(repo_path, purpose, share, None)
}

fn open_selector<W>(
    repo_path: &Path,
    purpose: Purpose,
    share: W,
    open_on: Option<&str>,
) -> Result<ForkPickResult>
where
    W: Fn(&str) -> std::result::Result<(), String> + Clone + Send + 'static,
{
    let (prep_tx, prep_rx) = mpsc::channel();
    let repo = repo_path.to_path_buf();
    #[cfg(feature = "profile")]
    let profile = profiling_enabled();
    #[cfg(not(feature = "profile"))]
    let profile = false;
    let row_cache: Arc<Mutex<Option<Vec<lineage_select::SessionRow>>>> = Arc::new(Mutex::new(None));
    let cache_for_thread = row_cache.clone();

    thread::spawn(move || {
        let result = prepare_selector(&repo, profile);
        if let Ok((rows, _search)) = &result {
            *cache_for_thread.lock().expect("row cache") = Some(rows.clone());
        }
        let _ = prep_tx.send(result);
    });

    let repo_path = repo_path.to_path_buf();
    let load =
        move |session_id: &str| load_session_entries(&repo_path, session_id).unwrap_or_default();

    let outcome = lineage_select::select_while_preparing(purpose, prep_rx, load, share, open_on)?;
    let Outcome::Chose(session_id) = outcome else {
        return Err(NO_SESSION_CHOSEN.into());
    };

    let title = row_cache
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().cloned())
        .and_then(|rows| {
            rows.iter()
                .find(|row| row.id == session_id)
                .map(|row| row.title.clone())
        })
        .unwrap_or_default();
    Ok(ForkPickResult {
        session_id,
        title,
        candidates: vec![],
    })
}

fn prepare_selector(repo_path: &Path, profile: bool) -> PrepResult {
    #[cfg(feature = "profile")]
    let mut phases = profile.then(Vec::new);
    #[cfg(feature = "profile")]
    let total = profile.then(std::time::Instant::now);

    #[cfg(feature = "profile")]
    let flush_t = phases.as_ref().map(|_| std::time::Instant::now());
    #[cfg(not(feature = "profile"))]
    let _profile = profile;
    let (flush_report, rows) =
        flush_and_collect_rows(repo_path, &mut |_, _| {}).map_err(|e| e.to_string())?;
    #[cfg(feature = "profile")]
    {
        if let (Some(phases), Some(t)) = (&mut phases, flush_t) {
            phases.push(CliPhase {
                name: "flush_and_collect_rows",
                millis: t.elapsed().as_millis(),
                detail: Some(format!(
                    "imported={} skipped={} failed={} sessions={}",
                    flush_report.imported,
                    flush_report.skipped,
                    flush_report.failed,
                    rows.len()
                )),
            });
        }
    }
    #[cfg(not(feature = "profile"))]
    let _ = flush_report;

    if rows.is_empty() {
        return Err("no sessions in this repository — import or pull one first".into());
    }

    #[cfg(feature = "profile")]
    let search_t = phases.as_ref().map(|_| std::time::Instant::now());
    let search = RepoSessionSearch::open(repo_path).map_err(|e| e.to_string())?;
    #[cfg(feature = "profile")]
    if let (Some(phases), Some(t)) = (&mut phases, search_t) {
        phases.push(CliPhase {
            name: "repo_session_search_open",
            millis: t.elapsed().as_millis(),
            detail: Some(format!("fused={}", search.is_fused())),
        });
    }

    #[cfg(feature = "profile")]
    if let Some(start) = total {
        emit_prep(&PrepReport {
            total_millis: start.elapsed().as_millis(),
            phases: phases.unwrap_or_default(),
        });
    }

    Ok((rows, search))
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
    interactive()
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
