//! Main coordinator: ties Slint UI to ProxyManager, settings/pending stores,
//! and the relaunch-args / elevation handshake.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};
use tokio::runtime::Runtime;
use tokio::sync::Notify;

use crate::ui_glue::CapturingBridge;
use crate::{AppWindow, CaptchaPoint};

use zju_connect_gui::backend::launch_options::LaunchOptions;
use zju_connect_gui::backend::paths::resolve_app_dir;
use zju_connect_gui::backend::pending_connect_store::PendingConnectStore;
use zju_connect_gui::backend::platform;
use zju_connect_gui::backend::proxy::{ProxyManager, ProxyManagerConfig};
use zju_connect_gui::backend::relaunch_args::ElevatedRelaunchArgs;
use zju_connect_gui::backend::settings_store::UserSettingsStore;

const PERSIST_DEBOUNCE: Duration = Duration::from_millis(250);
const MAX_LOG_ENTRIES: usize = 1000;

pub struct App {
    pub window: AppWindow,
    pub _manager: ProxyManager,
    pub _settings: Arc<UserSettingsStore>,
    pub _runtime: Runtime,
}

impl App {
    pub fn new(args: ElevatedRelaunchArgs) -> Result<Self, AppError> {
        let app_dir = resolve_app_dir().map_err(AppError::ResolveAppDir)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(AppError::Runtime)?;

        let settings = Arc::new(UserSettingsStore::new(&app_dir));
        let pending = Arc::new(PendingConnectStore::new(&app_dir));

        let saved = settings.load().unwrap_or_else(|err| {
            log::warn!("settings load failed: {err}; using defaults");
            zju_connect_gui::backend::settings_store::default_launch_options()
        });

        let cfg = ProxyManagerConfig::default();
        let manager = ProxyManager::with_config(app_dir.clone(), runtime.handle().clone(), cfg);

        let window = AppWindow::new().map_err(AppError::Window)?;

        // Install the log + captcha-points models on the window so the bridge can
        // mutate them via downcast in the event handler.
        let log_model: Rc<VecModel<SharedString>> = Rc::new(VecModel::default());
        window.set_logs(ModelRc::from(log_model.clone()));
        let captcha_model: Rc<VecModel<CaptchaPoint>> = Rc::new(VecModel::default());
        window.set_captcha_points(ModelRc::from(captcha_model.clone()));

        apply_options_to_window(&window, &saved);

        let persist_signal = Arc::new(Notify::new());

        manager.set_ui(Arc::new(CapturingBridge::new(
            window.as_weak(),
            MAX_LOG_ENTRIES,
        )));

        wire_ui_callbacks(
            &window,
            manager.clone(),
            settings.clone(),
            persist_signal.clone(),
            captcha_model.clone(),
            log_model.clone(),
            app_dir.clone(),
        );

        spawn_persist_debounce(
            runtime.handle().clone(),
            window.as_weak(),
            settings.clone(),
            persist_signal.clone(),
        );

        if args.resume_pending_connect {
            try_resume_pending_connect(
                &runtime,
                &pending,
                &settings,
                &manager,
                &window,
                args.wait_parent_pid,
            );
        }

        Ok(Self {
            window,
            _manager: manager,
            _settings: settings,
            _runtime: runtime,
        })
    }

    pub fn run(self) -> Result<(), AppError> {
        self.window.run().map_err(AppError::Window)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("failed to resolve app directory: {0}")]
    ResolveAppDir(std::io::Error),
    #[error("failed to create tokio runtime: {0}")]
    Runtime(std::io::Error),
    #[error("failed to create application window: {0}")]
    Window(slint::PlatformError),
}

fn apply_options_to_window(window: &AppWindow, options: &LaunchOptions) {
    window.set_username(SharedString::from(options.username.as_str()));
    window.set_password(SharedString::from(options.password.as_str()));
    window.set_socks_bind(SharedString::from(options.socks_bind.as_str()));
    window.set_http_bind(SharedString::from(options.http_bind.as_str()));
    window.set_proxy_only(!options.tun_mode);
    window.set_debug_dump(options.debug_dump);
    window.set_eip_browser_program(SharedString::from(options.eip_browser_program.as_str()));
    window.set_eip_browser_args(SharedString::from(options.eip_browser_args.join("\n")));
}

