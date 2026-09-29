//! Linux privilege escalation through polkit's `pkexec`.
//!
//! `pkexec` is the only portable way to raise a *graphical* authorization
//! dialog from a desktop application: it asks polkitd for the
//! `org.freedesktop.policykit.exec` action, and polkitd routes that to the
//! authentication agent registered for the calling session — on 统信 UOS /
//! Deepin that is `dde-polkit-agent`, on other desktops
//! `polkit-gnome-authentication-agent-1`, `xfce-polkit`, … That is exactly the
//! piece that has to be present for a window to pop up at all.
//!
//! Three details make the difference between "the dialog appears and the
//! elevated instance takes over" and "the UI waits forever":
//!
//! * `--disable-internal-agent` — without it, `pkexec` falls back to its own
//!   *textual* agent on the controlling terminal when no graphical agent is
//!   registered, which looks exactly like a hang when the app was started from
//!   a launcher or a terminal;
//! * a handshake marker (see [`crate::backend::elevation`]) — `pkexec` execs
//!   the program *in place*, so its exit status cannot be used to tell
//!   "authenticated" from "dismissed". Polling the marker plus the helper's
//!   death gives an unambiguous answer, and turns a dismissed dialog into a
//!   real error instead of a silent stall;
//! * `PR_SET_PDEATHSIG` — `pkexec` asks the kernel to SIGTERM it when its
//!   parent dies. The elevated instance inherits that setting across `execve`,
//!   so it must clear it before the original GUI exits (see
//!   [`detach_from_parent`]), or the VPN would die together with the window
//!   that requested it.

use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::ElevationError;
use crate::backend::elevation::ElevationHandshake;
use crate::backend::paths::resolve_app_dir;
use crate::backend::relaunch_args::parse_elevated_relaunch_args;

/// Where `pkexec` and `env` normally live. `pkexec` is setuid root, so it is
/// always in a system directory; PATH is only a last resort.
const PKEXEC_CANDIDATES: &[&str] = &["/usr/bin/pkexec", "/bin/pkexec", "/usr/local/bin/pkexec"];
const ENV_CANDIDATES: &[&str] = &["/usr/bin/env", "/bin/env"];

/// Environment variables the elevated GUI needs in order to reach the desktop
/// session it was started from: X11/Wayland display access, the session D-Bus
/// (tray icon), and the locale.
///
/// `pkexec` nukes the environment and only re-adds a fixed allowlist (and it
/// drops even `DISPLAY`/`XAUTHORITY` unless the policy action carries the
/// `allow_gui` annotation). Running `/usr/bin/env VAR=… prog` instead of `prog`
/// keeps the dialog working while still handing the program the variables it
/// needs to draw anything at all.
const GUI_ENV_PASSTHROUGH: &[&str] = &[
    "DISPLAY",
    "XAUTHORITY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_SESSION_TYPE",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "RUST_LOG",
];

/// How long the original instance waits for the elevated one to come up. The
/// polkit dialog is user-paced, so this is only a backstop against an agent
/// that never answers; it matches the elevated child's own lock wait.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(300);
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Number of captured helper stderr lines kept for the error message.
const STDERR_TAIL_LINES: usize = 20;

pub(super) async fn relaunch_self_elevated(args: &[String]) -> Result<(), ElevationError> {
    let handshake = parse_elevated_relaunch_args(args)
        .ok()
        .and_then(|parsed| parsed.handshake)
        .ok_or_else(|| {
            ElevationError::Failed("内部错误：提权请求缺少握手令牌，无法确认提权结果".into())
        })?;

    let app_dir = resolve_app_dir().map_err(|err| ElevationError::Io(err.to_string()))?;
    let exe = std::env::current_exe().map_err(|err| ElevationError::Io(err.to_string()))?;

    // No display means no agent can draw a dialog: report it instead of
    // waiting for an answer that can never arrive.
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err(ElevationError::NoAuthenticationAgent);
    }

    let pkexec = find_pkexec()?;
    let env_prog = find_executable(ENV_CANDIDATES)
        .unwrap_or_else(|| PathBuf::from(ENV_CANDIDATES.first().expect("env candidate")));
    let env_pairs = collect_passthrough_env(|key| std::env::var(key).ok());
    let argv = build_pkexec_argv(&env_prog, &env_pairs, &exe, args);

    log::info!(
        "requesting elevation through {} ({} environment variables passed through)",
        pkexec.display(),
        env_pairs.len()
    );

    wait_for_helper(&pkexec, &argv, &app_dir, &handshake, HANDSHAKE_TIMEOUT).await
}

