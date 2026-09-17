//! ProxyBridge integration – Proxifier-like kernel-level interception.
//!
//! ProxyBridge (github.com/InterceptSuite/ProxyBridge) intercepts traffic at
//! the kernel level (WinDivert on Windows, NFQUEUE on Linux) and routes it
//! through SOCKS5 / HTTP proxies.
//!
//! Instead of shelling out to a CLI (whose option sets differ wildly between
//! the Linux and Windows builds), we drive its C API directly — the same way
//! the official GUI does. This gives us:
//!
//! - graceful start/stop (no orphaned child processes, no stale iptables
//!   rules left behind by a SIGKILL);
//! - log lines via the library's callback, forwarded into our log stream;
//! - no CLI-argument compatibility problems.
//!
//! How that C API is reached differs per platform, but both supported
//! platforms link it in at build time (`build.rs` compiles the vendored source
//! into the binary):
//!
//! - **Linux** compiles `vendor/proxybridge-3.2.0/`. Run-time loading is simply
//!   not available to the release build: it is a fully static musl binary,
//!   which has no dynamic loader at all — musl's `dlopen` is a stub that always
//!   fails with "Dynamic loading not supported".
//! - **Windows x86_64** compiles `vendor/proxybridge-win-4.0.0/`, which carries
//!   the same local DNS patch as the Linux tree, so upstream's prebuilt
//!   `ProxyBridgeCore.dll` is no longer shipped or loaded. WinDivert is
//!   untouched: it stays the upstream DLL, resolved by the loader at run time.
//!
//! macOS is intentionally **not** supported, and neither is Windows arm64
//! (upstream ships no WinDivert build for it): the whole integration is stubbed
//! out there.

use std::path::{Path, PathBuf};

use tokio::sync::mpsc;

use crate::backend::launch_options::LaunchOptions;

#[cfg(proxybridge_native)]
use std::ffi::c_int;
#[cfg(proxybridge_native)]
use std::ffi::{c_char, CStr, CString};
#[cfg(proxybridge_native)]
use std::sync::{Mutex, OnceLock};

// ── C API types (see upstream `src/ProxyBridge.h`) ────────────────────

/// `ProxyType` enum in the C header.
#[cfg(proxybridge_native)]
const PROXY_TYPE_SOCKS5: i32 = 1;
/// `RuleProtocol` enum in the C header.
///
/// Rules must cover UDP as well as TCP. The library's `match_rule` skips a
/// TCP-only rule for UDP packets, so a TCP-only rule sends every UDP flow of a
/// listed process — DNS on port 53 included — straight out instead of through
/// the proxy. That is also what made DNS look un-hijacked: ProxyBridge only
/// routes port 53 via the SOCKS5 UDP relay when a rule matches the packet.
#[cfg(proxybridge_native)]
const RULE_PROTOCOL_BOTH: i32 = 2;
/// `RuleAction` enum in the C header.
#[cfg(proxybridge_native)]
const RULE_ACTION_PROXY: i32 = 0;

/// Log callback signature: `void (*)(const char* message)`.
#[cfg(proxybridge_native)]
type LogCallback = unsafe extern "C" fn(*const c_char);

/// Routes library log lines from ProxyBridge's internal threads to a tokio
/// task. The C API has no userdata slot on the callback, so we hand the
/// sender over through a process-global.
#[cfg(proxybridge_native)]
static LOG_TX: OnceLock<Mutex<Option<mpsc::UnboundedSender<String>>>> = OnceLock::new();

#[cfg(proxybridge_native)]
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

#[cfg(proxybridge_native)]
fn set_log_tx(tx: mpsc::UnboundedSender<String>) {
    LOG_TX
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .replace(tx);
}

// ── Platform-specific bindings ────────────────────────────────────────

