//! Terminal-facing intent retrieval: `tribal context query "<text>"`.
//!
//! Runs the retrieval spine (`lineage-retrieval`) against the repo's session
//! index and prints ranked session evidence. All three legs — lexical, dense,
//! and fused — are always available: the static model2vec embedder loads in
//! microseconds, so there is no build to opt into and no lexical-only fallback.
//! This is the hands-on surface for the intent-matching plateau — the
//! `UserPromptSubmit` hook wiring is separate.

use std::path::Path;

use chrono::Utc;
use lineage_core::normalize_repo_path_unscoped;
use lineage_embed::Model2VecEmbedder;
use lineage_git::{open_repo, resolve_anchor};
use lineage_retrieval::{
    fused_salient_turn_plan, line_anchored_temporal_plan, line_objects_of_turn, route,
    search_within_sessions, sessions_for_commit, turn_neighbourhood, DenseRetriever, Evidence,
    FtsRetriever, FusedRetriever, IntentQuery, IntentRetriever, LineRef, Plan, PlanResult,
    Retrieval, RouteDecision, StageTiming, Strength,
};
use lineage_search::LineageIndex;

use crate::digest::{affordances_for, turn_handle};
use crate::events::{EventLog, Outcome};
use crate::ui;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Which retrieval leg to run. Selecting a leg is what lets you *see* the
/// lexical-vs-semantic difference by hand. `Default` is the no-flag case: it
/// skips leg selection and hands the query to the dispatcher, which routes to the
/// temporal or fused plan. The three explicit legs bypass the dispatcher, so a
/// hand-chosen leg always runs exactly as asked.
#[derive(Debug, Clone, Copy)]
pub enum Leg {
    Default,
    Lexical,
    Dense,
    Fused,
}

impl Leg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Default => "auto",
            Self::Lexical => "lexical",
            Self::Dense => "dense",
            Self::Fused => "fused",
        }
    }
}

/// Wall budget threaded through the plan runner, matching the hook's default
/// (context_cmd `DEFAULT_BUDGET_MS`). The by-hand `query` surface is not the
/// hook, but running it under the same budget is what makes its stage timings a
/// faithful preview of the hook path.
const DEFAULT_BUDGET_MS: u64 = 200;

/// Precedence (documented in `--help`): `--file` forces the temporal plan on
/// that anchor; `--lexical`/`--dense`/`--fused` force one leg and bypass the
/// dispatcher (the flags exist precisely to see a leg in isolation, so wrapping
/// them in a plan would defeat their purpose); with none of those, the dispatcher
/// routes the free text to the temporal or fused plan. Only the dispatched and
/// `--file` paths run a plan; the explicit legs run bare.
pub fn query(
    repo_path: &Path,
    text: &str,
    leg: Leg,
    file: Option<&str>,
    timing: bool,
) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;

    if let Some(file) = file {
        return temporal_query(&repo, &index, file, text, timing);
    }
    if let Leg::Default = leg {
        return dispatched_query(&repo, &index, text, timing);
    }

    let intent = IntentQuery {
        text: text.to_string(),
        budget_ms: Some(DEFAULT_BUDGET_MS),
    };
    let fts = FtsRetriever::new(repo.inner(), &index);
    match leg {
        Leg::Lexical => {
            print_retrieval(text, leg, &fts.retrieve_intent(&intent)?);
        }
        Leg::Dense => {
            let embedder = Model2VecEmbedder::new(embed_cache_dir())?;
            let retrieval =
                DenseRetriever::new(repo.inner(), &index, &embedder).retrieve_intent(&intent)?;
            print_retrieval(text, leg, &retrieval);
        }
        Leg::Fused | Leg::Default => {
            let embedder = Model2VecEmbedder::new(embed_cache_dir())?;
            let dense = DenseRetriever::new(repo.inner(), &index, &embedder);
            let fused = FusedRetriever::new(fts, dense);
            let plan = fused_salient_turn_plan(&fused, &intent)?;
            print_plan_result(text, leg, &plan, timing);
        }
    }
    Ok(())
}

