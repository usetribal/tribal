use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::thread;

use lineage_git::lfs_batch::{
    collect_lfs_objects, discover_lfs_endpoint, fetch_via_http_batch, push_via_http_batch,
};
use lineage_store::{LfsObject, LfsStore};

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
    dir
}

fn spawn_lfs_batch_server(response_json: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!(
        "http://127.0.0.1:{}/info/lfs",
        listener.local_addr().unwrap().port()
    );
    let body = response_json.to_string();
    thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buf = vec![0u8; 16_384];
                    let _ = stream.read(&mut buf);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/vnd.git-lfs+json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    return;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => return,
            }
        }
    });
    endpoint
}

#[test]
fn discover_lfs_endpoint_reads_git_config() {
    let dir = init_repo();
    Command::new("git")
        .args(["config", "lfs.url", "https://example.com/repo.git/info/lfs"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let endpoint = discover_lfs_endpoint(&repo, "origin").unwrap();
    assert!(endpoint.contains("info/lfs"));
}

#[test]
fn collect_lfs_objects_empty_without_refs() {
    let dir = init_repo();
    let repo = git2::Repository::open(dir.path()).unwrap();
    let objects = collect_lfs_objects(&repo).unwrap();
    assert!(objects.is_empty());
}

#[test]
fn push_via_http_batch_hits_mock_server() {
    let dir = init_repo();
    let endpoint = spawn_lfs_batch_server(r#"{"objects":[]}"#);
    Command::new("git")
        .args(["config", "lfs.url", &endpoint])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let repo = git2::Repository::open(dir.path()).unwrap();
    let lfs = LfsStore::new(repo.path());
    let obj = lfs.put(b"batch-test-payload").unwrap();

    let uploaded = push_via_http_batch(
        &repo,
        "origin",
        &[LfsObject {
            oid: obj.oid,
            size: obj.size,
        }],
    )
    .unwrap();
    assert_eq!(uploaded, 0);
}

#[test]
fn fetch_via_http_batch_downloads_from_mock() {
    let dir = init_repo();
    let data_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let data_port = data_listener.local_addr().unwrap().port();
    let payload = b"fetched-lfs-bytes";
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = data_listener.accept() {
            let mut buf = vec![0u8; 4096];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(payload);
        }
    });

    let oid = lineage_store::normalize_oid(&format!("sha256:{}", sha256_hex(payload)));
    let batch_json = format!(
        r#"{{"objects":[{{"oid":"{oid}","actions":{{"download":{{"href":"http://127.0.0.1:{data_port}/","header":{{}}}}}}}}]}}"#
    );
    let endpoint = spawn_lfs_batch_server(&batch_json);
    Command::new("git")
        .args(["config", "lfs.url", &endpoint])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let repo = git2::Repository::open(dir.path()).unwrap();
    let downloaded = fetch_via_http_batch(
        &repo,
        "origin",
        &[LfsObject {
            oid: oid.strip_prefix("sha256:").unwrap_or(&oid).to_string(),
            size: payload.len(),
        }],
    )
    .unwrap();
    assert_eq!(downloaded, 1);
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}
