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