fn read_options_from_window(window: &AppWindow) -> LaunchOptions {
    LaunchOptions {
        username: window.get_username().to_string(),
        password: window.get_password().to_string(),
        socks_bind: window.get_socks_bind().to_string(),
        http_bind: window.get_http_bind().to_string(),
        tun_mode: !window.get_proxy_only(),
        debug_dump: window.get_debug_dump(),
        eip_browser_program: window.get_eip_browser_program().to_string(),
        eip_browser_args: parse_args_textarea(&window.get_eip_browser_args()),
        ..LaunchOptions::default()
    }
}

fn parse_args_textarea(value: &str) -> Vec<String> {
    value
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn wire_ui_callbacks(
    window: &AppWindow,
    manager: ProxyManager,
    settings: Arc<UserSettingsStore>,
    persist_signal: Arc<Notify>,
    captcha_model: Rc<VecModel<CaptchaPoint>>,
    log_model: Rc<VecModel<SharedString>>,
    app_dir: PathBuf,
) {
    // start
    {
        let manager = manager.clone();
        let settings = settings.clone();
        let weak = window.as_weak();
        let app_dir = app_dir.clone();
        window.on_start(move || {
            let Some(w) = weak.upgrade() else { return };
            let options = read_options_from_window(&w);

            if let Err(err) = options.validate() {
                w.set_status_message(SharedString::from(format!("{err}")));
                return;
            }

            if let Err(err) = settings.save(options.clone()) {
                log::warn!("settings save before start failed: {err}");
            }

            #[cfg(target_os = "windows")]
            if options.tun_mode && !platform::is_process_elevated() {
                w.set_status_message(SharedString::from("正在请求管理员权限..."));
                use zju_connect_gui::backend::relaunch_args::build_elevated_relaunch_args;
                let pending = PendingConnectStore::new(&app_dir);
                if let Err(err) = pending.mark_resume_connect() {
                    log::warn!("pending mark failed: {err}");
                }
                let parent_pid = std::process::id();
                if let Err(err) =
                    platform::relaunch_self_elevated(&build_elevated_relaunch_args(parent_pid))
                {
                    w.set_status_message(SharedString::from(format!("提权失败：{err}")));
                    return;
                }
                slint::quit_event_loop().ok();
                return;
            }
            // suppress unused warning on non-windows
            let _ = &app_dir;

            w.set_status_message(SharedString::from("正在启动..."));
            if let Err(err) = manager.start(options) {
                w.set_status_message(SharedString::from(format!("启动失败：{err}")));
            }
        });
    }

    // stop
    {
        let manager = manager.clone();
        let weak = window.as_weak();
        window.on_stop(move || {
            let Some(w) = weak.upgrade() else { return };
            w.set_status_message(SharedString::from("正在停止..."));
            if let Err(err) = manager.stop() {
                w.set_status_message(SharedString::from(format!("停止失败：{err}")));
            }
        });
    }

    // submit-input
    {
        let manager = manager.clone();
        let weak = window.as_weak();
        let captcha_model = captcha_model.clone();
        window.on_submit_input(move |value| {
            let Some(w) = weak.upgrade() else { return };
            if let Err(err) = manager.submit_input(value.as_str()) {
                w.set_status_message(SharedString::from(format!("提交失败：{err}")));
                return;
            }
            w.set_modal_open(false);
            w.set_modal_input(SharedString::from(""));
            captcha_model.set_vec(Vec::<CaptchaPoint>::new());
        });
    }

    // submit-captcha
    {
        let manager = manager.clone();
        let weak = window.as_weak();
        let captcha_model = captcha_model.clone();
        window.on_submit_captcha(move |natural_w, natural_h| {
            let Some(w) = weak.upgrade() else { return };
            if natural_w <= 0 || natural_h <= 0 {
                w.set_status_message(SharedString::from("验证码尺寸未就绪，请重新点击验证码"));
                return;
            }
            let points: Vec<CaptchaPoint> = captcha_model.iter().collect();
            if points.is_empty() {
                w.set_status_message(SharedString::from("请先点击验证码图片"));
                return;
            }
            let coords: Vec<[i32; 2]> = points.iter().map(|p| [p.x, p.y]).collect();
            let payload = serde_json::json!({
                "coordinates": coords,
                "width": natural_w,
                "height": natural_h,
            });
            if let Err(err) = manager.submit_input(&payload.to_string()) {
                w.set_status_message(SharedString::from(format!("提交失败：{err}")));
                return;
            }
            w.set_modal_open(false);
            captcha_model.set_vec(Vec::<CaptchaPoint>::new());
        });
    }

    // cancel-modal
    {
        let weak = window.as_weak();
        let captcha_model = captcha_model.clone();
        window.on_cancel_modal(move || {
            let Some(w) = weak.upgrade() else { return };
            w.set_modal_open(false);
            w.set_modal_input(SharedString::from(""));
            captcha_model.set_vec(Vec::<CaptchaPoint>::new());
        });
    }

    // clear-logs
    {
        let log_model = log_model.clone();
        window.on_clear_logs(move || {
            log_model.set_vec(Vec::<SharedString>::new());
        });
    }

    // pick-eip-browser
    {
        let weak = window.as_weak();
        window.on_pick_eip_browser(move || {
            let Some(w) = weak.upgrade() else { return };
            w.set_status_message(SharedString::from(
                "请直接粘贴浏览器可执行文件路径（暂未实现选择器）",
            ));
        });
    }

    // clear-eip-browser
    {
        let weak = window.as_weak();
        window.on_clear_eip_browser(move || {
            let Some(w) = weak.upgrade() else { return };
            w.set_eip_browser_program(SharedString::from(""));
        });
    }

    // persist-options
    {
        let signal = persist_signal.clone();
        window.on_persist_options(move || {
            signal.notify_one();
        });
    }

    // captcha point management
    {
        let captcha_model = captcha_model.clone();
        window.on_add_captcha_point(move |x, y| {
            captcha_model.push(CaptchaPoint { x, y });
        });
    }
    {
        let captcha_model = captcha_model.clone();
        window.on_remove_last_captcha_point(move || {
            let len = captcha_model.row_count();
            if len > 0 {
                captcha_model.remove(len - 1);
            }
        });
    }
    {
        let captcha_model = captcha_model;
        window.on_clear_captcha_points(move || {
            captcha_model.set_vec(Vec::<CaptchaPoint>::new());
        });
    }
}

/// Persist debounce: notify on each option change, sleep PERSIST_DEBOUNCE, then
/// snapshot the window state and write it.
fn spawn_persist_debounce(
    handle: tokio::runtime::Handle,
    weak: Weak<AppWindow>,
    settings: Arc<UserSettingsStore>,
    signal: Arc<Notify>,
) {
    handle.spawn(async move {
        loop {
            signal.notified().await;
            tokio::time::sleep(PERSIST_DEBOUNCE).await;
            // Snapshot must happen on the UI thread.
            let weak = weak.clone();
            let settings = settings.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = weak.upgrade() else { return };
                let options = read_options_from_window(&w);
                if let Err(err) = settings.save(options) {
                    log::warn!("autosave failed: {err}");
                }
            });
        }
    });
}

fn try_resume_pending_connect(
    runtime: &Runtime,
    pending: &Arc<PendingConnectStore>,
    settings: &Arc<UserSettingsStore>,
    manager: &ProxyManager,
    window: &AppWindow,
    parent_pid: u32,
) {
    if parent_pid > 0 {
        let _ = runtime.block_on(async move {
            tokio::task::spawn_blocking(move || {
                let _ = platform::wait_for_process_exit(parent_pid, Duration::from_secs(15));
            })
            .await
        });
    }

    let resume = match pending.has_resume_connect() {
        Ok(flag) => flag,
        Err(err) => {
            log::warn!("pending resume read failed: {err}");
            false
        }
    };
    let _ = pending.clear();
    if !resume {
        return;
    }

    let options = settings.load().unwrap_or_default();
    apply_options_to_window(window, &options);
    window.set_status_message(SharedString::from("已切换到管理员模式，正在恢复连接..."));
    if let Err(err) = manager.start(options) {
        window.set_status_message(SharedString::from(format!("恢复连接失败：{err}")));
    }
}
