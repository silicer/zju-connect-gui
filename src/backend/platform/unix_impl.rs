use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::time::Duration;

use tokio::process::Child;

use super::{SingleInstanceError, WaitError};

/// No-op on unix; the windows build attaches a hidden console here so CTRL_BREAK
/// can later be delivered to the child.
pub fn init_console_for_signaling() {}

/// True when running as root (euid 0).
///
/// TUN mode and ProxyBridge (NFQUEUE interception) both require root on
/// Linux; zju-connect's non-TUN mode runs fine as a regular user. The manager
/// rejects TUN/ProxyBridge starts when this is false, and the frontend offers
/// an elevation flow (see [`super::relaunch_self_elevated`]).
pub fn is_process_elevated() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Detach the elevated instance from the process that launched it.
///
/// Only meaningful on Linux, where `pkexec` sets a parent-death signal that
/// survives `execve` (and where the parent holds a pipe on our stderr) — see
/// [`super::pkexec`]. A no-op elsewhere.
pub fn detach_from_parent() {
    #[cfg(target_os = "linux")]
    super::pkexec::detach_from_parent();
}

/// Give back state files an elevated instance created (Linux/`pkexec` only).
pub fn reclaim_state_files(app_dir: &Path, client_data_file: &str) {
    #[cfg(target_os = "linux")]
    super::pkexec::reclaim_state_files(app_dir, client_data_file);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app_dir, client_data_file);
    }
}

/// Spawn a GUI helper (browser, file dialog) in the calling user's session when
/// this is an elevated Linux instance; plain spawn everywhere else.
pub fn spawn_gui_command(program: &str, args: &[String]) -> std::io::Result<std::process::Child> {
    #[cfg(target_os = "linux")]
    {
        super::pkexec::spawn_gui_command(program, args)
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::process::Command::new(program).args(args).spawn()
    }
}

/// Polls /proc/<pid> for existence. Linux-only convenience.
pub fn wait_for_process_exit(pid: u32, timeout: Duration) -> Result<(), WaitError> {
    let deadline = std::time::Instant::now() + timeout;
    let proc_path = Path::new("/proc").join(pid.to_string());
    while std::time::Instant::now() < deadline {
        if !proc_path.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(WaitError::Timeout)
}

/// Send SIGINT to the child process group. Returns Ok if the signal could be queued;
/// the caller is responsible for waiting for the process to actually exit.
pub fn signal_child_to_quit(child: &Child) -> std::io::Result<()> {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    let Some(pid) = child.id() else {
        return Ok(());
    };
    kill(Pid::from_raw(pid as i32), Signal::SIGINT)
        .map_err(|err| std::io::Error::other(format!("kill failed: {err}")))?;
    Ok(())
}

/// Quote a single argv entry for the underlying OS. On unix we pass argv directly,
/// so this is a no-op that exists for API parity with the windows side.
pub fn escape_arg(arg: &str) -> String {
    arg.to_string()
}

/// RAII guard for the single-instance lock. The kernel releases the flock when the
/// underlying file descriptor is closed (i.e. when this guard is dropped or the
/// process exits, including via SIGKILL).
pub struct SingleInstanceGuard {
    _file: File,
}

/// Acquire an exclusive, non-blocking flock on `lock_path`. Returns
/// `SingleInstanceError::AlreadyRunning` if another process already holds it. Other
/// failures (file open / underlying flock errors) surface as `Io`.
pub fn acquire_single_instance(
    lock_path: &Path,
) -> Result<SingleInstanceGuard, SingleInstanceError> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        return Ok(SingleInstanceGuard { _file: file });
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN => {
            Err(SingleInstanceError::AlreadyRunning)
        }
        _ => Err(SingleInstanceError::Io(err)),
    }
}
