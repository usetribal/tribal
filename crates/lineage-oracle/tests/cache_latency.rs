use lineage_oracle::{CacheKey, OracleCache, Retrieval};

/// Not a correctness test: measures cache-hit lookup latency for the plan's
/// budget check. Run on demand with `cargo test -p lineage-oracle --release
/// -- --ignored --nocapture`; never asserted in CI (wall-time asserts flake).
#[test]
#[ignore]
fn measure_cache_hit_latency() {
    let dir = tempfile::tempdir().unwrap();
    let cache = OracleCache::open(dir.path().join("oracle.db")).unwrap();
    let key = CacheKey {
        file_path: "src/auth.rs",
        file_blob_sha: "aa",
        corpus_generation: 1,
        retriever_version: "1",
    };
    cache.put(&key, &Retrieval::empty(), 0).unwrap();

    let iterations = 10_000;
    let started = std::time::Instant::now();
    for _ in 0..iterations {
        assert!(cache.get(&key).unwrap().is_some());
    }
    let elapsed = started.elapsed();
    eprintln!(
        "cache hit: {:?} avg over {iterations} lookups ({:?} total)",
        elapsed / iterations,
        elapsed
    );
}
