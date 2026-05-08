use thiserror::Error;

#[derive(Debug, Error)]
pub enum ElevationError {
    #[error("elevation is not supported on this platform")]
    Unsupported,
    #[error("user declined elevation prompt")]
    UserCancelled,
    #[error("io error during elevation: {0}")]
    Io(String),
    #[error("elevation failed: {0}")]
    Failed(String),
}

#[derive(Debug, Error)]
pub enum WaitError {
    #[error("io error during wait: {0}")]
    Io(#[from] std::io::Error),
    #[error("timed out waiting for process exit")]
    Timeout,
}

#[cfg(windows)]
mod windows_impl;
#[cfg(windows)]
pub use windows_impl::{
    escape_arg, init_console_for_signaling, is_process_elevated, relaunch_self_elevated,
    signal_child_to_quit, wait_for_process_exit,
};

#[cfg(unix)]
mod unix_impl;
#[cfg(unix)]
pub use unix_impl::{
    escape_arg, init_console_for_signaling, is_process_elevated, relaunch_self_elevated,
    signal_child_to_quit, wait_for_process_exit,
};
