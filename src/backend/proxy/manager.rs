use crate::backend::external_links::{open_eip, OpenEipError};
use crate::backend::launch_options::LaunchOptions;
use crate::backend::proxy::captcha::{
    encode_captcha, monitor_captcha_file, poll_for_stable_captcha,
};
use crate::backend::proxy::logs::{
    classify_prompt, consume_stream, is_route_added, is_vpn_started, DetectedPrompt,
};
use crate::backend::proxy::proxybridge::{self, is_active as pb_is_active, ProxyBridge};
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
    open_eip(options)
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
    #[error("session is no longer active")]
    SessionStopped,
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
    #[error("waiting for {awaiting}, not {requested}")]
    KindMismatch { awaiting: String, requested: String },
    #[error("input kind is required (sms/callback/captcha)")]
    MissingKind,
    #[error("no input is currently requested")]
    NotAwaiting,
    #[error("input contains newline or control characters")]
    InvalidValue,
    #[error("input is too long (max 4096 bytes)")]
    TooLong,
    #[error("io error: {0}")]
    Io(std::io::Error),
}

struct State {
    session_active: bool,
    awaiting: Option<String>,
    captcha_polling: bool,
    /// Session generation that owns the in-flight captcha poll task; the task
    /// only clears `captcha_polling` if it still owns the flag.
    captcha_poll_gen: u64,
    captcha_poll_handle: Option<JoinHandle<()>>,
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
    proxybridge: Option<ProxyBridge>,
    proxybridge_started: bool,
    proxybridge_handle: Option<JoinHandle<()>>,
}

