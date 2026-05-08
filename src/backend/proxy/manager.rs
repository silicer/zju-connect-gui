use crate::backend::external_links::{open_eip, OpenEipError};
use crate::backend::launch_options::LaunchOptions;
use crate::backend::proxy::captcha::{
    encode_captcha, monitor_captcha_file, poll_for_stable_captcha,
};
use crate::backend::proxy::logs::{
    classify_prompt, consume_stream, is_route_added, is_vpn_started, DetectedPrompt,
};
use crate::backend::proxy::readiness::{check_tcp_connect, readiness_dial_address};
use crate::backend::proxy::retry::{
    default_jitter, format_retry_delay, next_retry_delay, JitterFn, DEFAULT_RETRY_BASE_DELAY,
    DEFAULT_RETRY_MAX_DELAY,
};
use rand::Rng;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin};
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

const HTTP_READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HTTP_READY_DIAL_TIMEOUT: Duration = Duration::from_millis(200);
const MIN_EIP_AUTO_OPEN_DELAY: Duration = Duration::from_secs(3);
const MAX_EIP_AUTO_OPEN_DELAY: Duration = Duration::from_secs(5);
const STOP_GRACE_PERIOD: Duration = Duration::from_secs(5);
const CAPTCHA_FILE_NAME: &str = "gui_captcha.png";

/// Events the proxy manager emits to the UI side. The UI converts these into Slint
/// model updates / dialog spawns / status messages.
#[derive(Debug, Clone)]
pub enum ProxyEvent {
    Log(String),
    State {
        state: ProxyState,
        message: Option<String>,
        awaiting: Option<String>,
        running: bool,
        retry_attempt: u32,
        retry_delay_ms: u64,
    },
    NeedInput {
        kind: InputKind,
        prompt: String,
    },
    NeedCaptcha {
        base64: String,
        updated_at_ms: i64,
    },
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyState {
    Stopped,
    Running,
    Connected,
    Awaiting,
    Retrying,
    Connecting,
}

impl ProxyState {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProxyState::Stopped => "stopped",
            ProxyState::Running => "running",
            ProxyState::Connected => "connected",
            ProxyState::Awaiting => "awaiting",
            ProxyState::Retrying => "retrying",
            ProxyState::Connecting => "connecting",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Sms,
    Callback,
}

impl InputKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            InputKind::Sms => "sms",
            InputKind::Callback => "callback",
        }
    }
}

/// Bridge from manager → UI. Implementations forward events onto the Slint event loop.
pub trait UiBridge: Send + Sync + 'static {
    fn emit_event(&self, event: ProxyEvent);
    fn show_window(&self);
}

/// Function that opens the EIP browser. Returned errors propagate to the log stream.
pub type EipOpener = Arc<dyn Fn(&LaunchOptions) -> Result<(), OpenEipError> + Send + Sync>;

/// Test/runtime knobs that influence manager behavior. The default is the production
/// config; tests override fields to inject deterministic timing.
pub struct ProxyManagerConfig {
    pub retry_base_delay: Duration,
    pub retry_max_delay: Duration,
    pub retry_jitter: JitterFn,
    pub eip_auto_open_delay: fn() -> Duration,
    pub binary_path: Option<PathBuf>,
    pub eip_opener: EipOpener,
}

impl Default for ProxyManagerConfig {
    fn default() -> Self {
        Self {
            retry_base_delay: DEFAULT_RETRY_BASE_DELAY,
            retry_max_delay: DEFAULT_RETRY_MAX_DELAY,
            retry_jitter: default_jitter,
            eip_auto_open_delay: default_eip_auto_open_delay,
            binary_path: None,
            eip_opener: Arc::new(default_eip_opener),
        }
    }
}

fn default_eip_auto_open_delay() -> Duration {
    let secs = rand::thread_rng().gen_range(3..=5);
    Duration::from_secs(secs)
}

fn default_eip_opener(options: &LaunchOptions) -> Result<(), OpenEipError> {
    open_eip(&options.eip_browser_program, &options.eip_browser_args)
}

