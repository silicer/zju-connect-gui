// Use the windows GUI subsystem so launching from Explorer doesn't flash a
// console window. The hidden console we still need for CTRL_BREAK signaling
// is allocated explicitly via `platform::init_console_for_signaling()` below.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod tray;

use std::sync::Arc;
use std::time::Duration;

use zju_connect_gui::backend::paths::resolve_app_dir;
use zju_connect_gui::backend::platform;
use zju_connect_gui::backend::relaunch_args::{parse_elevated_relaunch_args, ElevatedRelaunchArgs};
use zju_connect_gui::backend::{
    pending_connect_store::PendingConnectStore,
    proxy::{self, ProxyManager, ProxyManagerConfig, UiBridge},
    settings_store::{default_launch_options, UserSettingsStore},
};
use zju_connect_gui::web::bridge::WebUiBridge;
use zju_connect_gui::web::server;

const INSTANCE_LOCK_FILE: &str = "instance.lock";

/// How long the elevated child polls for the instance lock before giving up.
/// The parent only releases the lock after `ShellExecuteW("runas")` returns,
/// which can be delayed by a slow UAC consent prompt — allow several minutes.
const ELEVATED_LOCK_WAIT: Duration = Duration::from_secs(300);

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

    // ── Single-instance lock ───────────────────────────────────────
    // Elevated child: the parent drops the lock right before relaunching us,
    // but `ShellExecuteW("runas")` can sit on the UAC consent prompt for a
    // long time — so poll for the lock instead of trusting a fixed delay.
    // Normal launch: refuse if another instance is already running.
    let mut _instance_guard: Option<platform::SingleInstanceGuard> = None;
    if relaunch.wait_parent_pid > 0 {
        let parent_pid = relaunch.wait_parent_pid;
        let deadline = tokio::time::Instant::now() + ELEVATED_LOCK_WAIT;
        while tokio::time::Instant::now() < deadline {
            match acquire_lock(&app_dir) {
                Ok(guard) => {
                    _instance_guard = guard;
                    break;
                }
                Err(InstanceLockOutcome::AlreadyRunning) => {
                    // The parent drops the lock before exiting. If it is already
                    // gone while the lock is still busy, another instance owns
                    // it — nothing left to wait for.
                    let parent_gone = tokio::task::block_in_place(|| {
                        platform::wait_for_process_exit(parent_pid, Duration::from_millis(100))
                            .is_ok()
                    });
                    if parent_gone {
                        log::warn!(
                            "parent instance exited but the lock is held elsewhere; exiting"
                        );
                        return Ok(());
                    }
                }
                Err(InstanceLockOutcome::Unavailable(err)) => {
                    log::warn!("single-instance lock unavailable: {err}");
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if _instance_guard.is_none() {
            log::warn!("parent instance never released the instance lock; exiting");
            return Ok(());
        }
    } else {
        _instance_guard = match acquire_lock(&app_dir) {
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
    }

    // ── Core subsystems ───────────────────────────────────────────
    let settings = Arc::new(UserSettingsStore::new(&app_dir));
    let pending = Arc::new(PendingConnectStore::new(&app_dir));

    let cfg = ProxyManagerConfig::default();
    let manager =
        ProxyManager::with_config(app_dir.clone(), tokio::runtime::Handle::current(), cfg);

    // ── UI bridge + SSE channel ──────────────────────────────────
    let (bridge, _rx) = WebUiBridge::new(256);
    let bridge = Arc::new(bridge);
    manager.set_ui(bridge.clone());

    // ── Web server ───────────────────────────────────────────────
    let server = server::run(
        app_dir.clone(),
        manager.clone(),
        settings.clone(),
        bridge.clone(),
    )
    .await?;
    let port = server.port;
    let token = server.token.clone();
    server::persist_port(&app_dir, port).await;
    let server_handle = server.handle;
    let mut elevate_rx = server.elevate_rx;

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
                bridge.emit_event(proxy::ProxyEvent::Error(
                    "提权失败：系统未授予管理员权限。请启用 UAC 或以管理员身份运行本程序。"
                        .to_string(),
                ));
                // Don't attempt TUN mode or ProxyBridge if we're not elevated —
                // they will just fail. Disable both before retrying.
                let mut options = settings.load().unwrap_or_else(|err| {
                    log::warn!("settings load failed: {err}; using defaults");
                    default_launch_options()
                });
                let needs_elev = options.tun_mode || proxy::proxybridge::is_active(&options);
                if needs_elev {
                    log::warn!("disabling TUN mode / ProxyBridge because process is not elevated");
                    options.tun_mode = false;
                    options.proxybridge_enabled = false;
                }
                if let Err(err) = manager.start(options) {
                    log::error!("resume connect failed: {err}");
                }
            } else {
                let options = settings.load().unwrap_or_else(|err| {
                    log::warn!("settings load failed: {err}; using defaults");
                    default_launch_options()
                });
                if let Err(err) = manager.start(options) {
                    log::error!("resume connect failed: {err}");
                }
            }
        }
    }

    // ── Tray icon (best-effort) ─────────────────────────────────
    let quit_rx = match spawn_tray(port, token.clone(), bridge.clone()) {
        Ok(rx) => Some(rx),
        Err(err) => {
            log::warn!("tray icon disabled: {err}");
            None
        }
    };

    // ── Open browser ─────────────────────────────────────────────
    open_browser(port, &token);

    // ── Event loop: elevation request, tray quit, Ctrl+C ─────────
    // A failed elevation (UAC denied / unsupported platform) keeps the app
    // running and surfaces the error via SSE; only a successful relaunch
    // exits this process.
    let quit_fut = async move {
        match quit_rx {
            Some(rx) => {
                if rx.await.is_err() {
                    // The tray thread ended without a quit click (e.g. tray
                    // creation failed on a desktop without a StatusNotifier
                    // host). Keep running — only an explicit quit ends us.
                    std::future::pending::<()>().await;
                }
            }
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(quit_fut);

    let mut relaunched = false;
    loop {
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        tokio::select! {
            Some(args) = elevate_rx.recv() => {
                log::info!("elevation requested from web UI");
                // Coalesce: if another request arrived while we were busy,
                // only handle the latest one.
                let mut args = args;
                while let Ok(latest) = elevate_rx.try_recv() {
                    args = latest;
                }
                if let Err(err) = pending.mark_resume_connect() {
                    log::error!("failed to mark resume connect: {err}");
                }
                server::persist_port(&app_dir, port).await;
                match platform::relaunch_self_elevated(&args) {
                    Ok(()) => {
                        log::info!("elevated process launched, exiting");
                        drop(_instance_guard);
                        relaunched = true;
                        break;
                    }
                    Err(err) => {
                        log::error!("elevation failed: {err}");
                        // Don't leave a stale resume marker behind: a later
                        // normal launch would otherwise auto-connect without
                        // the user asking for it.
                        let _ = pending.clear();
                        // Stale elevation clicks queued while this attempt was
                        // in flight are just repeat UAC prompts; drop them.
                        while elevate_rx.try_recv().is_ok() {}
                        bridge.emit_event(proxy::ProxyEvent::Error(format!("提权失败：{err}")));
                    }
                }
            }
            _ = &mut quit_fut => {
                log::info!("quit requested from tray menu");
                break;
            }
            _ = &mut ctrl_c => {
                log::info!("received Ctrl+C, shutting down");
                break;
            }
        }
    }

    if relaunched {
        // Lock already dropped and the elevated process launched; just exit.
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

/// Create the tray icon on a dedicated OS thread.
///
/// On Linux, ksni's blocking API builds its own current-thread tokio runtime
/// and calls `block_on` internally, which panics when invoked from inside an
/// existing runtime — so the tray must be created on a plain (non-tokio)
/// thread on every platform. Returns a oneshot that fires when the user picks
/// "退出".
fn spawn_tray(
    port: u16,
    token: String,
    bridge: Arc<WebUiBridge>,
) -> std::io::Result<tokio::sync::oneshot::Receiver<()>> {
    let (quit_tx, quit_rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("zju-connect-tray".into())
        .spawn(move || match tray::TrayController::new(port, &token) {
            Ok((tray, tray_quit_rx)) => {
                // Forward the tray's quit signal to the main loop; `tray` stays
                // alive (and the icon registered) until then.
                if tray_quit_rx.blocking_recv().is_ok() {
                    let _ = quit_tx.send(());
                }
                drop(tray);
            }
            Err(err) => {
                log::warn!("tray icon disabled: {err}");
                // Make the failure user-visible in the logs tab (and note that
                // quitting then requires Ctrl+C / task manager).
                bridge.emit_event(proxy::ProxyEvent::Log(format!(
                    "[tray] 系统托盘不可用：{err}（退出需通过 Ctrl+C 或任务管理器）"
                )));
            }
        })?;
    Ok(quit_rx)
}

fn open_browser(port: u16, token: &str) {
    let url = format!("http://localhost:{port}/?token={token}");
    // The token is intentionally not logged: it would end up in terminal
    // history and journald.
    log::info!("opening browser at http://localhost:{port}");
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
