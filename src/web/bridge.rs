//! WebUiBridge – implements UiBridge by forwarding ProxyEvents into a
//! tokio broadcast channel. SSE handlers subscribe to the receiver half
//! and stream events to connected browser clients.
//!
//! The bridge also buffers events emitted before the first SSE subscriber
//! exists. During the elevation flow the elevated process can start the VPN
//! and emit a `need_input` (SMS) event before the browser has reconnected;
//! without replay that event would be lost and the input modal would never
//! appear.

use crate::backend::proxy::{ProxyEvent, UiBridge};
use std::sync::Mutex;
use tokio::sync::broadcast;

/// Number of pre-subscriber events kept for replay. Matches the broadcast
/// capacity so a fast startup cannot grow the buffer without bound.
const PENDING_CAPACITY: usize = 256;

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

struct PendingState {
    /// Whether no SSE subscriber has ever connected. The first subscriber
    /// drains `events` into its broadcast receiver; later subscribers start
    /// from that point (avoids replaying stale modals on every reconnect).
    first_subscriber: bool,
    /// Events emitted while `first_subscriber` was still true.
    events: Vec<SseEvent>,
}

pub struct WebUiBridge {
    tx: broadcast::Sender<SseEvent>,
    pending: Mutex<PendingState>,
}

impl WebUiBridge {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            pending: Mutex::new(PendingState {
                first_subscriber: true,
                events: Vec::new(),
            }),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SseEvent> {
        let rx = self.tx.subscribe();
        let mut pending = self.pending.lock().expect("bridge pending mutex poisoned");
        if pending.first_subscriber {
            pending.first_subscriber = false;
            for sse in pending.events.drain(..) {
                let _ = self.tx.send(sse);
            }
        }
        rx
    }
}

impl UiBridge for WebUiBridge {
    fn emit_event(&self, event: ProxyEvent) {
        let sse = SseEvent::from_proxy_event(&event);
        {
            let mut pending = self.pending.lock().expect("bridge pending mutex poisoned");
            if pending.first_subscriber {
                if pending.events.len() >= PENDING_CAPACITY {
                    pending.events.remove(0);
                }
                pending.events.push(sse.clone());
            }
        }
        // broadcast send fails only if there are no receivers – that's fine,
        // no browser connected yet (the event is kept in `pending` for replay).
        let _ = self.tx.send(sse);
    }

    fn show_window(&self) {
        // For the web UI there is no window to raise. The browser is already
        // open and the SSE modal event will grab the user's attention.
        // We could send a desktop notification here in the future.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::proxy::InputKind;

    #[tokio::test]
    async fn first_subscriber_replays_pre_subscriber_events() {
        let bridge = WebUiBridge::new(8);
        bridge.emit_event(ProxyEvent::NeedInput {
            kind: InputKind::Sms,
            prompt: "Please enter the SMS code".into(),
        });

        let mut rx = bridge.subscribe();
        let event = rx.recv().await.expect("replayed event");
        assert_eq!(event.event_type, "need_input");
        let payload: serde_json::Value = serde_json::from_str(&event.data).unwrap();
        assert_eq!(payload["kind"], "sms");
    }

    #[tokio::test]
    async fn later_subscribers_do_not_replay_stale_pending_events() {
        let bridge = WebUiBridge::new(8);
        bridge.emit_event(ProxyEvent::NeedInput {
            kind: InputKind::Sms,
            prompt: "stale".into(),
        });

        // First subscriber drains the pending buffer.
        let _first = bridge.subscribe();

        // A later subscriber must only see events emitted after it subscribes.
        let mut second = bridge.subscribe();
        bridge.emit_event(ProxyEvent::Log("fresh".into()));

        let event = second.recv().await.expect("fresh event");
        assert_eq!(event.event_type, "log");
    }
}