impl State {
    fn new() -> Self {
        Self {
            session_active: false,
            awaiting: None,
            captcha_polling: false,
            captcha_poll_gen: 0,
            captcha_poll_handle: None,
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
            proxybridge: None,
            proxybridge_started: false,
            proxybridge_handle: None,
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

    fn cancel_captcha_poll(&mut self) {
        if let Some(handle) = self.captcha_poll_handle.take() {
            handle.abort();
        }
        self.captcha_polling = false;
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

        // TUN mode or ProxyBridge integration require administrator privileges.
        #[cfg(target_os = "windows")]
        if (options.tun_mode || pb_is_active(&options))
            && !crate::backend::platform::is_process_elevated()
        {
            return Err(StartError::NeedsElevation);
        }

        // On Linux, check for root if ProxyBridge is active.
        #[cfg(target_os = "linux")]
        if (options.tun_mode || pb_is_active(&options))
            && !crate::backend::platform::is_process_elevated()
        {
            return Err(StartError::NeedsElevation);
        }

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

        let generation = {
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
            state.cancel_captcha_poll();
            state.retry_generation
        };

        let _ = std::fs::remove_file(&captcha_path);

        match self.spawn_child(options.clone(), captcha_path.clone(), generation) {
            Ok(()) => Ok(()),
            Err(err) => {
                self.cleanup_failed_start(generation);
                Err(err)
            }
        }
    }

    fn cleanup_failed_start(&self, generation: u64) {
        let (pb, pb_handle, pb_started) = {
            let mut state = self.inner.state.lock().expect("state mutex poisoned");
            // Only tear the session down if it still belongs to this start
            // attempt: a concurrent start() (or a retry) may have taken over
            // in the meantime and must not be clobbered.
            if cleanup_failed_start_should_skip(&state, generation) {
                return;
            }
            state.session_active = false;
            state.awaiting = None;
            state.cancel_captcha_poll();
            state.child_pid = None;
            state.stdin_tx = None;
            state.stop_tx = None;
            (
                state.proxybridge.take(),
                state.proxybridge_handle.take(),
                std::mem::take(&mut state.proxybridge_started),
            )
        };
        stop_proxybridge(pb, pb_handle, pb_started);
        self.inner.emit_state(ProxyState::Stopped, None);
    }

    fn spawn_child(
        &self,
        options: LaunchOptions,
        captcha_path: PathBuf,
        generation: u64,
    ) -> Result<(), StartError> {
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
            .stderr(Stdio::piped())
            // Safety net so an aborted supervise_child task (e.g. process exit
            // without a graceful manager.stop()) doesn't leak the proxy child.
            // The normal stop path still drives a SIGINT / CTRL_BREAK first
            // and only falls back to start_kill on grace-period expiry.
            .kill_on_drop(true);
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
            // Guard against the stop()/retry/start races: a session that ended
            // (stop), was taken over by a newer generation (a concurrent
            // start() or retry that bumped retry_generation), or already owns a
            // child (concurrent start() that registered first) must not leave
            // this child running — it would hold the SOCKS port outside any
            // supervised session. First-registration wins: the loser kills its
            // child here and reports SessionStopped.
            if spawn_guard_violated(&state, generation) {
                let _ = child.start_kill();
                return Err(StartError::SessionStopped);
            }
            state.child_pid = pid;
            state.stdin_tx = Some(stdin_tx);
            state.stop_tx = Some(stop_tx);
        }

        self.inner.emit_state(ProxyState::Running, None);

        let inner = self.inner.clone();
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
        let (
            stop_tx,
            retry_handle,
            readiness_handle,
            delayed_eip,
            pb,
            pb_handle,
            pb_started,
            child_pid_present,
        ) = {
            let mut state = self.inner.state.lock().expect("state mutex poisoned");
            state.retry_generation = state.retry_generation.wrapping_add(1);
            state.session_active = false;
            state.retry_attempt = 0;
            state.ready = false;
            state.ready_wait_gen = 0;
            state.awaiting = None;
            state.cancel_captcha_poll();
            (
                state.stop_tx.take(),
                state.retry_handle.take(),
                state.readiness_handle.take(),
                state.delayed_eip.take(),
                state.proxybridge.take(),
                state.proxybridge_handle.take(),
                std::mem::take(&mut state.proxybridge_started),
                state.child_pid.is_some(),
            )
        };

        // Stop ProxyBridge first (before zju-connect) so kernel-level
        // interception is lifted while the SOCKS proxy is still alive.
        stop_proxybridge(pb, pb_handle, pb_started);

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

    /// Submit a line to the child's stdin. `kind` ("sms" / "callback" /
    /// "captcha") identifies the prompt the caller is answering; while the
    /// child is waiting for input the kind is *required* and must match, so a
    /// stale SMS code cannot be fed to a captcha prompt. Values containing
    /// newlines or control characters are rejected so a single submission
    /// cannot smuggle extra lines into the child's stdin.
    pub fn submit_input(&self, value: &str, kind: Option<&str>) -> Result<(), SubmitInputError> {
        let value = value.trim();
        if value.is_empty() {
            return Err(SubmitInputError::Empty);
        }
        if value.len() > 4096 {
            return Err(SubmitInputError::TooLong);
        }
        // Reject newlines and (Unicode) line/paragraph separators: a single
        // submission must not smuggle extra lines into the child's stdin.
        if value.contains(['\n', '\r', '\u{2028}', '\u{2029}'])
            || value.chars().any(|c| c.is_control())
        {
            return Err(SubmitInputError::InvalidValue);
        }
        let inner = self.inner.clone();
        {
            let mut state = inner.state.lock().expect("state mutex poisoned");
            match state.awaiting.as_deref() {
                Some(awaiting) => {
                    let kind = kind.ok_or(SubmitInputError::MissingKind)?;
                    if awaiting != kind {
                        return Err(SubmitInputError::KindMismatch {
                            awaiting: awaiting.to_string(),
                            requested: kind.to_string(),
                        });
                    }
                }
                None => {
                    // Nothing is waiting for input; writing anyway would sit in
                    // the child's stdin buffer and poison its next prompt.
                    return Err(SubmitInputError::NotAwaiting);
                }
            }
            let tx = state.stdin_tx.clone().ok_or(SubmitInputError::NotRunning)?;
            tx.send(value.to_string())
                .map_err(|_| SubmitInputError::NotRunning)?;
            // Clear awaiting while still holding the lock so two concurrent
            // submissions cannot both pass the kind check.
            state.awaiting = None;
        }
        inner.emit_state(ProxyState::Running, None);
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

    /// Open the EIP page on demand (the "打开 EIP" button in proxy-only
    /// mode). Uses the exact options of the live session, so in proxy-only
    /// mode the browser is pointed at the SOCKS listener this session owns.
    pub fn open_eip_manual(&self) -> Result<(), OpenEipManualError> {
        let options = {
            let state = self.inner.state.lock().expect("state mutex poisoned");
            if !state.session_active || !state.ready {
                return Err(OpenEipManualError::NotConnected);
            }
            state.eip_options.clone()
        };
        self.inner.emit_log("[eip] manual EIP open requested");
        let result = (self.inner.config.eip_opener)(&options);
        match &result {
            Ok(()) => self.inner.emit_log("[eip] EIP page opened"),
            Err(err) => self
                .inner
                .emit_log(format!("[eip] failed to open EIP URL: {err}")),
        }
        result.map_err(OpenEipManualError::Open)
    }
}

/// Errors surfaced by [`ProxyManager::open_eip_manual`].
#[derive(Debug, Error)]
pub enum OpenEipManualError {
    #[error("尚未连接，无法打开 EIP")]
    NotConnected,
    #[error("{0}")]
    Open(#[from] OpenEipError),
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
    // CREATE_NEW_PROCESS_GROUP (0x00000200): the child becomes the lead of its own
    // process group, which lets us target it with `GenerateConsoleCtrlEvent` for
    // graceful shutdown without affecting our own group. We deliberately do NOT set
    // CREATE_NO_WINDOW — that flag detaches the child from any console handle, which
    // also blocks console-signal delivery. Instead we allocate a hidden console for
    // ourselves at startup (see `platform::init_console_for_signaling`) which the
    // child inherits via the spawn.
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
        // captcha polling cleanup is left to handle_process_exit, which knows
        // whether this exit belongs to the current session or is stale.
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
        state.captcha_poll_gen = state.retry_generation;
        state.captcha_path.clone()
    };
    inner.emit_state(ProxyState::Awaiting, None);

    let inner_for_poll = inner.clone();
    let handle = inner.runtime.spawn(async move {
        match poll_for_stable_captcha(&captcha_path).await {
            Ok(Some(bytes)) => {
                // The session may have ended while we were polling (stop(), or
                // the child exited), or a newer session may have taken over;
                // never surface a captcha for a session we don't own.
                let session_still_active = {
                    let state = inner_for_poll.state.lock().expect("state mutex poisoned");
                    state.session_active && state.captcha_poll_gen == state.retry_generation
                };
                if session_still_active {
                    let encoded = encode_captcha(&bytes);
                    inner_for_poll.show_window();
                    inner_for_poll.emit_event(ProxyEvent::NeedCaptcha {
                        base64: encoded,
                        updated_at_ms: chrono::Utc::now().timestamp_millis(),
                    });
                }
            }
            _ => {
                let mut state = inner_for_poll.state.lock().expect("state mutex poisoned");
                if state.session_active && state.captcha_poll_gen == state.retry_generation {
                    state.awaiting = None;
                    drop(state);
                    inner_for_poll.emit_state(ProxyState::Awaiting, None);
                }
            }
        }
        let mut state = inner_for_poll.state.lock().expect("state mutex poisoned");
        // Only clear the flag if this task still owns it (a newer session may
        // have started its own poll while we were finishing).
        if state.captcha_poll_gen == state.retry_generation {
            state.captcha_polling = false;
        }
    });
    let mut state = inner.state.lock().expect("state mutex poisoned");
    state.captcha_poll_handle = Some(handle);
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
    let (should_open_eip, should_start_pb, pb_options) = {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        if generation != state.retry_generation || !state.session_active || state.ready {
            return;
        }
        state.ready = true;
        state.ready_wait_gen = 0;
        state.retry_attempt = 0;
        (
            state.eip_options.eip_auto_open && !state.eip_opened,
            pb_is_active(&state.eip_options),
            state.eip_options.clone(),
        )
    };
    inner.emit_state(ProxyState::Connected, Some("已启动".into()));
    if should_open_eip {
        schedule_delayed_eip_open(inner.clone(), generation);
    }
    if should_start_pb {
        start_proxybridge(inner, pb_options);
    }
}

/// True when a freshly spawned child must be killed immediately: the session
/// ended (stop), a newer session/start bumped `retry_generation` while we
/// were spawning, or a concurrent start() already registered its child
/// (first-registration wins).
/// True when a failed-start cleanup must leave the state alone: the session
/// no longer belongs to this start attempt (generation moved on), or a
/// concurrent start() already registered its child (first-registration wins;
/// on any spawn_child error path we never registered one ourselves, so a
/// present child_pid belongs to the winner).
fn cleanup_failed_start_should_skip(state: &State, generation: u64) -> bool {
    state.retry_generation != generation || state.child_pid.is_some()
}

fn spawn_guard_violated(state: &State, generation: u64) -> bool {
    !state.session_active || generation != state.retry_generation || state.child_pid.is_some()
}

fn schedule_delayed_eip_open(inner: Arc<Inner>, generation: u64) {
    let delay = clamp_eip_open_delay((inner.config.eip_auto_open_delay)());
    let options = {
        let state = inner.state.lock().expect("state mutex poisoned");
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
            inner_for_open.emit_log(format!("[eip] failed to open EIP URL: {err}"));
            return;
        }
        // Mark as opened only after the browser was actually launched. If the
        // session drops during the delay (or the opener fails), eip_opened
        // stays false and the next successful reconnect retries the open.
        let mut state = inner_for_open.state.lock().expect("state mutex poisoned");
        if state.retry_generation == generation && state.session_active && state.ready {
            state.eip_opened = true;
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
    generation: u64,
) {
    // After exit, decide whether to retry, surface a blocked-awaiting message, or
    // simply transition to stopped.
    let action = {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        decide_exit_action(&mut state, generation)
    };

    match action {
        ExitAction::Ignore => {
            // A stale exit from a previous session; the new session owns the
            // state. Nothing to emit.
        }
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
    /// Exit belongs to a previous session; do nothing.
    Ignore,
    EmitStopped,
    AwaitingBlocked(String),
    ScheduleRetry,
}

/// Decide what to do after the supervised child exited, mutating `state`.
///
/// `generation` is the session generation the exiting child belonged to. A
/// stale exit — the previous child's supervise task finishing its teardown
/// after a new session already started — must not touch the new session's
/// state, so it yields `Ignore`. Without this guard a stale exit would bump
/// `retry_generation` and silently abort the new session's readiness polling
/// (UI stuck on "connecting" despite a working VPN).
fn decide_exit_action(state: &mut State, generation: u64) -> ExitAction {
    if state.session_active && generation != state.retry_generation {
        return ExitAction::Ignore;
    }
    state.cancel_delayed_eip();
    state.cancel_captcha_poll();
    state.ready = false;
    if !state.session_active {
        state.eip_opened = false;
        state.ready_wait_gen = 0;
        state.retry_attempt = 0;
        ExitAction::EmitStopped
    } else if let Some(reason) = state.awaiting.clone() {
        state.session_active = false;
        state.awaiting = None;
        state.eip_opened = false;
        state.ready_wait_gen = 0;
        state.retry_attempt = 0;
        ExitAction::AwaitingBlocked(reason)
    } else {
        // Deliberately do NOT reset eip_opened here: the EIP browser opens
        // once per *manual* session, and an automatic reconnect must not spawn
        // a fresh browser tab. (start() resets it for manual sessions.)
        state.retry_generation = state.retry_generation.wrapping_add(1);
        ExitAction::ScheduleRetry
    }
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

    // Re-validate before announcing: a stop() may have landed between the
    // guard lock and this point, and announcing Connecting after Stopped
    // would leave the UI stuck on "正在重新连接..." while the session is
    // actually stopped.
    let still_valid = {
        let state = inner.state.lock().expect("state mutex poisoned");
        generation == state.retry_generation
            && state.session_active
            && state.awaiting.is_none()
            && state.child_pid.is_none()
    };
    if !still_valid {
        return;
    }

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
        // Note: eip_opened is deliberately NOT reset here. The EIP browser
        // opens once per *manual* session (start()); auto-reconnects must not
        // spawn a fresh browser tab each time.
    }
    if let Err(err) = manager.spawn_child(options, captcha_path, generation) {
        if matches!(err, StartError::SessionStopped) {
            // The session ended (stop) or a newer session/start took over
            // while we were spawning; step aside — the winner owns the state.
            return;
        }
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

// ── ProxyBridge integration ────────────────────────────────────────────

/// Stop ProxyBridge (if started) and abort its log-forwarding task.
/// Takes the resources out of `State` first so it can be called without
/// holding the state lock (the library's `Stop` may take a moment).
fn stop_proxybridge(pb: Option<ProxyBridge>, pb_handle: Option<JoinHandle<()>>, pb_started: bool) {
    if pb_started {
        if let Some(pb) = pb {
            // PB's Stop tears down iptables / WinDivert rules and may block for
            // a moment; we're on the tokio runtime here, so park the current
            // task on a blocking thread instead of tying up a worker.
            tokio::task::block_in_place(|| pb.stop());
        }
    }
    if let Some(handle) = pb_handle {
        handle.abort();
    }
}

/// Start ProxyBridge (in-process library binding). Called once zju-connect
/// has reached the "ready" state.
///
/// Idempotent: on reconnects the bridge keeps running (the SOCKS proxy
/// address does not change between retries), so this is a no-op when the
/// bridge is already started. Changing rules requires a full stop/start
/// cycle, which tears the bridge down in `stop()`.
fn start_proxybridge(inner: Arc<Inner>, options: LaunchOptions) {
    {
        let state = inner.state.lock().expect("state mutex poisoned");
        if state.proxybridge_started {
            return;
        }
    }

    let app_dir = inner.app_dir.clone();
    let pb_path =
        proxybridge::find_proxybridge_library(options.proxybridge_path.as_deref(), &app_dir);

    // Log forwarding channel: the C callback runs on ProxyBridge's internal
    // threads, so it only hands lines to an unbounded sender; a tokio task
    // forwards them into our log stream.
    let (log_tx, mut log_rx) = mpsc::unbounded_channel::<String>();
    let inner_for_logs = inner.clone();
    let log_handle = inner.runtime.spawn(async move {
        while let Some(line) = log_rx.recv().await {
            inner_for_logs.emit_log(format!("[proxybridge] {line}"));
        }
    });

    // dlopen / driver setup / Start can all block for a while; keep them off
    // the async worker threads.
    let pb = match tokio::task::block_in_place(|| {
        proxybridge::ProxyBridge::load(pb_path.as_deref(), log_tx)
    }) {
        Ok(pb) => pb,
        Err(e) => {
            log_handle.abort();
            let hint = proxybridge::install_hint();
            inner.emit_log(format!(
                "[proxybridge] failed to load ProxyBridge library: {e} {hint}"
            ));
            return;
        }
    };

    // Make sure the WinDivert kernel driver is available before starting
    // interception (Windows only; no-op elsewhere).
    #[cfg(target_os = "windows")]
    if let Err(e) = tokio::task::block_in_place(|| {
        crate::backend::proxy::windivert::ensure_windivert_driver(&app_dir)
    }) {
        log_handle.abort();
        let hint = proxybridge::install_hint();
        inner.emit_log(format!(
            "[proxybridge] WinDivert driver unavailable: {e} {hint}"
        ));
        return;
    }

    if let Err(e) = tokio::task::block_in_place(|| pb.start(&options)) {
        log_handle.abort();
        inner.emit_log(format!("[proxybridge] failed to start: {e}"));
        return;
    }

    {
        let mut state = inner.state.lock().expect("state mutex poisoned");
        // A concurrent stop() may have ended the session while we were
        // loading; tear the bridge down again in that case.
        if !state.session_active {
            drop(state);
            tokio::task::block_in_place(|| pb.stop());
            log_handle.abort();
            return;
        }
        state.proxybridge = Some(pb);
        state.proxybridge_started = true;
        state.proxybridge_handle = Some(log_handle);
    }
    inner.emit_log("[proxybridge] started".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with_session(generation: u64) -> State {
        let mut state = State::new();
        state.session_active = true;
        state.retry_generation = generation;
        state
    }

    #[test]
    fn spawn_guard_rejects_inactive_session() {
        let mut state = state_with_session(1);
        state.session_active = false;
        assert!(spawn_guard_violated(&state, 1));
    }

    #[test]
    fn spawn_guard_rejects_stale_generation() {
        // A concurrent start() bumped the generation while we were spawning.
        let state = state_with_session(3);
        assert!(spawn_guard_violated(&state, 2));
    }

    #[test]
    fn spawn_guard_rejects_occupied_child_slot() {
        // Concurrent double start(): the other call registered first. The
        // generation alone cannot distinguish the loser (both calls bumped it
        // before either spawned), so the registered child_pid is the tiebreak.
        let mut state = state_with_session(2);
        state.child_pid = Some(1234);
        assert!(spawn_guard_violated(&state, 2));
    }

    #[test]
    fn spawn_guard_accepts_clean_session() {
        let state = state_with_session(5);
        assert!(!spawn_guard_violated(&state, 5));
    }

    #[test]
    fn cleanup_skips_when_winner_registered() {
        // The losing start() must not tear down the winner's session: even
        // with a matching generation (both calls bumped it before either
        // spawned), a registered child_pid means the session belongs to the
        // concurrent winner.
        let mut state = state_with_session(2);
        state.child_pid = Some(5678);
        assert!(cleanup_failed_start_should_skip(&state, 2));
    }

    #[test]
    fn cleanup_proceeds_when_no_winner() {
        // Normal failed start: nobody else registered, cleanup must run.
        let state = state_with_session(2);
        assert!(!cleanup_failed_start_should_skip(&state, 2));
        let state = state_with_session(3);
        assert!(cleanup_failed_start_should_skip(&state, 2));
    }

    #[test]
    fn exit_action_ignores_stale_generation() {
        // A stale exit from the previous session must not disturb the new one.
        let mut state = state_with_session(2);
        let action = decide_exit_action(&mut state, 1);
        assert!(matches!(action, ExitAction::Ignore));
        assert!(state.session_active);
        assert_eq!(state.retry_generation, 2);
        assert_eq!(state.retry_attempt, 0);
    }

    #[test]
    fn exit_action_schedules_retry_for_current_generation() {
        let mut state = state_with_session(7);
        let action = decide_exit_action(&mut state, 7);
        assert!(matches!(action, ExitAction::ScheduleRetry));
        assert_eq!(state.retry_generation, 8);
        assert!(state.session_active);
    }

    #[test]
    fn exit_action_emits_stopped_when_session_inactive_even_if_stale() {
        // The stop() path must still emit Stopped regardless of generation.
        let mut state = state_with_session(9);
        state.session_active = false;
        let action = decide_exit_action(&mut state, 3);
        assert!(matches!(action, ExitAction::EmitStopped));
    }

    #[test]
    fn exit_action_awaiting_blocked_ends_session() {
        let mut state = state_with_session(1);
        state.awaiting = Some("sms".into());
        let action = decide_exit_action(&mut state, 1);
        assert!(matches!(
            action,
            ExitAction::AwaitingBlocked(reason) if reason == "sms"
        ));
        assert!(!state.session_active);
        assert!(state.awaiting.is_none());
    }

    #[test]
    fn exit_action_schedule_retry_preserves_eip_opened() {
        // Automatic reconnect must NOT reopen the EIP browser tab.
        let mut state = state_with_session(1);
        state.eip_opened = true;
        assert!(matches!(
            decide_exit_action(&mut state, 1),
            ExitAction::ScheduleRetry
        ));
        assert!(state.eip_opened);
    }

    #[test]
    fn exit_action_stopped_resets_eip_opened() {
        let mut state = state_with_session(1);
        state.session_active = false;
        state.eip_opened = true;
        assert!(matches!(
            decide_exit_action(&mut state, 1),
            ExitAction::EmitStopped
        ));
        assert!(!state.eip_opened);
    }

    #[test]
    fn submit_input_rejects_kind_mismatch() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
        {
            let mut state = manager.inner.state.lock().unwrap();
            state.session_active = true;
            state.awaiting = Some("captcha".to_string());
        }
        let err = manager.submit_input("1234", Some("sms")).unwrap_err();
        assert!(
            matches!(err, SubmitInputError::KindMismatch { .. }),
            "got {err:?}"
        );
        assert_eq!(err.to_string(), "waiting for captcha, not sms");
    }

    #[test]
    fn submit_input_requires_kind_when_awaiting() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
        {
            let mut state = manager.inner.state.lock().unwrap();
            state.session_active = true;
            state.awaiting = Some("sms".to_string());
        }
        let err = manager.submit_input("1234", None).unwrap_err();
        assert!(matches!(err, SubmitInputError::MissingKind), "got {err:?}");
    }

    #[test]
    fn submit_input_rejects_when_nothing_awaiting() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
        let err = manager.submit_input("1234", Some("sms")).unwrap_err();
        assert!(matches!(err, SubmitInputError::NotAwaiting), "got {err:?}");
    }

    #[test]
    fn submit_input_rejects_control_characters() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
        {
            let mut state = manager.inner.state.lock().unwrap();
            state.session_active = true;
            state.awaiting = Some("sms".to_string());
            let (tx, _rx) = mpsc::unbounded_channel::<String>();
            state.stdin_tx = Some(tx);
        }
        let err = manager.submit_input("123\n456", Some("sms")).unwrap_err();
        assert!(matches!(err, SubmitInputError::InvalidValue), "got {err:?}");
    }

    #[test]
    fn submit_input_rejects_unicode_line_separators() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
        {
            let mut state = manager.inner.state.lock().unwrap();
            state.session_active = true;
            state.awaiting = Some("sms".to_string());
            let (tx, _rx) = mpsc::unbounded_channel::<String>();
            state.stdin_tx = Some(tx);
        }
        // U+2028 (LINE SEPARATOR) / U+2029 (PARAGRAPH SEPARATOR) are not C0/C1
        // controls, but must not be smuggled into the child's stdin either.
        for value in ["12\u{2028}34", "12\u{2029}34"] {
            let err = manager.submit_input(value, Some("sms")).unwrap_err();
            assert!(matches!(err, SubmitInputError::InvalidValue), "got {err:?}");
        }
    }

    #[test]
    fn submit_input_rejects_oversized_value() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
        {
            let mut state = manager.inner.state.lock().unwrap();
            state.session_active = true;
            state.awaiting = Some("sms".to_string());
            let (tx, _rx) = mpsc::unbounded_channel::<String>();
            state.stdin_tx = Some(tx);
        }
        let big = "x".repeat(5000);
        let err = manager.submit_input(&big, Some("sms")).unwrap_err();
        assert!(matches!(err, SubmitInputError::TooLong), "got {err:?}");
    }

    #[test]
    fn submit_input_matching_kind_proceeds_when_running() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
        {
            let mut state = manager.inner.state.lock().unwrap();
            state.session_active = true;
            state.awaiting = Some("sms".to_string());
        }
        // Kind matches, but there is no child → NotRunning (kind check passed).
        let err = manager.submit_input("1234", Some("sms")).unwrap_err();
        assert!(matches!(err, SubmitInputError::NotRunning), "got {err:?}");
    }

