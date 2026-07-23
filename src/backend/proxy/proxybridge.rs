//! ProxyBridge integration – manages the ProxyBridge CLI child process.
//!
//! ProxyBridge (github.com/InterceptSuite/ProxyBridge) is a cross-platform
//! Proxifier-like tool that uses kernel-level packet interception (WinDivert
//! on Windows, NFQUEUE on Linux, Network Extension on macOS) to redirect
//! traffic from specific processes through SOCKS5 / HTTP proxies.
//!
//! This module generates the correct CLI command for the current platform and
//! provides helpers to locate the ProxyBridge binary.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use crate::backend::launch_options::LaunchOptions;

/// Name of the ProxyBridge CLI binary on the current platform.
#[cfg(target_os = "windows")]
const PB_CLI_NAME: &str = "ProxyBridge_CLI.exe";

#[cfg(target_os = "linux")]
const PB_CLI_NAME: &str = "ProxyBridge";

#[cfg(target_os = "macos")]
const PB_CLI_NAME: &str = "ProxyBridge_CLI";

/// Names of companion files that must sit next to the CLI binary.
#[cfg(target_os = "windows")]
const PB_COMPANION_FILES: &[&str] = &["ProxyBridgeCore.dll"];

#[cfg(target_os = "linux")]
const PB_COMPANION_FILES: &[&str] = &["libproxybridge.so"];

#[cfg(target_os = "macos")]
const PB_COMPANION_FILES: &[&str] = &[];

/// Default installation directories to search when the user hasn't
/// provided an explicit path.
#[cfg(target_os = "windows")]
fn default_install_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\Program Files\ProxyBridge"),
        PathBuf::from(r"C:\Program Files (x86)\ProxyBridge"),
    ]
}

#[cfg(target_os = "linux")]
fn default_install_dirs() -> Vec<PathBuf> {
    vec![PathBuf::from("/usr/local/bin"), PathBuf::from("/usr/bin")]
}

#[cfg(target_os = "macos")]
fn default_install_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/Applications/ProxyBridge.app/Contents/MacOS"),
    ]
}

/// Locate the ProxyBridge CLI binary.
///
/// Resolution order:
/// 1. User-supplied `proxybridge_path` (pointing at the directory or the exe).
/// 2. Bundled binary next to our own executable (CI builds).
/// 3. `PATH` lookup.
/// 4. Well-known install directories (e.g. `C:\Program Files\ProxyBridge`).
///
/// Returns `None` when no usable binary could be found.
pub fn find_proxybridge_binary(user_path: Option<&str>, app_dir: &Path) -> Option<PathBuf> {
    // 1) User-supplied path
    if let Some(user) = user_path {
        let p = Path::new(user);
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            app_dir.join(p)
        };
        // If the user gave us a directory, append the CLI name.
        if resolved.is_dir() {
            return Some(resolved.join(PB_CLI_NAME));
        }
        // If they gave us a file directly, check it exists.
        if resolved.is_file() {
            return Some(resolved);
        }
    }

    // 2) Bundled next to our own exe
    let bundled = bundled_proxybridge_dir(app_dir);
    let bundled_cli = bundled.join(PB_CLI_NAME);
    if bundled_cli.is_file() {
        log::info!("using bundled proxybridge at {}", bundled_cli.display());
        return Some(bundled_cli);
    }

    // 3) PATH
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(if cfg!(windows) { ';' } else { ':' }) {
            let candidate = Path::new(dir).join(PB_CLI_NAME);
            if candidate.is_file() {
                log::info!("found proxybridge on PATH: {}", candidate.display());
                return Some(candidate);
            }
        }
    }

    // 4) Well-known install directories
    for dir in default_install_dirs() {
        let candidate = dir.join(PB_CLI_NAME);
        if candidate.is_file() {
            log::info!("found proxybridge at {}", candidate.display());
            return Some(candidate);
        }
    }

    log::debug!("proxybridge CLI not found ({})", PB_CLI_NAME);
    None
}

/// Returns the directory where bundled ProxyBridge files reside relative to our
/// own executable. In CI builds the build script places them in a `proxybridge/`
/// subdirectory next to the output binary.
fn bundled_proxybridge_dir(app_dir: &Path) -> PathBuf {
    // In production the app_dir is the parent of current_exe, so pb files sit at
    //   <app_dir>/proxybridge/
    app_dir.join("proxybridge")
}