#[derive(Debug, Error)]
pub enum StartError {
    #[error("zju-connect is already running")]
    AlreadyRunning,
    #[error("zju-connect binary not found: {0}")]
    BinaryMissing(PathBuf),
    #[error("invalid options: {0}")]
    Validation(crate::backend::launch_options::ValidationError),
    #[error("failed to spawn child: {0}")]
    Spawn(std::io::Error),
    #[error("tun mode requires elevated application restart")]
    NeedsElevation,
}

#[derive(Debug, Error)]
pub enum StopError {
    #[error("io error during stop: {0}")]
    Io(#[from] std::io::Error),
    #[error("timeout waiting for process to stop")]
    Timeout,
}

#[derive(Debug, Error)]
pub enum SubmitInputError {
    #[error("input cannot be empty")]
    Empty,
    #[error("process is not running")]
    NotRunning,
    #[error("io error: {0}")]
    Io(std::io::Error),
}

struct State {
    session_active: bool,
    awaiting: Option<String>,
    captcha_polling: bool,
    captcha_path: PathBuf,
    eip_options: LaunchOptions,
    eip_opened: bool,
    last_options: LaunchOptions,
    ready: bool,
    ready_wait_gen: u64,
    retry_attempt: u32,
    retry_generation: u64,
    child_pid: Option<u32>,
    stdin_tx: Option<mpsc::UnboundedSender<String>>,
    stop_tx: Option<oneshot::Sender<()>>,
    delayed_eip: Option<JoinHandle<()>>,
    retry_handle: Option<JoinHandle<()>>,
    readiness_handle: Option<JoinHandle<()>>,
}

impl State {
    fn new() -> Self {
        Self {
            session_active: false,
            awaiting: None,
            captcha_polling: false,
            captcha_path: PathBuf::new(),
            eip_options: LaunchOptions::default(),
            eip_opened: false,
            last_options: LaunchOptions::default(),
            ready: false,
            ready_wait_gen: 0,
            retry_attempt: 0,
            retry_generation: 0,
            child_pid: None,
            stdin_tx: None,
            stop_tx: None,
            delayed_eip: None,
            retry_handle: None,
            readiness_handle: None,
        }
    }

    fn cancel_delayed_eip(&mut self) {
        if let Some(handle) = self.delayed_eip.take() {
            handle.abort();
        }
    }

    fn cancel_retry(&mut self) {
        if let Some(handle) = self.retry_handle.take() {
            handle.abort();
        }
    }

    fn cancel_readiness(&mut self) {
        if let Some(handle) = self.readiness_handle.take() {
            handle.abort();
        }
    }
}

struct Inner {
    app_dir: PathBuf,
    ui: Mutex<Option<Arc<dyn UiBridge>>>,
    state: Mutex<State>,
    config: ProxyManagerConfig,
    runtime: Handle,
}

impl Inner {
    fn emit_event(&self, event: ProxyEvent) {
        if let Some(ui) = self.ui.lock().expect("ui mutex poisoned").as_ref() {
            ui.emit_event(event);
        }
    }

    fn show_window(&self) {
        if let Some(ui) = self.ui.lock().expect("ui mutex poisoned").as_ref() {
            ui.show_window();
        }
    }

    fn emit_log(&self, line: impl Into<String>) {
        self.emit_event(ProxyEvent::Log(line.into()));
    }

    fn emit_state(self: &Arc<Self>, state: ProxyState, message: Option<String>) {
        let payload = {
            let s = self.state.lock().expect("state mutex poisoned");
            ProxyEvent::State {
                state,
                awaiting: s.awaiting.clone(),
                running: s.session_active,
                message,
                retry_attempt: 0,
                retry_delay_ms: 0,
            }
        };
        self.emit_event(payload);
    }

    fn emit_state_with_details(
        self: &Arc<Self>,
        state: ProxyState,
        message: Option<String>,
        retry_attempt: u32,
        retry_delay: Duration,
    ) {
        let payload = {
            let s = self.state.lock().expect("state mutex poisoned");
            ProxyEvent::State {
                state,
                awaiting: s.awaiting.clone(),
                running: s.session_active,
                message,
                retry_attempt,
                retry_delay_ms: retry_delay.as_millis() as u64,
            }
        };
        self.emit_event(payload);
    }
}

#[derive(Clone)]
pub struct ProxyManager {
    inner: Arc<Inner>,
}

impl ProxyManager {
    pub fn new(app_dir: PathBuf, runtime: Handle) -> Self {
        Self::with_config(app_dir, runtime, ProxyManagerConfig::default())
    }

