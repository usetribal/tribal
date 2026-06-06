use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::LineageId;

pub const LAST_INGEST_SCHEMA: &str = "last-ingest-v0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastIngestState {
    #[serde(default = "default_schema")]
    pub schema_version: String,
    pub ingested_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub session_ids: Vec<LineageId>,
}

fn default_schema() -> String {
    LAST_INGEST_SCHEMA.into()
}

impl Default for LastIngestState {
    fn default() -> Self {
        Self {
            schema_version: LAST_INGEST_SCHEMA.into(),
            ingested_at: None,
            session_ids: Vec::new(),
        }
    }
}

impl LastIngestState {
    pub fn new(session_ids: Vec<LineageId>) -> Self {
        Self {
            schema_version: LAST_INGEST_SCHEMA.into(),
            ingested_at: Some(Utc::now()),
            session_ids,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_has_schema_version() {
        let state = LastIngestState::default();
        assert_eq!(state.schema_version, LAST_INGEST_SCHEMA);
        assert!(state.ingested_at.is_none());
    }

    #[test]
    fn new_state_records_session_ids() {
        let id = LineageId::new();
        let state = LastIngestState::new(vec![id.clone()]);
        assert_eq!(state.session_ids, vec![id]);
        assert!(state.ingested_at.is_some());
    }
}
