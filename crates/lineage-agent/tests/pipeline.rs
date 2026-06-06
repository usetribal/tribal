use std::path::PathBuf;

use lineage_adapters::CursorAdapter;
use lineage_agent::IngestPipeline;
use lineage_core::AgentKind;

#[test]
fn pipeline_ingests_fixture_sessions() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/cursor-history");
    let adapter = CursorAdapter::new(&fixture);
    let pipeline = IngestPipeline::default();
    let result = pipeline.ingest(&adapter, &adapter);
    assert!(!result.conversations.is_empty());
}

#[test]
fn filter_agent_respects_selection() {
    assert!(IngestPipeline::filter_agent(&[AgentKind::Cursor], AgentKind::Cursor));
    assert!(!IngestPipeline::filter_agent(&[AgentKind::Claude], AgentKind::Cursor));
    assert!(IngestPipeline::filter_agent(&[], AgentKind::Codex));
}

#[test]
fn ingest_all_combines_sources() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/cursor-history");
    let pipeline = IngestPipeline::default();
    let result = pipeline.ingest_all(&[(
        CursorAdapter::new(&fixture),
        CursorAdapter::new(&fixture),
    )]);
    assert!(!result.conversations.is_empty());
}

#[test]
fn pipeline_handles_missing_discover_gracefully() {
    let adapter = CursorAdapter::new("/nonexistent/path/for-lineage-tests");
    let pipeline = IngestPipeline::default();
    let result = pipeline.ingest(&adapter, &adapter);
    assert!(result.conversations.is_empty());
}
