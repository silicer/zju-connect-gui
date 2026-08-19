//! Axum web server: router construction, port binding, SSE streaming.

use std::io;
use std::net::{TcpListener, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use rand::Rng;
use tokio::sync::mpsc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::backend::proxy::ProxyManager;
use crate::backend::settings_store::UserSettingsStore;
use crate::web::bridge::WebUiBridge;
use crate::web::handlers::{self, AppState};

const PORT_FILE: &str = "web_port.txt";
const DEFAULT_PORT: u16 = 19823;

/// Handle bundle returned by [`run`].
pub struct RunningServer {
    pub port: u16,
    /// Per-launch auth token embedded in the UI URL; not persisted server-side.
    /// The web UI may keep the current launch token in browser localStorage to
    /// allow refreshing / reopening the page without the query-string token.
    pub token: String,
    /// Elevation requests coming in from the web UI.
    pub elevate_rx: mpsc::Receiver<Vec<String>>,
    pub handle: tokio::task::JoinHandle<()>,
}

/// Run the web server, returning the bound port, the auth token and the
/// server join handle.
pub async fn run(
    app_dir: PathBuf,
    manager: ProxyManager,
    settings: Arc<UserSettingsStore>,
    bridge: Arc<WebUiBridge>,
) -> io::Result<RunningServer> {
    let (port, listener) = bind_listener(&app_dir).await?;
    let token = generate_token();
    // Capacity 1: elevation requests are single-flight (try_send drops a new
    // request while one is already queued), so a scripted caller cannot stack
    // up a queue of UAC prompts.
    let (elevate_tx, elevate_rx) = mpsc::channel::<Vec<String>>(1);

    let state = AppState {
        manager: manager.clone(),
        settings,
        bridge: bridge.clone(),
        port,
        auth_token: token.clone(),
        elevate_tx,
    };

    let router = build_router(state.clone());
    log::info!("web UI listening on http://127.0.0.1:{port}");

    let handle = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, router).await {
            log::error!("web server exited: {err}");
        }
    });

    Ok(RunningServer {
        port,
        token,
        elevate_rx,
        handle,
    })
}

