use std::path::PathBuf;

use lineage_adapters::CursorAdapter;
use lineage_agent::{AgentSource, SessionReader};
use lineage_core::{AgentKind, ResolveStrategy, ToolTargetKind};

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

/// Regression coverage for three gaps confirmed by running every adapter
/// against real local Cursor transcripts (a hand-written fixture alone never
/// exercised these shapes): `ApplyPatch` passes its whole payload as a raw
/// V4A patch string rather than a JSON object, so every object-keyed lookup
/// silently returned nothing for it; `Glob` names its argument
/// `glob_pattern`, not `pattern`; `ReadLints` takes a `paths` array rather
/// than a single `path` string. All three previously resolved to zero
/// artifacts / no `ToolTarget`, which made every file an `ApplyPatch` edited
/// invisible to line-object materialization and blame.
#[test]
fn resolves_v4a_patch_glob_and_multi_path_tool_calls() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/cursor-history");
    let adapter = CursorAdapter::new(&fixture);
    let sessions = adapter.discover().unwrap();
    let conv = adapter.read(&sessions[0]).unwrap();

    let apply_patch = conv
        .turns
        .iter()
        .flat_map(|t| &t.tool_calls)
        .find(|tc| tc.name == "ApplyPatch")
        .expect("ApplyPatch call in fixture");
    assert_eq!(
        apply_patch.target.as_ref().map(|t| t.kind),
        Some(ToolTargetKind::Path)
    );
    assert_eq!(
        apply_patch.target.as_ref().map(|t| t.value.as_str()),
        Some("src/config.rs"),
        "target should be the first file section's path"
    );
    let patch_artifacts: Vec<_> = conv
        .turns
        .iter()
        .flat_map(|t| &t.artifacts)
        .filter(|a| a.resolve.as_ref().map(|r| r.strategy) == Some(ResolveStrategy::DiffHunk))
        .collect();
    assert_eq!(
        patch_artifacts.len(),
        2,
        "one artifact per file section in the patch"
    );
    assert!(patch_artifacts.iter().any(|a| a.path == "src/config.rs"));
    assert!(patch_artifacts
        .iter()
        .any(|a| a.path == "src/new_module.rs"));

    let glob = conv
        .turns
        .iter()
        .flat_map(|t| &t.tool_calls)
        .find(|tc| tc.name == "Glob")
        .expect("Glob call in fixture");
    assert_eq!(
        glob.target.as_ref().map(|t| t.value.as_str()),
        Some("src/**/*.rs")
    );

    let read_lints = conv
        .turns
        .iter()
        .flat_map(|t| &t.tool_calls)
        .find(|tc| tc.name == "ReadLints")
        .expect("ReadLints call in fixture");
    let target = read_lints.target.as_ref().expect("ReadLints target");
    assert_eq!(target.kind, ToolTargetKind::Subject);
    assert!(target.value.contains("src/config.rs"));
    assert!(target.value.contains("src/new_module.rs"));
}
