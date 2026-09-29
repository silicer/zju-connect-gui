//! System tray icon, split by platform.
//!
//! Linux uses `ksni` (StatusNotifierItem over D-Bus, pure Rust).
//! Windows and macOS use `tray-icon`.
//!
//! Behaviour:
//!   * Left-click opens the web UI in the system browser.
//!   * Right-click opens a context menu: "打开网页" / "退出".
//!
//! A `tokio::sync::oneshot` sender is fired when the user selects "退出",
//! signalling the main loop to begin graceful shutdown.

#[derive(Debug, thiserror::Error)]
pub enum TrayError {
    #[error("failed to decode tray icon: {0}")]
    Icon(String),
    #[error("failed to build tray icon: {0}")]
    #[allow(dead_code)]
    Build(String),
}

const ICON_BYTES: &[u8] = include_bytes!("../assets/gemini.png");

#[cfg(target_os = "linux")]
mod linux_impl;
#[cfg(target_os = "linux")]
pub use linux_impl::TrayController;

#[cfg(not(target_os = "linux"))]
mod desktop_impl;
#[cfg(not(target_os = "linux"))]
pub use desktop_impl::TrayController;

fn open_web_ui(port: u16, token: &str) {
    let url = format!("http://localhost:{port}/?token={token}");
    // Same helper as the startup auto-open: on Linux it re-enters the calling
    // user's session when this is an elevated instance, so the browser is not
    // started as root (Chrome-family browsers refuse to).
    if let Err(err) = zju_connect_gui::backend::external_links::open_url(&url) {
        log::warn!("failed to open the web UI from the tray: {err}");
    }
}