/// Spawn the elevation helper and wait until either the elevated instance
/// announces itself or the helper dies.
///
/// `pkexec` `exec`s the program in place, so the helper's exit status cannot be
/// read as "authentication succeeded": a live helper means "prompt still open
/// or elevated instance running", a dead one means failure.
async fn wait_for_helper(
    program: &Path,
    argv: &[String],
    app_dir: &Path,
    handshake: &ElevationHandshake,
    timeout: Duration,
) -> Result<(), ElevationError> {
    let mut command = Command::new(program);
    command
        .args(argv)
        // Never let pkexec's textual agent read from our stdin.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // Own session: the elevated instance must not inherit our terminal's
    // process group (Ctrl+C in the launching terminal must not reach it).
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|err| ElevationError::Io(format!("无法启动 pkexec: {err}")))?;

    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| ElevationError::Io("无法读取 pkexec 的错误输出".into()))?;

    let deadline = Instant::now() + timeout;
    loop {
        // Checked before `try_wait`: the marker is written while the elevated
        // instance is still starting up, and for a successful launch the helper
        // stays alive as that instance.
        if handshake.is_ready(app_dir) {
            log::info!("elevated instance is up");
            return Ok(());
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                // The helper is gone and no marker ever appeared: authentication
                // failed, was dismissed, or the elevated instance died early.
                // Its stderr is fully buffered by now (the pipe is closed).
                let tail = read_stderr_tail(&mut stderr).await;
                return Err(classify_helper_exit(status.code(), &tail));
            }
            Ok(None) => {}
            Err(err) => return Err(ElevationError::Io(format!("等待 pkexec 失败: {err}"))),
        }

        if Instant::now() >= deadline {
            // Cancel the pending authorization: kill the helper so the dialog
            // goes away instead of lingering behind a reported failure.
            let _ = child.start_kill();
            return Err(ElevationError::Timeout);
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Resolve `pkexec`, rejecting the ones that cannot possibly elevate. A
/// hardening pass (or the CVE-2021-4034 mitigation) that strips the setuid bit
/// is a common reason for "nothing happens" on managed Linux desktops.
fn find_pkexec() -> Result<PathBuf, ElevationError> {
    let Some(path) = find_executable(PKEXEC_CANDIDATES) else {
        return Err(ElevationError::PkexecUnavailable("未找到可执行文件".into()));
    };
    let metadata = fs::metadata(&path).map_err(|err| {
        ElevationError::PkexecUnavailable(format!("无法读取 {}: {err}", path.display()))
    })?;
    if metadata.mode() & 0o4000 == 0 {
        return Err(ElevationError::PkexecUnavailable(format!(
            "{} 没有 setuid 位",
            path.display()
        )));
    }
    if metadata.uid() != 0 {
        return Err(ElevationError::PkexecUnavailable(format!(
            "{} 的属主不是 root",
            path.display()
        )));
    }
    Ok(path)
}

fn find_executable(candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(metadata) => metadata.is_file() && metadata.mode() & 0o111 != 0,
        Err(_) => false,
    }
}

/// Collect the session environment that must survive `pkexec`'s environment
/// wipe, as `KEY=VALUE` strings. Empty values are skipped.
fn collect_passthrough_env<F>(lookup: F) -> Vec<String>
where
    F: Fn(&str) -> Option<String>,
{
    GUI_ENV_PASSTHROUGH
        .iter()
        .filter_map(|key| {
            let value = lookup(key)?;
            if value.is_empty() {
                return None;
            }
            Some(format!("{key}={value}"))
        })
        .collect()
}

