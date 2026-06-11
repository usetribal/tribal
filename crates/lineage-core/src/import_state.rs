use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::LineageId;

pub const LAST_IMPORT_SCHEMA: &str = "last-import-v0";
pub const LAST_IMPORT_SCHEMA_LEGACY: &str = "last-ingest-v0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastImportState {
    #[serde(default = "default_schema")]
    pub schema_version: String,
    #[serde(default, alias = "ingested_at")]
    pub imported_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub session_ids: Vec<LineageId>,
}

fn default_schema() -> String {
    LAST_IMPORT_SCHEMA.into()
}

impl Default for LastImportState {
    fn default() -> Self {
        Self {
            schema_version: LAST_IMPORT_SCHEMA.into(),
            imported_at: None,
            session_ids: Vec::new(),
        }
    }
}

impl LastImportState {
    pub fn new(session_ids: Vec<LineageId>) -> Self {
        Self {
            schema_version: LAST_IMPORT_SCHEMA.into(),
            imported_at: Some(Utc::now()),
            session_ids,
        }
    }
}

pub fn is_valid_import_schema(version: &str) -> bool {
    version == LAST_IMPORT_SCHEMA || version == LAST_IMPORT_SCHEMA_LEGACY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_state_has_schema_version() {
        let state = LastImportState::default();
        assert_eq!(state.schema_version, LAST_IMPORT_SCHEMA);
        assert!(state.imported_at.is_none());
    }

    #[test]
    fn new_state_records_session_ids() {
        let id = LineageId::new();
        let state = LastImportState::new(vec![id.clone()]);
        assert_eq!(state.session_ids, vec![id]);
        assert!(state.imported_at.is_some());
    }

    #[test]
    fn deserializes_legacy_ingested_at_field() {
        let json = r#"{"schema_version":"last-ingest-v0","ingested_at":"2026-01-01T00:00:00Z","session_ids":[]}"#;
        let state: LastImportState = serde_json::from_str(json).unwrap();
        assert!(state.imported_at.is_some());
    }
}