// Linux: the vendored stack is linked into this executable, so the symbols
// below are ordinary link-time imports. `Bindings` deliberately mirrors the
// Windows one so the call sites further down need no `cfg` of their own.
#[cfg(target_os = "linux")]
extern "C" {
    fn ProxyBridge_SetProxyConfig(
        proxy_type: i32,
        proxy_ip: *const c_char,
        proxy_port: u16,
        username: *const c_char,
        password: *const c_char,
    );
    fn ProxyBridge_AddRule(
        process_name: *const c_char,
        target_hosts: *const c_char,
        target_ports: *const c_char,
        protocol: i32,
        action: i32,
    ) -> u32;
    fn ProxyBridge_SetDnsViaProxy(enable: u8);
    /// Local patch to the vendored C (see `vendor/README.md`).
    fn ProxyBridge_SetDnsRedirect(dns_server: *const c_char, dns_port: c_int);
    fn ProxyBridge_SetLogCallback(callback: Option<LogCallback>);
    fn ProxyBridge_Start() -> u8;
    fn ProxyBridge_Stop() -> u8;
}

#[cfg(target_os = "linux")]
struct Bindings {
    set_proxy_config: unsafe extern "C" fn(i32, *const c_char, u16, *const c_char, *const c_char),
    add_rule: unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, i32, i32) -> u32,
    set_dns_via_proxy: unsafe extern "C" fn(u8),
    set_dns_redirect: unsafe extern "C" fn(*const c_char, c_int),
    set_log_callback: unsafe extern "C" fn(Option<LogCallback>),
    start: unsafe extern "C" fn() -> u8,
    stop: unsafe extern "C" fn() -> u8,
}

/// Statically linked, so every symbol resolves at link time and nothing here
/// can fail — unlike the Windows `dlopen` path.
#[cfg(target_os = "linux")]
fn bind_symbols() -> Bindings {
    Bindings {
        set_proxy_config: ProxyBridge_SetProxyConfig,
        add_rule: ProxyBridge_AddRule,
        set_dns_via_proxy: ProxyBridge_SetDnsViaProxy,
        set_dns_redirect: ProxyBridge_SetDnsRedirect,
        set_log_callback: ProxyBridge_SetLogCallback,
        start: ProxyBridge_Start,
        stop: ProxyBridge_Stop,
    }
}

