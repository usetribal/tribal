//! Retrieval spine: given a query, return ranked session evidence.
//!
//! This is the substrate the oracle (context injection), proactive recall, an
//! MCP search tool, and the future server retriever all consume — retrieval is
//! its own concern, not part of any one feature. The [`Retriever`] trait is
//! transport-neutral (in-process over local data now, a server endpoint later);
//! the evidence model ([`Evidence`], [`Retrieval`], [`Strength`]) and the cache
//! live here so no consumer redeclares them.

mod cache;
mod dense;
mod fts;
mod fusion;
mod local;
mod retriever;
mod session;
mod types;

pub use cache::{CacheKey, RetrievalCache};
pub use dense::{embed_and_store_session, DenseRetriever, DENSE_RETRIEVER_VERSION};
pub use fts::{FtsRetriever, FTS_RETRIEVER_VERSION};
pub use fusion::{FusedRetriever, DEFAULT_RRF_K};
pub use local::{LocalRetriever, LOCAL_RETRIEVER_VERSION};
pub use retriever::{IntentRetriever, Result, RetrievalError, Retriever};
pub use types::{
    strength_for, ContextQuery, Evidence, EvidenceTier, IntentQuery, Retrieval, Strength,
    CONTEXT_QUERY_SCHEMA, RETRIEVAL_SCHEMA,
};
