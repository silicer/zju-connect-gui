use askama::Template;
use axum::response::sse::{Event, Sse};
use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::{get, post},
    Form, Router,
};
use futures_util::stream::Stream;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::services::ServeDir;
use zju_connect_gui::backend::launch_options::LaunchOptions;
use zju_connect_gui::backend::proxy::{InputKind, ProxyEvent, ProxyManager};
use zju_connect_gui::backend::settings_store::UserSettingsStore;

#[derive(Clone)]
pub struct AppState {
    pub manager: ProxyManager,
    pub settings_store: Arc<Mutex<UserSettingsStore>>,
    #[allow(dead_code)]
    pub broadcaster: async_broadcast::Sender<ProxyEvent>,
    pub receiver: async_broadcast::Receiver<ProxyEvent>,
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    settings: LaunchOptions,
    running: bool,
    status_message: String,
    logs: Vec<String>,
}

#[derive(Template)]
#[template(path = "input_modal.html")]
struct InputModalTemplate {
    title: String,
    prompt: String,
    kind: String,
}

#[derive(Template)]
#[template(path = "captcha_modal.html")]
struct CaptchaModalTemplate {
    title: String,
    prompt: String,
    base64_image: String,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/events", get(sse_handler))
        .route("/settings", post(save_settings))
        .route("/start", post(start_proxy))
        .route("/stop", post(stop_proxy))
        .route("/submit_input", post(submit_input))
        .route("/submit_captcha", post(submit_captcha))
        .nest_service("/static", ServeDir::new("static"))
        .with_state(state)
}

async fn index_handler(State(state): State<AppState>) -> impl IntoResponse {
    let settings = {
        let store = state.settings_store.lock().await;
        store.load().unwrap_or_default()
    };

    let snapshot = state.manager.snapshot();
    let running = snapshot.session_active;
    let status_message = if snapshot.session_active {
        "Running".to_string()
    } else {
        "Stopped".to_string()
    };

    let logs = vec![];

    let template = IndexTemplate {
        settings,
        running,
        status_message,
        logs,
    };
    Html(template.render().unwrap())
}

#[derive(Deserialize)]
struct SettingsForm {
    username: String,
    password: String,
    socks_bind: String,
    #[allow(dead_code)]
    tun_gateway: Option<String>,
    #[serde(default)]
    tun_mode: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    udp_proxy: Option<String>,
}

async fn save_settings(
    State(state): State<AppState>,
    Form(form): Form<SettingsForm>,
) -> impl IntoResponse {
    let store = state.settings_store.lock().await;
    let mut current = store.load().unwrap_or_default();

    current.username = form.username;
    current.password = form.password;
    current.socks_bind = form.socks_bind;
    current.tun_mode = form.tun_mode == Some("on".to_string());

    if let Err(e) = store.save(current) {
        log::error!("Failed to save settings: {}", e);
    }
    StatusCode::OK
}

async fn start_proxy(State(state): State<AppState>) -> impl IntoResponse {
    let settings = {
        let store = state.settings_store.lock().await;
        store.load().unwrap_or_default()
    };
    if let Err(e) = state.manager.start(settings) {
        log::error!("Failed to start proxy: {}", e);
    }
    StatusCode::OK
}

async fn stop_proxy(State(state): State<AppState>) -> impl IntoResponse {
    if let Err(e) = state.manager.stop() {
        log::error!("Failed to stop proxy: {}", e);
    }
    StatusCode::OK
}

#[derive(Deserialize)]
struct InputForm {
    #[allow(dead_code)]
    kind: String,
    input: String,
}

async fn submit_input(
    State(state): State<AppState>,
    Form(form): Form<InputForm>,
) -> impl IntoResponse {
    if let Err(e) = state.manager.submit_input(&form.input) {
        log::error!("Failed to submit input: {}", e);
    }
    Html("")
}

#[derive(Deserialize)]
struct CaptchaForm {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

async fn submit_captcha(
    State(state): State<AppState>,
    Form(form): Form<CaptchaForm>,
) -> impl IntoResponse {
    let payload = serde_json::json!({
        "coordinates": [[form.x, form.y]],
        "width": form.width,
        "height": form.height,
    });
    if let Err(e) = state.manager.submit_input(&payload.to_string()) {
        log::error!("Failed to submit captcha: {}", e);
    }
    Html("")
}

async fn sse_handler(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut receiver = state.receiver.clone();

    let stream = async_stream::stream! {
        while let Ok(event) = receiver.recv().await {
            let html_fragment = match event {
                ProxyEvent::Log(line) => {
                    let log_html = format!("<div class=\"log-entry\">{}</div>", askama_escape::escape(&line, askama_escape::Html));
                    Some(Event::default().event("log").data(log_html))
                }
                ProxyEvent::State { state: _, message, running, awaiting, .. } => {
                    let status;
                    if let Some(msg) = message {
                        status = msg;
                    } else if let Some(reason) = awaiting {
                        status = format!("Waiting for input: {}", reason);
                    } else if running {
                        status = "Running".to_string();
                    } else {
                        status = "Stopped".to_string();
                    }

                    let oob_html = format!(
                        r#"<span id="status-text" hx-swap-oob="true">{}</span>
                        <div id="controls" class="grid" style="margin-bottom: 1rem;" hx-swap-oob="true">
                            <button hx-post="/start" hx-swap="none" id="start-btn" {start_disabled}>Start</button>
                            <button hx-post="/stop" hx-swap="none" id="stop-btn" class="secondary" {stop_disabled}>Stop</button>
                        </div>"#,
                        askama_escape::escape(&status, askama_escape::Html),
                        start_disabled = if running { "disabled" } else { "" },
                        stop_disabled = if !running { "disabled" } else { "" }
                    );
                    Some(Event::default().event("update").data(oob_html))
                }
                ProxyEvent::NeedInput { kind, prompt } => {
                    let template = InputModalTemplate {
                        title: if matches!(kind, InputKind::Sms) { "SMS Code".into() } else { "Input Required".into() },
                        prompt,
                        kind: if matches!(kind, InputKind::Sms) { "Sms".into() } else { "Totp".into() },
                    };
                    Some(Event::default().event("modal").data(template.render().unwrap()))
                }
                ProxyEvent::NeedCaptcha { base64, .. } => {
                    let template = CaptchaModalTemplate {
                        title: "Captcha Required".into(),
                        prompt: "Please click the correct locations on the image in order.".into(),
                        base64_image: base64,
                    };
                    Some(Event::default().event("modal").data(template.render().unwrap()))
                }
                ProxyEvent::Error(msg) => {
                    let oob_html = format!(
                        r#"<span id="status-text" hx-swap-oob="true">Error: {}</span>"#,
                        askama_escape::escape(&msg, askama_escape::Html)
                    );
                    Some(Event::default().event("update").data(oob_html))
                }
            };

            if let Some(ev) = html_fragment {
                yield Ok(ev);
            }
        }
    };

    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}
