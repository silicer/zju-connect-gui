//! End-to-end tests for the web server: token + Host auth middleware, SSE
//! gating, public page, and submit-input validation.

#![cfg(unix)]

use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use zju_connect_gui::backend::external_links::EIP_URL;
use zju_connect_gui::backend::proxy::ProxyManager;
use zju_connect_gui::backend::settings_store::UserSettingsStore;
use zju_connect_gui::web::bridge::WebUiBridge;
use zju_connect_gui::web::server::{self, RunningServer};

async fn start_server() -> (TempDir, RunningServer) {
    let tmp = TempDir::new().unwrap();
    let app_dir = tmp.path().to_path_buf();
    let manager = ProxyManager::new(app_dir.clone(), tokio::runtime::Handle::current());
    let settings = Arc::new(UserSettingsStore::new(&app_dir));
    let bridge = WebUiBridge::new(8);
    let server = server::run(app_dir, manager, settings, Arc::new(bridge))
        .await
        .unwrap();
    (tmp, server)
}

/// Send one raw HTTP/1.1 request (Connection: close) and return (status, body).
/// Reads with a short timeout: SSE responses never close, we only need the
/// status line + headers (and any body that arrives with them).
async fn raw_request(addr: &str, request_line_and_headers: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!("{request_line_and_headers}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match tokio::time::timeout(Duration::from_millis(500), stream.read(&mut chunk)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => {
                buf.extend_from_slice(&chunk[..n]);
                // Headers received (plus any body in the same chunk) is enough.
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Ok(Err(_)) => break,
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let status: u16 = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, text)
}

#[tokio::test]
async fn api_requires_token_and_local_host() {
    let (_tmp, server) = start_server().await;
    let addr = format!("127.0.0.1:{}", server.port);
    let host = format!("localhost:{}", server.port);
    let token = &server.token;

    // No token → 403.
    let (code, _) = raw_request(&addr, &format!("GET /api/status HTTP/1.1\r\nHost: {host}")).await;
    assert_eq!(code, 403);

    // DNS rebinding: attacker origin in Host, even with a valid token → 403.
    let (code, _) = raw_request(
        &addr,
        &format!("GET /api/status HTTP/1.1\r\nHost: evil.example.com\r\nX-Auth-Token: {token}"),
    )
    .await;
    assert_eq!(code, 403);

    // Wrong token → 403.
    let (code, _) = raw_request(
        &addr,
        &format!("GET /api/status HTTP/1.1\r\nHost: {host}\r\nX-Auth-Token: deadbeef"),
    )
    .await;
    assert_eq!(code, 403);

    // Correct token → 200.
    let (code, _) = raw_request(
        &addr,
        &format!("GET /api/status HTTP/1.1\r\nHost: {host}\r\nX-Auth-Token: {token}"),
    )
    .await;
    assert_eq!(code, 200);

    // The page itself is public (it contains no secrets).
    let (code, _) = raw_request(&addr, &format!("GET / HTTP/1.1\r\nHost: {host}")).await;
    assert_eq!(code, 200);

    // SSE without a token → 403.
    let (code, _) = raw_request(&addr, &format!("GET /api/events HTTP/1.1\r\nHost: {host}")).await;
    assert_eq!(code, 403);

    // SSE with ?token= (EventSource cannot set headers) → 200.
    let (code, _) = raw_request(
        &addr,
        &format!("GET /api/events?token={token} HTTP/1.1\r\nHost: {host}"),
    )
    .await;
    assert_eq!(code, 200);
}

