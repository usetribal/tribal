use std::path::PathBuf;

use lineage_adapters::CursorAdapter;
use lineage_agent::ImportPipeline;
use lineage_core::AgentKind;

#[test]
fn pipeline_imports_fixture_sessions() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/cursor-history");
    let adapter = CursorAdapter::new(&fixture);
    let pipeline = ImportPipeline::default();
    let result = pipeline.import(&adapter, &adapter);
    assert!(!result.conversations.is_empty());
}

#[test]
fn filter_agent_respects_selection() {
    assert!(ImportPipeline::filter_agent(
        &[AgentKind::Cursor],
        AgentKind::Cursor
    ));
    assert!(!ImportPipeline::filter_agent(
        &[AgentKind::Claude],
        AgentKind::Cursor
    ));
    assert!(ImportPipeline::filter_agent(&[], AgentKind::Codex));
}

#[test]
fn import_all_combines_sources() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/cursor-history");
    let pipeline = ImportPipeline::default();
    let result =
        pipeline.import_all(&[(CursorAdapter::new(&fixture), CursorAdapter::new(&fixture))]);
    assert!(!result.conversations.is_empty());
}

#[test]
fn pipeline_handles_missing_discover_gracefully() {
    let adapter = CursorAdapter::new("/nonexistent/path/for-lineage-tests");
    let pipeline = ImportPipeline::default();
    let result = pipeline.import(&adapter, &adapter);
    assert!(result.conversations.is_empty());
}
