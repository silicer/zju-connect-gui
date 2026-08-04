//! WebUiBridge – implements UiBridge by forwarding ProxyEvents into a
//! tokio broadcast channel. SSE handlers subscribe to the receiver half
//! and stream events to connected browser clients.

use crate::backend::proxy::{ProxyEvent, UiBridge};
use tokio::sync::broadcast;

/// Wrapper for a ProxyEvent that can be sent through a broadcast channel.
#[derive(Debug, Clone)]
pub struct SseEvent {
    /// SSE event type: "log", "state", "need_input", "need_captcha", "error"
    pub event_type: &'static str,
    /// JSON payload
    pub data: String,
}

impl SseEvent {
    pub fn from_proxy_event(event: &ProxyEvent) -> Self {
        match event {
            ProxyEvent::Log(line) => Self {
                event_type: "log",
                data: serde_json::json!({ "line": line }).to_string(),
            },
            ProxyEvent::State {
                state,
                message,
                awaiting,
                running,
                retry_attempt,
                retry_delay_ms,
            } => Self {
                event_type: "state",
                data: serde_json::json!({
                    "state": state.as_str(),
                    "message": message,
                    "awaiting": awaiting,
                    "running": running,
                    "retry_attempt": retry_attempt,
                    "retry_delay_ms": retry_delay_ms,
                })
                .to_string(),
            },
            ProxyEvent::NeedInput { kind, prompt } => Self {
                event_type: "need_input",
                data: serde_json::json!({
                    "kind": kind.as_str(),
                    "prompt": prompt,
                })
                .to_string(),
            },
            ProxyEvent::NeedCaptcha {
                base64,
                updated_at_ms,
            } => Self {
                event_type: "need_captcha",
                data: serde_json::json!({
                    "base64": base64,
                    "updated_at_ms": updated_at_ms,
                })
                .to_string(),
            },
            ProxyEvent::Error(msg) => Self {
                event_type: "error",
                data: serde_json::json!({ "message": msg }).to_string(),
            },
        }
    }
}

pub struct WebUiBridge {
    tx: broadcast::Sender<SseEvent>,
}

impl WebUiBridge {
    pub fn new(capacity: usize) -> (Self, broadcast::Receiver<SseEvent>) {
        let (tx, rx) = broadcast::channel(capacity);
        (Self { tx }, rx)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SseEvent> {
        self.tx.subscribe()
    }
}

impl UiBridge for WebUiBridge {
    fn emit_event(&self, event: ProxyEvent) {
        let sse = SseEvent::from_proxy_event(&event);
        // broadcast send fails only if there are no receivers – that's fine,
        // no browser connected yet.
        let _ = self.tx.send(sse);
    }

    fn show_window(&self) {
        // For the web UI there is no window to raise. The browser is already
        // open and the SSE modal event will grab the user's attention.
        // We could send a desktop notification here in the future.
    }
}