// Windows: upstream's `ProxyBridgeCore.dll` is no longer loaded at run time.
// The very same source is compiled into this executable by `build.rs` — with a
// local patch applied to it (the DNS redirect, see `vendor/README.md`) — so the
// symbols below are ordinary link-time imports here too. WinDivert stays
// upstream's DLL: it is resolved by the loader at run time, exactly as before.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
extern "C" {
    fn ProxyBridge_AddProxyConfig(
        proxy_type: i32,
        proxy_ip: *const c_char,
        proxy_port: u16,
        username: *const c_char,
        password: *const c_char,
    ) -> u32;
    // Exact upstream v4.0.0 signature: no `target_domains` parameter, which
    // only exists on post-4.0.0 master. Passing an extra pointer would shift
    // `protocol` and `action` into the wrong registers.
    fn ProxyBridge_AddRule(
        process_name: *const c_char,
        target_hosts: *const c_char,
        target_ports: *const c_char,
        protocol: i32,
        action: i32,
        proxy_config_id: u32,
    ) -> u32;
    fn ProxyBridge_SetLocalhostViaProxy(enable: i32);
    /// Local patch to the vendored C (see `vendor/README.md`).
    fn ProxyBridge_SetDnsRedirect(dns_server: *const c_char, dns_port: c_int);
    fn ProxyBridge_SetLogCallback(callback: Option<LogCallback>);
    fn ProxyBridge_Start() -> i32;
    fn ProxyBridge_Stop() -> i32;
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
struct Bindings {
    add_proxy_config:
        unsafe extern "C" fn(i32, *const c_char, u16, *const c_char, *const c_char) -> u32,
    add_rule:
        unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, i32, i32, u32) -> u32,
    set_localhost_via_proxy: unsafe extern "C" fn(i32),
    set_dns_redirect: unsafe extern "C" fn(*const c_char, c_int),
    set_log_callback: unsafe extern "C" fn(Option<LogCallback>),
    // Windows BOOL is a 4-byte int; model it as i32 on the Rust side.
    start: unsafe extern "C" fn() -> i32,
    stop: unsafe extern "C" fn() -> i32,
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn bind_symbols() -> Bindings {
    Bindings {
        add_proxy_config: ProxyBridge_AddProxyConfig,
        add_rule: ProxyBridge_AddRule,
        set_localhost_via_proxy: ProxyBridge_SetLocalhostViaProxy,
        set_dns_redirect: ProxyBridge_SetDnsRedirect,
        set_log_callback: ProxyBridge_SetLogCallback,
        start: ProxyBridge_Start,
        stop: ProxyBridge_Stop,
    }
}

/// A ProxyBridge core that is ready to be driven.
///
/// The stack is compiled into this executable (see `build.rs`), so there is
/// nothing to keep alive. All calls are safe to make from any thread (the core
/// spawns its own worker threads).
#[cfg(proxybridge_native)]
pub struct ProxyBridge {
    b: Bindings,
}

#[cfg(proxybridge_native)]
impl ProxyBridge {
    /// Prepare the ProxyBridge core.
    ///
    /// The stack is already part of this binary (see `build.rs`), so `path` is
    /// ignored and this cannot fail.
    pub fn load(
        path: Option<&Path>,
        log_tx: mpsc::UnboundedSender<String>,
    ) -> Result<Self, String> {
        let _ = path;
        let b = bind_symbols();
        // Install the log sink *before* registering the callback so no early
        // log lines are dropped.
        set_log_tx(log_tx);
        unsafe {
            (b.set_log_callback)(Some(on_pb_log));
        }
        Ok(Self { b })
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
            // Route DNS (port 53) through the proxy as well; without this the
            // library lets lookups from the listed processes go direct even
            // when a rule matches them.
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

        // The pinned v4.0.0 Windows core has no `ProxyBridge_SetDnsViaProxy`
        // (that setter only exists on post-4.0.0 master); there DNS via proxy
        // is on by default, so binding it is neither possible nor needed.
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
                    RULE_PROTOCOL_BOTH,
                    RULE_ACTION_PROXY,
                );
            }
            #[cfg(target_os = "windows")]
            unsafe {
                (self.b.add_rule)(
                    process_c.as_ptr(),
                    hosts_c.as_ptr(),
                    ports_c.as_ptr(),
                    RULE_PROTOCOL_BOTH,
                    RULE_ACTION_PROXY,
                    proxy_config_id,
                );
            }
        }

        #[cfg(target_os = "windows")]
        if options.dns_hijack_enabled() {
            // The Windows resolver does not run inside the requesting process:
            // a browser's lookup is emitted by the DNS Client service
            // (`svchost.exe`), so a rule naming the browser never matches it.
            // Cover the service itself, scoped to port 53 so no other traffic
            // of that shared host process is captured.
            let resolver = CString::new("svchost.exe").expect("static string has no NUL");
            let dns_ports_c = CString::new("53").expect("static string has no NUL");
            unsafe {
                (self.b.add_rule)(
                    resolver.as_ptr(),
                    hosts_c.as_ptr(),
                    dns_ports_c.as_ptr(),
                    RULE_PROTOCOL_BOTH,
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
        unsafe {
            (self.b.stop)();
        }
    }

    /// Send the listed processes' UDP DNS queries to `server` instead of letting
    /// them be answered by whichever resolver the process was pointed at.
    ///
    /// Only meaningful with a core started on `-dns-server-bind`: the C entry
    /// point is a local patch to the vendored sources (see `vendor/README.md`).
    /// Safe to call while interception is running — the packet path reads the
    /// address per packet — and a `None` clears it.
    #[cfg(proxybridge_native)]
    pub fn set_dns_redirect(&self, server: Option<std::net::SocketAddrV4>) {
        let (ip, port) = match server {
            Some(addr) => (
                CString::new(addr.ip().to_string()).ok(),
                i32::from(addr.port()),
            ),
            None => (None, 0),
        };
        unsafe {
            (self.b.set_dns_redirect)(ip.as_ref().map_or(std::ptr::null(), |ip| ip.as_ptr()), port);
        }
    }
}

// ── Platforms without a ProxyBridge core: compiled out ────────────────

#[cfg(not(proxybridge_native))]
pub struct ProxyBridge;

#[cfg(not(proxybridge_native))]
impl ProxyBridge {
    pub fn load(
        _path: Option<&Path>,
        _log_tx: mpsc::UnboundedSender<String>,
    ) -> Result<Self, String> {
        Err(unsupported_reason().to_string())
    }

    pub fn start(&self, _options: &LaunchOptions) -> Result<(), String> {
        Err(unsupported_reason().to_string())
    }

    pub fn stop(&self) {}

    pub fn set_dns_redirect(&self, _server: Option<std::net::SocketAddrV4>) {}
}

#[cfg(not(proxybridge_native))]
fn unsupported_reason() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "ProxyBridge is not supported on macOS"
    }
    #[cfg(all(target_os = "windows", not(target_arch = "x86_64")))]
    {
        "ProxyBridge is not available on Windows arm64: upstream ships no WinDivert build for it"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "ProxyBridge is not supported on this platform"
    }
}

