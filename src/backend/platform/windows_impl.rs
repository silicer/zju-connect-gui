use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Child;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_CANCELLED, HANDLE, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Console::{
    AllocConsole, GenerateConsoleCtrlEvent, GetConsoleWindow, CTRL_BREAK_EVENT,
};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, OpenProcess, OpenProcessToken, WaitForSingleObject,
    PROCESS_SYNCHRONIZE,
};
use windows::Win32::UI::Shell::{
    ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_NORMAL};

use super::{ElevationError, SingleInstanceError, WaitError};

/// Allocate (and hide) a console at startup so we can later use
/// `GenerateConsoleCtrlEvent` to deliver CTRL_BREAK to the spawned zju-connect
/// child. GUI processes launched from Explorer have no console by default; without
/// one, the child cannot be reached via console signals and we'd be limited to
/// `TerminateProcess`, which leaves TUN/DNS state dangling.
///
/// Safe to call at startup; if a console is already attached (e.g. launched from
/// `cmd.exe`), `AllocConsole` returns Err and we fall through to hiding whatever
/// console we have.
pub fn init_console_for_signaling() {
    unsafe {
        let _ = AllocConsole();
        let hwnd = GetConsoleWindow();
        if hwnd.0 as usize != 0 {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

pub fn is_process_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;
        let res = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut std::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        );
        let _ = CloseHandle(token);
        res.is_ok() && elevation.TokenIsElevated != 0
    }
}

pub fn relaunch_self_elevated(args: &[String]) -> Result<(), ElevationError> {
    let exe = std::env::current_exe().map_err(|err| ElevationError::Io(err.to_string()))?;
    let cwd: PathBuf = exe
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let parameters = args
        .iter()
        .map(|a| escape_arg(a))
        .collect::<Vec<_>>()
        .join(" ");

    let exe_w = to_wide_os(exe.as_os_str());
    let cwd_w = to_wide_os(cwd.as_os_str());
    let params_w = to_wide_str(&parameters);
    let verb_w = to_wide_str("runas");

    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        lpVerb: PCWSTR(verb_w.as_ptr()),
        lpFile: PCWSTR(exe_w.as_ptr()),
        lpParameters: PCWSTR(params_w.as_ptr()),
        lpDirectory: PCWSTR(cwd_w.as_ptr()),
        nShow: SW_NORMAL.0,
        ..Default::default()
    };
    let result = unsafe { ShellExecuteExW(&mut info) };
    if result.is_ok() {
        return Ok(());
    }
    let last = unsafe { GetLastError() };
    if last == ERROR_CANCELLED {
        Err(ElevationError::UserCancelled)
    } else {
        Err(ElevationError::Failed(format!(
            "ShellExecuteExW failed: error {}",
            last.0
        )))
    }
}

pub fn wait_for_process_exit(pid: u32, timeout: Duration) -> Result<(), WaitError> {
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }
        .map_err(|err| WaitError::Io(std::io::Error::other(format!("OpenProcess: {err}"))))?;
    let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
    let result = unsafe { WaitForSingleObject(handle, timeout_ms) };
    let _ = unsafe { CloseHandle(handle) };
    match result {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err(WaitError::Timeout),
        _ => Err(WaitError::Io(std::io::Error::other(
            "WaitForSingleObject failed",
        ))),
    }
}

pub fn signal_child_to_quit(child: &Child) -> std::io::Result<()> {
    let Some(pid) = child.id() else {
        return Ok(());
    };
    let result = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) };
    result.map_err(|err| std::io::Error::other(format!("GenerateConsoleCtrlEvent: {err}")))
}

/// Reproduces Go's `syscall.EscapeArg` argv-quoting algorithm. Empty strings become
/// `""`; strings without space/tab/quote/backslash pass through; otherwise the value
/// is wrapped in quotes with internal `"` escaped as `\"` and runs of backslashes
/// preceding a quote (or the closing quote) doubled.
pub fn escape_arg(s: &str) -> String {
    if s.is_empty() {
        return "\"\"".to_string();
    }
    let needs_quotes = s.chars().any(|c| matches!(c, ' ' | '\t' | '"' | '\\'));
    if !needs_quotes {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut backslashes: u32 = 0;
    for c in s.chars() {
        match c {
            '\\' => {
                backslashes += 1;
            }
            '"' => {
                for _ in 0..(backslashes * 2 + 1) {
                    out.push('\\');
                }
                out.push('"');
                backslashes = 0;
            }
            other => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                backslashes = 0;
                out.push(other);
            }
        }
    }
    for _ in 0..(backslashes * 2) {
        out.push('\\');
    }
    out.push('"');
    out
}

/// RAII guard for the single-instance named-mutex lock. Drop calls `CloseHandle`,
/// which releases the kernel object so a future launch can acquire it.
pub struct SingleInstanceGuard {
    handle: HANDLE,
}

// HANDLE is a raw pointer; the OS-level mutex it points to is safe to ship across
// threads as long as we only call CloseHandle once (in Drop).
unsafe impl Send for SingleInstanceGuard {}
unsafe impl Sync for SingleInstanceGuard {}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Acquire a per-user named mutex. Returns `AlreadyRunning` when the OS reports
/// `ERROR_ALREADY_EXISTS`, meaning another process in the same session already
/// owns it. The `lock_path` argument is unused on Windows (named mutexes don't
/// need a filesystem entry) but kept for cross-platform API parity with Unix.
pub fn acquire_single_instance(
    _lock_path: &Path,
) -> Result<SingleInstanceGuard, SingleInstanceError> {
    // The "Local\" prefix scopes the mutex to the current logon session, which is
    // what we want — different users on the same machine get independent locks.
    let name = HSTRING::from("Local\\zju-connect-gui-singleton-mutex");
    unsafe {
        let handle = CreateMutexW(None, false, &name).map_err(|err| {
            SingleInstanceError::Io(std::io::Error::other(format!("CreateMutexW: {err}")))
        })?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(handle);
            return Err(SingleInstanceError::AlreadyRunning);
        }
        Ok(SingleInstanceGuard { handle })
    }
}

fn to_wide_os(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

fn to_wide_str(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_arg_empty_is_empty_quoted() {
        assert_eq!(escape_arg(""), "\"\"");
    }

    #[test]
    fn escape_arg_simple_passes_through() {
        assert_eq!(escape_arg("hello"), "hello");
        assert_eq!(escape_arg("a/b/c"), "a/b/c");
    }

    #[test]
    fn escape_arg_with_space_quoted() {
        assert_eq!(escape_arg("hello world"), "\"hello world\"");
    }

    #[test]
    fn escape_arg_internal_quote_escaped() {
        assert_eq!(escape_arg("he said \"hi\""), "\"he said \\\"hi\\\"\"");
    }

    #[test]
    fn escape_arg_trailing_backslash_doubled() {
        assert_eq!(escape_arg("path\\"), "\"path\\\\\"");
    }

    #[test]
    fn escape_arg_backslash_before_quote_doubled() {
        assert_eq!(escape_arg("a\\\"b"), "\"a\\\\\\\"b\"");
    }
}