/// `pkexec --disable-internal-agent /usr/bin/env VAR=… <exe> <args…>`
///
/// `env` is what carries the session variables past `pkexec`'s sanitizing; the
/// program path must stay absolute (`pkexec` refuses relative ones).
fn build_pkexec_argv(
    env_prog: &Path,
    env_pairs: &[String],
    exe: &Path,
    args: &[String],
) -> Vec<String> {
    let mut argv = Vec::with_capacity(env_pairs.len() + args.len() + 3);
    argv.push("--disable-internal-agent".to_string());
    argv.push(env_prog.to_string_lossy().into_owned());
    argv.extend(env_pairs.iter().cloned());
    argv.push(exe.to_string_lossy().into_owned());
    argv.extend(args.iter().cloned());
    argv
}

/// Map how the elevation helper died onto a user-actionable error.
///
/// `pkexec` exits 126 when the dialog was dismissed and 127 when the
/// authorization was refused or could not be obtained at all (see
/// `pkexec(1)`), so the stderr tail is what distinguishes "no agent" from
/// "wrong password".
fn classify_helper_exit(code: Option<i32>, stderr_tail: &str) -> ElevationError {
    let tail = stderr_tail.trim();
    let has = |needle: &str| tail.to_ascii_lowercase().contains(needle);

    match code {
        Some(126) => ElevationError::UserCancelled,
        Some(127) if has("no authentication agent") => ElevationError::NoAuthenticationAgent,
        Some(127) if has("must be setuid root") => {
            ElevationError::PkexecUnavailable("pkexec 没有 setuid 位".into())
        }
        Some(127) if has("not authorized") => ElevationError::Failed(
            "授权被系统拒绝（Not authorized），请确认当前用户具有管理员权限".into(),
        ),
        Some(127) => ElevationError::Failed(non_empty_tail(
            tail,
            "授权失败或系统未授权（pkexec 退出码 127）",
        )),
        Some(other) => ElevationError::Failed(non_empty_tail(
            tail,
            &format!("提权后的进程在就绪前退出（退出码 {other}）"),
        )),
        None => ElevationError::Failed(non_empty_tail(tail, "授权进程被信号终止")),
    }
}

fn non_empty_tail(tail: &str, fallback: &str) -> String {
    if tail.is_empty() {
        fallback.to_string()
    } else {
        format!("{fallback}：{tail}")
    }
}

/// Read whatever the helper wrote to stderr (bounded, so a chatty failure
/// cannot flood the UI).
async fn read_stderr_tail(stderr: &mut tokio::process::ChildStderr) -> String {
    let mut lines: VecDeque<String> = VecDeque::new();
    let mut buffer = Vec::new();
    // Bounded: a helper that somehow keeps the pipe open (or floods it) must
    // not turn a failed elevation into a hang.
    let read = tokio::time::timeout(Duration::from_secs(2), stderr.read_to_end(&mut buffer)).await;
    if !matches!(read, Ok(Ok(_))) {
        return String::new();
    }
    for line in String::from_utf8_lossy(&buffer).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if lines.len() == STDERR_TAIL_LINES {
            lines.pop_front();
        }
        lines.push_back(line.to_string());
    }
    lines.into_iter().collect::<Vec<_>>().join(" / ")
}

