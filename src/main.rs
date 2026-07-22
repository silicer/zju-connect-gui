// Use the windows GUI subsystem so launching from Explorer doesn't flash a
// console window. The hidden console we still need for CTRL_BREAK signaling
// is allocated explicitly via `platform::init_console_for_signaling()` below.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod tray;
mod web;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;

use zju_connect_gui::backend::paths::resolve_app_dir;
use zju_connect_gui::backend::platform;
use zju_connect_gui::backend::relaunch_args::{parse_elevated_relaunch_args, ElevatedRelaunchArgs};
use zju_connect_gui::backend::{
    pending_connect_store::PendingConnectStore,
    proxy::{ProxyManager, ProxyManagerConfig, UiBridge},
    settings_store::UserSettingsStore,
};

use web::bridge::WebUiBridge;

const INSTANCE_LOCK_FILE: &str = "instance.lock";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // On Windows, allocate a hidden console so we can deliver CTRL_BREAK to children.
    platform::init_console_for_signaling();

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let relaunch = parse_elevated_relaunch_args(&argv).unwrap_or_default();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async_main(relaunch))?;
    Ok(())
}

enum InstanceLockOutcome {
    AlreadyRunning,
    Unavailable(std::io::Error),
}

fn acquire_lock(
    app_dir: &std::path::Path,
) -> Result<Option<platform::SingleInstanceGuard>, InstanceLockOutcome> {
    let lock_path = app_dir.join(INSTANCE_LOCK_FILE);
    match platform::acquire_single_instance(&lock_path) {
        Ok(guard) => Ok(Some(guard)),
        Err(platform::SingleInstanceError::AlreadyRunning) => {
            Err(InstanceLockOutcome::AlreadyRunning)
        }
        Err(platform::SingleInstanceError::Io(err)) => Err(InstanceLockOutcome::Unavailable(err)),
    }
}

