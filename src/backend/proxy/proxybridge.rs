//! ProxyBridge integration – in-process library binding.
//!
//! ProxyBridge (github.com/InterceptSuite/ProxyBridge) is a cross-platform
//! Proxifier-like tool that intercepts traffic at the kernel level (WinDivert
//! on Windows, NFQUEUE on Linux) and routes it through SOCKS5 / HTTP proxies.
//!
//! Instead of shelling out to a CLI (whose option sets differ wildly between
//! the Linux and Windows builds), we load the core library directly
//! (`libproxybridge.so` / `ProxyBridgeCore.dll`) and drive its C API — the
//! same way the official GUI does. This gives us:
//!
//! - graceful start/stop (no orphaned child processes, no stale iptables
//!   rules left behind by a SIGKILL);
//! - log lines via the library's callback, forwarded into our log stream;
//! - no CLI-argument compatibility problems.
//!
//! macOS is intentionally **not** supported: upstream ships no reusable
//! library or CLI for macOS (only a Swift GUI + Network Extension), so the
//! whole integration is stubbed out there.

use std::path::{Path, PathBuf};

use tokio::sync::mpsc;

use crate::backend::launch_options::LaunchOptions;

#[cfg(not(target_os = "macos"))]
use libloading::{Library, Symbol};
#[cfg(not(target_os = "macos"))]
use std::ffi::{c_char, CStr, CString};
#[cfg(not(target_os = "macos"))]
use std::sync::{Mutex, OnceLock};

/// Name of the ProxyBridge core library on the current platform.
#[cfg(target_os = "windows")]
pub const PB_LIB_NAME: &str = "ProxyBridgeCore.dll";

#[cfg(target_os = "linux")]
pub const PB_LIB_NAME: &str = "libproxybridge.so";

// ── C API types (see upstream `src/ProxyBridge.h`) ────────────────────

/// `ProxyType` enum in the C header.
#[cfg(not(target_os = "macos"))]
const PROXY_TYPE_SOCKS5: i32 = 1;
/// `RuleProtocol` enum in the C header.
#[cfg(not(target_os = "macos"))]
const RULE_PROTOCOL_TCP: i32 = 0;
/// `RuleAction` enum in the C header.
#[cfg(not(target_os = "macos"))]
const RULE_ACTION_PROXY: i32 = 0;

/// Log callback signature: `void (*)(const char* message)`.
#[cfg(not(target_os = "macos"))]
type LogCallback = unsafe extern "C" fn(*const c_char);

/// Routes library log lines from ProxyBridge's internal threads to a tokio
/// task. The C API has no userdata slot on the callback, so we hand the
/// sender over through a process-global.
#[cfg(not(target_os = "macos"))]
static LOG_TX: OnceLock<Mutex<Option<mpsc::UnboundedSender<String>>>> = OnceLock::new();

#[cfg(not(target_os = "macos"))]
extern "C" fn on_pb_log(msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(msg) }
        .to_string_lossy()
        .into_owned();
    if let Some(tx) = LOG_TX.get().and_then(|m| m.lock().unwrap().clone()) {
        let _ = tx.send(text);
    }
}

#[cfg(not(target_os = "macos"))]
fn set_log_tx(tx: mpsc::UnboundedSender<String>) {
    LOG_TX
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .replace(tx);
}

/// Resolve a single exported symbol; `*sym` copies the (copyable) value out
/// of the `Symbol` so the struct doesn't borrow the `Library`.
#[cfg(not(target_os = "macos"))]
unsafe fn get_sym<T: Copy>(lib: &Library, name: &[u8]) -> Result<T, String> {
    let sym: Symbol<T> = unsafe { lib.get(name) }
        .map_err(|e| format!("symbol {} missing: {e}", String::from_utf8_lossy(name)))?;
    Ok(*sym)
}

// ── Platform-specific bindings ────────────────────────────────────────

#[cfg(target_os = "linux")]
struct Bindings {
    set_proxy_config: unsafe extern "C" fn(i32, *const c_char, u16, *const c_char, *const c_char),
    add_rule: unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, i32, i32) -> u32,
    set_dns_via_proxy: unsafe extern "C" fn(u8),
    set_log_callback: unsafe extern "C" fn(Option<LogCallback>),
    start: unsafe extern "C" fn() -> u8,
    stop: unsafe extern "C" fn() -> u8,
}