#[tokio::test]
async fn submit_input_is_rejected_when_nothing_is_awaiting() {
    let (_tmp, server) = start_server().await;
    let addr = format!("127.0.0.1:{}", server.port);
    let host = format!("localhost:{}", server.port);
    let token = &server.token;

    let body = r#"{"value":"1234","kind":"sms"}"#;
    let (code, text) = raw_request(
        &addr,
        &format!(
            "POST /api/submit-input HTTP/1.1\r\nHost: {host}\r\nX-Auth-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
    .await;
    assert_eq!(code, 400);
    assert!(text.contains("no input is currently requested"), "{text}");
}

#[tokio::test]
async fn browsers_and_open_eip_endpoints_behave() {
    let (_tmp, server) = start_server().await;
    let addr = format!("127.0.0.1:{}", server.port);
    let host = format!("localhost:{}", server.port);
    let token = &server.token;

    // /api/browsers requires the token like every other API route...
    let (code, _) = raw_request(
        &addr,
        &format!("GET /api/browsers HTTP/1.1\r\nHost: {host}"),
    )
    .await;
    assert_eq!(code, 403);

    // ...and returns a JSON list when authorized.
    let (code, text) = raw_request(
        &addr,
        &format!("GET /api/browsers HTTP/1.1\r\nHost: {host}\r\nX-Auth-Token: {token}"),
    )
    .await;
    assert_eq!(code, 200);
    let parsed: serde_json::Value =
        serde_json::from_str(text.split("\r\n\r\n").nth(1).unwrap_or(""))
            .expect("browsers JSON body");
    assert!(parsed["browsers"].is_array(), "{parsed}");

    // /api/open-eip is token-gated like the rest of the API. Only the auth gate
    // is asserted here: an authorized call spawns the host's real default
    // browser handler, which a test has no business doing.
    let (code, _) = raw_request(
        &addr,
        &format!("POST /api/open-eip HTTP/1.1\r\nHost: {host}\r\nContent-Length: 0\r\n\r\n"),
    )
    .await;
    assert_eq!(code, 403);
}

/// A fake browser script that appends its argv to `<dir>/<name>.args`.
fn fake_browser(dir: &std::path::Path, name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let capture = dir.join(format!("{name}.args"));
    let bin = dir.join(name);
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\n",
            capture.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    (bin, capture)
}

/// Poll until the capture file holds at least `min_lines` lines — the fake
/// browser appends its argv one write at a time — and return them.
fn wait_for_lines(path: &std::path::Path, min_lines: usize) -> Vec<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let lines: Vec<String> = text.lines().map(str::to_string).collect();
            if lines.len() >= min_lines {
                return lines;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{} never reached {min_lines} lines",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The browser picked — and the launch args typed — in the UI must take effect
/// without reconnecting: options in the request body win over the settings
/// store, while a bodyless call still falls back to the store.
#[tokio::test]
async fn open_eip_prefers_the_request_options_over_the_stored_ones() {
    let (tmp, server) = start_server().await;
    let addr = format!("127.0.0.1:{}", server.port);
    let host = format!("localhost:{}", server.port);
    let token = &server.token;

    let (stored_bin, stored_capture) = fake_browser(tmp.path(), "stored-browser");
    let (picked_bin, picked_capture) = fake_browser(tmp.path(), "picked-browser");

    // What a previous connect left in the settings store.
    let body = serde_json::json!({ "eipBrowserProgram": stored_bin.to_str().unwrap() }).to_string();
    let (code, text) = raw_request(
        &addr,
        &format!(
            "POST /api/settings HTTP/1.1\r\nHost: {host}\r\nX-Auth-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
    .await;
    assert_eq!(code, 200, "{text}");

    // Open EIP with a *different* browser and extra launch args selected now.
    let body = serde_json::json!({
        "options": {
            "eipBrowserProgram": picked_bin.to_str().unwrap(),
            "eipBrowserArgs": ["--kiosk"],
        }
    })
    .to_string();
    let (code, text) = raw_request(
        &addr,
        &format!(
            "POST /api/open-eip HTTP/1.1\r\nHost: {host}\r\nX-Auth-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
    .await;
    assert_eq!(code, 200, "{text}");
    assert_eq!(
        wait_for_lines(&picked_capture, 3),
        vec!["--kiosk", "--new-window", EIP_URL],
        "the browser and the launch args from the request body must reach the command line"
    );
    assert!(
        !stored_capture.exists(),
        "the stored browser must not be launched when the body overrides it"
    );

    // No options in the body → the stored settings are used.
    let body = "{}";
    let (code, text) = raw_request(
        &addr,
        &format!(
            "POST /api/open-eip HTTP/1.1\r\nHost: {host}\r\nX-Auth-Token: {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ),
    )
    .await;
    assert_eq!(code, 200, "{text}");
    assert_eq!(
        wait_for_lines(&stored_capture, 2),
        vec!["--new-window", EIP_URL]
    );
}

/// Regression cases for the auth middleware, mirroring the attack variants
/// probed in review: everything must fail closed.
#[tokio::test]
async fn auth_middleware_fails_closed_on_bypass_attempts() {
    let (_tmp, server) = start_server().await;
    let addr = format!("127.0.0.1:{}", server.port);
    let host = format!("localhost:{}", server.port);
    let token = &server.token;

    // 1. State-changing POSTs without a token → 403 (query token no longer
    //    accepted outside /api/events).
    for path in ["/api/start", "/api/stop", "/api/elevate", "/api/settings"] {
        let (code, _) = raw_request(
            &addr,
            &format!("POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"),
        )
        .await;
        assert_eq!(code, 403, "POST {path} without token");
        // A leaked ?token= URL must not drive POSTs either.
        let (code, _) = raw_request(
            &addr,
            &format!("POST {path}?token={token} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"),
        )
        .await;
        assert_eq!(code, 403, "POST {path} with query token only");
    }

    // 2. Path normalization variants are not routed (axum does not
    //    normalize; a 404 leaks nothing).
    for path in [
        "//api/status",
        "/api//status",
        "/api/./status",
        "/API/status",
        "/api/status/",
        "/static/../api/status",
    ] {
        let (code, _) = raw_request(
            &addr,
            &format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nX-Auth-Token: {token}"),
        )
        .await;
        assert_eq!(code, 404, "GET {path}");
    }

    // 3. Host edge cases → 403 even with a valid token. Ports are derived
    //    from the live server: a hardcoded port would make these assertions
    //    pass merely because the port differs, not because the property
    //    under test holds.
    for bad_host in [
        "localhost".to_string(),                  // no port
        format!("localhost:{}", server.port + 1), // wrong port
        "localhost:99999".to_string(),            // overflow port
        format!("localhost.:{}", server.port),    // trailing dot
        format!("127.0.0.2:{}", server.port),     // wrong loopback
        "evil.example.com".to_string(),           // rebinding
    ] {
        let (code, _) = raw_request(
            &addr,
            &format!("GET /api/status HTTP/1.1\r\nHost: {bad_host}\r\nX-Auth-Token: {token}"),
        )
        .await;
        assert_eq!(code, 403, "Host {bad_host:?}");
    }
    // Note: a Host value with trailing spaces/tabs is normalized by the HTTP
    // parser (OWS stripped per RFC 9110 §5.5) before the middleware sees it,
    // so `Host: <valid>\t` is authorized — parser behavior, not a bypass.

    // 4. Duplicate X-Auth-Token headers: the first one wins; wrong-first must
    //    not be rescued by a correct second value.
    let (code, _) = raw_request(
        &addr,
        &format!("GET /api/status HTTP/1.1\r\nHost: {host}\r\nX-Auth-Token: wrong\r\nX-Auth-Token: {token}"),
    )
    .await;
    assert_eq!(code, 403, "duplicate header wrong-first");

    // 5. HTTP/1.0 without a Host header → 403.
    let (code, _) = raw_request(
        &addr,
        &format!("GET /api/status HTTP/1.0\r\nX-Auth-Token: {token}"),
    )
    .await;
    assert_eq!(code, 403, "HTTP/1.0 without Host");
}