async fn async_main(relaunch: ElevatedRelaunchArgs) -> Result<(), Box<dyn std::error::Error>> {
    let app_dir = resolve_app_dir()?;

    // ── For elevated child: wait for parent to exit so we can acquire the
    //    instance lock and rebind the web port. ────────────────────────
    if relaunch.wait_parent_pid > 0 {
        let pid = relaunch.wait_parent_pid;
        log::info!("waiting for parent process {pid} to exit...");
        let _ = tokio::task::spawn_blocking(move || {
            let _ = platform::wait_for_process_exit(pid, Duration::from_secs(15));
        })
        .await;
    }

    // ── Single-instance lock ───────────────────────────────────────
    // Elevated child: parent should be dead by now, lock is free.
    // Normal launch: refuse if another instance is already running.
    let _instance_guard = match acquire_lock(&app_dir) {
        Ok(guard) => guard,
        Err(InstanceLockOutcome::AlreadyRunning) => {
            log::warn!("zju-connect-gui is already running; exiting");
            return Ok(());
        }
        Err(InstanceLockOutcome::Unavailable(err)) => {
            log::warn!("single-instance lock unavailable: {err}");
            None
        }
    };

    // ── Core subsystems ───────────────────────────────────────────
    let settings = Arc::new(UserSettingsStore::new(&app_dir));
    let pending = Arc::new(PendingConnectStore::new(&app_dir));

    let _saved = settings.load().unwrap_or_else(|err| {
        log::warn!("settings load failed: {err}; using defaults");
        zju_connect_gui::backend::settings_store::default_launch_options()
    });

    let cfg = ProxyManagerConfig::default();
    let manager =
        ProxyManager::with_config(app_dir.clone(), tokio::runtime::Handle::current(), cfg);

    // ── UI bridge + SSE channel ──────────────────────────────────
    let (bridge, _rx) = WebUiBridge::new(256);
    let bridge = Arc::new(bridge);
    manager.set_ui(bridge.clone());

    // ── Web server ───────────────────────────────────────────────
    let (port, app_state, server_handle) = web::server::run(
        app_dir.clone(),
        manager.clone(),
        settings.clone(),
        bridge.clone(),
    )
    .await?;
    web::server::persist_port(&app_dir, port).await;

    // ── Elevate channel ──────────────────────────────────────────
    let elevate_rx = {
        let mut guard = app_state.elevate_tx.lock().await;
        // Swap out the tx so the handler can use the oneshot.
        let (tx, rx) = oneshot::channel();
        *guard = Some(tx);
        rx
    };

    // ── Resume pending connect (elevation flow) ──────────────────
    if relaunch.resume_pending_connect {
        let resume = match pending.has_resume_connect() {
            Ok(flag) => flag,
            Err(err) => {
                log::warn!("pending resume read failed: {err}");
                false
            }
        };
        let _ = pending.clear();

        if resume {
            let elevated = platform::is_process_elevated();
            log::info!("resuming pending connect after elevation (elevated={elevated})");
            if !elevated {
                log::error!(
                    "Process is NOT elevated after UAC. UAC may be disabled. \
                     Please enable UAC or run this program as Administrator."
                );
                bridge.emit_event(zju_connect_gui::backend::proxy::ProxyEvent::Error(
                    "提权失败：系统未授予管理员权限。请启用 UAC 或以管理员身份运行本程序。"
                        .to_string(),
                ));
                // Don't attempt TUN mode if we're not elevated — it will just fail.
                // Load settings and disable TUN mode before retrying.
                let mut options = settings.load().unwrap_or_default();
                if options.tun_mode {
                    log::warn!("disabling TUN mode because process is not elevated");
                    options.tun_mode = false;
                }
                if let Err(err) = manager.start(options) {
                    log::error!("resume connect failed: {err}");
                }
            } else {
                let options = settings.load().unwrap_or_default();
                if let Err(err) = manager.start(options) {
                    log::error!("resume connect failed: {err}");
                }
            }
        }
    }

    // ── Tray icon (best-effort) ─────────────────────────────────
    let (quit_rx, _tray) = match tray::TrayController::new(port) {
        Ok((t, rx)) => (Some(rx), Some(t)),
        Err(err) => {
            log::warn!("tray icon disabled: {err}");
            (None, None)
        }
    };

    // ── Open browser ─────────────────────────────────────────────
    open_browser(port);

    // ── Wait for shutdown or elevation signal ────────────────────
    let should_relaunch = {
        let elevate = elevate_rx;
        let ctrl_c = tokio::signal::ctrl_c();

        tokio::pin!(elevate);
        tokio::pin!(ctrl_c);

        let mut result = false;
        if let Some(mut quit_rx) = quit_rx {
            tokio::select! {
                Ok(args) = &mut elevate => {
                    log::info!("elevation requested from web UI");
                    drop(_instance_guard);
                    if let Err(err) = pending.mark_resume_connect() {
                        log::error!("failed to mark resume connect: {err}");
                    }
                    web::server::persist_port(&app_dir, port).await;
                    if let Err(err) = platform::relaunch_self_elevated(&args) {
                        log::error!("elevation failed: {err}");
                    } else {
                        log::info!("elevated process launched, exiting");
                    }
                    result = true;
                }
                _ = &mut quit_rx => {
                    log::info!("quit requested from tray menu");
                }
                _ = &mut ctrl_c => {
                    log::info!("received Ctrl+C, shutting down");
                }
            }
        } else {
            tokio::select! {
                Ok(args) = &mut elevate => {
                    log::info!("elevation requested from web UI");
                    drop(_instance_guard);
                    if let Err(err) = pending.mark_resume_connect() {
                        log::error!("failed to mark resume connect: {err}");
                    }
                    web::server::persist_port(&app_dir, port).await;
                    if let Err(err) = platform::relaunch_self_elevated(&args) {
                        log::error!("elevation failed: {err}");
                    } else {
                        log::info!("elevated process launched, exiting");
                    }
                    result = true;
                }
                _ = ctrl_c => {
                    log::info!("received Ctrl+C, shutting down");
                }
            }
        }
        result
    };

    if should_relaunch {
        // Already dropped lock and launched new process; just exit.
        log::info!("goodbye (relaunch)");
        return Ok(());
    }

    // ── Graceful shutdown ────────────────────────────────────────
    log::info!("stopping proxy before exit");
    if manager.is_running() {
        if let Err(err) = manager.stop() {
            log::warn!("manager.stop returned {err}");
        }
        for _ in 0..70 {
            if manager.snapshot().child_pid.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    server_handle.abort();
    log::info!("goodbye");
    Ok(())
}

fn open_browser(port: u16) {
    let url = format!("http://localhost:{port}");
    log::info!("opening browser at {url}");
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("rundll32")
            .arg("url.dll,FileProtocolHandler")
            .arg(&url)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
}
