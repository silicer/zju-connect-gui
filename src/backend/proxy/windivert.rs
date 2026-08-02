//! WinDivert kernel driver management (Windows only).
//!
//! ProxyBridge intercepts traffic through the WinDivert kernel driver. The
//! driver ships inside the official ProxyBridge installer, and we also ship a
//! copy under `proxybridge/` in our own package — so users never have to run
//! an installer by hand. The bundled driver is EV + WHQL signed (verified),
//! so a plain `CreateService` + `StartService` works on stock x64 Windows
//! without test mode or disabled signature enforcement.
//!
//! On non-Windows platforms this module is a no-op stub.

use std::path::Path;
use std::time::Duration;

#[cfg(target_os = "windows")]
use windows::core::{w, PCWSTR};
#[cfg(target_os = "windows")]
use windows::Win32::System::Services::{
    CloseServiceHandle, CreateServiceW, OpenSCManagerW, OpenServiceW, QueryServiceStatus,
    StartServiceW, SC_HANDLE, SC_MANAGER_ALL_ACCESS, SERVICE_ALL_ACCESS, SERVICE_DEMAND_START,
    SERVICE_ERROR_NORMAL, SERVICE_KERNEL_DRIVER, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
    SERVICE_START, SERVICE_START_PENDING, SERVICE_STATUS, SERVICE_STOPPED,
};

/// Name of the WinDivert kernel service (shared with other WinDivert users,
/// e.g. Proxifier-style tools — never delete it, only ensure it runs).
#[cfg(target_os = "windows")]
const WIN_DIVERT_SERVICE: PCWSTR = w!("WinDivert");

#[cfg(target_os = "windows")]
const DRIVER_FILE_NAME: &str = "WinDivert64.sys";

#[cfg(target_os = "windows")]
const DRIVER_INSTALL_DIR: &str = r"C:\Windows\System32\drivers";

#[cfg(target_os = "windows")]
const DLL_FILE_NAME: &str = "WinDivert.dll";

/// Ensure the WinDivert kernel driver service exists and is running.
///
/// Resolution order:
/// 1. Service already installed and running → `Ok`.
/// 2. Service installed but stopped → start it and wait for RUNNING.
/// 3. Not installed → copy the bundled driver (`<app_dir>/proxybridge/`)
///    into `System32\drivers`, register a kernel service, start it. The
///    user-mode `WinDivert.dll` is copied next to our own exe so that
///    `ProxyBridgeCore.dll` can resolve it when loaded from the bundled
///    `proxybridge/` directory (the loader searches the exe directory first).
///
/// Fails with a descriptive error when the driver is missing and cannot be
/// obtained, or the service cannot be manipulated (e.g. not elevated).
#[cfg(target_os = "windows")]
pub fn ensure_windivert_driver(app_dir: &Path) -> Result<(), String> {
    let scm = unsafe { OpenSCManagerW(None, None, SC_MANAGER_ALL_ACCESS) }
        .map_err(|e| format!("cannot open service manager: {e} (requires administrator)"))?;
    let result = ensure_impl(scm, app_dir);
    let _ = unsafe { CloseServiceHandle(scm) };
    result
}

#[cfg(target_os = "windows")]
fn ensure_impl(scm: SC_HANDLE, app_dir: &Path) -> Result<(), String> {
    let access = SERVICE_QUERY_STATUS | SERVICE_START;
    match unsafe { OpenServiceW(scm, WIN_DIVERT_SERVICE, access) } {
        Ok(service) => {
            let result = start_service_and_wait(service);
            let _ = unsafe { CloseServiceHandle(service) };
            result
        }
        Err(_) => install_driver(scm, app_dir),
    }
}

#[cfg(target_os = "windows")]
fn install_driver(scm: SC_HANDLE, app_dir: &Path) -> Result<(), String> {
    let bundled_dir = app_dir.join("proxybridge");

    // 1) Copy the signed kernel driver into System32\drivers.
    let sys_src = bundled_dir.join(DRIVER_FILE_NAME);
    if !sys_src.is_file() {
        return Err(format!(
            "WinDivert driver is not installed and no bundled copy was found at {}",
            sys_src.display()
        ));
    }
    let sys_dest = Path::new(DRIVER_INSTALL_DIR).join(DRIVER_FILE_NAME);
    std::fs::copy(&sys_src, &sys_dest)
        .map_err(|e| format!("failed to install driver file into System32\\drivers: {e}"))?;

    // 2) Copy WinDivert.dll next to our exe so ProxyBridgeCore.dll (loaded
    //    from the bundled proxybridge/ dir) can resolve it.
    let dll_src = bundled_dir.join(DLL_FILE_NAME);
    if dll_src.is_file() {
        if let Err(e) = std::fs::copy(&dll_src, app_dir.join(DLL_FILE_NAME)) {
            log::warn!("failed to copy {} next to exe: {e}", DLL_FILE_NAME);
        }
    }

    // 3) Register the kernel service. binPath uses \SystemRoot so the path
    //    survives Windows on a non-C: drive.
    let bin_path = format!(r"\SystemRoot\System32\drivers\{DRIVER_FILE_NAME}");
    let bin_path_w: Vec<u16> = bin_path.encode_utf16().chain(Some(0)).collect();
    let service = unsafe {
        CreateServiceW(
            scm,
            WIN_DIVERT_SERVICE,
            WIN_DIVERT_SERVICE,
            SERVICE_ALL_ACCESS,
            SERVICE_KERNEL_DRIVER,
            SERVICE_DEMAND_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(bin_path_w.as_ptr()),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )
    }
    .map_err(|e| format!("failed to register WinDivert service: {e}"))?;

    let result = start_service_and_wait(service);
    let _ = unsafe { CloseServiceHandle(service) };
    result?;
    log::info!("WinDivert kernel driver installed and started");
    Ok(())
}

#[cfg(target_os = "windows")]
fn start_service_and_wait(service: SC_HANDLE) -> Result<(), String> {
    let mut status = SERVICE_STATUS::default();
    unsafe { QueryServiceStatus(service, &mut status) }
        .map_err(|e| format!("QueryServiceStatus failed: {e}"))?;
    if status.dwCurrentState == SERVICE_RUNNING {
        return Ok(());
    }
    if status.dwCurrentState != SERVICE_START_PENDING {
        unsafe { StartServiceW(service, None) }
            .map_err(|e| format!("failed to start WinDivert service: {e}"))?;
    }
    // Wait for the driver to reach RUNNING (kernel driver start is fast).
    for _ in 0..50 {
        std::thread::sleep(Duration::from_millis(100));
        unsafe { QueryServiceStatus(service, &mut status) }
            .map_err(|e| format!("QueryServiceStatus failed: {e}"))?;
        if status.dwCurrentState == SERVICE_RUNNING {
            return Ok(());
        }
        if status.dwCurrentState == SERVICE_STOPPED {
            return Err("WinDivert service stopped immediately after start".to_string());
        }
    }
    Err("WinDivert service did not reach RUNNING within 5s".to_string())
}

/// No-op on non-Windows platforms (Linux uses NFQUEUE, which is built into
/// the kernel; macOS has no ProxyBridge support at all).
#[cfg(not(target_os = "windows"))]
pub fn ensure_windivert_driver(_app_dir: &Path) -> Result<(), String> {
    Ok(())
}
