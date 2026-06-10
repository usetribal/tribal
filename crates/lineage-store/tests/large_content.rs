use lineage_store::{LargeBlobBackend, LargeContentStore};

#[test]
fn externalize_and_get_lfs_backend() {
    let dir = tempfile::tempdir().unwrap();
    let store = LargeContentStore::new(dir.path(), LargeBlobBackend::Lfs);
    let content = "z".repeat(5000);
    let (compact, blob_ref) = store.maybe_externalize(&content, 100);
    assert!(compact.starts_with("[blob:"));
    let blob_ref = blob_ref.expect("blob ref");
    let loaded = store.get(&blob_ref).unwrap();
    assert_eq!(String::from_utf8(loaded).unwrap(), content);
}

#[test]
fn cache_backend_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = LargeContentStore::new(dir.path(), LargeBlobBackend::Cache);
    let content = "c".repeat(500);
    let (compact, blob_ref) = store.maybe_externalize(&content, 100);
    assert!(compact.starts_with("[blob:"));
    let loaded = store.get(&blob_ref.unwrap()).unwrap();
    assert_eq!(String::from_utf8(loaded).unwrap(), content);
}