/// The no-flag path: the dispatcher routes the free text, the decision is logged,
/// and the chosen plan runs. A temporal route behaves exactly as if `--file` had
/// been passed on the matched anchor (same `temporal_query`), so the dispatcher
/// only *selects*; the plan code is unchanged.
fn dispatched_query(
    repo: &lineage_git::LineageRepo,
    index: &LineageIndex,
    text: &str,
    timing: bool,
) -> Result<()> {
    let decision = route(text, index, repo.workdir());
    log_route(repo, text, &decision);
    if timing {
        print_route(&decision);
    }

    match decision.plan {
        Plan::Temporal => {
            // The dispatcher only routes temporal on a hit-tested anchor, so this
            // always resolves; `temporal_query` re-parses the anchor exactly as
            // the `--file` path does.
            let anchor = decision.matched_anchor.as_deref().unwrap_or(text);
            temporal_query(repo, index, anchor, "", timing)
        }
        Plan::Fused => {
            let intent = IntentQuery {
                text: text.to_string(),
                budget_ms: Some(DEFAULT_BUDGET_MS),
            };
            let embedder = Model2VecEmbedder::new(embed_cache_dir())?;
            let fts = FtsRetriever::new(repo.inner(), index);
            let dense = DenseRetriever::new(repo.inner(), index, &embedder);
            let fused = FusedRetriever::new(fts, dense);
            let plan = fused_salient_turn_plan(&fused, &intent)?;
            print_plan_result(text, Leg::Default, &plan, timing);
            Ok(())
        }
    }
}

/// The routing decision goes to the event log under `context_query`, best-effort
/// (a failed log write never fails the query — the same discipline as
/// `context_hook`). The plan, matched anchor, and signals are what makes the
/// route auditable after the fact.
fn log_route(repo: &lineage_git::LineageRepo, text: &str, decision: &RouteDecision) {
    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "context_query",
        Outcome::Ok,
        serde_json::json!({
            "query": text,
            "plan": decision.plan.as_str(),
            "anchor": decision.matched_anchor,
            "signals": decision.signals,
        }),
    );
}

/// A traversal verb run goes to the event log under `context_traversal`,
/// best-effort like every other event write. Without this the four verbs are
/// the only agent-facing lineage operations that leave no trace: `context log`
/// and doctor's activity section can show that context was *injected* but not
/// that the agent went on to follow it, which is the one thing the handle
/// round-trip makes observable.
///
/// `relation` is the abstract verb name from `lineage-retrieval::VERBS`, not the
/// CLI spelling, so an MCP-issued traversal logs identically to a shelled one.
fn log_traversal(repo: &lineage_git::LineageRepo, relation: &str, handle: &str, results: usize) {
    EventLog::for_git_dir(&repo.git_dir()).append(
        Utc::now(),
        "context_traversal",
        Outcome::Ok,
        serde_json::json!({
            "relation": relation,
            "handle": handle,
            "session_ids": traversal_session_ids(relation, handle),
            "results": results,
        }),
    );
}

/// The session(s) a traversal addressed, so a consumer can tie it back to an
/// injection without re-deriving handle syntax.
///
/// The shapes differ per verb and none of them is a `session#turn` handle:
/// `search-within` takes a comma-joined session list, `sessions-for-commit` a
/// commit sha (no session), and the turn-addressed verbs take a turn id, which
/// is `{conversation_id}-{turn_index}` — so the session is the part before the
/// last dash, not a `#` split.
fn traversal_session_ids(relation: &str, handle: &str) -> Vec<String> {
    match relation {
        "search-within" => handle
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect(),
        "sessions-for-commit" => Vec::new(),
        _ => match handle.rsplit_once('-') {
            Some((session_id, turn_index)) if turn_index.chars().all(|c| c.is_ascii_digit()) => {
                vec![session_id.to_string()]
            }
            _ => vec![handle.to_string()],
        },
    }
}

/// The `--timing` route line, e.g.
/// `route: temporal (anchor README.md, signals: path-token+line-objects-hit)`.
fn print_route(decision: &RouteDecision) {
    let anchor = decision
        .matched_anchor
        .as_deref()
        .map(|a| format!(" (anchor {a})"))
        .unwrap_or_default();
    ui::indent(format!(
        "{} {}{anchor}  signals: {}",
        ui::dim("route:"),
        ui::accent(decision.plan.as_str()),
        ui::dim(decision.signals.join("+")),
    ));
}

