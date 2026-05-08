slint::include_modules!();

mod app;
mod ui_glue;

use zju_connect_gui::backend::platform;
use zju_connect_gui::backend::relaunch_args::parse_elevated_relaunch_args;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    // On Windows, allocate a hidden console so we can deliver CTRL_BREAK to children.
    platform::init_console_for_signaling();

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let relaunch = parse_elevated_relaunch_args(&argv).unwrap_or_default();

    let app = app::App::new(relaunch)?;
    app.run()?;
    Ok(())
}
