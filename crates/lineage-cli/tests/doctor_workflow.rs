//! Doctor fixtures for the diagnostics-v0 failure modes: each scenario the
//! report exists to surface must be visible in `doctor --json` output.

use std::fs;
use std::path::Path;
use std::process::Command;

use lineage_cli::{commands, context_cmd, doctor_cmd};
use lineage_core::{
    AgentKind, Artifact, ArtifactKind, ArtifactResolve, Conversation, LineageId, ResolveStrategy,
    Role, Turn,
};
use lineage_git::{open_repo, persist_conversation};

fn git(dir: &Path, args: &[&str]) {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
}

fn init_repo_at(dir: &Path) {
    git(dir, &["init"]);
    git(dir, &["config", "user.email", "test@test.dev"]);
    git(dir, &["config", "user.name", "Test"]);
    fs::write(dir.join("src.txt"), "hello\n").unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", "init"]);
}

fn head_sha(dir: &Path) -> String {
    let repo = open_repo(dir).unwrap();
    let sha = repo
        .inner()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    sha
}

fn store_session(dir: &Path, workspace_root: &str, artifacts: Vec<Artifact>) -> String {
    let repo = open_repo(dir).unwrap();
    let mut conv = Conversation::new(AgentKind::Claude, workspace_root);
    conv.turns.push(Turn {
        id: LineageId::new(),
        role: Role::Assistant,
        content: "edit".into(),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts,
    });
    persist_conversation(repo.inner(), &conv).unwrap();
    conv.id.to_string()
}

fn edit_artifact(path: &str, resolve: Option<ArtifactResolve>) -> Artifact {
    Artifact {
        kind: ArtifactKind::FileEdit,
        path: path.into(),
        blob_ref: None,
        content_hash: None,
        mime_type: None,
        preview_data_url: None,
        line_range: None,
        resolve,
    }
}

fn report(dir: &Path) -> serde_json::Value {
    doctor_cmd::doctor_report(dir, 20).unwrap()
}

#[test]
fn hook_unloadable_from_session_root_is_flagged() {
    let parent = tempfile::tempdir().unwrap();
    let repo_dir = parent.path().join("project");
    fs::create_dir_all(&repo_dir).unwrap();
    init_repo_at(&repo_dir);
    // The repo itself is correctly wired…
    context_cmd::install_claude_agent_hook(&repo_dir).unwrap();
    // …but the authoring session was opened from the parent directory, whose
    // settings do not register the hook.
    store_session(&repo_dir, &parent.path().display().to_string(), vec![]);

    let doctor = report(&repo_dir);
    assert_eq!(
        doctor["setup"]["hook_wiring"]["lineage_hook_registered"],
        true
    );
    assert_eq!(
        doctor["setup"]["hook_wiring"]["loadable_from_session_root"],
        false
    );
    assert!(doctor["setup"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w.as_str().unwrap().contains("not loadable")));
}

#[test]
fn stale_index_schema_is_flagged() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_at(dir.path());
    // An index written by an older binary: sessions table only, none of the
    // newer tables.
    let index_path = dir.path().join(".git/lineage/index.db");
    fs::create_dir_all(index_path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&index_path).unwrap();
    conn.execute("CREATE TABLE sessions (id TEXT PRIMARY KEY)", [])
        .unwrap();
    drop(conn);

    let doctor = report(dir.path());
    let schema = &doctor["setup"]["index_schema"];
    assert_eq!(schema["has_session_files"], false);
    assert_eq!(schema["has_index_meta"], false);
    assert!(doctor["setup"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w.as_str().unwrap().contains("rebuild-index")));
}

#[test]
fn authoring_session_from_parent_workspace_is_a_capture_mismatch() {
    let parent = tempfile::tempdir().unwrap();
    let repo_dir = parent.path().join("project");
    fs::create_dir_all(&repo_dir).unwrap();
    init_repo_at(&repo_dir);
    let session_id = store_session(&repo_dir, &parent.path().display().to_string(), vec![]);

    let doctor = report(&repo_dir);
    let mismatches = doctor["capture"]["workspace_mismatches"]
        .as_array()
        .unwrap();
    assert_eq!(mismatches.len(), 1);
    assert_eq!(mismatches[0]["session_id"], session_id.as_str());
    assert!(mismatches[0]["workspace_root"]
        .as_str()
        .unwrap()
        .ends_with(parent.path().file_name().unwrap().to_str().unwrap()));
}

#[test]
fn edit_artifacts_resolving_to_zero_show_their_loss_reason() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_at(dir.path());
    let root = dir.path().display().to_string();
    let sha = head_sha(dir.path());

    // Linked, resolvable, but the recorded pre-edit text no longer exists in
    // the committed file: resolves to zero line objects.
    let unresolvable = store_session(
        dir.path(),
        &root,
        vec![edit_artifact(
            "src.txt",
            Some(ArtifactResolve {
                strategy: ResolveStrategy::OldString,
                old_string: Some("text that is not in the file".into()),
                new_string: None,
                patch: None,
            }),
        )],
    );
    commands::link(dir.path(), &unresolvable, &sha).unwrap();

    // Resolvable but never linked to any commit.
    store_session(
        dir.path(),
        &root,
        vec![edit_artifact(
            "src.txt",
            Some(ArtifactResolve {
                strategy: ResolveStrategy::OldString,
                old_string: Some("hello".into()),
                new_string: None,
                patch: None,
            }),
        )],
    );

    // No resolve payload at all.
    store_session(dir.path(), &root, vec![edit_artifact("src.txt", None)]);

    let doctor = report(dir.path());
    let m = &doctor["materialization"];
    assert_eq!(m["total_artifacts"], 3);
    assert_eq!(m["resolvable"], 2);
    assert_eq!(m["resolved"], 0);
    assert_eq!(m["line_objects"], 0);
    assert_eq!(m["failure_reasons"]["old_string_not_found"], 1);
    assert_eq!(m["failure_reasons"]["commit_not_linked"], 1);
    assert_eq!(m["failure_reasons"]["no_resolve_payload"], 1);

    // The link section attributes the manual link from the event log.
    let links = doctor["links"].as_array().unwrap();
    let link = links
        .iter()
        .find(|l| l["commit_sha"] == sha.as_str())
        .unwrap();
    assert!(link["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["session_id"] == unresolvable.as_str() && s["established_by"] == "manual"));

    // The activity tail shows the link event.
    assert!(doctor["activity"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["op"] == "link"));
}

#[test]
fn section_filter_keeps_only_requested_sections_in_json() {
    let dir = tempfile::tempdir().unwrap();
    init_repo_at(dir.path());

    let full = report(dir.path());
    for section in ["setup", "capture", "materialization", "links", "activity"] {
        assert!(full.get(section).is_some(), "missing section {section}");
    }

    doctor_cmd::run(
        dir.path(),
        &doctor_cmd::DoctorArgs {
            json: true,
            sections: vec!["setup".into()],
            activity_limit: 20,
        },
    )
    .unwrap();
    assert!(doctor_cmd::run(
        dir.path(),
        &doctor_cmd::DoctorArgs {
            json: true,
            sections: vec!["bogus".into()],
            activity_limit: 20,
        },
    )
    .is_err());
}