// ── Location / activation helpers ─────────────────────────────────────

/// Returns true if ProxyBridge integration should be active for this
/// configuration (enabled, has processes, and not in TUN mode — TUN already
/// provides system-wide routing).
#[cfg(proxybridge_native)]
pub fn is_active(options: &LaunchOptions) -> bool {
    options.proxybridge_enabled && !options.proxybridge_processes.is_empty() && !options.tun_mode
}

#[cfg(not(proxybridge_native))]
pub fn is_active(_options: &LaunchOptions) -> bool {
    false
}

/// Locate a ProxyBridge core library to load.
///
/// Always `None`: the core is compiled into this binary by `build.rs` (see
/// `vendor/README.md`) on every platform, Windows included — upstream's
/// `ProxyBridgeCore.dll` is no longer loaded at run time. The `proxybridge_path`
/// setting is still tolerated for hand-edited settings files, but it no longer
/// selects a library.
#[cfg(proxybridge_native)]
pub fn find_proxybridge_library(user_path: Option<&str>, _app_dir: &Path) -> Option<PathBuf> {
    if user_path.is_some_and(|p| !p.trim().is_empty()) {
        log::debug!(
            "proxybridge_path is ignored: ProxyBridge is linked into this binary statically"
        );
    }
    None
}

#[cfg(not(proxybridge_native))]
pub fn find_proxybridge_library(_user_path: Option<&str>, _app_dir: &Path) -> Option<PathBuf> {
    None
}

/// Returns a platform-specific hint explaining why ProxyBridge could not be
/// started, including the official download URL where one is still needed.
#[cfg(proxybridge_native)]
pub fn install_hint() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "ProxyBridge is built into this binary; it needs the WinDivert driver, \
         which is installed from the bundled copy when missing (and requires \
         administrator rights). If that still fails, install WinDivert, or \
         ProxyBridge which bundles it, from \
         https://interceptsuite.com/download/proxybridge."
    }
    #[cfg(target_os = "linux")]
    {
        "ProxyBridge is built into this binary. Starting it requires root and a \
         kernel with NFQUEUE support (nfnetlink_queue)."
    }
}

#[cfg(not(proxybridge_native))]
pub fn install_hint() -> &'static str {
    ""
}