#[cfg(target_os = "linux")]
unsafe fn bind_symbols(lib: &Library) -> Result<Bindings, String> {
    // C `bool` is a 1-byte _Bool; model it as u8 on the Rust side.
    unsafe {
        Ok(Bindings {
            set_proxy_config: get_sym(lib, b"ProxyBridge_SetProxyConfig\0")?,
            add_rule: get_sym(lib, b"ProxyBridge_AddRule\0")?,
            set_dns_via_proxy: get_sym(lib, b"ProxyBridge_SetDnsViaProxy\0")?,
            set_log_callback: get_sym(lib, b"ProxyBridge_SetLogCallback\0")?,
            start: get_sym(lib, b"ProxyBridge_Start\0")?,
            stop: get_sym(lib, b"ProxyBridge_Stop\0")?,
        })
    }
}

#[cfg(target_os = "windows")]
struct Bindings {
    add_proxy_config:
        unsafe extern "C" fn(i32, *const c_char, u16, *const c_char, *const c_char) -> u32,
    // Bound with the master-branch signature (includes `target_domains`);
    // calling an older 6-arg build with an extra argument is harmless on x64.
    add_rule: unsafe extern "C" fn(
        *const c_char,
        *const c_char,
        *const c_char,
        *const c_char,
        i32,
        i32,
        u32,
    ) -> u32,
    set_localhost_via_proxy: unsafe extern "C" fn(i32),
    set_log_callback: unsafe extern "C" fn(Option<LogCallback>),
    start: unsafe extern "C" fn() -> i32,
    stop: unsafe extern "C" fn() -> i32,
}

#[cfg(target_os = "windows")]
unsafe fn bind_symbols(lib: &Library) -> Result<Bindings, String> {
    // Windows BOOL is a 4-byte int; model it as i32 on the Rust side.
    unsafe {
        Ok(Bindings {
            add_proxy_config: get_sym(lib, b"ProxyBridge_AddProxyConfig\0")?,
            add_rule: get_sym(lib, b"ProxyBridge_AddRule\0")?,
            set_localhost_via_proxy: get_sym(lib, b"ProxyBridge_SetLocalhostViaProxy\0")?,
            set_log_callback: get_sym(lib, b"ProxyBridge_SetLogCallback\0")?,
            start: get_sym(lib, b"ProxyBridge_Start\0")?,
            stop: get_sym(lib, b"ProxyBridge_Stop\0")?,
        })
    }
}

