//! Integration tests for ProxyManager. Exercises the spawn/log/stop pipeline against
//! a real child process — a small shell script that prints fake zju-connect output and
//! waits on stdin. Run with `cargo test --test proxy_manager`.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tempfile::TempDir;
use tokio::runtime::Runtime;

use zju_connect_gui::backend::launch_options::LaunchOptions;
use zju_connect_gui::backend::proxy::{
    ProxyEvent, ProxyManager, ProxyManagerConfig, ProxyState, StartError, SubmitInputError,
    UiBridge,
};

#[derive(Clone, Default)]
struct CapturingBridge {
    events: Arc<Mutex<Vec<ProxyEvent>>>,
}

impl CapturingBridge {
    fn snapshot(&self) -> Vec<ProxyEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl UiBridge for CapturingBridge {
    fn emit_event(&self, event: ProxyEvent) {
        self.events.lock().unwrap().push(event);
    }
    fn show_window(&self) {}
}

fn write_mock_binary(dir: &std::path::Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = dir.join("mock_zju_connect.sh");
    std::fs::write(&bin, body).unwrap();
    let mut perms = std::fs::metadata(&bin).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&bin, perms).unwrap();
    bin
}

fn base_options() -> LaunchOptions {
    LaunchOptions {
        username: "alice".into(),
        password: "secret".into(),
        tun_mode: false,
        eip_auto_open: false,
        ..LaunchOptions::default()
    }
}

#[test]
fn start_returns_binary_missing_when_path_does_not_exist() {
    let rt = Runtime::new().unwrap();
    let _guard = rt.enter();
    let tmp = TempDir::new().unwrap();
    let app_dir = tmp.path().to_path_buf();
    let cfg = ProxyManagerConfig {
        binary_path: Some(app_dir.join("nonexistent")),
        ..ProxyManagerConfig::default()
    };
    let manager = ProxyManager::with_config(app_dir, rt.handle().clone(), cfg);

    let err = manager.start(base_options()).unwrap_err();
    assert!(matches!(err, StartError::BinaryMissing(_)), "got {err:?}");
    assert!(!manager.is_running());
}

#[test]
fn submit_input_returns_not_running_when_idle() {
    let rt = Runtime::new().unwrap();
    let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
    let err = manager.submit_input("123456").unwrap_err();
    assert!(matches!(err, SubmitInputError::NotRunning), "got {err:?}");
}

#[test]
fn submit_input_rejects_empty_value() {
    let rt = Runtime::new().unwrap();
    let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
    let err = manager.submit_input("   ").unwrap_err();
    assert!(matches!(err, SubmitInputError::Empty), "got {err:?}");
}

#[test]
fn stop_when_idle_emits_stopped_state() {
    let rt = Runtime::new().unwrap();
    let bridge = Arc::new(CapturingBridge::default());
    let manager = ProxyManager::new(PathBuf::from("/tmp"), rt.handle().clone());
    manager.set_ui(bridge.clone());

    manager.stop().unwrap();
    let events = bridge.snapshot();
    assert!(matches!(
        events.last(),
        Some(ProxyEvent::State {
            state: ProxyState::Stopped,
            ..
        })
    ));
}

#[test]
fn lifecycle_with_mock_binary_streams_logs_and_stops_cleanly() {
    let rt = Runtime::new().unwrap();
    let _guard = rt.enter();
    let tmp = TempDir::new().unwrap();
    let app_dir = tmp.path().to_path_buf();

    // Mock binary: prints VPN-started log, then idles reading stdin until killed.
    let body =
        "#!/bin/sh\necho 'VPN client started'\nwhile read -r line; do echo got=\"$line\"; done\n";
    let bin = write_mock_binary(&app_dir, body);

    let bridge = Arc::new(CapturingBridge::default());
    let cfg = ProxyManagerConfig {
        binary_path: Some(bin),
        ..ProxyManagerConfig::default()
    };
    let manager = ProxyManager::with_config(app_dir, rt.handle().clone(), cfg);
    manager.set_ui(bridge.clone());

    manager.start(base_options()).unwrap();

    // Give the child a moment to print and the log task to forward it.
    rt.block_on(async {
        for _ in 0..50 {
            let logs: Vec<String> = bridge
                .snapshot()
                .into_iter()
                .filter_map(|e| match e {
                    ProxyEvent::Log(line) => Some(line),
                    _ => None,
                })
                .collect();
            if logs.iter().any(|l| l.contains("VPN client started")) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "did not observe 'VPN client started' log within 2.5s; events={:?}",
            bridge.snapshot()
        );
    });

    assert!(manager.is_running());
    let snap = manager.snapshot();
    assert!(snap.session_active);
    assert!(snap.child_pid.is_some());

    manager.stop().unwrap();

    // The supervisor task drives the SIGINT + grace-period dance asynchronously.
    // Wait for the Stopped state event (is_running flips to false synchronously, so
    // we have to look at the event stream to know the supervisor finished cleanup).
    rt.block_on(async {
        for _ in 0..100 {
            let saw_stopped = bridge.snapshot().iter().any(|e| {
                matches!(
                    e,
                    ProxyEvent::State {
                        state: ProxyState::Stopped,
                        ..
                    }
                )
            });
            if saw_stopped {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "supervisor did not emit Stopped within 5s; events={:?}",
            bridge.snapshot()
        );
    });

    assert!(!manager.is_running());
}
