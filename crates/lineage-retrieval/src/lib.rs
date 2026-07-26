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
mod dispatch;
mod fts;
mod fusion;
mod local;
mod plan;
mod primitives;
mod retriever;
mod session;
mod types;
mod verbs;

pub use cache::{is_cacheable, CacheKey, RetrievalCache};
pub use dense::{embed_and_store_session, DenseRetriever, DENSE_RETRIEVER_VERSION};
pub use dispatch::{route, Plan, RouteDecision};
pub use fts::{FtsRetriever, FTS_RETRIEVER_VERSION};
pub use fusion::{FusedRetriever, DEFAULT_RRF_K};
pub use local::{LocalRetriever, LOCAL_RETRIEVER_VERSION};
pub use plan::{
    fused_salient_turn_plan, line_anchored_temporal_plan, PlanResult, PlanRun, StageTiming,
};
pub use primitives::{
    line_objects_of_turn, materialize_turns, search_within_sessions, sessions_for_commit,
    time_search, turn_neighbourhood, turns_from_line_objects, turns_to_sessions, AnchoredTurn,
    LineRef, MaterializeAnchor, ProducedLines, RankedSession, SessionRef, TurnRef,
    MIN_ADMITTED_STRENGTH,
};
pub use retriever::{IntentRetriever, Result, RetrievalError, Retriever};
pub use session::Gated;
pub use types::{
    strength_for, ContextQuery, Evidence, EvidenceTier, IntentQuery, Retrieval, Strength,
    CONTEXT_QUERY_SCHEMA, RETRIEVAL_SCHEMA,
};
pub use verbs::{verb_for_relation, Verb, DEFAULT_AROUND_RADIUS, DEFAULT_TRAVERSAL_LIMIT, VERBS};
