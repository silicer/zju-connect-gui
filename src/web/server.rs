//! Axum web server: router construction, port binding, SSE streaming.

use std::io;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::web::bridge::WebUiBridge;
use crate::web::handlers::{self, AppState};
use tokio::sync::{oneshot, Mutex};
use zju_connect_gui::backend::proxy::ProxyManager;
use zju_connect_gui::backend::settings_store::UserSettingsStore;

const PORT_FILE: &str = "web_port.txt";
const DEFAULT_PORT: u16 = 19823;

/// Run the web server, returning the bound port and the server join handle.
pub async fn run(
    app_dir: PathBuf,
    manager: ProxyManager,
    settings: Arc<UserSettingsStore>,
    bridge: Arc<WebUiBridge>,
) -> io::Result<(u16, AppState, tokio::task::JoinHandle<()>)> {
    let port = find_port(&app_dir).await?;
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    let (elevate_tx, _elevate_rx) = oneshot::channel();

    let state = AppState {
        manager: manager.clone(),
        settings,
        bridge: bridge.clone(),
        port,
        elevate_tx: Arc::new(Mutex::new(Some(elevate_tx))),
    };

    let router = build_router(state.clone());

    let listener = tokio::net::TcpListener::bind(addr).await?;
    log::info!("web UI listening on http://{addr}");

    let handle = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, router).await {
            log::error!("web server exited: {err}");
        }
    });

    Ok((port, state, handle))
}

/// Try the last-used port, falling back to an OS-assigned random port.
async fn find_port(app_dir: &Path) -> io::Result<u16> {
    // 1. Try the last-used port from file.
    let port_file = app_dir.join(PORT_FILE);
    if let Ok(contents) = tokio::fs::read_to_string(&port_file).await {
        if let Ok(port) = contents.trim().parse::<u16>() {
            if port >= 1024 {
                let addr = SocketAddr::from(([127, 0, 0, 1], port));
                if TcpListener::bind(addr).is_ok() {
                    // Don't persist yet — do that after axum binds
                    return Ok(port);
                }
                log::info!("last port {port} is occupied, picking a new one");
            }
        }
    }

    // 2. Try the default port.
    let addr = SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT));
    if TcpListener::bind(addr).is_ok() {
        return Ok(DEFAULT_PORT);
    }

    // 3. Fall back to OS-assigned.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
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
        // Static assets
        .route("/", get(serve_index))
        .route("/static/{*path}", get(serve_static))
        .with_state(state)
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
        "htmx.min.js" => (crate::web::assets::HTMX_JS, "application/javascript"),
        "alpine.min.js" => (crate::web::assets::ALPINE_JS, "application/javascript"),
        _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
    };

    axum::response::Response::builder()
        .header("Content-Type", content_type)
        .header("Cache-Control", "public, max-age=86400")
        .body(axum::body::Body::from(content))
        .unwrap()
}
