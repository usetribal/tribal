//! Event-log coverage for diagnostics-v0: every instrumented operation appends
//! a schema-valid entry, the sync entry carries the server's full response,
//! and a failed log write never fails the operation it records.

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use lineage_cli::{commands, hooks_cmd, skill_cmd};
use lineage_git::open_repo;

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
    fs::write(dir.path().join("src.txt"), "hello\n").unwrap();
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

fn read_events(dir: &Path) -> Vec<serde_json::Value> {
    let contents = fs::read_to_string(dir.join(".git/lineage/events.jsonl")).unwrap();
    contents
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn entries_for<'a>(events: &'a [serde_json::Value], op: &str) -> Vec<&'a serde_json::Value> {
    events.iter().filter(|e| e["op"] == op).collect()
}

fn assert_schema_valid(entry: &serde_json::Value) {
    assert_eq!(entry["schema_version"], "lineage-events-v0");
    let ts = entry["ts"].as_str().expect("ts must be a string");
    chrono::DateTime::parse_from_rfc3339(ts).expect("ts must be RFC 3339");
    assert!(entry["op"].is_string());
    let outcome = entry["outcome"].as_str().unwrap();
    assert!(
        matches!(outcome, "ok" | "error" | "silent"),
        "got: {outcome}"
    );
    assert!(entry["detail"].is_object());
}

#[test]
fn every_instrumented_operation_appends_a_schema_valid_entry() {
    let dir = init_repo();
    install_cursor_fixture(dir.path());
    commands::init_config(dir.path()).unwrap();

    commands::import(dir.path(), &["cursor".into()], None, true, false).unwrap();
    hooks_cmd::install_hook(dir.path(), true).unwrap();
    hooks_cmd::post_commit(dir.path()).unwrap();
    commands::rebuild_index(dir.path(), false).unwrap();
    skill_cmd::init_skill(dir.path(), &["claude".into()], false).unwrap();

    let repo = open_repo(dir.path()).unwrap();
    let session_id = lineage_git::list_session_ids(repo.inner()).unwrap()[0].to_string();
    let head_sha = repo
        .inner()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    commands::link(dir.path(), &session_id, &head_sha).unwrap();
    commands::materialize(dir.path(), Some(&head_sha), Some(&session_id)).unwrap();

    let events = read_events(dir.path());
    for entry in &events {
        assert_schema_valid(entry);
    }

    let import = entries_for(&events, "import")[0];
    assert!(import["detail"]["discovered"]["cursor"].as_u64().unwrap() > 0);
    assert!(!import["detail"]["session_ids"]
        .as_array()
        .unwrap()
        .is_empty());

    let install = entries_for(&events, "install_hook")[0];
    assert_eq!(
        install["detail"]["hooks"],
        serde_json::json!(["pre-commit", "post-commit"])
    );

    let links = entries_for(&events, "link");
    assert!(links
        .iter()
        .any(|e| e["detail"]["trigger"] == "post_commit"));
    let manual = links
        .iter()
        .find(|e| e["detail"]["trigger"] == "manual")
        .unwrap();
    assert_eq!(manual["detail"]["commit_sha"], head_sha.as_str());
    assert_eq!(
        manual["detail"]["sessions"][0]["session_id"],
        session_id.as_str()
    );

    let rebuild = entries_for(&events, "rebuild_index")[0];
    assert!(rebuild["detail"]["sessions_indexed"].as_u64().unwrap() > 0);

    let skill = entries_for(&events, "install_skill")[0];
    assert_eq!(skill["detail"]["targets"], serde_json::json!(["claude"]));

    let materialize = entries_for(&events, "materialize")[0];
    assert_eq!(materialize["detail"]["commit_sha"], head_sha.as_str());
    assert_eq!(
        materialize["detail"]["sessions"][0]["session_id"],
        session_id.as_str()
    );
}

#[test]
fn sync_records_the_full_server_response_verbatim() {
    let dir = init_repo();
    install_cursor_fixture(dir.path());
    commands::init_config(dir.path()).unwrap();
    commands::import(dir.path(), &["cursor".into()], None, true, false).unwrap();
    Command::new("git")
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widgets.git",
        ])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let repo = open_repo(dir.path()).unwrap();
    let session_id = lineage_git::list_session_ids(repo.inner()).unwrap()[0].to_string();
    let server = spawn_sync_server(&session_id);

    // One rejection: the command must exit with an error *and* the event must
    // still hold the server's verdicts — recorded before the failure returns.
    let err = commands::sync(dir.path(), Some(&server), Some("dev-token"), "origin")
        .expect_err("a rejected object should fail the sync command");
    assert!(err.to_string().contains("rejected"), "got: {err}");

    let events = read_events(dir.path());
    let sync = entries_for(&events, "sync")[0];
    assert_schema_valid(sync);
    assert_eq!(sync["outcome"], "error");
    assert_eq!(sync["detail"]["server"], server.as_str());
    assert_eq!(sync["detail"]["remote"], "origin");
    assert!(sync["detail"]["batch"]["conversations"].as_u64().unwrap() > 0);

    let response = &sync["detail"]["response"];
    assert_eq!(response["schema_version"], "sync-response-v0");
    assert_eq!(response["repo_id"], "repo-1");
    let results = response["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert!(results
        .iter()
        .any(|r| r["status"] == "rejected" && r["reason"] == "invalid"));
}

#[test]
fn a_failed_log_write_never_fails_the_operation() {
    let dir = init_repo();
    commands::init_config(dir.path()).unwrap();

    // A directory where the log file should be makes every append fail.
    fs::create_dir_all(dir.path().join(".git/lineage/events.jsonl")).unwrap();

    commands::rebuild_index(dir.path(), false).unwrap();
    hooks_cmd::install_hook(dir.path(), true).unwrap();
    hooks_cmd::post_commit(dir.path()).unwrap();
}

/// Mock ingest endpoint: accepts the blob PUTs, answers the sync POST with a
/// fixed two-verdict response (one accepted, one rejected).
fn spawn_sync_server(session_id: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let response_body = format!(
        r#"{{"schema_version":"sync-response-v0","repo_id":"repo-1","results":[{{"kind":"conversation","id":"{session_id}","status":"accepted"}},{{"kind":"line_object","id":"{session_id}","status":"rejected","reason":"invalid"}}]}}"#
    );

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let request = read_full_request(&mut stream);
            let body = if request.starts_with("PUT") {
                "{}".to_string()
            } else {
                response_body.clone()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    base
}

/// Drains headers plus the declared Content-Length so the client never sees a
/// response (or a closed socket) while still writing its request body.
fn read_full_request(stream: &mut std::net::TcpStream) -> String {
    let mut raw = Vec::new();
    let mut buf = [0u8; 16_384];
    loop {
        let n = stream.read(&mut buf).unwrap_or(0);
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&raw);
        let Some(header_end) = text.find("\r\n\r\n") else {
            continue;
        };
        let content_length = text
            .lines()
            .find_map(|l| {
                let lower = l.to_ascii_lowercase();
                lower
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().to_string())
            })
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        if raw.len() >= header_end + 4 + content_length {
            break;
        }
    }
    String::from_utf8_lossy(&raw).to_string()
}
