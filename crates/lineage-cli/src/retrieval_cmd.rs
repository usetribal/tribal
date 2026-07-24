//! Terminal-facing intent retrieval: `git lineage context query "<text>"`.
//!
//! Runs the retrieval spine (`lineage-retrieval`) against the repo's session
//! index and prints ranked session evidence. All three legs — lexical, dense,
//! and fused — are always available: the static model2vec embedder loads in
//! microseconds, so there is no build to opt into and no lexical-only fallback.
//! This is the hands-on surface for the intent-matching plateau — the
//! `UserPromptSubmit` hook wiring is separate.

use std::path::Path;

use lineage_core::normalize_repo_path;
use lineage_embed::Model2VecEmbedder;
use lineage_git::{open_repo, resolve_anchor};
use lineage_retrieval::{
    affordances_for, fused_salient_turn_plan, line_anchored_temporal_plan, DenseRetriever,
    Evidence, FtsRetriever, FusedRetriever, IntentQuery, IntentRetriever, LineRef, PlanResult,
    Retrieval, StageTiming,
};
use lineage_search::LineageIndex;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Which retrieval leg to run. Selecting a leg is what lets you *see* the
/// lexical-vs-semantic difference by hand.
#[derive(Debug, Clone, Copy)]
pub enum Leg {
    Lexical,
    Dense,
    Fused,
}

/// Wall budget threaded through the plan runner, matching the hook's default
/// (context_cmd `DEFAULT_BUDGET_MS`). The by-hand `query` surface is not the
/// hook, but running it under the same budget is what makes its stage timings a
/// faithful preview of the hook path.
const DEFAULT_BUDGET_MS: u64 = 200;

/// A single-leg query bypasses the plan runner: the `--lexical` / `--dense`
/// flags exist precisely to see one leg in isolation, so wrapping them in the
/// fused plan would defeat their purpose. Only the default (fused) and the
/// `--file` (temporal) paths run through a plan.
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
        Leg::Fused => {
            let embedder = Model2VecEmbedder::new(embed_cache_dir())?;
            let dense = DenseRetriever::new(repo.inner(), &index, &embedder);
            let fused = FusedRetriever::new(fts, dense);
            let plan = fused_salient_turn_plan(&fused, &intent)?;
            print_plan_result(text, leg, &plan, timing);
        }
    }
    Ok(())
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
            return (normalize_repo_path(path, None), Some(line));
        }
    }
    (normalize_repo_path(file, None), None)
}

fn print_plan_result(query: &str, leg: Leg, plan: &PlanResult, timing: bool) {
    print_retrieval_with_affordances(query, leg, &plan.retrieval, plan.anchor_file.as_deref());
    if timing {
        print_timings(plan.timings.as_slice());
    }
}

fn print_timings(timings: &[StageTiming]) {
    println!("  timing:");
    for t in timings {
        println!("    {:<24} {} ms", t.name, t.elapsed_ms);
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
    println!(
        "query: {text:?}  leg: {leg:?}  strength: {:?}",
        retrieval.strength
    );
    if retrieval.evidence.is_empty() {
        println!("  (no matches — honest nothing)");
        return;
    }
    for (rank, e) in retrieval.evidence.iter().enumerate() {
        println!("  {}. [{:?}] {}", rank + 1, e.strength, e.attribution,);
        // The summary is multi-line; indent it so the ranked list stays readable.
        for line in e.summary.lines() {
            println!("       {line}");
        }
        print_affordances(e, anchor_file);
    }
    if retrieval.truncated {
        println!("  (truncated on budget)");
    }
}

/// The affordance footer: runnable `git lineage` commands for the graph edges
/// adjacent to this evidence (spec: Verbatim-turn digest — affordance pointers).
fn print_affordances(evidence: &Evidence, anchor_file: Option<&str>) {
    for cmd in affordances_for(evidence, anchor_file) {
        println!("       → {cmd}");
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
        println!("no indexed sessions — run `git lineage import` first");
        return Ok(());
    }

    println!(
        "salience breakdown: {} session(s), {} turn(s), {:.0}% indexed",
        indexed_fractions.len(),
        total_turns,
        indexed_turns as f64 * 100.0 / total_turns as f64,
    );
    for (class, (count, is_indexed)) in &counts {
        println!(
            "  {class:<12} {count:>7}  ({:>5.1}%)  indexed {}",
            *count as f64 * 100.0 / total_turns as f64,
            if *is_indexed { "yes" } else { "no" },
        );
    }
    indexed_fractions.sort_by(f64::total_cmp);
    let median = indexed_fractions[indexed_fractions.len() / 2];
    println!(
        "  median session keeps {:.0}% of turns indexed",
        median * 100.0
    );
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
