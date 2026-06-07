use std::path::PathBuf;

use lineage_adapters::CodexAdapter;
use lineage_agent::{AgentSource, SessionReader};
use lineage_core::{AgentKind, Role};

#[test]
fn reads_codex_rollout_fixture() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/codex-history");
    let adapter = CodexAdapter::new(&fixture);
    let sessions = adapter.discover().unwrap();
    assert!(!sessions.is_empty());
    let conv = adapter.read(&sessions[0]).unwrap();
    assert_eq!(conv.agent, AgentKind::Codex);
    assert!(conv.turns.len() >= 3);
    assert!(conv.turns.iter().any(|t| matches!(t.role, Role::User)));
    assert!(conv
        .turns
        .iter()
        .any(|t| t.artifacts.iter().any(|a| a.path.contains("auth.rs"))));
}