/// Bind a listener, preferring the last-used port, then the default port, then
/// an OS-assigned one. The listener is bound exactly once (no
/// check-then-bind race that would let another process steal the port between
/// the probe and the real bind).
async fn bind_listener(app_dir: &Path) -> io::Result<(u16, tokio::net::TcpListener)> {
    // 1. Try the last-used port from file.
    let port_file = app_dir.join(PORT_FILE);
    if let Ok(contents) = tokio::fs::read_to_string(&port_file).await {
        if let Ok(port) = contents.trim().parse::<u16>() {
            if port >= 1024 {
                if let Ok(listener) = bind_std(("127.0.0.1", port)) {
                    log::info!("reusing last port {port}");
                    return Ok((port, listener));
                }
                log::info!("last port {port} is occupied, picking a new one");
            }
        }
    }

    // 2. Try the default port.
    if let Ok(listener) = bind_std(("127.0.0.1", DEFAULT_PORT)) {
        return Ok((DEFAULT_PORT, listener));
    }

    // 3. Fall back to OS-assigned.
    let listener = bind_std("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    Ok((port, listener))
}

fn bind_std(addr: impl ToSocketAddrs) -> io::Result<tokio::net::TcpListener> {
    let std_listener = TcpListener::bind(addr)?;
    std_listener.set_nonblocking(true)?;
    tokio::net::TcpListener::from_std(std_listener)
}

/// 128 bits of randomness, hex-encoded (32 chars).
fn generate_token() -> String {
    let bytes: [u8; 16] = rand::thread_rng().gen();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Persist the port so the next launch can re-use it.
pub async fn persist_port(app_dir: &Path, port: u16) {
    let port_file = app_dir.join(PORT_FILE);
    if let Err(err) = tokio::fs::write(&port_file, port.to_string()).await {
        log::warn!("failed to persist port to {}: {err}", port_file.display());
    }
}

fn build_router(state: AppState) -> Router {
    // SSE event streaming
    let sse_state = state.clone();
    let auth_state = state.clone();

    Router::new()
        // API
        .route("/api/settings", get(handlers::get_settings))
        .route("/api/settings", post(handlers::save_settings))
        .route("/api/start", post(handlers::start_proxy))
        .route("/api/stop", post(handlers::stop_proxy))
        .route("/api/submit-input", post(handlers::submit_input))
        .route("/api/status", get(handlers::get_status))
        .route("/api/elevate", post(handlers::elevate))
        .route("/api/events", get(move || sse_handler(State(sse_state))))
        // Token + Host gate for everything above; the page itself and its
        // static assets stay public (they contain no secrets — the token only
        // ever lives in the URL / request headers).
        .route_layer(middleware::from_fn_with_state(auth_state, require_auth))
        // Static assets
        .route("/", get(serve_index))
        .route("/static/{*path}", get(serve_static))
        .with_state(state)
}

// ── Auth middleware ────────────────────────────────────────────────────

/// Gate for all `/api/*` routes.
///
/// Two independent checks:
/// 1. The per-launch token, passed by the page as the `X-Auth-Token` header;
///    SSE clients (EventSource cannot set headers) pass it as `?token=`.
/// 2. The `Host` header must be `localhost:<port>` / `127.0.0.1:<port>`, which
///    blocks DNS-rebinding attempts (a malicious page would send its own
///    origin as Host).
///
/// Together these stop any local process / remote page from driving the
/// (possibly elevated) app: starting the VPN with attacker-chosen arguments,
/// executing an arbitrary `eip_browser_program`, or reading credentials.
async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    let presented = req
        .headers()
        .get("x-auth-token")
        .and_then(|v| v.to_str().ok())
        .or_else(|| {
            // EventSource cannot set headers, so the SSE stream accepts the
            // token as `?token=`. Every other endpoint requires the header: a
            // leaked `?token=` URL (history, devtools, screen share) must not
            // be able to drive state-changing requests.
            if req.uri().path() == "/api/events" {
                req.uri().query().and_then(token_from_query)
            } else {
                None
            }
        });
    if !is_authorized(host, presented, state.port, &state.auth_token) {
        return StatusCode::FORBIDDEN.into_response();
    }
    next.run(req).await
}

fn is_authorized(
    host: Option<&str>,
    presented_token: Option<&str>,
    expected_port: u16,
    expected_token: &str,
) -> bool {
    // RFC 9110 §7.2: the Host field value is case-insensitive.
    let host_ok = matches!(
        host,
        Some(h)
            if h.eq_ignore_ascii_case(&format!("localhost:{expected_port}"))
                || h.eq_ignore_ascii_case(&format!("127.0.0.1:{expected_port}"))
    );
    host_ok && presented_token == Some(expected_token)
}

fn token_from_query(query: &str) -> Option<&str> {
    query.split('&').find_map(|kv| kv.strip_prefix("token="))
}

// ── SSE handler ───────────────────────────────────────────────────────

async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.bridge.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(sse) => Some(Ok(Event::default().event(sse.event_type).data(sse.data))),
        Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
            log::warn!("SSE client lagged by {n} messages");
            None
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ── Static assets ─────────────────────────────────────────────────────

async fn serve_index() -> impl axum::response::IntoResponse {
    axum::response::Html(crate::web::assets::INDEX_HTML)
}

async fn serve_static(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl axum::response::IntoResponse {
    let (content, content_type) = match path.as_str() {
        "pico.min.css" => (crate::web::assets::PICO_CSS, "text/css"),
        "alpine.min.js" => (crate::web::assets::ALPINE_JS, "application/javascript"),
        _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
    };

    axum::response::Response::builder()
        .header("Content-Type", content_type)
        .header("Cache-Control", "public, max-age=86400")
        .body(axum::body::Body::from(content))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_from_query_parses() {
        assert_eq!(token_from_query("token=abc123&x=1"), Some("abc123"));
        assert_eq!(token_from_query("x=1"), None);
        assert_eq!(token_from_query(""), None);
    }

    #[test]
    fn generate_token_is_hex() {
        let t = generate_token();
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn is_authorized_requires_host_and_token() {
        let port = 19823;
        let token = "t0k3n";
        assert!(is_authorized(
            Some("localhost:19823"),
            Some(token),
            port,
            token
        ));
        assert!(is_authorized(
            Some("127.0.0.1:19823"),
            Some(token),
            port,
            token
        ));
        // Host is case-insensitive per RFC 9110.
        assert!(is_authorized(
            Some("LOCALHOST:19823"),
            Some(token),
            port,
            token
        ));
        // DNS rebinding: attacker-controlled origin in Host.
        assert!(!is_authorized(
            Some("evil.example.com"),
            Some(token),
            port,
            token
        ));
        // Wrong / missing token.
        assert!(!is_authorized(
            Some("localhost:19823"),
            Some("wrong"),
            port,
            token
        ));
        assert!(!is_authorized(Some("localhost:19823"), None, port, token));
        assert!(!is_authorized(None, Some(token), port, token));
    }
}