/// The line-anchored temporal plan: `--file <path>[:<line>]`. One live blame
/// anchors the line at HEAD; the plan walks from there over the index tables.
/// When `text` is non-empty it re-ranks the anchored turns by FTS score.
fn temporal_query(
    repo: &lineage_git::LineageRepo,
    index: &LineageIndex,
    file: &str,
    text: &str,
    timing: bool,
) -> Result<()> {
    let (file_path, line) = parse_file_anchor(file);
    let head = repo
        .inner()
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id().to_string())
        .unwrap_or_default();

    // Resolve HEAD → introducing commit for the anchor line (the one allowed
    // live blame). A file-level anchor (no line) uses line 1 as the seed for the
    // blame; the plan still aggregates every line object for the file.
    let anchor_line = line.unwrap_or(1);
    let anchor = match resolve_anchor(repo.inner(), &head, &file_path, anchor_line)? {
        Some((commit, orig_line)) => LineRef {
            file_path: file_path.clone(),
            line: orig_line,
            commit_sha: commit,
        },
        // No blame at HEAD: the file/line has no committed history to walk, but
        // the line_objects aggregation can still answer from HEAD directly.
        None => LineRef {
            file_path: file_path.clone(),
            line: anchor_line,
            commit_sha: head,
        },
    };

    let text = (!text.is_empty()).then_some(text);
    let plan = line_anchored_temporal_plan(
        repo.inner(),
        index,
        &anchor,
        line,
        text,
        Some(DEFAULT_BUDGET_MS),
    )?;
    print_plan_result(
        &format!("{file_path}:{anchor_line}"),
        Leg::Fused,
        &plan,
        timing,
    );
    Ok(())
}

/// `<path>` or `<path>:<line>`. A trailing `:<number>` is the line; anything
/// else is treated as part of the path (Windows drive letters, colons in names).
fn parse_file_anchor(file: &str) -> (String, Option<u32>) {
    if let Some((path, line_str)) = file.rsplit_once(':') {
        if let Ok(line) = line_str.parse::<u32>() {
            return (normalize_repo_path_unscoped(path, None), Some(line));
        }
    }
    (normalize_repo_path_unscoped(file, None), None)
}

fn print_plan_result(query: &str, leg: Leg, plan: &PlanResult, timing: bool) {
    print_retrieval_with_affordances(query, leg, &plan.retrieval, plan.anchor_file.as_deref());
    if timing {
        print_timings(plan.timings.as_slice());
    }
}

fn print_timings(timings: &[StageTiming]) {
    ui::indent(ui::dim("timing:"));
    for t in timings {
        ui::indent(format!(
            "  {} {}",
            ui::dim(format!("{:<24}", t.name)),
            ui::accent(format!("{} ms", t.elapsed_ms))
        ));
    }
}

fn print_retrieval(text: &str, leg: Leg, retrieval: &Retrieval) {
    print_retrieval_with_affordances(text, leg, retrieval, None);
}

fn print_retrieval_with_affordances(
    text: &str,
    leg: Leg,
    retrieval: &Retrieval,
    anchor_file: Option<&str>,
) {
    ui::kv("Query", text);
    ui::kv("Leg", leg.as_str());
    ui::kv("Strength", strength_name(retrieval.strength));
    if retrieval.evidence.is_empty() {
        ui::empty("no matches — honest nothing");
        return;
    }
    for (rank, e) in retrieval.evidence.iter().enumerate() {
        if rank > 0 {
            ui::blank();
        }
        ui::ranked_hit(rank + 1, strength_name(e.strength), &e.attribution);
        print_summary_lines(&e.summary);
        print_affordances(e, anchor_file);
    }
    if retrieval.truncated {
        ui::empty("truncated on budget");
    }
}

/// The affordance footer: runnable `tribal` commands for the graph edges
/// adjacent to this evidence (spec: Verbatim-turn digest — affordance pointers).
fn print_affordances(evidence: &Evidence, anchor_file: Option<&str>) {
    for cmd in affordances_for(evidence, anchor_file) {
        ui::affordance(cmd);
    }
}

/// `tribal context search-within <session-id>... <text>` — scoped FTS: the
/// repair for "right sessions, wrong turns".
pub fn search_within(
    repo_path: &Path,
    session_ids: &[String],
    text: &str,
    limit: usize,
) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    let evidence = search_within_sessions(repo.inner(), &index, session_ids, text, limit)?;
    log_traversal(
        &repo,
        "search-within",
        &session_ids.join(","),
        evidence.get().len(),
    );
    print_turn_evidence(
        &format!("search-within {} session(s): {text:?}", session_ids.len()),
        evidence.get(),
    );
    Ok(())
}

