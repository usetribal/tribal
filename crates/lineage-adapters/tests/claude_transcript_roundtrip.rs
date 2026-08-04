//! The strongest available check on the transcript writer: render a
//! `Conversation` to Claude JSONL, then parse it back with the adapter that
//! reads real Claude transcripts. The reader is the closest thing to Claude
//! Code's own parser that can run without invoking the harness, so agreement
//! between the two is what says the written file is well-formed.

use std::fs;
use std::path::Path;

use chrono::{TimeZone, Utc};
use lineage_adapters::claude_transcript::render_claude_transcript;
use lineage_adapters::{claude_project_dir, ClaudeAdapter};
use lineage_agent::{SessionReader, SessionRef};
use lineage_core::{AgentKind, Conversation, LineageId, Role, ToolCall, Turn};

fn turn(role: Role, content: &str, tool_calls: Vec<ToolCall>) -> Turn {
    Turn {
        id: LineageId::new(),
        role,
        content: content.into(),
        tool_calls,
        model: None,
        timestamp: Some(Utc.with_ymd_and_hms(2026, 6, 6, 10, 1, 0).unwrap()),
        artifacts: Vec::new(),
    }
}

fn alices_session(workspace: &Path) -> Conversation {
    let mut conv = Conversation::new(AgentKind::Claude, workspace.display().to_string());
    conv.turns = vec![
        turn(
            Role::User,
            "Why does the auth middleware skip HEAD?",
            vec![],
        ),
        turn(
            Role::Assistant,
            "Because the upstream proxy strips bodies; the marker fact is BLUE-PANGOLIN.",
            vec![ToolCall {
                id: "tu-1".into(),
                name: "Read".into(),
                arguments: r#"{"file_path":"src/auth.rs"}"#.into(),
                result: None,
                target: None,
            }],
        ),
        turn(
            Role::Tool,
            "",
            vec![ToolCall {
                id: "tu-1".into(),
                name: "tool_result".into(),
                arguments: String::new(),
                result: Some("pub fn middleware() {}".into()),
                target: None,
            }],
        ),
        turn(Role::Assistant, "Fixed by matching on method.", vec![]),
    ];
    conv
}

#[test]
fn rendered_transcript_parses_back_through_the_claude_adapter() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let alice = alices_session(workspace.path());

    let rendered = render_claude_transcript(&alice, home.path(), workspace.path());
    fs::create_dir_all(rendered.path.parent().unwrap()).unwrap();
    fs::write(&rendered.path, &rendered.contents).unwrap();

    let adapter = ClaudeAdapter::new(workspace.path());
    let session = SessionRef {
        id_hint: rendered.session_handle.clone(),
        agent: AgentKind::Claude,
        source_path: rendered.path.clone(),
        started_at: Some(alice.started_at),
    };
    let parsed = adapter.read(&session).unwrap();

    assert_eq!(parsed.agent, AgentKind::Claude);
    assert_eq!(parsed.turns.len(), alice.turns.len());

    // The minted id is what the harness will resolve the session by, and the
    // reader recovers it from the records rather than the filename.
    assert_eq!(
        parsed
            .metadata
            .get("claude_session_id")
            .and_then(|v| v.as_str()),
        Some(rendered.session_handle.as_str())
    );

    let text: String = parsed
        .turns
        .iter()
        .map(|t| t.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    // Content survives the round trip: the user's question, the assistant's
    // reasoning (with the marker fact), and the flattened tool output.
    assert!(text.contains("Why does the auth middleware skip HEAD?"));
    assert!(text.contains("BLUE-PANGOLIN"));
    assert!(text.contains("Read src/auth.rs"));
    assert!(text.contains("pub fn middleware() {}"));
    assert!(text.contains("Fixed by matching on method."));

    // Roles survive as the user/assistant alternation Claude replays. The tool
    // turn comes back as a user record because that is what it is written as.
    let roles: Vec<Role> = parsed.turns.iter().map(|t| t.role).collect();
    assert_eq!(
        roles,
        vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
    );

    // No fabricated tool state anywhere in the file the harness will read.
    assert!(!rendered.contents.contains("\"tool_use\""));
    assert!(!rendered.contents.contains("\"tool_result\""));
    assert!(parsed.turns.iter().all(|t| t.tool_calls.is_empty()));
}

#[test]
fn rendered_transcript_lands_where_the_harness_looks_for_it() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let alice = alices_session(workspace.path());

    let rendered = render_claude_transcript(&alice, home.path(), workspace.path());

    // The harness resolves `projects/<slug(cwd)>/<sessionId>.jsonl`, and the
    // reader's own discovery derives that directory the same way. Asserted
    // against the shared derivation rather than `discover()`, which reads the
    // real `$HOME` and cannot be pointed at a tempdir without mutating env.
    //
    // Calls `claude_project_key` rather than restating the substitution: an
    // earlier version inlined `.replace('/', "-")` here and so agreed with a
    // writer that had the same rule wrong, hiding the fact that Claude also
    // substitutes `.` and `_`. Tempdir paths contain dots, so this assertion
    // now fails if the two ever diverge again.
    let expected_dir = claude_project_dir(home.path(), workspace.path());

    assert_eq!(rendered.path.parent().unwrap(), expected_dir);
    assert_eq!(
        rendered.path.file_name().unwrap().to_str().unwrap(),
        format!("{}.jsonl", rendered.session_handle)
    );
}
