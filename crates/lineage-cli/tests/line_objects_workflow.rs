use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lineage_cli::commands;
use lineage_core::{
    AgentKind, Artifact, ArtifactKind, ArtifactResolve, Conversation, LineageId, ResolveStrategy,
    Role, Turn,
};
use lineage_git::{
    blame_with_lineage, open_repo, persist_conversation, read_line_object, read_note_for_commit,
};
use lineage_policy::{apply_policy, is_private_session, policy_from_repo_config};

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@test.dev"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/auth.rs"), "pub fn validate() {}\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn install_cursor_fixture(dir: &Path) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/cursor-history/.cursor");
    copy_dir_all(&fixture, &dir.join(".cursor")).unwrap();
}

#[test]
fn import_cursor_fixture_materializes_line_objects() {
    let dir = init_repo();
    install_cursor_fixture(dir.path());
    commands::init_config(dir.path()).unwrap();
    commands::import(dir.path(), &["cursor".into()], None, true, false).unwrap();

    let repo = open_repo(dir.path()).unwrap();
    let inner = repo.inner();
    let ids = lineage_git::list_session_ids(inner).unwrap();
    assert!(!ids.is_empty(), "expected imported session");

    let conv = lineage_git::read_conversation(inner, &ids[0])
        .unwrap()
        .expect("session blob");
    assert!(
        !conv.turns.is_empty(),
        "import should preserve turns (not strip as private on macOS temp paths)"
    );
    assert!(
        !conv.private,
        "fixture session should not be marked private"
    );

    let sha = inner
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    let note = read_note_for_commit(inner, &sha)
        .unwrap()
        .expect("commit note");
    assert!(
        !note.line_object_ids.is_empty(),
        "import should materialize line objects for StrReplace fixture"
    );
    let obj = read_line_object(inner, &note.line_object_ids[0])
        .unwrap()
        .expect("line object blob");
    assert_eq!(obj.file_path, "src/auth.rs");
}

#[test]
fn blame_after_import_returns_matches() {
    let dir = init_repo();
    install_cursor_fixture(dir.path());
    commands::init_config(dir.path()).unwrap();
    commands::import(dir.path(), &["cursor".into()], None, true, false).unwrap();

    commands::blame(dir.path(), "src/auth.rs:1", true).unwrap();
    let repo = open_repo(dir.path()).unwrap();
    let result = blame_with_lineage(repo.inner(), Path::new("src/auth.rs"), 1).unwrap();
    assert!(
        !result.matches.is_empty() || !result.line_objects.is_empty(),
        "blame should return lineage matches after import"
    );
}

#[test]
fn materialize_absolute_path_session_writes_line_objects() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let inner = repo.inner();
    let sha = inner
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    let abs_path = dir.path().join("src/auth.rs").display().to_string();

    let mut conv = Conversation::new(AgentKind::Cursor, dir.path().display().to_string());
    conv.commit_shas.push(sha.clone());
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "update validate".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![Artifact {
            kind: ArtifactKind::Diff,
            path: abs_path,
            blob_ref: None,
            content_hash: None,
            mime_type: None,
            preview_data_url: None,
            line_range: None,
            resolve: Some(ArtifactResolve {
                strategy: ResolveStrategy::OldString,
                old_string: Some("pub fn validate() {}".into()),
                new_string: None,
                patch: None,
            }),
        }],
    });
    persist_conversation(inner, &conv).unwrap();

    commands::materialize(dir.path(), None, Some(&conv.id.to_string())).unwrap();

    let note = read_note_for_commit(inner, &sha).unwrap().unwrap();
    assert!(!note.line_object_ids.is_empty());
}

#[test]
fn macos_private_var_path_does_not_strip_imported_turns() {
    let config = lineage_core::LineageRepoConfig::default();
    let source = "/private/var/folders/T/tmp/.cursor/agent-transcripts/session-001.jsonl";
    assert!(
        !is_private_session(source, &config),
        "macOS /private/var temp paths must not match private session patterns"
    );

    let mut conv = Conversation::new(AgentKind::Cursor, "/private/var/folders/T/repo");
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::User,
        content: "hello".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    conv.metadata
        .insert("source".into(), serde_json::Value::String(source.into()));

    let policy = policy_from_repo_config(&config);
    let result = apply_policy(&policy, conv);
    assert!(
        !result.conversation.turns.is_empty(),
        "policy must not strip turns for normal sessions under /private/var"
    );
}
