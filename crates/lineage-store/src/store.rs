use lineage_core::LineageError;

pub type Result<T> = std::result::Result<T, LineageError>;

#[derive(Debug, Clone)]
pub struct StoredObject {
    pub oid: String,
    pub size: usize,
}

pub trait ObjectStore {
    fn put(&self, data: &[u8]) -> Result<StoredObject>;
    fn get(&self, oid: &str) -> Result<Vec<u8>>;
    fn exists(&self, oid: &str) -> bool;
}