    #[test]
    fn open_eip_manual_requires_connected_session() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
        // Idle: not connected.
        assert!(matches!(
            manager.open_eip_manual().unwrap_err(),
            OpenEipManualError::NotConnected
        ));
        // Running but not ready yet: still rejected.
        {
            let mut state = manager.inner.state.lock().unwrap();
            state.session_active = true;
            state.ready = false;
        }
        assert!(matches!(
            manager.open_eip_manual().unwrap_err(),
            OpenEipManualError::NotConnected
        ));
    }

    #[test]
    fn open_eip_manual_uses_live_session_options_and_reports_errors() {
        use std::sync::Mutex as StdMutex;

        let rt = tokio::runtime::Runtime::new().unwrap();
        let seen: Arc<StdMutex<Vec<LaunchOptions>>> = Arc::new(StdMutex::new(Vec::new()));
        let seen_for_opener = seen.clone();
        let cfg = ProxyManagerConfig {
            eip_opener: Arc::new(move |options: &LaunchOptions| {
                seen_for_opener.lock().unwrap().push(options.clone());
                // Second call simulates a launcher failure.
                if seen_for_opener.lock().unwrap().len() > 1 {
                    Err(OpenEipError::NoBrowserFound)
                } else {
                    Ok(())
                }
            }),
            ..ProxyManagerConfig::default()
        };
        let manager = ProxyManager::with_config(PathBuf::from("/tmp"), rt.handle().clone(), cfg);
        {
            let mut state = manager.inner.state.lock().unwrap();
            state.session_active = true;
            state.ready = true;
            state.eip_options = LaunchOptions {
                tun_mode: false,
                socks_bind: "127.0.0.1:9999".into(),
                ..LaunchOptions::default()
            };
        }

        manager.open_eip_manual().expect("first open succeeds");
        let opened = seen.lock().unwrap();
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].socks_bind, "127.0.0.1:9999");
        drop(opened);

        // Opener failures surface to the caller (and into the log stream).
        match manager.open_eip_manual().unwrap_err() {
            OpenEipManualError::Open(OpenEipError::NoBrowserFound) => {}
            other => panic!("expected opener error, got {other:?}"),
        }
    }

    #[derive(Clone, Default)]
    struct TestBridge(Arc<std::sync::Mutex<Vec<ProxyEvent>>>);

    impl UiBridge for TestBridge {
        fn emit_event(&self, event: ProxyEvent) {
            self.0.lock().unwrap().push(event);
        }
        fn show_window(&self) {}
    }

    #[test]
    fn submit_input_clears_awaiting_and_emits_running() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
        let bridge = Arc::new(TestBridge::default());
        manager.set_ui(bridge.clone());
        let (tx, _rx) = mpsc::unbounded_channel::<String>();
        {
            let mut state = manager.inner.state.lock().unwrap();
            state.session_active = true;
            state.awaiting = Some("sms".to_string());
            state.stdin_tx = Some(tx);
        }
        manager.submit_input("1234", Some("sms")).unwrap();
        {
            let state = manager.inner.state.lock().unwrap();
            assert!(state.awaiting.is_none());
        }
        let events = bridge.0.lock().unwrap();
        assert!(matches!(
            events.last(),
            Some(ProxyEvent::State {
                state: ProxyState::Running,
                ..
            })
        ));
    }
}
