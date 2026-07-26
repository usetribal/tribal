use std::process::Command;

use lineage_core::{enriched_indexable_body, AgentKind, Conversation, LineageId, Role, Turn};
use lineage_git::{open_repo, persist_conversation};
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
    assert!(enriched_indexable_body(&conv).contains("caching"));

    index.rebuild(repo.inner()).unwrap();
    let hits2 = index.search("redis", 5).unwrap();
    assert!(!hits2.is_empty());
}

#[test]
fn session_files_key_worktree_paths_the_way_the_graph_does() {
    // A session recorded through a linked worktree names its edit
    // `.claude/worktrees/feature/AGENTS.md`. Stored verbatim, every lookup for
    // `AGENTS.md` misses it and the index disagrees with the provenance graph
    // about who wrote the file.
    let dir = init_repo();
    for args in [
        vec!["config", "user.email", "test@example.com"],
        vec!["config", "user.name", "Test"],
    ] {
        Command::new("git")
            .args(&args)
            .current_dir(dir.path())
            .output()
            .unwrap();
    }
    std::fs::write(dir.path().join("AGENTS.md"), "guide\n").unwrap();
    Command::new("git")
        .args(["add", "AGENTS.md"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-qm", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args([
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            dir.path()
                .join(".claude/worktrees/feature")
                .to_str()
                .unwrap(),
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let mut conv = Conversation::new(AgentKind::Claude, dir.path().display().to_string());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "updated the guide".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![lineage_core::Artifact {
            kind: lineage_core::ArtifactKind::FileEdit,
            path: ".claude/worktrees/feature/AGENTS.md".into(),
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: None,
            resolve: None,
        }],
    });

    let db = dir.path().join(".git").join("lineage").join("index.db");
    let index = LineageIndex::open(&db).unwrap();
    index.index_conversation(&conv).unwrap();

    assert_eq!(
        index.sessions_that_wrote_file("AGENTS.md").unwrap(),
        vec![conv.id.to_string()],
        "the worktree-recorded edit must be findable under its repo-relative path"
    );
}
