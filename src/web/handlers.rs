//! Axum HTTP handlers for the web UI API.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::backend::launch_options::{normalize_launch_options, LaunchOptions};
use crate::backend::proxy::ProxyManager;
use crate::backend::settings_store::UserSettingsStore;

/// Shared application state passed to every handler via axum `State`.
#[derive(Clone)]
pub struct AppState {
    pub manager: ProxyManager,
    pub settings: Arc<UserSettingsStore>,
    pub bridge: Arc<crate::web::bridge::WebUiBridge>,
    /// Bound port; also used by the auth middleware for the Host check.
    pub port: u16,
    /// Per-launch auth token; required (header or `?token=`) on all /api routes.
    pub auth_token: String,
    /// Elevation requests from the web UI, consumed by the main event loop.
    /// Capacity-1 channel: requests are single-flight.
    pub elevate_tx: mpsc::Sender<Vec<String>>,
}

// ── Settings ──────────────────────────────────────────────────────────

/// Returns the current LaunchOptions from the settings store, augmented
/// with any live overrides the caller wants (currently none).
pub async fn get_settings(State(state): State<AppState>) -> impl IntoResponse {
    match state.settings.load() {
        Ok(opts) => Json(opts).into_response(),
        Err(err) => {
            log::error!("get_settings: {err}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Saves settings and returns the normalized result.
pub async fn save_settings(
    State(state): State<AppState>,
    Json(raw): Json<LaunchOptions>,
) -> impl IntoResponse {
    let normalized = normalize_launch_options(raw);
    if let Err(err) = state.settings.save(normalized.clone()) {
        log::error!("save_settings: {err}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    Json(normalized).into_response()
}

// ── Proxy control ─────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct StartRequest {
    /// If present, use these options. Otherwise load from settings store.
    #[serde(default)]
    pub options: Option<LaunchOptions>,
}

pub async fn start_proxy(
    State(state): State<AppState>,
    Json(req): Json<StartRequest>,
) -> impl IntoResponse {
    let options = match req.options {
        Some(opts) => normalize_launch_options(opts),
        None => match state.settings.load() {
            Ok(opts) => opts,
            Err(err) => {
                log::error!("start_proxy: settings load: {err}");
                return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
            }
        },
    };

    if let Err(err) = options.validate() {
        return (StatusCode::BAD_REQUEST, err.to_string()).into_response();
    }

    // Save before starting so the next cold-start picks up the latest.
    if let Err(err) = state.settings.save(options.clone()) {
        log::warn!("start_proxy: settings save: {err}");
    }

    match state.manager.start(options) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(err) => {
            let msg = err.to_string();
            let code = match &err {
                crate::backend::proxy::StartError::AlreadyRunning => StatusCode::CONFLICT,
                crate::backend::proxy::StartError::SessionStopped => StatusCode::CONFLICT,
                crate::backend::proxy::StartError::Validation(_) => StatusCode::BAD_REQUEST,
                crate::backend::proxy::StartError::NeedsElevation => {
                    StatusCode::PRECONDITION_FAILED
                }
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            (code, msg).into_response()
        }
    }
}

pub async fn stop_proxy(State(state): State<AppState>) -> impl IntoResponse {
    match state.manager.stop() {
        Ok(()) => StatusCode::OK.into_response(),
        Err(err) => {
            log::error!("stop_proxy: {err}");
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}

// ── Elevate (UAC for TUN mode) ────────────────────────────────────────

#[derive(Deserialize)]
pub struct ElevateRequest {
    /// The options that require elevation (TUN mode).
    pub options: LaunchOptions,
}

pub async fn elevate(
    State(state): State<AppState>,
    Json(req): Json<ElevateRequest>,
) -> impl IntoResponse {
    let options = normalize_launch_options(req.options);

    if let Err(err) = state.settings.save(options) {
        log::error!("elevate: settings save: {err}");
        return (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response();
    }

    use crate::backend::relaunch_args::build_elevated_relaunch_args;
    let parent_pid = std::process::id();
    let args = build_elevated_relaunch_args(parent_pid);

    // The main event loop consumes this and performs the elevated relaunch. It
    // keeps listening afterwards, so a failed relaunch (UAC denied / platform
    // unsupported) can simply be retried from the UI. try_send: while a request
    // is already queued, additional clicks are dropped (single-flight).
    let _ = state.elevate_tx.try_send(args);

    StatusCode::OK.into_response()
}

// ── Interactive input ─────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SubmitInputRequest {
    pub value: String,
    /// Optional input kind ("sms" / "callback" / "captcha") so the backend
    /// can reject submissions that answer a different pending prompt.
    #[serde(default)]
    pub kind: Option<String>,
}

pub async fn submit_input(
    State(state): State<AppState>,
    Json(req): Json<SubmitInputRequest>,
) -> impl IntoResponse {
    match state.manager.submit_input(&req.value, req.kind.as_deref()) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(err) => (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    }
}

// ── Status snapshot ───────────────────────────────────────────────────

#[derive(Serialize)]
pub struct StatusSnapshot {
    pub session_active: bool,
    pub ready: bool,
    pub awaiting: Option<String>,
}

pub async fn get_status(State(state): State<AppState>) -> impl IntoResponse {
    let snap = state.manager.snapshot();
    Json(StatusSnapshot {
        session_active: snap.session_active,
        ready: snap.ready,
        awaiting: snap.awaiting,
    })
    .into_response()
}