    pub fn with_config(app_dir: PathBuf, runtime: Handle, config: ProxyManagerConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                app_dir,
                ui: Mutex::new(None),
                state: Mutex::new(State::new()),
                config,
                runtime,
            }),
        }
    }

    pub fn set_ui(&self, ui: Arc<dyn UiBridge>) {
        *self.inner.ui.lock().expect("ui mutex poisoned") = Some(ui);
    }

    pub fn is_running(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("state mutex poisoned")
            .session_active
    }

    pub fn start(&self, options: LaunchOptions) -> Result<(), StartError> {
        use crate::backend::launch_options::normalize_launch_options;
        let mut options = normalize_launch_options(options);
        if !Path::new(&options.client_data_file).is_absolute() {
            options.client_data_file = self
                .inner
                .app_dir
                .join(&options.client_data_file)
                .to_string_lossy()
                .into_owned();
        }
        options.validate().map_err(StartError::Validation)?;

        // Acquire session_active and bump generation atomically.
        {
            let mut state = self.inner.state.lock().expect("state mutex poisoned");
            if state.child_pid.is_some() {
                return Err(StartError::AlreadyRunning);
            }
            state.retry_generation = state.retry_generation.wrapping_add(1);
            state.cancel_retry();
            state.cancel_delayed_eip();
        }

        let captcha_path = self.inner.app_dir.join(CAPTCHA_FILE_NAME);

        {
            let mut state = self.inner.state.lock().expect("state mutex poisoned");
            state.captcha_path = captcha_path.clone();
            state.eip_options = options.clone();
            state.eip_opened = false;
            state.last_options = options.clone();
            state.ready = false;
            state.ready_wait_gen = 0;
            state.session_active = true;
            state.retry_attempt = 0;
            state.awaiting = None;
            state.captcha_polling = false;
        }

        let _ = std::fs::remove_file(&captcha_path);

        match self.spawn_child(options.clone(), captcha_path.clone()) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.cleanup_failed_start();
                Err(err)
            }
        }
    }

    fn cleanup_failed_start(&self) {
        let mut state = self.inner.state.lock().expect("state mutex poisoned");
        state.session_active = false;
        state.awaiting = None;
        state.captcha_polling = false;
        state.child_pid = None;
        state.stdin_tx = None;
        state.stop_tx = None;
        drop(state);
        self.inner.emit_state(ProxyState::Stopped, None);
    }

    fn spawn_child(&self, options: LaunchOptions, captcha_path: PathBuf) -> Result<(), StartError> {
        let bin_path = self
            .inner
            .config
            .binary_path
            .clone()
            .unwrap_or_else(|| binary_path_under(&self.inner.app_dir));
        if !bin_path.exists() {
            return Err(StartError::BinaryMissing(bin_path));
        }
        let mut command = tokio::process::Command::new(&bin_path);
        command
            .args(options.build_args(captcha_path.to_string_lossy().as_ref()))
            .current_dir(&self.inner.app_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_child_process(&mut command);
        let mut child = command.spawn().map_err(StartError::Spawn)?;
        let pid = child.id();

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let child_stdin = child.stdin.take().expect("piped stdin");

        let (stdin_tx, stdin_rx) = mpsc::unbounded_channel::<String>();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();

        {
            let mut state = self.inner.state.lock().expect("state mutex poisoned");
            state.child_pid = pid;
            state.stdin_tx = Some(stdin_tx);
            state.stop_tx = Some(stop_tx);
        }

        self.inner.emit_state(ProxyState::Running, None);

        let inner = self.inner.clone();
        let generation = inner
            .state
            .lock()
            .expect("state mutex poisoned")
            .retry_generation;
        let captcha_for_supervise = captcha_path.clone();
        self.inner.runtime.spawn(supervise_child(
            inner,
            child,
            stdout,
            stderr,
            child_stdin,
            stdin_rx,
            stop_rx,
            captcha_for_supervise,
            generation,
        ));

        Ok(())
    }

    pub fn stop(&self) -> Result<(), StopError> {
        let (stop_tx, retry_handle, readiness_handle, delayed_eip, child_pid_present) = {
            let mut state = self.inner.state.lock().expect("state mutex poisoned");
            state.retry_generation = state.retry_generation.wrapping_add(1);
            state.session_active = false;
            state.retry_attempt = 0;
            state.ready = false;
            state.ready_wait_gen = 0;
            state.awaiting = None;
            state.captcha_polling = false;
            (
                state.stop_tx.take(),
                state.retry_handle.take(),
                state.readiness_handle.take(),
                state.delayed_eip.take(),
                state.child_pid.is_some(),
            )
        };
        if let Some(handle) = retry_handle {
            handle.abort();
        }
        if let Some(handle) = readiness_handle {
            handle.abort();
        }
        if let Some(handle) = delayed_eip {
            handle.abort();
        }

        if !child_pid_present {
            self.inner.emit_state(ProxyState::Stopped, None);
            return Ok(());
        }
        if let Some(tx) = stop_tx {
            let _ = tx.send(());
        }
        Ok(())
    }

    pub fn submit_input(&self, value: &str) -> Result<(), SubmitInputError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(SubmitInputError::Empty);
        }
        let stdin_tx = {
            let state = self.inner.state.lock().expect("state mutex poisoned");
            state.stdin_tx.clone()
        };
        let Some(tx) = stdin_tx else {
            return Err(SubmitInputError::NotRunning);
        };
        tx.send(value.to_string())
            .map_err(|_| SubmitInputError::NotRunning)?;
        // Clear awaiting state on successful submit.
        let inner = self.inner.clone();
        let mut state = inner.state.lock().expect("state mutex poisoned");
        if state.awaiting.is_some() {
            state.awaiting = None;
            drop(state);
            inner.emit_state(ProxyState::Awaiting, None);
        }
        Ok(())
    }

    /// Snapshot of internal state for diagnostics / tests.
    pub fn snapshot(&self) -> StateSnapshot {
        let s = self.inner.state.lock().expect("state mutex poisoned");
        StateSnapshot {
            session_active: s.session_active,
            ready: s.ready,
            retry_attempt: s.retry_attempt,
            awaiting: s.awaiting.clone(),
            child_pid: s.child_pid,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateSnapshot {
    pub session_active: bool,
    pub ready: bool,
    pub retry_attempt: u32,
    pub awaiting: Option<String>,
    pub child_pid: Option<u32>,
}

fn binary_path_under(app_dir: &Path) -> PathBuf {
    if cfg!(target_os = "windows") {
        app_dir.join("bin").join("zju-connect.exe")
    } else {
        app_dir.join("bin").join("zju-connect")
    }
}

#[cfg(target_os = "windows")]
fn configure_child_process(command: &mut tokio::process::Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NEW_PROCESS_GROUP (0x00000200): the child becomes the lead of its own
    // process group, which lets us target it with `GenerateConsoleCtrlEvent` for
    // graceful shutdown without affecting our own group. Note: we deliberately do NOT
    // set CREATE_NO_WINDOW — that flag detaches the child from any console handle,
    // which would also block console-signal delivery. Instead we allocate a hidden
    // console for ourselves at startup (see `platform::init_console_for_signaling`),
    // which the child inherits via the spawn.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(target_os = "windows"))]
fn configure_child_process(_command: &mut tokio::process::Command) {}

#[allow(clippy::too_many_arguments)]
async fn supervise_child(
    inner: Arc<Inner>,
    mut child: Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
    mut child_stdin: ChildStdin,
    mut stdin_rx: mpsc::UnboundedReceiver<String>,
    stop_rx: oneshot::Receiver<()>,
    captcha_path: PathBuf,
    generation: u64,
) {
    // Stdout / stderr readers
    let stdout_inner = inner.clone();
    let stdout_task: JoinHandle<std::io::Result<()>> = tokio::spawn(async move {
        consume_stream(
            stdout,
            |line| handle_log_line(&stdout_inner, line),
            |partial| handle_partial_line(&stdout_inner, partial),
        )
        .await
    });

    let stderr_inner = inner.clone();
    let stderr_task: JoinHandle<std::io::Result<()>> = tokio::spawn(async move {
        consume_stream(
            stderr,
            |line| handle_log_line(&stderr_inner, line),
            |partial| handle_partial_line(&stderr_inner, partial),
        )
        .await
    });

    // Stdin forwarder
    let stdin_task = tokio::spawn(async move {
        while let Some(line) = stdin_rx.recv().await {
            let payload = format!("{line}\n");
            if child_stdin.write_all(payload.as_bytes()).await.is_err() {
                break;
            }
            let _ = child_stdin.flush().await;
        }
    });

    // Captcha file monitor
    let captcha_inner = inner.clone();
    let captcha_path_for_monitor = captcha_path.clone();
    let captcha_task = tokio::spawn(async move {
        monitor_captcha_file(captcha_path_for_monitor, |path| {
            log::info!("captcha file updated: {}", path.display());
            request_captcha(captcha_inner.clone());
        })
        .await;
    });

    // Wait for child exit or graceful stop request.
    let exit_status = tokio::select! {
        result = child.wait() => result,
        _ = stop_rx => {
            // Try to deliver an OS-appropriate graceful-stop signal (SIGINT on unix,
            // CTRL_BREAK on windows). If that fails, or if the child doesn't exit
            // within STOP_GRACE_PERIOD, fall back to start_kill (SIGKILL / TerminateProcess).
            if let Err(err) = crate::backend::platform::signal_child_to_quit(&child) {
                log::warn!("graceful signal failed: {err}; falling back to kill");
                let _ = child.start_kill();
            }
            match tokio::time::timeout(STOP_GRACE_PERIOD, child.wait()).await {
                Ok(res) => res,
                Err(_) => {
                    let _ = child.start_kill();
                    child.wait().await
                }
            }
        }
    };

    captcha_task.abort();
    let _ = stdout_task.await;
    let _ = stderr_task.await;
    drop(stdin_task);

    {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        state.child_pid = None;
        state.stdin_tx = None;
        state.stop_tx = None;
        state.captcha_polling = false;
    }

    handle_process_exit(inner, exit_status, generation).await;
}

fn handle_log_line(inner: &Arc<Inner>, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    inner.emit_log(trimmed);
    detect_prompt_and_react(inner, trimmed);

    // Readiness wiring depends on the active mode.
    let (tun_mode, http_bind) = {
        let state = inner.state.lock().expect("state mutex poisoned");
        (
            state.eip_options.tun_mode,
            state.eip_options.http_bind.clone(),
        )
    };
    if tun_mode {
        if is_route_added(trimmed) {
            begin_http_ready_wait(inner.clone(), http_bind);
        }
        return;
    }
    if is_vpn_started(trimmed) {
        begin_http_ready_wait(inner.clone(), http_bind);
    }
}

fn handle_partial_line(inner: &Arc<Inner>, partial: &str) {
    detect_prompt_and_react(inner, partial);
}

fn detect_prompt_and_react(inner: &Arc<Inner>, line: &str) {
    let Some(prompt) = classify_prompt(line) else {
        return;
    };
    match prompt {
        DetectedPrompt::Sms => {
            request_input(inner.clone(), InputKind::Sms, "Please enter the SMS code")
        }
        DetectedPrompt::Callback => request_input(
            inner.clone(),
            InputKind::Callback,
            "Please enter the callback URL",
        ),
        DetectedPrompt::Captcha => request_captcha(inner.clone()),
    }
}

fn request_input(inner: Arc<Inner>, kind: InputKind, prompt: &str) {
    let kind_str = kind.as_str();
    let changed = {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        let already = state.awaiting.as_deref() == Some(kind_str);
        if !already {
            state.awaiting = Some(kind_str.to_string());
        }
        !already
    };
    if !changed {
        return;
    }
    inner.show_window();
    inner.emit_event(ProxyEvent::NeedInput {
        kind,
        prompt: prompt.to_string(),
    });
    inner.emit_state(ProxyState::Awaiting, None);
}

fn request_captcha(inner: Arc<Inner>) {
    let captcha_path = {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        let already = state.awaiting.as_deref() == Some("captcha");
        if !already {
            state.awaiting = Some("captcha".to_string());
        }
        if state.captcha_polling {
            return;
        }
        state.captcha_polling = true;
        state.captcha_path.clone()
    };
    inner.emit_state(ProxyState::Awaiting, None);

    let inner_for_poll = inner.clone();
    inner.runtime.spawn(async move {
        match poll_for_stable_captcha(&captcha_path).await {
            Ok(Some(bytes)) => {
                let encoded = encode_captcha(&bytes);
                inner_for_poll.show_window();
                inner_for_poll.emit_event(ProxyEvent::NeedCaptcha {
                    base64: encoded,
                    updated_at_ms: chrono::Utc::now().timestamp_millis(),
                });
            }
            _ => {
                let mut state = inner_for_poll.state.lock().expect("state mutex poisoned");
                state.awaiting = None;
                drop(state);
                inner_for_poll.emit_state(ProxyState::Awaiting, None);
            }
        }
        let mut state = inner_for_poll.state.lock().expect("state mutex poisoned");
        state.captcha_polling = false;
    });
}

fn begin_http_ready_wait(inner: Arc<Inner>, bind: String) {
    let generation = {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        if !state.session_active || state.ready || state.ready_wait_gen == state.retry_generation {
            return;
        }
        state.ready_wait_gen = state.retry_generation;
        state.retry_generation
    };
    let dial = readiness_dial_address(&bind);
    if dial.is_empty() {
        mark_ready(inner, generation);
        return;
    }
    let inner_for_wait = inner.clone();
    let handle = inner.runtime.spawn(async move {
        loop {
            if !should_continue_ready_wait(&inner_for_wait, generation) {
                return;
            }
            if check_tcp_connect(&dial, HTTP_READY_DIAL_TIMEOUT).await {
                mark_ready(inner_for_wait.clone(), generation);
                return;
            }
            tokio::time::sleep(HTTP_READY_POLL_INTERVAL).await;
        }
    });
    let mut state = inner.state.lock().expect("state mutex poisoned");
    state.cancel_readiness();
    state.readiness_handle = Some(handle);
}

fn should_continue_ready_wait(inner: &Arc<Inner>, generation: u64) -> bool {
    let state = inner.state.lock().expect("state mutex poisoned");
    state.session_active
        && !state.ready
        && state.retry_generation == generation
        && state.ready_wait_gen == generation
}

fn mark_ready(inner: Arc<Inner>, generation: u64) {
    let should_open_eip = {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        if generation != state.retry_generation || !state.session_active || state.ready {
            return;
        }
        state.ready = true;
        state.ready_wait_gen = 0;
        state.retry_attempt = 0;
        state.eip_options.eip_auto_open && !state.eip_opened
    };
    inner.emit_state(ProxyState::Connected, Some("已启动".into()));
    if should_open_eip {
        schedule_delayed_eip_open(inner, generation);
    }
}

fn schedule_delayed_eip_open(inner: Arc<Inner>, generation: u64) {
    let delay = clamp_eip_open_delay((inner.config.eip_auto_open_delay)());
    let options = {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        state.eip_opened = true;
        state.eip_options.clone()
    };
    let inner_for_open = inner.clone();
    let handle = inner.runtime.spawn(async move {
        tokio::time::sleep(delay).await;
        let still_valid = {
            let state = inner_for_open.state.lock().expect("state mutex poisoned");
            state.retry_generation == generation && state.session_active && state.ready
        };
        if !still_valid {
            return;
        }
        if let Err(err) = (inner_for_open.config.eip_opener)(&options) {
            let mut state = inner_for_open.state.lock().expect("state mutex poisoned");
            if state.retry_generation == generation && state.session_active && state.ready {
                state.eip_opened = false;
            }
            drop(state);
            inner_for_open.emit_log(format!("[eip] failed to open EIP URL: {err}"));
        }
    });
    let mut state = inner.state.lock().expect("state mutex poisoned");
    state.cancel_delayed_eip();
    state.delayed_eip = Some(handle);
}

fn clamp_eip_open_delay(delay: Duration) -> Duration {
    if delay < MIN_EIP_AUTO_OPEN_DELAY {
        MIN_EIP_AUTO_OPEN_DELAY
    } else if delay > MAX_EIP_AUTO_OPEN_DELAY {
        MAX_EIP_AUTO_OPEN_DELAY
    } else {
        delay
    }
}

async fn handle_process_exit(
    inner: Arc<Inner>,
    exit_status: std::io::Result<std::process::ExitStatus>,
    _generation: u64,
) {
    // After exit, decide whether to retry, surface a blocked-awaiting message, or
    // simply transition to stopped.
    let action = {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        if !state.session_active {
            state.cancel_delayed_eip();
            state.eip_opened = false;
            state.ready = false;
            state.ready_wait_gen = 0;
            state.retry_attempt = 0;
            ExitAction::EmitStopped
        } else if let Some(reason) = state.awaiting.clone() {
            state.session_active = false;
            state.cancel_delayed_eip();
            state.eip_opened = false;
            state.ready = false;
            state.ready_wait_gen = 0;
            state.retry_attempt = 0;
            state.awaiting = None;
            state.captcha_polling = false;
            ExitAction::AwaitingBlocked(reason)
        } else {
            state.cancel_delayed_eip();
            state.eip_opened = false;
            state.ready = false;
            state.retry_generation = state.retry_generation.wrapping_add(1);
            ExitAction::ScheduleRetry
        }
    };

    match action {
        ExitAction::EmitStopped => {
            inner.emit_state(ProxyState::Stopped, None);
        }
        ExitAction::AwaitingBlocked(reason) => {
            inner.emit_log(format!(
                "[reconnect] process exited while awaiting {reason}; automatic reconnect paused until manual restart",
            ));
            inner.emit_state(
                ProxyState::Stopped,
                Some(format!("连接在等待 {reason} 时中断，请手动重新连接")),
            );
        }
        ExitAction::ScheduleRetry => {
            let (attempt, delay) = schedule_retry(inner.clone());
            let exit_descr = match exit_status {
                Ok(status) => format!("{status}"),
                Err(err) => format!("{err}"),
            };
            inner.emit_log(format!(
                "[reconnect] process exited ({exit_descr}), retrying in {} (attempt {attempt})",
                format_retry_delay(delay)
            ));
            inner.emit_state_with_details(
                ProxyState::Retrying,
                Some(format!(
                    "连接已断开，将在 {} 后重试（第 {attempt} 次）",
                    format_retry_delay(delay)
                )),
                attempt,
                delay,
            );
        }
    }
}

enum ExitAction {
    EmitStopped,
    AwaitingBlocked(String),
    ScheduleRetry,
}

fn schedule_retry(inner: Arc<Inner>) -> (u32, Duration) {
    let (attempt, delay, generation) = {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        state.retry_attempt = state.retry_attempt.saturating_add(1);
        let attempt = state.retry_attempt;
        let delay = next_retry_delay(
            attempt,
            inner.config.retry_base_delay,
            inner.config.retry_max_delay,
            inner.config.retry_jitter,
        );
        (attempt, delay, state.retry_generation)
    };
    let inner_for_timer = inner.clone();
    let handle = inner.runtime.spawn(async move {
        tokio::time::sleep(delay).await;
        run_retry_attempt(inner_for_timer, generation).await;
    });
    let mut state = inner.state.lock().expect("state mutex poisoned");
    state.cancel_retry();
    state.retry_handle = Some(handle);
    (attempt, delay)
}

async fn run_retry_attempt(inner: Arc<Inner>, generation: u64) {
    let options = {
        let state = inner.state.lock().expect("state mutex poisoned");
        if generation != state.retry_generation
            || !state.session_active
            || state.awaiting.is_some()
            || state.child_pid.is_some()
        {
            return;
        }
        state.last_options.clone()
    };

    inner.emit_state(ProxyState::Connecting, Some("正在重新连接...".into()));

    // Spawn child afresh; we re-enter the same path as Start::spawn_child but without
    // touching `session_active` (already true).
    let manager = ProxyManager {
        inner: inner.clone(),
    };
    let captcha_path = inner.app_dir.join(CAPTCHA_FILE_NAME);
    {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        state.captcha_path = captcha_path.clone();
        state.eip_options = options.clone();
        state.eip_opened = false;
    }
    if let Err(err) = manager.spawn_child(options, captcha_path) {
        let (attempt, delay) = schedule_retry(inner.clone());
        inner.emit_log(format!(
            "[reconnect] retry start failed: {err}; retrying in {} (attempt {attempt})",
            format_retry_delay(delay)
        ));
        inner.emit_state_with_details(
            ProxyState::Retrying,
            Some(format!(
                "重新连接失败，将在 {} 后重试（第 {attempt} 次）",
                format_retry_delay(delay)
            )),
            attempt,
            delay,
        );
    }
}
