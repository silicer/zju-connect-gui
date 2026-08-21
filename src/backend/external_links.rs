//! Launching the EIP page (http://eip.scmcc.com.cn/) in a browser.
//!
//! Two launch modes exist, keyed off `LaunchOptions::tun_mode`:
//!
//! - **TUN / global VPN mode**: traffic is already routed system-wide, so any
//!   browser works. The configured browser (or the OS default handler) is
//!   launched with a *new window* by default.
//! - **Proxy-only mode** (`tun_mode = false`): only the SOCKS5 listener is
//!   up, so the browser must be pointed at the proxy explicitly:
//!   - Chrome-family browsers take `--proxy-server=socks5://host:port`;
//!   - Firefox-family browsers ignore proxy switches, so we generate a
//!     throwaway profile directory with SOCKS prefs and launch it with
//!     `-no-remote -profile <dir>` (a fresh profile also suppresses
//!     first-run pages).
//!
//! When no browser is configured in proxy-only mode we auto-detect an
//! installed one (see [`crate::backend::browser_detect`]) and log the choice.

use std::path::PathBuf;
use std::process::Command;

use crate::backend::browser_detect::{classify_browser, detect_installed_browsers, BrowserKind};
use crate::backend::launch_options::{LaunchOptions, DEFAULT_SOCKS_PORT};

pub const EIP_URL: &str = "http://eip.scmcc.com.cn/";

/// Flags that already mean "open a new window/tab". If the user configured
/// one of these we must not append a duplicate.
const NEW_WINDOW_FLAGS: &[&str] = &["--new-window", "-new-window", "--new-tab", "-new-tab"];

#[derive(Debug, thiserror::Error)]
pub enum OpenEipError {
    #[error("failed to spawn browser process: {0}")]
    Spawn(std::io::Error),
    #[error("failed to prepare Firefox proxy profile: {0}")]
    Profile(String),
    #[error("仅代理模式需要浏览器：未配置且未检测到已安装的浏览器，请在设置中选择")]
    NoBrowserFound,
}

/// Open the EIP page honouring the current mode (see module docs).
pub fn open_eip(options: &LaunchOptions) -> Result<(), OpenEipError> {
    if options.tun_mode {
        open_direct(options)
    } else {
        open_via_proxy(options)
    }
}

/// TUN mode: everything is routed already; just open a new window.
fn open_direct(options: &LaunchOptions) -> Result<(), OpenEipError> {
    let program = options.eip_browser_program.trim();
    if program.is_empty() {
        return default_open_command()
            .spawn()
            .map(|_| ())
            .map_err(OpenEipError::Spawn);
    }
    let kind = classify_browser(program).unwrap_or(BrowserKind::Chrome);
    let mut args = options.eip_browser_args.clone();
    ensure_new_window_flag(&mut args, kind);
    spawn_browser(program, &args)
}

/// Proxy-only mode: attach the zju-connect SOCKS5 proxy to the browser.
fn open_via_proxy(options: &LaunchOptions) -> Result<(), OpenEipError> {
    let configured = options.eip_browser_program.trim();
    let Some((program, kind)) = resolve_browser(configured) else {
        log::warn!("[eip] proxy-only mode needs a browser but none is configured or detected");
        return Err(OpenEipError::NoBrowserFound);
    };
    let proxy = socks_proxy_address(&options.socks_bind);
    let mut args = options.eip_browser_args.clone();
    ensure_new_window_flag(&mut args, kind);
    match kind {
        BrowserKind::Chrome => {
            log::info!("[eip] launching Chrome-family browser with SOCKS5 proxy {proxy}");
            args.push(format!("--proxy-server=socks5://{proxy}"));
        }
        BrowserKind::Firefox => {
            log::info!(
                "[eip] launching Firefox-family browser with temporary SOCKS5 profile ({proxy})"
            );
            let profile = create_firefox_profile(&proxy)?;
            args.push("-no-remote".into());
            args.push("-profile".into());
            args.push(profile.display().to_string());
        }
    }
    spawn_browser(&program, &args)
}

/// The browser to launch: the user's configured program if set (family
/// classified from its name; unknown names are assumed Chrome-family),
/// otherwise the first auto-detected installation.
fn resolve_browser(configured: &str) -> Option<(String, BrowserKind)> {
    if !configured.is_empty() {
        return Some((
            configured.to_string(),
            classify_browser(configured).unwrap_or(BrowserKind::Chrome),
        ));
    }
    detect_installed_browsers()
        .into_iter()
        .next()
        .map(|b| (b.path, b.kind))
}

