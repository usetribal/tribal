use std::process::Command;

use lineage_core::{AgentKind, Conversation, LineageId, Role, Turn};
use lineage_git::{indexable_body, open_repo, persist_conversation};
use lineage_search::LineageIndex;

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

#[test]
fn index_search_and_rebuild() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let mut conv = Conversation::new(AgentKind::Codex, dir.path().display().to_string());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "implement caching layer for redis".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    persist_conversation(repo.inner(), &conv).unwrap();

    let db = dir.path().join(".git").join("lineage").join("index.db");
    let index = LineageIndex::open(&db).unwrap();
    index.index_conversation(&conv).unwrap();
    let hits = index.search("caching", 5).unwrap();
    assert!(!hits.is_empty());
    assert!(indexable_body(&conv).contains("caching"));

    index.rebuild(repo.inner()).unwrap();
    let hits2 = index.search("redis", 5).unwrap();
    assert!(!hits2.is_empty());
}