/// Called by the elevated instance as one of its first actions.
///
/// * clears `PR_SET_PDEATHSIG`, which `pkexec` set on itself and which survives
///   `execve` — without this the VPN would be SIGTERMed the moment the original
///   GUI exits;
/// * moves into its own session so a terminal that launched the original GUI
///   cannot signal it;
/// * detaches stdio: the original instance holds the other end of our stderr
///   pipe and exits as soon as we are up.
pub(super) fn detach_from_parent() {
    unsafe {
        if libc::prctl(libc::PR_SET_PDEATHSIG, 0) != 0 {
            log::warn!(
                "failed to clear the parent-death signal: {}",
                std::io::Error::last_os_error()
            );
        }
        if libc::setsid() == -1 {
            // Already a session leader — harmless.
            log::debug!("setsid failed: {}", std::io::Error::last_os_error());
        }
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    detach_stdio();
}

/// Point stdin/stdout/stderr at `/dev/null`. The elevated instance is a GUI
/// process whose only output channel is the web UI; inheriting the parent's
/// pipes would make it die of `EPIPE` (or block on a full pipe) once the parent
/// exits.
fn detach_stdio() {
    let Ok(dev_null) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
    else {
        return;
    };
    use std::os::unix::io::AsRawFd;
    let fd = dev_null.as_raw_fd();
    unsafe {
        for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
            libc::dup2(fd, target);
        }
    }
}

/// State files the app keeps next to the executable. An elevated instance
/// creates some of them as root; without giving them back, the next
/// *unprivileged* run (proxy-only mode) could no longer write them.
const STATE_FILE_NAMES: &[&str] = &[
    "gui_settings.json",
    "gui_pending_connect.json",
    "gui_captcha.png",
    "web_port.txt",
    "instance.lock",
];

/// Chown the state files this elevated instance may have created back to the
/// user that authorized it (`PKEXEC_UID`). No-op when not elevated through
/// polkit, or when the file is already owned by that user.
///
/// Only files from [`STATE_FILE_NAMES`] (plus the zju-connect client-data file)
/// are touched: a packaged install may have root-owned *program* files in the
/// same directory, and those must never change owner.
pub(super) fn reclaim_state_files(app_dir: &Path, client_data_file: &str) {
    let Some((uid, gid)) = calling_user() else {
        return;
    };
    if !super::is_process_elevated() {
        return;
    }

    let mut paths: Vec<PathBuf> = STATE_FILE_NAMES
        .iter()
        .map(|name| app_dir.join(name))
        .collect();
    if !client_data_file.is_empty() {
        let client = Path::new(client_data_file);
        paths.push(if client.is_absolute() {
            client.to_path_buf()
        } else {
            app_dir.join(client)
        });
    }

    for path in paths {
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.uid() == uid {
            continue;
        }
        if let Err(err) = std::os::unix::fs::chown(&path, Some(uid), Some(gid)) {
            log::debug!("failed to hand {} back to uid {uid}: {err}", path.display());
        } else {
            log::debug!("handed {} back to uid {uid}", path.display());
        }
    }
}

/// The uid/gid of the user that authorized this elevated process, as reported
/// by `pkexec`. `None` when this process was not elevated through polkit.
fn calling_user() -> Option<(u32, u32)> {
    let uid: u32 = std::env::var("PKEXEC_UID").ok()?.parse().ok()?;
    if uid == 0 {
        return None;
    }
    let gid = user_primary_gid(uid).unwrap_or(uid);
    Some((uid, gid))
}

fn user_primary_gid(uid: u32) -> Option<u32> {
    passwd_field(uid, 3)?.parse().ok()
}

/// Run a GUI helper (browser, file dialog) in the desktop session of the user
/// that authorized this elevated process.
///
/// A root process cannot simply launch the user's browser: Chrome refuses to
/// start as root, and anything that does start would read `$HOME=/root`. When
/// this is an elevated instance we therefore re-enter the calling user through
/// `setpriv`/`runuser` (both unprivileged-user switches that need no password
/// when already root) with the user's own `HOME`/`USER`. Everywhere else the
/// program is spawned as-is.
pub(super) fn spawn_gui_command(
    program: &str,
    args: &[String],
) -> std::io::Result<std::process::Child> {
    if let Some((uid, gid)) = calling_user() {
        if super::is_process_elevated() {
            if let Some((switcher, mut prefix)) = user_switcher(uid, gid) {
                prefix.push("--".to_string());
                prefix.push(program.to_string());
                prefix.extend(args.iter().cloned());
                let mut command = std::process::Command::new(switcher);
                command
                    .args(&prefix)
                    .env("HOME", user_home(uid).unwrap_or_default())
                    .env("USER", user_name(uid).unwrap_or_default())
                    .env("LOGNAME", user_name(uid).unwrap_or_default());
                return command.spawn();
            }
        }
    }
    std::process::Command::new(program).args(args).spawn()
}