/// Append the platform-appropriate new-window flag unless the user already
/// supplied one (requirement: EIP opens in a new window by default).
fn ensure_new_window_flag(args: &mut Vec<String>, kind: BrowserKind) {
    let has_flag = args
        .iter()
        .any(|a| NEW_WINDOW_FLAGS.contains(&a.to_lowercase().as_str()));
    if !has_flag {
        args.push(kind.new_window_flag().to_string());
    }
}

fn spawn_browser(program: &str, args: &[String]) -> Result<(), OpenEipError> {
    Command::new(program)
        .args(args)
        .arg(EIP_URL)
        .spawn()
        .map(|_| ())
        .map_err(OpenEipError::Spawn)
}

/// Normalize the SOCKS bind address into `host:port` usable from a browser
/// command line: wildcard binds become loopback and IPv6 literals get
/// bracketed (`socks5://[::1]:1080`).
fn socks_proxy_address(bind: &str) -> String {
    let bind = bind.trim();
    let (host, port) = match bind.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(port) => (h, port),
            Err(_) => (bind, DEFAULT_SOCKS_PORT),
        },
        None => (bind, DEFAULT_SOCKS_PORT),
    };
    let host = match host {
        "" | "0.0.0.0" | "::" => "127.0.0.1",
        _ => host,
    };
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Create a throwaway Firefox profile directory whose `prefs.js` routes all
/// traffic through the given `host:port` SOCKS5 proxy (DNS included).
///
/// A unique directory per launch avoids "profile in use" clashes when a
/// previous EIP window is still open; stale directories live in the OS temp
/// dir and are reclaimed by normal temp cleanup.
fn create_firefox_profile(proxy_addr: &str) -> Result<PathBuf, OpenEipError> {
    let unique = format!(
        "zju-connect-eip-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    );
    let dir = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&dir)
        .map_err(|e| OpenEipError::Profile(format!("create {}: {e}", dir.display())))?;
    let (host, port) = split_host_port(proxy_addr);
    std::fs::write(dir.join("prefs.js"), firefox_prefs_js(host, port))
        .map_err(|e| OpenEipError::Profile(e.to_string()))?;
    Ok(dir)
}

fn split_host_port(addr: &str) -> (&str, u16) {
    match addr.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().unwrap_or(DEFAULT_SOCKS_PORT)),
        None => (addr, DEFAULT_SOCKS_PORT),
    }
}

/// prefs.js contents for the throwaway Firefox profile. Pure so tests can
/// assert on it; the host is sanitized to a conservative charset because it
/// ends up inside a JS file.
fn firefox_prefs_js(socks_host: &str, socks_port: u16) -> String {
    let safe_host: String = socks_host
        .trim_matches(['[', ']'])
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-:".contains(c) {
                c
            } else {
                '?'
            }
        })
        .collect();
    format!(
        concat!(
            "// Generated by zju-connect-gui: SOCKS5 proxy profile for EIP.\n",
            r#"user_pref("network.proxy.type", 1);"#,
            "\n",
            r#"user_pref("network.proxy.socks", "{host}");"#,
            "\n",
            r#"user_pref("network.proxy.socks_port", {port});"#,
            "\n",
            r#"user_pref("network.proxy.socks_version", 5);"#,
            "\n",
            r#"user_pref("network.proxy.socks_remote_dns", true);"#,
            "\n",
            r#"user_pref("network.proxy.no_proxies_on", "");"#,
            "\n",
            r#"user_pref("browser.aboutwelcome.enabled", false);"#,
            "\n",
            r#"user_pref("browser.startup.homepage_override.mstone", "ignore");"#,
            "\n",
            r#"user_pref("datareporting.policy.dataSubmissionEnabled", false);"#,
            "\n",
            r#"user_pref("toolkit.telemetry.enabled", false);"#,
            "\n",
        ),
        host = safe_host,
        port = socks_port
    )
}

#[cfg(target_os = "windows")]
fn default_open_command() -> Command {
    let mut cmd = Command::new("rundll32");
    cmd.arg("url.dll,FileProtocolHandler").arg(EIP_URL);
    cmd
}

#[cfg(target_os = "macos")]
fn default_open_command() -> Command {
    let mut cmd = Command::new("open");
    cmd.arg(EIP_URL);
    cmd
}

