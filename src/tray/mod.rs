use tokio::sync::mpsc::Sender;

#[cfg(any(target_os = "windows", target_os = "macos"))]
pub mod desktop_impl;
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub use desktop_impl::TrayController;

#[cfg(target_os = "linux")]
pub mod linux_impl;
#[cfg(target_os = "linux")]
pub use linux_impl::TrayController;

pub const ICON_BYTES: &[u8] = include_bytes!("../../assets/gemini.png");

#[derive(Debug)]
pub enum TrayError {
    Build(String),
    #[allow(dead_code)]
    Icon(String),
}

impl std::fmt::Display for TrayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Build(s) => write!(f, "build: {s}"),
            Self::Icon(s) => write!(f, "icon: {s}"),
        }
    }
}
impl std::error::Error for TrayError {}

pub fn dispatch_show(url: String) {
    if let Err(e) = open::that(&url) {
        log::error!("Failed to open browser from tray: {}", e);
    }
}

pub fn dispatch_quit(tx: Sender<()>) {
    // Send stop signal
    let _ = tx.try_send(());
}