/// `tribal context around <turn-id>` — the turns either side of one turn,
/// in conversation order: the repair for "right turn, missing its argument".
pub fn around(repo_path: &Path, turn_id: &str, radius: u32, limit: usize) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    let evidence = turn_neighbourhood(repo.inner(), &index, turn_id, radius, limit)?;
    log_traversal(&repo, "around", turn_id, evidence.get().len());
    print_turn_evidence(&format!("around {turn_id} (±{radius})"), evidence.get());
    Ok(())
}

/// `tribal context produced-by <turn-id>` — the code a turn produced. Refs
/// only, so no privacy gate is needed and none is claimed.
pub fn produced_by(repo_path: &Path, turn_id: &str, limit: usize) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    let produced = line_objects_of_turn(&index, turn_id, limit)?;
    // Logged before the empty-result return: an honest-nothing traversal is
    // still a traversal the agent chose to make.
    log_traversal(&repo, "produced-by", turn_id, produced.len());

    ui::heading(&format!("produced-by {turn_id}"));
    if produced.is_empty() {
        ui::empty("no code attributed to this turn — honest nothing");
        return Ok(());
    }
    for line in &produced {
        ui::indent(format!(
            "{}  {}  [{}]",
            ui::accent(format!(
                "{}:{}-{}",
                line.anchor.file_path, line.line_range[0], line.line_range[1]
            )),
            ui::dim(short(&line.anchor.commit_sha)),
            ui::confidence_label(line.confidence),
        ));
    }
    Ok(())
}

/// `tribal context sessions-for-commit <sha>` — the sessions behind a
/// commit: the one verb whose entry point is ordinary git work.
pub fn sessions_for_commit_cmd(repo_path: &Path, commit_sha: &str, limit: usize) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    // A short sha from `git log` must resolve like it does everywhere else in
    // git; the mirror is keyed by full sha.
    let full_sha = repo
        .inner()
        .revparse_single(commit_sha)
        .map(|obj| obj.id().to_string())
        .unwrap_or_else(|_| commit_sha.to_string());
    let sessions = sessions_for_commit(repo.inner(), &index, &full_sha, limit)?;
    log_traversal(
        &repo,
        "sessions-for-commit",
        &full_sha,
        sessions.get().len(),
    );

    ui::heading(&format!("sessions-for-commit {}", short(&full_sha)));
    if sessions.get().is_empty() {
        ui::empty("no sessions linked to this commit — honest nothing");
        return Ok(());
    }
    for session in sessions.get() {
        ui::row(&session.session_id, &session.attribution);
    }
    Ok(())
}

/// The shared rendering for the two text-returning verbs: a `session#turn`
/// handle per entry so the agent can address what it found, then the turn's own
/// words.
fn print_summary_lines(summary: &str) {
    for line in summary.lines() {
        let clean = lineage_core::humanize_text(line);
        if clean.is_empty() {
            continue;
        }
        ui::indent(format!("     {clean}"));
    }
}

fn print_turn_evidence(header: &str, evidence: &[Evidence]) {
    ui::heading(header);
    if evidence.is_empty() {
        ui::empty("no matches — honest nothing");
        return;
    }
    for entry in evidence {
        ui::indent(format!(
            "{}  {}",
            ui::accent(turn_handle(entry)),
            ui::dim(&entry.attribution)
        ));
        print_summary_lines(&entry.summary);
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(9)]
}

fn strength_name(strength: Strength) -> &'static str {
    match strength {
        Strength::None => "none",
        Strength::Low => "low",
        Strength::Medium => "medium",
        Strength::High => "high",
    }
}

