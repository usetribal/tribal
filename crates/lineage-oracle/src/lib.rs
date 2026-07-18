mod cache;
mod local;
mod retriever;
mod types;

pub use cache::{CacheKey, OracleCache};
pub use local::{LocalRetriever, LOCAL_RETRIEVER_VERSION};
pub use retriever::{OracleError, Result, Retriever};
pub use types::{
    strength_for, ContextQuery, Evidence, EvidenceTier, Retrieval, Strength, CONTEXT_QUERY_SCHEMA,
    RETRIEVAL_SCHEMA,
};
