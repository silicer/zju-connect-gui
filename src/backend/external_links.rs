//! Launching the EIP portal
//! (<http://eipapp.scmcc.com.cn:82/mclient/portal/#/home/work>) in a browser.
//!
//! Opening the portal is deliberately **mode independent** — the launcher never
//! touches the browser's own proxy settings, neither for Chrome-family nor for
//! Firefox-family browsers. In TUN mode the tunnel already routes traffic
//! system-wide; in proxy-only mode steering the browser is left to the user.
//!
//! So there are exactly two paths:
//!
//! - a browser configured in the settings
//!   ([`LaunchOptions::eip_browser_program`]) is launched with the URL, adding a
//!   platform-appropriate *new window* flag unless the user already supplied
//!   one;
//! - with no browser configured the URL goes to the OS default handler
//!   (`rundll32` / `open` / `xdg-open`).
//!
//! Browser-family classification (see [`crate::backend::browser_detect`]) is
//! used only to choose that flag: `--new-window` for Chrome-family,
//! `-new-window` for Firefox-family.

use std::process::Command;

use crate::backend::browser_detect::{classify_browser, BrowserKind};
use crate::backend::launch_options::LaunchOptions;

pub const EIP_URL: &str = "http://eipapp.scmcc.com.cn:82/mclient/portal/#/home/work";

/// Flags that already mean "open a new window/tab". If the user configured
/// one of these we must not append a duplicate.
const NEW_WINDOW_FLAGS: &[&str] = &["--new-window", "-new-window", "--new-tab", "-new-tab"];

#[derive(Debug, thiserror::Error)]
pub enum OpenEipError {
    #[error("failed to spawn browser process: {0}")]
    Spawn(std::io::Error),
}

/// Open the EIP portal in the configured browser, or pass it to the OS default
/// handler when no browser is configured.
pub fn open_eip(options: &LaunchOptions) -> Result<(), OpenEipError> {
    let program = options.eip_browser_program.trim();
    if program.is_empty() {
        return default_open_command()
            .spawn()
            .map(|_| ())
            .map_err(OpenEipError::Spawn);
    }
    // Unknown binaries fall back to Chrome-family flags.
    let kind = classify_browser(program).unwrap_or(BrowserKind::Chrome);
    let mut args = options.eip_browser_args.clone();
    ensure_new_window_flag(&mut args, kind);
    spawn_browser(program, &args)
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

    /// Serializes the fake-browser tests below: each one writes an executable
    /// stub and immediately spawns it. Exec of a file that another thread has
    /// open for writing — even briefly, inside its own write+spawn pair — makes
    /// the kernel reject the exec with `ETXTBSY` ("Text file busy"), so the
    /// write+spawn pairs must not overlap.
    #[cfg(unix)]
    static FAKE_BROWSER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Pinned on purpose: the portal moved from `http://eip.scmcc.com.cn/` to
    /// this address, and a stale URL is silent until a user clicks the button.
    #[test]
    fn eip_url_points_at_the_mclient_portal() {
        assert_eq!(
            EIP_URL,
            "http://eipapp.scmcc.com.cn:82/mclient/portal/#/home/work"
        );
    }

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

    /// Both modes must produce the exact same command line: a new-window flag
    /// and the URL, with no proxy switch and no temporary profile.
    #[cfg(unix)]
    #[test]
    fn open_eip_is_mode_independent_and_proxy_free() {
        let _guard = FAKE_BROWSER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for tun_mode in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            let capture = tmp.path().join("args.txt");
            let browser = write_fake_browser(tmp.path(), &capture, "fake-chrome");

            let options = LaunchOptions {
                tun_mode,
                eip_browser_program: browser,
                // A SOCKS bind in the options must not leak into argv.
                socks_bind: "127.0.0.1:9999".into(),
                ..LaunchOptions::default()
            };
            open_eip(&options).unwrap();

            assert_eq!(
                read_captured(&capture, 2),
                vec!["--new-window", EIP_URL],
                "tun_mode={tun_mode}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn open_eip_firefox_family_gets_dash_new_window() {
        let _guard = FAKE_BROWSER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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

        assert_eq!(read_captured(&capture, 2), vec!["-new-window", EIP_URL]);
    }

    #[cfg(unix)]
    #[test]
    fn open_eip_user_args_take_precedence_over_new_window_default() {
        let _guard = FAKE_BROWSER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
}
