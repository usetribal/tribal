use git2::Repository;
use lineage_core::{
    AgentKind, Artifact, ArtifactKind, Confidence, Conversation, LineObject, LineageId,
    RepoBinding, Role, Turn,
};
use lineage_git::{write_conversation, write_line_object};
use lineage_oracle::{ContextQuery, EvidenceTier, LocalRetriever, Retriever, Strength};
use lineage_search::LineageIndex;

struct Fixture {
    _dir: tempfile::TempDir,
    repo: Repository,
    index: LineageIndex,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let index = LineageIndex::open(dir.path().join(".git/lineage/index.db")).unwrap();
    Fixture {
        _dir: dir,
        repo,
        index,
    }
}

fn conversation_touching(root: &str, paths: &[&str]) -> Conversation {
    let mut conv = Conversation::new(AgentKind::Claude, root);
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "Refactor auth token handling".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    for path in paths {
        conv.turns.push(Turn {
            id: LineageId::new(),
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![],
            model: None,
            timestamp: None,
            artifacts: vec![Artifact {
                kind: ArtifactKind::FileEdit,
                path: (*path).into(),
                blob_ref: None,
                content_hash: None,
                mime_type: None,
                preview_data_url: None,
                line_range: None,
                resolve: None,
            }],
        });
    }
    conv
}

fn store(fixture: &Fixture, conv: &Conversation) {
    write_conversation(&fixture.repo, conv).unwrap();
    fixture.index.index_conversation(conv).unwrap();
}

fn query_for(file_path: &str) -> ContextQuery {
    ContextQuery {
        file_path: file_path.into(),
        file_blob_sha: "00".repeat(32),
        repo: RepoBinding {
            normalized_remote_url: "github.com/acme/widgets".into(),
            root_commit_sha: "11".repeat(20),
            server_repo_id: None,
        },
        budget_ms: None,
    }
}

#[test]
fn line_object_evidence_outranks_files_touched() {
    let f = fixture();

    let with_lines = conversation_touching("/repo", &["src/auth.rs"]);
    store(&f, &with_lines);
    let turn_id = with_lines.turns[1].id.clone();
    write_line_object(
        &f.repo,
        &LineObject::new(
            "src/auth.rs",
            [10, 14],
            "a".repeat(40),
            with_lines.id.clone(),
            turn_id,
            Confidence::Exact,
        ),
    )
    .unwrap();

    let touched_only = conversation_touching("/repo", &["src/auth.rs"]);
    store(&f, &touched_only);

    let retriever = LocalRetriever::new(&f.repo, &f.index);
    let retrieval = retriever.retrieve(&query_for("src/auth.rs")).unwrap();

    assert_eq!(retrieval.strength, Strength::High);
    assert_eq!(retrieval.evidence.len(), 2);

    let strongest = &retrieval.evidence[0];
    assert_eq!(strongest.session_id, with_lines.id);
    assert_eq!(strongest.tier, EvidenceTier::LineObjects);
    assert_eq!(strongest.match_confidence, Some(Confidence::Exact));
    assert_eq!(strongest.line_ranges, vec![[10, 14]]);
    assert!(strongest.summary.contains("Refactor auth token handling"));
    assert!(strongest.attribution.contains("claude"));

    let weaker = &retrieval.evidence[1];
    assert_eq!(weaker.session_id, touched_only.id);
    assert_eq!(weaker.tier, EvidenceTier::FilesTouched);
    assert_eq!(weaker.strength, Strength::Low);
}

#[test]
fn uncovered_file_yields_honest_nothing() {
    let f = fixture();
    store(&f, &conversation_touching("/repo", &["src/auth.rs"]));

    let retriever = LocalRetriever::new(&f.repo, &f.index);
    let retrieval = retriever.retrieve(&query_for("src/untouched.rs")).unwrap();

    assert!(retrieval.evidence.is_empty());
    assert_eq!(retrieval.strength, Strength::None);
}

#[test]
fn private_sessions_and_their_forks_never_surface() {
    let f = fixture();

    let mut private_conv = conversation_touching("/repo", &["src/auth.rs"]);
    private_conv.private = true;
    store(&f, &private_conv);

    // A fork of a private session inherits exclusion through the parent
    // chain even though the fork itself is not marked private.
    let mut fork = conversation_touching("/repo", &["src/auth.rs"]);
    fork.parent_session_id = Some(private_conv.id.clone());
    store(&f, &fork);

    let public_conv = conversation_touching("/repo", &["src/auth.rs"]);
    store(&f, &public_conv);

    let retriever = LocalRetriever::new(&f.repo, &f.index);
    let retrieval = retriever.retrieve(&query_for("src/auth.rs")).unwrap();

    let sessions: Vec<&str> = retrieval
        .evidence
        .iter()
        .map(|e| e.session_id.as_str())
        .collect();
    assert_eq!(sessions, vec![public_conv.id.as_str()]);
}

#[test]
fn zero_budget_returns_partial_or_empty_without_error() {
    let f = fixture();
    store(&f, &conversation_touching("/repo", &["src/auth.rs"]));

    let mut query = query_for("src/auth.rs");
    query.budget_ms = Some(0);

    let retriever = LocalRetriever::new(&f.repo, &f.index);
    // An exhausted budget is not a failure — the contract is "return what
    // you have", and with no time at all that is the empty answer.
    let retrieval = retriever.retrieve(&query).unwrap();
    assert!(retrieval.evidence.is_empty());
}

#[test]
fn absolute_tool_paths_match_repo_relative_queries() {
    let f = fixture();
    let conv = conversation_touching("/repo", &["/repo/src/deep/mod.rs"]);
    store(&f, &conv);

    let retriever = LocalRetriever::new(&f.repo, &f.index);
    let retrieval = retriever.retrieve(&query_for("src/deep/mod.rs")).unwrap();

    assert_eq!(retrieval.evidence.len(), 1);
    assert_eq!(retrieval.evidence[0].session_id, conv.id);
}

#[test]
fn read_only_sessions_are_not_evidence() {
    let f = fixture();

    // A session that only *read* the file: path in tool_calls, no artifacts.
    let mut reader = Conversation::new(AgentKind::Claude, "/repo");
    reader.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: String::new(),
        tool_calls: vec![lineage_core::ToolCall {
            id: "t".into(),
            name: "Read".into(),
            arguments: "{\"file_path\": \"src/auth.rs\"}".into(),
            result: None,
        }],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    store(&f, &reader);

    let retriever = LocalRetriever::new(&f.repo, &f.index);
    let retrieval = retriever.retrieve(&query_for("src/auth.rs")).unwrap();
    // The read-only session never surfaces — a session that merely consulted
    // a file must not become its provenance (gap 9's echo-chamber rule).
    assert!(retrieval.evidence.is_empty());

    let writer = conversation_touching("/repo", &["src/auth.rs"]);
    store(&f, &writer);
    let retrieval = retriever.retrieve(&query_for("src/auth.rs")).unwrap();
    let sessions: Vec<&str> = retrieval
        .evidence
        .iter()
        .map(|e| e.session_id.as_str())
        .collect();
    assert_eq!(sessions, vec![writer.id.as_str()]);
}
