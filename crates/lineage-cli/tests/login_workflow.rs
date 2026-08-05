use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use lineage_cli::auth;

// One test function: LINEAGE_CONFIG_DIR is process-global env, so sequencing
// the scenarios here avoids cross-test races without a serial-test dependency.
#[test]
fn login_stores_handle_and_exchange_resolves_token() {
    let config_dir = tempfile::tempdir().unwrap();
    std::env::set_var(auth::CONFIG_DIR_ENV, config_dir.path());

    assert_eq!(auth::resolve_server(None).unwrap(), auth::DEFAULT_SERVER);

    let server = spawn_mock_server();

    // Login: device start → pending poll → complete poll → stored credentials.
    lineage_cli::commands::login(Some(server.as_str())).unwrap();

    let credentials = auth::load_credentials().unwrap();
    assert_eq!(credentials.default_server.as_deref(), Some(server.as_str()));
    assert_eq!(
        credentials
            .servers
            .get(&server)
            .map(|c| c.session_handle.as_str()),
        Some("session-1.secret-1")
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(auth::credentials_path().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // The stored default server is picked up when no flag is passed.
    assert_eq!(auth::resolve_server(None).unwrap(), server);

    // Exchange path: stored handle → fresh access token.
    assert_eq!(auth::access_token_for(&server).unwrap(), "jwt-2");

    // A dead session (server 401) must tell the user to log in again.
    let error = auth::access_token_for(&server).unwrap_err().to_string();
    assert!(error.contains("git lineage login"), "got: {error}");

    // No login recorded for an unknown server.
    let error = auth::access_token_for("http://127.0.0.1:1/api")
        .unwrap_err()
        .to_string();
    assert!(error.contains("not logged in"), "got: {error}");
}

/// Mock Lineage auth API: first poll is pending, second completes; first
/// exchange succeeds, second 401s (simulating a revoked session).
fn spawn_mock_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!(
        "http://127.0.0.1:{}/api",
        listener.local_addr().unwrap().port()
    );
    let polls = Arc::new(AtomicUsize::new(0));
    let exchanges = Arc::new(AtomicUsize::new(0));

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut buf = vec![0u8; 16_384];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = request.split_whitespace().nth(1).unwrap_or("").to_string();

            let (status, body) = if path.ends_with("/auth/cli/device") {
                (
                    "200 OK",
                    // interval 0 keeps the poll loop fast under test.
                    r#"{"deviceCode":"dev-1","userCode":"ABCD-1234","verificationUri":"http://v","verificationUriComplete":"http://v?c=ABCD-1234","expiresIn":60,"interval":0}"#
                        .to_string(),
                )
            } else if path.ends_with("/auth/cli/device/poll") {
                if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                    (
                        "200 OK",
                        r#"{"status":"pending","slowDown":false}"#.to_string(),
                    )
                } else {
                    (
                        "200 OK",
                        r#"{"status":"complete","accessToken":"jwt-1","tokenType":"Bearer","expiresIn":900,"sessionHandle":"session-1.secret-1"}"#
                            .to_string(),
                    )
                }
            } else if path.ends_with("/auth/cli/exchange") {
                if exchanges.fetch_add(1, Ordering::SeqCst) == 0 {
                    (
                        "200 OK",
                        r#"{"accessToken":"jwt-2","tokenType":"Bearer","expiresIn":900}"#
                            .to_string(),
                    )
                } else {
                    (
                        "401 Unauthorized",
                        r#"{"message":"Your session is no longer valid. Sign in again."}"#
                            .to_string(),
                    )
                }
            } else {
                ("404 Not Found", "{}".to_string())
            };

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    base
}
