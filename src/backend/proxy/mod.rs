pub mod captcha;
#[cfg(target_os = "linux")]
pub mod dns_probe;
pub mod logs;
pub mod proxybridge;
pub mod readiness;
pub mod retry;
pub mod windivert;

mod manager;

pub use manager::{
    EipOpener, InputKind, ProxyEvent, ProxyManager, ProxyManagerConfig, ProxyState, StartError,
    StateSnapshot, StopError, SubmitInputError, UiBridge,
};
