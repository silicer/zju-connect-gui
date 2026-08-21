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
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("rundll32")
            .arg("url.dll,FileProtocolHandler")
            .arg(&url)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
}