/// Copy files from the bundle directory to a working directory.
///
/// On Linux, `libproxybridge.so` needs to be next to the CLI binary (or on
/// `LD_LIBRARY_PATH`), so we copy both the CLI and the library. On Windows
/// we copy the CLI and the DLL. On macOS there are no companion files.
pub fn ensure_companion_files(bin_dir: &Path, app_dir: &Path) -> std::io::Result<()> {
    if PB_COMPANION_FILES.is_empty() {
        return Ok(());
    }
    let bundled = bundled_proxybridge_dir(app_dir);
    for name in PB_COMPANION_FILES {
        let src = bundled.join(name);
        let dst = bin_dir.join(name);
        if !dst.exists() && src.exists() {
            std::fs::copy(&src, &dst)?;
            log::debug!("copied {} -> {}", src.display(), dst.display());
        }
    }
    Ok(())
}

/// Build a `tokio::process::Command` to launch ProxyBridge with the given
/// settings. The CLI interface is intentionally uniform across platforms:
///
/// ```text
/// ProxyBridge_CLI --proxy socks5://<bind> \
///     --rule "proc1:*:*:TCP:PROXY" \
///     --rule "proc2:*:*:TCP:PROXY" \
///     --verbose 1
/// ```
///
/// On Linux (v3.2.0) we additionally pass `--dns-via-proxy true`.
/// On Windows/macOS (v4.0.0) Domain Name Forwarding is automatic.
pub fn build_command(
    binary: &Path,
    options: &LaunchOptions,
    app_dir: &Path,
) -> std::io::Result<tokio::process::Command> {
    let socks_addr = extract_host_port(&options.socks_bind);

    let mut cmd = tokio::process::Command::new(binary);

    // --proxy
    cmd.arg("--proxy")
        .arg(format!("socks5://{}:{}", socks_addr.0, socks_addr.1));

    // --rule for each process
    if options.proxybridge_processes.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "proxybridge_processes is empty",
        ));
    }

    let process_list = options.proxybridge_processes.join(";");
    cmd.arg("--rule")
        .arg(format!("{}:*:*:TCP:PROXY", process_list));

    // Exclude ProxyBridge itself to prevent proxy loops
    cmd.arg("--rule")
        .arg(format!("{}:*:*:BOTH:DIRECT", PB_CLI_NAME));

    // --verbose 1 (log messages only)
    cmd.arg("--verbose").arg("1");

    // On Linux v3.2.0, DNS via proxy is a separate flag (default true, but be explicit)
    #[cfg(target_os = "linux")]
    {
        cmd.arg("--dns-via-proxy").arg("true");
    }

    cmd.current_dir(app_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    Ok(cmd)
}

/// Extract (host, port) from a "host:port" or "host" string.
/// Defaults port to 1080 if not specified.
fn extract_host_port(bind: &str) -> (&str, u16) {
    if let Some((host, port_str)) = bind.rsplit_once(':') {
        if let Ok(port) = port_str.parse::<u16>() {
            return (host, port);
        }
    }
    (bind, 1080)
}

/// Returns true if ProxyBridge integration should be active for this
/// configuration (enabled, has processes, and not in TUN mode — TUN
/// already provides system-wide routing).
pub fn is_active(options: &LaunchOptions) -> bool {
    options.proxybridge_enabled && !options.proxybridge_processes.is_empty() && !options.tun_mode
}

/// Returns a platform-specific hint for installing ProxyBridge, including
/// the official download URL. Used in UI prompts and log messages.
pub fn install_hint() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "https://interceptsuite.com/download/proxybridge"
    }
    #[cfg(target_os = "macos")]
    {
        "macOS 上使用 ProxyBridge 需要先安装网络扩展（Network Extension）。\n\
         请访问 https://interceptsuite.com/download/proxybridge 下载 .pkg 安装包，\n\
         安装后需要在 系统设置 → 隐私与安全性 → 网络扩展 中批准 ProxyBridge。"
    }
    #[cfg(target_os = "linux")]
    {
        "https://interceptsuite.com/download/proxybridge"
    }
}

/// Returns true if ProxyBridge needs administrator/root privileges.
/// Currently: always true when active (WinDivert on Windows, NFQUEUE on Linux).
pub fn needs_elevation() -> bool {
    // All platforms require admin/root because ProxyBridge uses kernel-level
    // packet interception.
    true
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
    }

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
}
