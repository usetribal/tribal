//! Terminal-facing intent retrieval: `git lineage context query "<text>"`.
//!
//! Runs the retrieval spine (`lineage-retrieval`) against the repo's session
//! index and prints ranked session evidence. The lexical leg always works; the
//! dense and fused legs need the `dense` feature (the ONNX model). This is the
//! hands-on surface for the intent-matching plateau — the `UserPromptSubmit`
//! hook wiring is separate.

use std::path::Path;

use lineage_git::open_repo;
use lineage_retrieval::{FtsRetriever, IntentQuery, IntentRetriever, Retrieval};
use lineage_search::LineageIndex;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Which retrieval leg to run. Selecting a leg is what lets you *see* the
/// lexical-vs-semantic difference by hand.
#[derive(Debug, Clone, Copy)]
pub enum Leg {
    Lexical,
    #[cfg(feature = "dense")]
    Dense,
    #[cfg(feature = "dense")]
    Fused,
}

#[cfg(not(feature = "dense"))]
pub fn query(repo_path: &Path, text: &str, leg: Leg) -> Result<()> {
    let repo = open_repo(repo_path)?;
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    let intent = IntentQuery {
        text: text.to_string(),
        budget_ms: None,
    };

    let retrieval = FtsRetriever::new(repo.inner(), &index).retrieve_intent(&intent)?;
    print_retrieval(text, leg, &retrieval);
    Ok(())
}

#[cfg(feature = "dense")]
pub fn query(repo_path: &Path, text: &str, leg: Leg) -> Result<()> {
    use lineage_embed::FastEmbedder;
    use lineage_retrieval::{DenseRetriever, FusedRetriever};

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
            let embedder = FastEmbedder::new(embed_cache_dir())?;
            DenseRetriever::new(repo.inner(), &index, &embedder).retrieve_intent(&intent)?
        }
        Leg::Fused => {
            let embedder = FastEmbedder::new(embed_cache_dir())?;
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
/// repo to compare its distribution.
pub fn salience_report(repo_path: &Path) -> Result<()> {
    use std::collections::BTreeMap;

    use lineage_core::turn_salience;
    use lineage_git::{hydrate_conversation, list_session_ids, read_conversation_stored};

    let repo = open_repo(repo_path)?;

    let mut counts: BTreeMap<&'static str, (usize, f32)> = BTreeMap::new();
    let mut total_turns = 0usize;
    let mut kept_fractions: Vec<f64> = Vec::new();
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
        let mut kept = 0usize;
        for turn in &conv.turns {
            let class = turn_salience(turn);
            let entry = counts.entry(class.as_str()).or_insert((0, class.weight()));
            entry.0 += 1;
            total_turns += 1;
            if class.weight() >= 1.0 {
                kept += 1;
            }
        }
        kept_fractions.push(kept as f64 / conv.turns.len() as f64);
    }

    if total_turns == 0 {
        println!("no indexed sessions — run `git lineage import` first");
        return Ok(());
    }

    println!(
        "salience breakdown: {} session(s), {} turn(s)",
        kept_fractions.len(),
        total_turns
    );
    for (class, (count, weight)) in &counts {
        println!(
            "  {class:<12} {count:>7}  ({:>5.1}%)  weight {weight}",
            *count as f64 * 100.0 / total_turns as f64,
        );
    }
    kept_fractions.sort_by(f64::total_cmp);
    let median = kept_fractions[kept_fractions.len() / 2];
    println!(
        "  median session keeps {:.0}% of turns at full weight",
        median * 100.0
    );
    Ok(())
}

/// Embed sessions that are not already embedded at the current model version,
/// storing their chunk vectors — the dense index pass. Run from import and
/// `rebuild-index` so `context query --dense` has vectors to search.
///
/// Incremental: sessions already embedded at the current version are skipped, so
/// a backfill pays only for new or model-changed sessions — steady-state cost is
/// the one session just imported, not the whole corpus. A no-op without the
/// `dense` feature.
#[cfg(feature = "dense")]
pub fn embed_all_sessions(repo_path: &Path) -> Result<usize> {
    use std::collections::HashSet;

    use lineage_embed::FastEmbedder;
    use lineage_git::{hydrate_conversation, list_session_ids, read_conversation_stored};
    use lineage_retrieval::{embed_and_store_session, DENSE_RETRIEVER_VERSION};

    let repo = open_repo(repo_path)?;
    let index = LineageIndex::open(repo.git_dir().join("lineage").join("index.db"))?;
    // The user is waiting on a backfill, so use more threads than the hook path.
    let embedder = FastEmbedder::new_for_backfill(embed_cache_dir())?;

    let already: HashSet<String> = index
        .sessions_embedded_at_version(DENSE_RETRIEVER_VERSION)?
        .into_iter()
        .collect();

    let mut embedded = 0usize;
    for id in list_session_ids(repo.inner())? {
        if already.contains(id.as_str()) {
            continue;
        }
        if let Some(mut conv) = read_conversation_stored(repo.inner(), &id)? {
            hydrate_conversation(repo.inner(), &mut conv)?;
            let chunks = embed_and_store_session(&index, &embedder, &conv)?;
            if chunks > 0 {
                embedded += 1;
            }
        }
    }
    Ok(embedded)
}

#[cfg(not(feature = "dense"))]
pub fn embed_all_sessions(_repo_path: &Path) -> Result<usize> {
    Ok(0)
}

/// Where the ONNX model is cached. A stable per-user location so the model
/// downloads once and is shared across repos, then runs offline. Overridable
/// via `LINEAGE_EMBED_CACHE`; falls back to `~/.cache/lineage/embed`, or the
/// current dir if home is unknown.
#[cfg(feature = "dense")]
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