#[cfg(all(unix, not(target_os = "macos")))]
fn default_open_command() -> Command {
    let mut cmd = Command::new("xdg-open");
    cmd.arg(EIP_URL);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_window_flag_appended_when_missing() {
        let mut args = vec!["--kiosk".to_string()];
        ensure_new_window_flag(&mut args, BrowserKind::Chrome);
        assert_eq!(args, vec!["--kiosk", "--new-window"]);

        let mut args = Vec::new();
        ensure_new_window_flag(&mut args, BrowserKind::Firefox);
        assert_eq!(args, vec!["-new-window"]);
    }

    #[test]
    fn new_window_flag_not_duplicated() {
        for existing in ["--new-window", "-new-window", "--NEW-WINDOW"] {
            let mut args = vec![existing.to_string()];
            ensure_new_window_flag(&mut args, BrowserKind::Chrome);
            assert_eq!(args.len(), 1, "{existing}");
        }
    }

    #[test]
    fn socks_proxy_address_normalizes_binds() {
        assert_eq!(socks_proxy_address("127.0.0.1:1080"), "127.0.0.1:1080");
        // Wildcard binds must be reachable from a client process as loopback.
        assert_eq!(socks_proxy_address("0.0.0.0:1080"), "127.0.0.1:1080");
        assert_eq!(socks_proxy_address(":1080"), "127.0.0.1:1080");
        assert_eq!(socks_proxy_address("::"), "127.0.0.1:1080");
        // Missing port falls back to the zju-connect default.
        assert_eq!(socks_proxy_address("127.0.0.1"), "127.0.0.1:1080");
        // IPv6 literals are bracketed for use inside socks5:// URLs.
        assert_eq!(socks_proxy_address("[::1]:1080"), "[::1]:1080");
        assert_eq!(socks_proxy_address("::1:1080"), "[::1]:1080");
    }

    #[test]
    fn firefox_prefs_contain_socks_settings() {
        let prefs = firefox_prefs_js("127.0.0.1", 1080);
        assert!(prefs.contains(r#"user_pref("network.proxy.type", 1);"#));
        assert!(prefs.contains(r#"user_pref("network.proxy.socks", "127.0.0.1");"#));
        assert!(prefs.contains(r#"user_pref("network.proxy.socks_port", 1080);"#));
        assert!(prefs.contains(r#"user_pref("network.proxy.socks_version", 5);"#));
        assert!(prefs.contains(r#"user_pref("network.proxy.socks_remote_dns", true);"#));
    }

    #[test]
    fn firefox_prefs_sanitize_host() {
        // Quotes/newlines must not survive into the generated JS.
        let prefs = firefox_prefs_js("a\"b\nc", 1080);
        assert!(prefs.contains(r#"user_pref("network.proxy.socks", "a?b?c");"#));
        // A hostile host cannot inject an extra pref line.
        let hostile = firefox_prefs_js("x\";\nuser_pref(\"evil\",1);", 1080);
        assert!(!hostile.contains(r#"user_pref("evil""#));
        // Exactly the ten known prefs, one per line.
        assert_eq!(
            hostile
                .lines()
                .filter(|l| l.starts_with("user_pref"))
                .count(),
            10
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_firefox_profile_writes_prefs() {
        let addr = socks_proxy_address("127.0.0.1:1080");
        let dir = create_firefox_profile(&addr).unwrap();
        let prefs = std::fs::read_to_string(dir.join("prefs.js")).unwrap();
        assert!(prefs.contains("network.proxy.socks"));
        std::fs::remove_dir_all(dir).ok();
    }

    // ── End-to-end launcher tests against a fake browser script ─────

    /// Writes an executable stub that appends its argv to `capture`. The
    /// binary name decides the family classification ("…firefox…" → Firefox,
    /// anything unrecognized → Chrome fallback).
    #[cfg(unix)]
    fn write_fake_browser(dir: &std::path::Path, capture: &std::path::Path, name: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join(name);
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\n",
            capture.display()
        );
        std::fs::write(&bin, script).unwrap();
        let mut perms = std::fs::metadata(&bin).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bin, perms).unwrap();
        bin.display().to_string()
    }

    #[cfg(unix)]
    fn read_captured(capture: &std::path::Path, min_lines: usize) -> Vec<String> {
        // spawn() returns before the child has necessarily finished writing;
        // poll for the expected number of lines instead of racing it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Ok(text) = std::fs::read_to_string(capture) {
                let lines: Vec<String> = text.lines().map(str::to_string).collect();
                if lines.len() >= min_lines {
                    return lines;
                }
            }
            if std::time::Instant::now() >= deadline {
                panic!(
                    "capture file never reached {min_lines} lines: {}",
                    capture.display()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    #[test]
    fn open_eip_tun_mode_opens_new_window_without_proxy() {
        let tmp = tempfile::tempdir().unwrap();
        let capture = tmp.path().join("args.txt");
        let browser = write_fake_browser(tmp.path(), &capture, "fake-chrome");

        let options = LaunchOptions {
            tun_mode: true,
            eip_browser_program: browser,
            ..LaunchOptions::default()
        };
        open_eip(&options).unwrap();

        assert_eq!(read_captured(&capture, 2), vec!["--new-window", EIP_URL]);
    }

    #[cfg(unix)]
    #[test]
    fn open_eip_proxy_mode_chrome_gets_socks_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let capture = tmp.path().join("args.txt");
        let browser = write_fake_browser(tmp.path(), &capture, "fake-chrome");

        let options = LaunchOptions {
            tun_mode: false,
            eip_browser_program: browser,
            socks_bind: "127.0.0.1:9999".into(),
            ..LaunchOptions::default()
        };
        open_eip(&options).unwrap();

        assert_eq!(
            read_captured(&capture, 3),
            vec![
                "--new-window",
                "--proxy-server=socks5://127.0.0.1:9999",
                EIP_URL
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_eip_proxy_mode_firefox_gets_temp_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let capture = tmp.path().join("args.txt");
        let browser = write_fake_browser(tmp.path(), &capture, "fake-firefox");

        let options = LaunchOptions {
            tun_mode: false,
            eip_browser_program: browser,
            socks_bind: "0.0.0.0:8888".into(),
            ..LaunchOptions::default()
        };
        open_eip(&options).unwrap();

        let args = read_captured(&capture, 5);
        assert_eq!(args.len(), 5, "{args:?}");
        assert_eq!(args[0], "-new-window");
        assert_eq!(args[1], "-no-remote");
        assert_eq!(args[2], "-profile");
        // The profile directory carries prefs.js pointing at the (normalized)
        // wildcard bind → loopback.
        let prefs = std::fs::read_to_string(std::path::Path::new(&args[3]).join("prefs.js"))
            .expect("profile prefs.js exists");
        assert!(prefs.contains(r#"user_pref("network.proxy.socks", "127.0.0.1");"#));
        assert!(prefs.contains(r#"user_pref("network.proxy.socks_port", 8888);"#));
        assert_eq!(args[4], EIP_URL);
        std::fs::remove_dir_all(&args[3]).ok();
    }

    #[cfg(unix)]
    #[test]
    fn open_eip_user_args_take_precedence_over_new_window_default() {
        let tmp = tempfile::tempdir().unwrap();
        let capture = tmp.path().join("args.txt");
        let browser = write_fake_browser(tmp.path(), &capture, "fake-chrome");

        let options = LaunchOptions {
            tun_mode: true,
            eip_browser_program: browser.clone(),
            eip_browser_args: vec!["--kiosk".into()],
            ..LaunchOptions::default()
        };
        open_eip(&options).unwrap();

        assert_eq!(
            read_captured(&capture, 3),
            vec!["--kiosk", "--new-window", EIP_URL]
        );

        // ...but an explicit new-window flag from the user is not duplicated.
        std::fs::remove_file(&capture).ok();
        let options = LaunchOptions {
            tun_mode: true,
            eip_browser_program: browser.clone(),
            eip_browser_args: vec!["-new-window".into()],
            ..LaunchOptions::default()
        };
        open_eip(&options).unwrap();
        assert_eq!(read_captured(&capture, 2), vec!["-new-window", EIP_URL]);
    }

    #[test]
    fn resolve_browser_uses_configured_program_first() {
        let (program, kind) =
            resolve_browser("/usr/bin/firefox").expect("configured browser resolves");
        assert_eq!(program, "/usr/bin/firefox");
        assert_eq!(kind, BrowserKind::Firefox);

        // Unknown binaries still resolve, assuming Chrome-family flags.
        let (_, kind) = resolve_browser(r"C:\Tools\mybrowser.exe").expect("resolves");
        assert_eq!(kind, BrowserKind::Chrome);
    }
}
