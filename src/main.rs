// Use the windows GUI subsystem so launching from Explorer doesn't flash a
// console window. The hidden console we still need for CTRL_BREAK signaling
// is allocated explicitly via `platform::init_console_for_signaling()` below.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;

mod tray;
mod web;

use zju_connect_gui::backend::paths::resolve_app_dir;
use zju_connect_gui::backend::platform;
use zju_connect_gui::backend::relaunch_args::parse_elevated_relaunch_args;

const INSTANCE_LOCK_FILE: &str = "instance.lock";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // On Windows, allocate a hidden console so we can deliver CTRL_BREAK to children.
    platform::init_console_for_signaling();

    // Single-instance: bail out cleanly if another GUI is already running. The
    // guard is bound to a named local so Drop runs at process exit.
    let _instance_guard = match acquire_instance_lock() {
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

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let relaunch = parse_elevated_relaunch_args(&argv).unwrap_or_default();

    let app = app::App::new(relaunch).await?;
    app.run().await?;
    Ok(())
}

enum InstanceLockOutcome {
    AlreadyRunning,
    Unavailable(std::io::Error),
}

fn acquire_instance_lock() -> Result<Option<platform::SingleInstanceGuard>, InstanceLockOutcome> {
    let app_dir = resolve_app_dir().map_err(InstanceLockOutcome::Unavailable)?;
    let lock_path = app_dir.join(INSTANCE_LOCK_FILE);
    match platform::acquire_single_instance(&lock_path) {
        Ok(guard) => Ok(Some(guard)),
        Err(platform::SingleInstanceError::AlreadyRunning) => {
            Err(InstanceLockOutcome::AlreadyRunning)
        }
        Err(platform::SingleInstanceError::Io(err)) => Err(InstanceLockOutcome::Unavailable(err)),
    }
}