/// Extract (host, port) from a "host:port" or "host" string.
/// Defaults port to 1080 if not specified.
#[cfg(proxybridge_native)]
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

    #[cfg(proxybridge_native)]
    #[test]
    fn extract_host_port_parses_correctly() {
        assert_eq!(extract_host_port("127.0.0.1:1080"), ("127.0.0.1", 1080));
        assert_eq!(extract_host_port("0.0.0.0:8888"), ("0.0.0.0", 8888));
        assert_eq!(extract_host_port("127.0.0.1"), ("127.0.0.1", 1080));
        assert_eq!(extract_host_port(":1080"), ("", 1080));
        assert_eq!(extract_host_port("[::1]:1080"), ("::1", 1080));
    }

    /// On Linux the vendored C stack is linked into the binary rather than
    /// `dlopen`ed, so this walks the whole path: statically linked symbol →
    /// C `log_message` → our `extern "C"` callback → the log channel. It fails
    /// at link time if the archive ever stops being linked in, and at run time
    /// if the callback wiring breaks.
    #[cfg(target_os = "linux")]
    #[test]
    fn linked_c_api_logs_through_the_callback() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let pb = ProxyBridge::load(None, tx).expect("a statically linked core always loads");

        let process = CString::new("zju-connect-gui-test").expect("no NUL in the name");
        let any = CString::new("*").expect("no NUL in the pattern");
        unsafe {
            (pb.b.add_rule)(
                process.as_ptr(),
                any.as_ptr(),
                any.as_ptr(),
                RULE_PROTOCOL_BOTH,
                RULE_ACTION_PROXY,
            );
        }

        let line = rx
            .try_recv()
            .expect("ProxyBridge_AddRule should log the rule it just added");
        assert!(
            line.contains("added rule id"),
            "unexpected log line: {line}"
        );
    }

    /// The DNS redirect entry point is a local patch to the vendored C (see
    /// `vendor/README.md`), so this checks the two things a patch can silently
    /// break: the symbol being linked in at all, and the address surviving the
    /// C-side `inet_pton` round trip (the C setter logs it back in dotted form).
    #[cfg(proxybridge_native)]
    #[test]
    fn dns_redirect_api_round_trips_the_address() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let pb = ProxyBridge::load(None, tx).expect("a statically linked core always loads");

        pb.set_dns_redirect(Some("127.0.0.2:5353".parse().expect("literal address")));
        let line = rx
            .try_recv()
            .expect("ProxyBridge_SetDnsRedirect should log the redirect");
        assert!(line.contains("dns redirect 127.0.0.2:5353"), "{line}");

        pb.set_dns_redirect(None);
        let line = rx
            .try_recv()
            .expect("clearing the redirect should be logged too");
        assert!(line.contains("dns redirect disabled"), "{line}");
    }

    /// The rule protocol is passed to the C library as a bare `int`, so nothing
    /// but this test ties it to the vendored header `build.rs` compiles in.
    /// It must stay `BOTH`: `match_rule` in `vendor/proxybridge-3.2.0/
    /// ProxyBridge.c` drops a `TCP`-only rule for UDP packets, which is what
    /// silently sent DNS (UDP :53) direct and made it look un-hijacked.
    #[cfg(target_os = "linux")]
    #[test]
    fn rule_protocol_covers_udp_and_matches_the_vendored_header() {
        let header = include_str!("../../../vendor/proxybridge-3.2.0/ProxyBridge.h");
        assert!(
            header.contains("RULE_PROTOCOL_TCP = 0")
                && header.contains("RULE_PROTOCOL_UDP = 1")
                && header.contains("RULE_PROTOCOL_BOTH = 2"),
            "vendored ProxyBridge.h renumbered RuleProtocol; update the constants"
        );
        assert_eq!(RULE_PROTOCOL_BOTH, 2);
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
        let opts = LaunchOptions {
            proxybridge_enabled: true,
            proxybridge_processes: vec!["chrome.exe".into()],
            ..LaunchOptions::default()
        };
        assert!(!is_active(&opts));
    }
}
