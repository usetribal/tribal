use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

use lineage_core::{AgentKind, Conversation, LargeBlobBackend, LineageRepoConfig, LfsTransport, Role, Turn, LINEAGE_CONFIG_SCHEMA};
use lineage_git::{lfs_fetch, open_repo, persist_conversation, write_repo_config};
use lineage_store::{normalize_oid, LfsStore};

fn init_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "t@t.dev"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "T"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("f.txt"), "x").unwrap();
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

fn spawn_batch_and_data_servers(payload: &[u8]) -> String {
    let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let data_port = data_listener.local_addr().unwrap().port();
    let payload = payload.to_vec();
    let payload_for_oid = payload.clone();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = data_listener.accept() {
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(&payload);
        }
    });

    let batch_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    batch_listener.set_nonblocking(true).unwrap();
    let endpoint = format!(
        "http://127.0.0.1:{}/info/lfs",
        batch_listener.local_addr().unwrap().port()
    );
    let oid = format!("sha256:{}", sha256_hex(&payload_for_oid));
    let batch_json = format!(
        r#"{{"objects":[{{"oid":"{oid}","actions":{{"download":{{"href":"http://127.0.0.1:{data_port}/","header":{{}}}}}}}}]}}"#
    );
    thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok((mut stream, _)) = batch_listener.accept() {
                let mut buf = vec![0u8; 16_384];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.git-lfs+json\r\nContent-Length: {}\r\n\r\n{}",
                    batch_json.len(),
                    batch_json
                );
                let _ = stream.write_all(resp.as_bytes());
                return;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        }
    });
    endpoint
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[test]
fn lfs_fetch_http_batch_restores_missing_blob() {
    let dir = init_repo();
    let repo = open_repo(dir.path()).unwrap();
    let inner = repo.inner();
    let payload = b"restore-me-via-http-fetch";

    let config = LineageRepoConfig {
        large_blob_backend: LargeBlobBackend::Lfs,
        large_blob_threshold_bytes: 16,
        lfs_transport: LfsTransport::Http,
        schema_version: LINEAGE_CONFIG_SCHEMA.into(),
        ..LineageRepoConfig::default()
    };
    write_repo_config(inner, &config).unwrap();

    let mut conv = Conversation::new(AgentKind::Cursor, dir.path().display().to_string());
    conv.turns.push(Turn {
        id: lineage_core::LineageId::new(),
        role: Role::User,
        content: "x".repeat(100),
        tool_calls: vec![],
        model: None,
        timestamp: None,
        artifacts: vec![],
    });
    persist_conversation(inner, &conv).unwrap();

    let stored = lineage_git::read_conversation_stored(inner, &conv.id)
        .unwrap()
        .unwrap();
    let blob_ref = stored.turns[0]
        .artifacts
        .iter()
        .find_map(|a| a.blob_ref.clone())
        .or_else(|| {
            lineage_store::parse_blob_placeholder(&stored.turns[0].content)
                .map(|r| lineage_store::format_blob_ref(&r))
        })
        .expect("blob ref");
    let oid = normalize_oid(&blob_ref);
    let lfs = LfsStore::new(inner.path());
    std::fs::remove_file(lfs.object_path(&oid)).ok();

    let endpoint = spawn_batch_and_data_servers(payload);
    Command::new("git")
        .args(["config", "lfs.url", &endpoint])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let report = lfs_fetch(inner, "origin").unwrap();
    assert!(report.downloaded >= 1 || lfs.exists(&oid));
}