/// A loaded ProxyBridge core library with its resolved symbols.
///
/// Holds the `Library` alive for as long as the session lives; all calls are
/// safe to make from any thread (the library spawns its own worker threads).
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub struct ProxyBridge {
    _lib: Library,
    b: Bindings,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl ProxyBridge {
    /// Load the library and resolve its symbols.
    ///
    /// `path` may be a concrete path, or `None` to fall back to the OS loader
    /// search (PATH / DLL search dirs on Windows, ldconfig cache on Linux).
    pub fn load(
        path: Option<&Path>,
        log_tx: mpsc::UnboundedSender<String>,
    ) -> Result<Self, String> {
        let lib = match path {
            Some(p) => unsafe { Library::new(p) },
            None => unsafe { Library::new(PB_LIB_NAME) },
        }
        .map_err(|e| format!("failed to load {PB_LIB_NAME}: {e}"))?;
        let b = unsafe { bind_symbols(&lib) }?;
        // Install the log sink *before* registering the callback so no early
        // log lines are dropped.
        set_log_tx(log_tx);
        unsafe {
            (b.set_log_callback)(Some(on_pb_log));
        }
        Ok(Self { _lib: lib, b })
    }

    /// Configure the SOCKS proxy + per-process rules and start interception.
    ///
    /// The zju-connect SOCKS listener has no authentication, so username and
    /// password are passed as empty strings.
    pub fn start(&self, options: &LaunchOptions) -> Result<(), String> {
        let (host, port) = extract_host_port(&options.socks_bind);
        let host_c = CString::new(host).map_err(|_| "invalid proxy host".to_string())?;
        let empty_c = CString::new("").expect("empty string has no NUL");

        #[cfg(target_os = "linux")]
        {
            unsafe {
                (self.b.set_proxy_config)(
                    PROXY_TYPE_SOCKS5,
                    host_c.as_ptr(),
                    port,
                    empty_c.as_ptr(),
                    empty_c.as_ptr(),
                );
                (self.b.set_dns_via_proxy)(1);
            }
        }

        #[cfg(target_os = "windows")]
        let proxy_config_id = unsafe {
            (self.b.add_proxy_config)(
                PROXY_TYPE_SOCKS5,
                host_c.as_ptr(),
                port,
                empty_c.as_ptr(),
                empty_c.as_ptr(),
            )
        };

        let hosts_c = CString::new("*").expect("static string has no NUL");
        let ports_c = CString::new("*").expect("static string has no NUL");
        #[cfg(target_os = "windows")]
        let domains_c = CString::new("").expect("empty string has no NUL");

        let processes = options.proxybridge_processes.clone();
        for process in &processes {
            let process_c = CString::new(process.as_str())
                .map_err(|_| format!("invalid process name: {process}"))?;
            #[cfg(target_os = "linux")]
            unsafe {
                (self.b.add_rule)(
                    process_c.as_ptr(),
                    hosts_c.as_ptr(),
                    ports_c.as_ptr(),
                    RULE_PROTOCOL_TCP,
                    RULE_ACTION_PROXY,
                );
            }
            #[cfg(target_os = "windows")]
            unsafe {
                (self.b.add_rule)(
                    process_c.as_ptr(),
                    hosts_c.as_ptr(),
                    ports_c.as_ptr(),
                    domains_c.as_ptr(),
                    RULE_PROTOCOL_TCP,
                    RULE_ACTION_PROXY,
                    proxy_config_id,
                );
            }
        }

        #[cfg(target_os = "windows")]
        unsafe {
            // Localhost stays direct; the proxy itself is on 127.0.0.1 and
            // must not be re-routed. This matches the CLI default behavior.
            (self.b.set_localhost_via_proxy)(0);
        }

        #[cfg(target_os = "linux")]
        let ok = unsafe { (self.b.start)() != 0 };
        #[cfg(target_os = "windows")]
        let ok = unsafe { (self.b.start)() != 0 };
        if !ok {
            return Err(
                "ProxyBridge_Start returned false (is the kernel module/driver installed?)"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Stop interception (the library removes its iptables / WinDivert rules).
    pub fn stop(&self) {
        #[cfg(target_os = "linux")]
        unsafe {
            (self.b.stop)();
        }
        #[cfg(target_os = "windows")]
        unsafe {
            (self.b.stop)();
        }
    }
}

// ── macOS: not supported, compiled out ────────────────────────────────

#[cfg(target_os = "macos")]
pub struct ProxyBridge;

#[cfg(target_os = "macos")]
impl ProxyBridge {
    pub fn load(
        _path: Option<&Path>,
        _log_tx: mpsc::UnboundedSender<String>,
    ) -> Result<Self, String> {
        Err("ProxyBridge is not supported on macOS".to_string())
    }

    pub fn start(&self, _options: &LaunchOptions) -> Result<(), String> {
        Err("ProxyBridge is not supported on macOS".to_string())
    }

    pub fn stop(&self) {}
}

// ── Location / activation helpers ─────────────────────────────────────

/// Returns true if ProxyBridge integration should be active for this
/// configuration (enabled, has processes, and not in TUN mode — TUN
/// already provides system-wide routing). Never true on macOS.
#[cfg(not(target_os = "macos"))]
pub fn is_active(options: &LaunchOptions) -> bool {
    options.proxybridge_enabled && !options.proxybridge_processes.is_empty() && !options.tun_mode
}

#[cfg(target_os = "macos")]
pub fn is_active(_options: &LaunchOptions) -> bool {
    false
}

/// Default installation directories to search when the user hasn't provided
/// an explicit path.
#[cfg(target_os = "windows")]
fn default_install_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\Program Files\ProxyBridge"),
        PathBuf::from(r"C:\Program Files (x86)\ProxyBridge"),
    ]
}

#[cfg(target_os = "linux")]
fn default_install_dirs() -> Vec<PathBuf> {
    vec![PathBuf::from("/usr/local/lib"), PathBuf::from("/usr/lib")]
}

/// Locate the ProxyBridge core library.
///
/// Resolution order:
/// 1. User-supplied `proxybridge_path` (pointing at the directory or the lib).
/// 2. Bundled library in `<app_dir>/proxybridge/` (CI builds).
/// 3. Well-known install directories (e.g. `C:\Program Files\ProxyBridge`).
///
/// Returns `None` when no usable file was found; `ProxyBridge::load(None)`
/// then falls back to the OS loader search (PATH on Windows, ldconfig on
/// Linux).
#[cfg(not(target_os = "macos"))]
pub fn find_proxybridge_library(user_path: Option<&str>, app_dir: &Path) -> Option<PathBuf> {
    if let Some(user) = user_path {
        let p = Path::new(user);
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            app_dir.join(p)
        };
        if resolved.is_dir() {
            let candidate = resolved.join(PB_LIB_NAME);
            if candidate.is_file() {
                log::info!("using proxybridge from settings: {}", candidate.display());
                return Some(candidate);
            }
        } else if resolved.is_file() {
            log::info!("using proxybridge from settings: {}", resolved.display());
            return Some(resolved);
        }
    }

    let bundled = app_dir.join("proxybridge").join(PB_LIB_NAME);
    if bundled.is_file() {
        log::info!("using bundled proxybridge at {}", bundled.display());
        return Some(bundled);
    }

    for dir in default_install_dirs() {
        let candidate = dir.join(PB_LIB_NAME);
        if candidate.is_file() {
            log::info!("found proxybridge at {}", candidate.display());
            return Some(candidate);
        }
    }

    log::debug!("proxybridge library not found ({PB_LIB_NAME}); will try OS loader search");
    None
}

#[cfg(target_os = "macos")]
pub fn find_proxybridge_library(_user_path: Option<&str>, _app_dir: &Path) -> Option<PathBuf> {
    None
}

/// Returns a platform-specific hint for installing ProxyBridge, including
/// the official download URL. Used in UI prompts and log messages.
#[cfg(not(target_os = "macos"))]
pub fn install_hint() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "The bundled WinDivert driver is installed automatically when missing. \
         If this still fails, install ProxyBridge (includes the WinDivert driver) \
         from https://interceptsuite.com/download/proxybridge."
    }
    #[cfg(target_os = "linux")]
    {
        "Install ProxyBridge (libnetfilter-queue + root required) from \
         https://interceptsuite.com/download/proxybridge, or bundle it next to this app."
    }
}