/// `setpriv` (util-linux) first: no PAM, no extra session bookkeeping.
fn user_switcher(uid: u32, gid: u32) -> Option<(PathBuf, Vec<String>)> {
    if let Some(setpriv) = find_executable(&["/usr/bin/setpriv", "/bin/setpriv"]) {
        return Some((
            setpriv,
            vec![
                format!("--reuid={uid}"),
                format!("--regid={gid}"),
                "--init-groups".to_string(),
            ],
        ));
    }
    let user = user_name(uid)?;
    let runuser = find_executable(&["/usr/sbin/runuser", "/sbin/runuser", "/usr/bin/runuser"])?;
    Some((runuser, vec!["-u".to_string(), user]))
}

fn user_name(uid: u32) -> Option<String> {
    passwd_field(uid, 0)
}

fn user_home(uid: u32) -> Option<String> {
    passwd_field(uid, 5)
}

fn passwd_field(uid: u32, index: usize) -> Option<String> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        let id: u32 = fields.get(2)?.parse().ok()?;
        (id == uid).then(|| fields.get(index).copied().unwrap_or_default().to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_env_keeps_session_variables_and_skips_empty_ones() {
        let pairs = collect_passthrough_env(|key| match key {
            "DISPLAY" => Some(":0".to_string()),
            "XAUTHORITY" => Some("/home/alice/.Xauthority".to_string()),
            "DBUS_SESSION_BUS_ADDRESS" => Some("unix:path=/run/user/1000/bus".to_string()),
            "WAYLAND_DISPLAY" => Some(String::new()),
            _ => None,
        });
        assert_eq!(
            pairs,
            vec![
                "DISPLAY=:0".to_string(),
                "XAUTHORITY=/home/alice/.Xauthority".to_string(),
                "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus".to_string(),
            ]
        );
    }

    #[test]
    fn argv_disables_the_textual_agent_and_keeps_the_program_absolute() {
        let argv = build_pkexec_argv(
            Path::new("/usr/bin/env"),
            &["DISPLAY=:0".to_string()],
            Path::new("/opt/zju/zju-connect-gui"),
            &[
                "--resume-pending-connect".to_string(),
                "--elevate-handshake=abc".to_string(),
            ],
        );
        assert_eq!(
            argv,
            vec![
                "--disable-internal-agent".to_string(),
                "/usr/bin/env".to_string(),
                "DISPLAY=:0".to_string(),
                "/opt/zju/zju-connect-gui".to_string(),
                "--resume-pending-connect".to_string(),
                "--elevate-handshake=abc".to_string(),
            ]
        );
    }

    #[test]
    fn dismissed_dialog_is_reported_as_cancelled() {
        // pkexec(1): 126 means the user dismissed the authentication dialog.
        assert!(matches!(
            classify_helper_exit(
                Some(126),
                "Error executing command as another user: Request dismissed"
            ),
            ElevationError::UserCancelled
        ));
    }

    #[test]
    fn missing_agent_is_reported_as_such() {
        assert!(matches!(
            classify_helper_exit(
                Some(127),
                "Error executing command as another user: No authentication agent found."
            ),
            ElevationError::NoAuthenticationAgent
        ));
    }

    #[test]
    fn stripped_setuid_bit_is_reported_as_pkexec_unavailable() {
        assert!(matches!(
            classify_helper_exit(Some(127), "pkexec must be setuid root"),
            ElevationError::PkexecUnavailable(_)
        ));
    }

    #[test]
    fn refused_authorization_and_unexpected_exit_keep_their_detail() {
        match classify_helper_exit(
            Some(127),
            "Error executing command as another user: Not authorized",
        ) {
            ElevationError::Failed(message) => assert!(message.contains("Not authorized")),
            other => panic!("expected Failed, got {other:?}"),
        }
        match classify_helper_exit(Some(1), "boom") {
            ElevationError::Failed(message) => {
                assert!(message.contains("退出码 1"), "{message}");
                assert!(message.contains("boom"), "{message}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        match classify_helper_exit(None, "") {
            ElevationError::Failed(message) => assert!(message.contains("信号")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    // ── The helper-wait loop, against real (fake) helper processes ──

    /// Write an executable stub helper and return its path.
    fn write_fake_helper(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-pkexec");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    /// The stub derives the app dir from its own location, so no environment
    /// has to be mutated for the test.
    const FAKE_MARKER_BODY: &str = r#"
dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
for arg in "$@"; do
  case "$arg" in
    --elevate-handshake=*) token="${arg#--elevate-handshake=}" ;;
  esac
done
: > "$dir/.elevate-ready-$token"
"#;

    #[tokio::test]
    async fn wait_for_helper_returns_ok_once_the_elevated_instance_announces_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let helper = write_fake_helper(tmp.path(), FAKE_MARKER_BODY);
        let handshake = ElevationHandshake::new();

        let result = wait_for_helper(
            &helper,
            &["--disable-internal-agent".to_string(), handshake.arg()],
            tmp.path(),
            &handshake,
            Duration::from_secs(10),
        )
        .await;

        assert!(result.is_ok(), "{result:?}");
        assert!(handshake.is_ready(tmp.path()));
    }

    #[tokio::test]
    async fn wait_for_helper_reports_a_dismissed_prompt() {
        let tmp = tempfile::tempdir().unwrap();
        let helper = write_fake_helper(
            tmp.path(),
            "echo 'Error executing command as another user: Request dismissed' >&2\nexit 126",
        );
        let handshake = ElevationHandshake::new();

        let result = wait_for_helper(
            &helper,
            &[],
            tmp.path(),
            &handshake,
            Duration::from_secs(10),
        )
        .await;

        assert!(
            matches!(result, Err(ElevationError::UserCancelled)),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn wait_for_helper_surfaces_the_missing_agent_and_its_stderr() {
        let tmp = tempfile::tempdir().unwrap();
        let helper = write_fake_helper(
            tmp.path(),
            "echo 'Error executing command as another user: No authentication agent found.' >&2\nexit 127",
        );
        let handshake = ElevationHandshake::new();

        let result = wait_for_helper(
            &helper,
            &[],
            tmp.path(),
            &handshake,
            Duration::from_secs(10),
        )
        .await;

        assert!(
            matches!(result, Err(ElevationError::NoAuthenticationAgent)),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn wait_for_helper_times_out_and_kills_a_helper_that_never_answers() {
        let tmp = tempfile::tempdir().unwrap();
        let helper = write_fake_helper(tmp.path(), "exec sleep 30");
        let handshake = ElevationHandshake::new();

        let started = Instant::now();
        let result = wait_for_helper(
            &helper,
            &[],
            tmp.path(),
            &handshake,
            Duration::from_millis(300),
        )
        .await;

        assert!(matches!(result, Err(ElevationError::Timeout)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!handshake.is_ready(tmp.path()));
    }

    #[test]
    fn user_messages_are_actionable() {
        assert!(ElevationError::NoAuthenticationAgent
            .user_message()
            .contains("dde-polkit-agent"));
        assert!(ElevationError::PkexecUnavailable("x".into())
            .user_message()
            .contains("setuid"));
        assert_eq!(
            ElevationError::UserCancelled.user_message(),
            "已取消授权".to_string()
        );
    }

    #[test]
    fn state_file_list_never_contains_program_files() {
        for name in STATE_FILE_NAMES {
            assert!(!name.starts_with("zju-connect"), "{name}");
            assert!(!name.contains('/'), "{name}");
        }
        assert!(!STATE_FILE_NAMES.contains(&"bin"));
    }
}