/// Per-corpus breakdown of the v0 salience rules: how many turns land in each
/// class and what fraction of the corpus indexing keeps. This is the
/// reproducibility surface for the measured baseline the rules were designed
/// against (docs/plans/xifong/enhanced-semantic-retrieval) — run it on any
/// repo to compare its distribution. Salience is binary now, so the report
/// shows per-class counts plus whether each class is indexed, and the summary
/// is the % of turns that reach the index.
pub fn salience_report(repo_path: &Path) -> Result<()> {
    use std::collections::BTreeMap;

    use lineage_core::turn_salience;
    use lineage_git::{hydrate_conversation, list_session_ids, read_conversation_stored};

    let repo = open_repo(repo_path)?;

    // Per class: (count, is_indexed).
    let mut counts: BTreeMap<&'static str, (usize, bool)> = BTreeMap::new();
    let mut total_turns = 0usize;
    let mut indexed_turns = 0usize;
    let mut indexed_fractions: Vec<f64> = Vec::new();
    for id in list_session_ids(repo.inner())? {
        let Some(mut conv) = read_conversation_stored(repo.inner(), &id)? else {
            continue;
        };
        // Hydrated content: the explore rule reads prose length, and large
        // turn bodies may be compacted out of the stored blob.
        hydrate_conversation(repo.inner(), &mut conv)?;
        if conv.turns.is_empty() {
            continue;
        }
        let mut indexed = 0usize;
        for turn in &conv.turns {
            let class = turn_salience(turn);
            let entry = counts
                .entry(class.as_str())
                .or_insert((0, class.is_salient()));
            entry.0 += 1;
            total_turns += 1;
            if class.is_salient() {
                indexed += 1;
                indexed_turns += 1;
            }
        }
        indexed_fractions.push(indexed as f64 / conv.turns.len() as f64);
    }

    if total_turns == 0 {
        ui::empty("no indexed sessions — run `tribal import` first");
        return Ok(());
    }

    ui::heading(&format!(
        "salience breakdown: {} session(s), {} turn(s), {:.0}% indexed",
        indexed_fractions.len(),
        total_turns,
        indexed_turns as f64 * 100.0 / total_turns as f64,
    ));
    for (class, (count, is_indexed)) in &counts {
        let indexed = if *is_indexed {
            ui::ok("yes")
        } else {
            ui::caution("no")
        };
        ui::indent(format!(
            "{} {:>7}  ({:>5.1}%)  indexed {indexed}",
            ui::accent(format!("{class:<12}")),
            count,
            *count as f64 * 100.0 / total_turns as f64,
        ));
    }
    indexed_fractions.sort_by(f64::total_cmp);
    let median = indexed_fractions[indexed_fractions.len() / 2];
    ui::indent(format!(
        "median session keeps {:.0}% of turns indexed",
        median * 100.0
    ));
    Ok(())
}

/// Embed sessions that are not already embedded at the current model version,
/// storing their chunk vectors — the dense index pass. Opted into via
/// `rebuild embeddings` (and `rebuild [index] --embed`) so `context query
/// --dense` has vectors to search.
///
/// Incremental: sessions already embedded at the current version are skipped, so
/// a backfill pays only for new or model-changed sessions — steady-state cost is
/// the one session just imported, not the whole corpus.
///
/// `progress(done, remaining)` is called once as `(0, remaining)` before the
/// loop (`remaining` counts only sessions still needing embedding, so
/// already-current sessions never inflate the bar) and again after each
/// embedded session, so the CLI can drive a progress bar without this crate
/// depending on a rendering library.
pub fn embed_all_sessions(
    repo_path: &Path,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<usize> {
    use std::collections::HashSet;

    use lineage_git::{hydrate_conversation, list_session_ids, read_conversation_stored};
    use lineage_retrieval::{embed_and_store_session, DENSE_RETRIEVER_VERSION};

    let repo = open_repo(repo_path)?;
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;

    let already: HashSet<String> = index
        .sessions_embedded_at_version(DENSE_RETRIEVER_VERSION)?
        .into_iter()
        .collect();

    let pending: Vec<_> = list_session_ids(repo.inner())?
        .into_iter()
        .filter(|id| !already.contains(id.as_str()))
        .collect();

    // Loading the model reads a ~130 MB matrix; skip it (and the bar) when
    // nothing is due to embed.
    if pending.is_empty() {
        return Ok(0);
    }

    let embedder = Model2VecEmbedder::new(embed_cache_dir())?;

    let remaining = pending.len();
    progress(0, remaining);
    let mut embedded = 0usize;
    for id in pending {
        if let Some(mut conv) = read_conversation_stored(repo.inner(), &id)? {
            hydrate_conversation(repo.inner(), &mut conv)?;
            let chunks = embed_and_store_session(&index, &embedder, &conv)?;
            if chunks > 0 {
                embedded += 1;
            }
        }
        progress(embedded, remaining);
    }
    Ok(embedded)
}

/// Where the model is cached. A stable per-user location so the model downloads
/// once and is shared across repos, then runs offline. Overridable via
/// `LINEAGE_EMBED_CACHE`; falls back to `~/.cache/lineage/embed`, or the current
/// dir if home is unknown.
fn embed_cache_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("LINEAGE_EMBED_CACHE") {
        return std::path::PathBuf::from(dir);
    }
    match std::env::var("HOME") {
        Ok(home) => std::path::Path::new(&home)
            .join(".cache")
            .join("lineage")
            .join("embed"),
        Err(_) => std::path::PathBuf::from(".lineage-embed-cache"),
    }
}
