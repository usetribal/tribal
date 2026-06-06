use std::path::PathBuf;

use lineage_adapters::CursorAdapter;
use lineage_agent::{AgentSource, SessionReader};
use lineage_core::AgentKind;

#[test]
fn reads_cursor_fixture() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/cursor-history");
    let adapter = CursorAdapter::new(&fixture);
    let sessions = adapter.discover().unwrap();
    assert!(!sessions.is_empty());
    let conv = adapter.read(&sessions[0]).unwrap();
    assert_eq!(conv.agent, AgentKind::Cursor);
    assert!(!conv.turns.is_empty());
    assert!(conv.turns.iter().any(|t| !t.tool_calls.is_empty()));
}