#[cfg(target_os = "macos")]
pub fn install_hint() -> &'static str {
    ""
}

/// Extract (host, port) from a "host:port" or "host" string.
/// Defaults port to 1080 if not specified.
#[cfg(not(target_os = "macos"))]
fn extract_host_port(bind: &str) -> (&str, u16) {
    if let Some((host, port_str)) = bind.rsplit_once(':') {
        if let Ok(port) = port_str.parse::<u16>() {
            // Strip IPv6 brackets so `[::1]:1080` yields host `::1`.
            let host = host
                .strip_prefix('[')
                .and_then(|h| h.strip_suffix(']'))
                .unwrap_or(host);
            return (host, port);
        }
    }
    (bind, 1080)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_port_parses_correctly() {
        assert_eq!(extract_host_port("127.0.0.1:1080"), ("127.0.0.1", 1080));
        assert_eq!(extract_host_port("0.0.0.0:8888"), ("0.0.0.0", 8888));
        assert_eq!(extract_host_port("127.0.0.1"), ("127.0.0.1", 1080));
        assert_eq!(extract_host_port(":1080"), ("", 1080));
        assert_eq!(extract_host_port("[::1]:1080"), ("::1", 1080));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn is_active_respects_all_conditions() {
        let mut opts = LaunchOptions::default();
        assert!(!is_active(&opts));

        opts.proxybridge_enabled = true;
        assert!(!is_active(&opts)); // no processes

        opts.proxybridge_processes = vec!["chrome.exe".into()];
        opts.tun_mode = true;
        assert!(!is_active(&opts)); // TUN mode

        opts.tun_mode = false;
        assert!(is_active(&opts));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn is_active_never_true_on_macos() {
        let mut opts = LaunchOptions::default();
        opts.proxybridge_enabled = true;
        opts.proxybridge_processes = vec!["chrome.exe".into()];
        assert!(!is_active(&opts));
    }
}
