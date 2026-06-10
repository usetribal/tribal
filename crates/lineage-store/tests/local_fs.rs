use lineage_store::{LocalFsStore, ObjectStore};

#[test]
fn local_fs_store_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = LocalFsStore::new(dir.path()).unwrap();
    let data = b"local fs object";
    let obj = store.put(data).unwrap();
    assert!(store.exists(&obj.oid));
    assert_eq!(store.get(&obj.oid).unwrap(), data);
}
