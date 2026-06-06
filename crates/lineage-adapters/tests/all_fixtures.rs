use std::path::PathBuf;

use lineage_adapters::{all_adapters, ClaudeAdapter, CodexAdapter, CursorAdapter};
use lineage_agent::{AgentSource, SessionReader};

#[test]
fn all_agent_fixtures_parse() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cases = [
        ("cursor", root.join("../../tests/fixtures/cursor-history")),
        ("claude", root.join("../../tests/fixtures/claude-code-history")),
        ("codex", root.join("../../tests/fixtures/codex-rollout")),
    ];
    for (name, path) in cases {
        if !path.exists() {
            continue;
        }
        let adapter = match name {
            "cursor" => CursorAdapter::new(&path).discover().map(|_| ()),
            "claude" => ClaudeAdapter::new(&path).discover().map(|_| ()),
            "codex" => CodexAdapter::new(&path).discover().map(|_| ()),
            _ => Ok(()),
        };
        assert!(adapter.is_ok(), "discover failed for {name}");
        if name == "cursor" {
            let a = CursorAdapter::new(&path);
            let sessions = a.discover().unwrap();
            assert!(!sessions.is_empty());
            assert!(!a.read(&sessions[0]).unwrap().turns.is_empty());
        }
    }
    let adapters = all_adapters(&root.join("../../tests/fixtures/cursor-history"));
    assert!(!adapters.is_empty());
}
