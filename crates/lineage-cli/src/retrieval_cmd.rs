//! Terminal-facing intent retrieval: `git lineage context query "<text>"`.
//!
//! Runs the retrieval spine (`lineage-retrieval`) against the repo's session
//! index and prints ranked session evidence. All three legs — lexical, dense,
//! and fused — are always available: the static model2vec embedder loads in
//! microseconds, so there is no build to opt into and no lexical-only fallback.
//! This is the hands-on surface for the intent-matching plateau — the
//! `UserPromptSubmit` hook wiring is separate.

use std::path::Path;

use lineage_embed::Model2VecEmbedder;
use lineage_git::open_repo;
use lineage_retrieval::{
    DenseRetriever, FtsRetriever, FusedRetriever, IntentQuery, IntentRetriever, Retrieval,
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

pub fn query(repo_path: &Path, text: &str, leg: Leg) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    let intent = IntentQuery {
        text: text.to_string(),
        budget_ms: None,
    };

    let fts = FtsRetriever::new(repo.inner(), &index);
    let retrieval = match leg {
        Leg::Lexical => fts.retrieve_intent(&intent)?,
        Leg::Dense => {
            let embedder = Model2VecEmbedder::new(embed_cache_dir())?;
            DenseRetriever::new(repo.inner(), &index, &embedder).retrieve_intent(&intent)?
        }
        Leg::Fused => {
            let embedder = Model2VecEmbedder::new(embed_cache_dir())?;
            let dense = DenseRetriever::new(repo.inner(), &index, &embedder);
            FusedRetriever::new(fts, dense).retrieve_intent(&intent)?
        }
    };
    print_retrieval(text, leg, &retrieval);
    Ok(())
}

fn print_retrieval(text: &str, leg: Leg, retrieval: &Retrieval) {
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
    }
    if retrieval.truncated {
        println!("  (truncated on budget)");
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
