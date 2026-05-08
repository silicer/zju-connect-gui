use std::path::Path;
use std::time::Duration;

use tokio::process::Child;

use super::{ElevationError, WaitError};

/// No-op on unix; the windows build attaches a hidden console here so CTRL_BREAK
/// can later be delivered to the child.
pub fn init_console_for_signaling() {}

/// On unix we never elevate; the GUI starts with whatever privileges the user has.
/// Reported as `true` so the manager treats every start as "ready to spawn directly".
pub fn is_process_elevated() -> bool {
    true
}

pub fn relaunch_self_elevated(_args: &[String]) -> Result<(), ElevationError> {
    Err(ElevationError::Unsupported)
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
