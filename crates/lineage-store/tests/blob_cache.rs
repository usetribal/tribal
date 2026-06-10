use lineage_store::BlobCache;

#[test]
fn blob_cache_round_trip_and_externalize() {
    let dir = tempfile::tempdir().unwrap();
    let cache = BlobCache::new(dir.path());
    let data = b"hello lineage cache";
    let blob_ref = cache.put(data).unwrap();
    assert_eq!(cache.get(&blob_ref).unwrap(), data);

    let (inline, external) = cache.maybe_externalize("short", 100);
    assert_eq!(inline, "short");
    assert!(external.is_none());

    let big = "x".repeat(10);
    let (placeholder, external) = cache.maybe_externalize(&big, 5);
    assert!(placeholder.starts_with("[blob:"));
    assert!(external.is_some());
}
