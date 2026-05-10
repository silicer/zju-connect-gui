//! System tray icon, split by platform.
//!
//! Linux uses `ksni` so we don't pull GTK3 into the build (StatusNotifierItem
//! over zbus, pure Rust, works on KDE natively and on GNOME with the
//! AppIndicator extension). Windows and macOS use `tray-icon`. Both impls
//! expose a `TrayController` that owns its OS resources for the program's
//! lifetime; `app.rs` wraps construction in `Option<TrayController>` so a
//! failure on a desktop without StatusNotifierItem support is non-fatal.
//!
//! Behaviour contract shared by both backends:
//!   * Left single-click and left double-click on the tray icon both restore
//!     the main window (always show — never hide via the icon itself).
//!   * Right-click opens the context menu, which has three entries:
//!     "显示主窗口" / "隐藏到托盘" / "退出".
//!
//! Every UI-touching action is forwarded back through
//! `slint::invoke_from_event_loop` because both impls deliver events from
//! threads outside the Slint event loop.

use slint::{ComponentHandle, Weak};

use crate::AppWindow;

#[derive(Debug, thiserror::Error)]
pub enum TrayError {
    #[error("failed to decode tray icon: {0}")]
    Icon(String),
    #[error("failed to build tray icon: {0}")]
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

fn dispatch_show(weak: Weak<AppWindow>) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = weak.upgrade() {
            w.window().set_minimized(false);
            let _ = w.show();
        }
    });
}

fn dispatch_hide(weak: Weak<AppWindow>) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = weak.upgrade() {
            let _ = w.hide();
        }
    });
}

fn dispatch_quit(weak: Weak<AppWindow>) {
    // Route through the AppWindow `request-quit` callback so app.rs can do
    // graceful proxy shutdown before tearing down the event loop.
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = weak.upgrade() {
            w.invoke_request_quit();
        }
    });
}
