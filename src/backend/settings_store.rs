use crate::backend::launch_options::{
    default_proxybridge_processes, normalize_launch_options, LaunchOptions, DEFAULT_AUTH_TYPE,
    DEFAULT_CLIENT_DATA_FILE, DEFAULT_EIP_AUTO_OPEN, DEFAULT_LOGIN_DOMAIN, DEFAULT_PORT,
    DEFAULT_PROTOCOL, DEFAULT_SECONDARY_DNS_SERVER, DEFAULT_SERVER,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

const SETTINGS_FILE_NAME: &str = "gui_settings.json";

#[derive(Debug, Error)]
pub enum SettingsError {
    #[error("failed to read settings: {0}")]
    Read(io::Error),
    #[error("failed to parse settings: {0}")]
    Parse(serde_json::Error),
    #[error("failed to write settings: {0}")]
    Write(io::Error),
    #[error("failed to marshal settings: {0}")]
    Marshal(serde_json::Error),
}

#[derive(Debug)]
pub struct UserSettingsStore {
    path: PathBuf,
    lock: Mutex<()>,
}

pub fn default_launch_options() -> LaunchOptions {
    let mut defaults = normalize_launch_options(LaunchOptions::default());
    defaults.protocol = DEFAULT_PROTOCOL.into();
    defaults.server = DEFAULT_SERVER.into();
    defaults.port = DEFAULT_PORT;
    defaults.secondary_dns_server = DEFAULT_SECONDARY_DNS_SERVER.into();
    defaults.auth_type = DEFAULT_AUTH_TYPE.into();
    defaults.login_domain = DEFAULT_LOGIN_DOMAIN.into();
    defaults.client_data_file = DEFAULT_CLIENT_DATA_FILE.into();
    defaults.eip_auto_open = DEFAULT_EIP_AUTO_OPEN;
    defaults.tun_mode = true;
    defaults.proxybridge_processes = default_proxybridge_processes();
    defaults
}

impl UserSettingsStore {
    pub fn new(app_dir: impl AsRef<Path>) -> Self {
        Self {
            path: app_dir.as_ref().join(SETTINGS_FILE_NAME),
            lock: Mutex::new(()),
        }
    }

    pub fn load(&self) -> Result<LaunchOptions, SettingsError> {
        let _guard = self.lock.lock().expect("settings mutex poisoned");
        let data = match fs::read(&self.path) {
            Ok(d) => d,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(default_launch_options())
            }
            Err(err) => return Err(SettingsError::Read(err)),
        };

        let raw: serde_json::Value = serde_json::from_slice(&data).map_err(SettingsError::Parse)?;
        let has_eip_auto_open = raw
            .as_object()
            .map(|map| map.contains_key("eipAutoOpen"))
            .unwrap_or(false);
        let has_proxybridge_processes = raw
            .as_object()
            .map(|map| map.contains_key("proxybridgeProcesses"))
            .unwrap_or(false);

        let mut options: LaunchOptions =
            serde_json::from_value(raw).map_err(SettingsError::Parse)?;

        options = apply_fixed_defaults(normalize_launch_options(options));
        if !has_eip_auto_open {
            options.eip_auto_open = DEFAULT_EIP_AUTO_OPEN;
        }
        if !has_proxybridge_processes {
            options.proxybridge_processes = default_proxybridge_processes();
        }
        Ok(options)
    }

    pub fn save(&self, options: LaunchOptions) -> Result<(), SettingsError> {
        let _guard = self.lock.lock().expect("settings mutex poisoned");
        let prepared = apply_fixed_defaults(normalize_launch_options(options));
        let payload = serde_json::to_vec_pretty(&prepared).map_err(SettingsError::Marshal)?;
        write_private(&self.path, &payload).map_err(SettingsError::Write)
    }
}

fn apply_fixed_defaults(mut options: LaunchOptions) -> LaunchOptions {
    options.protocol = DEFAULT_PROTOCOL.into();
    options.server = DEFAULT_SERVER.into();
    options.port = DEFAULT_PORT;
    options.secondary_dns_server = DEFAULT_SECONDARY_DNS_SERVER.into();
    options.auth_type = DEFAULT_AUTH_TYPE.into();
    options.login_domain = DEFAULT_LOGIN_DOMAIN.into();
    options.client_data_file = DEFAULT_CLIENT_DATA_FILE.into();
    options
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(data)
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    fs::write(path, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn default_launch_options_fixed_defaults() {
        let defaults = default_launch_options();
        assert_eq!(defaults.protocol, DEFAULT_PROTOCOL);
        assert_eq!(defaults.server, DEFAULT_SERVER);
        assert_eq!(defaults.port, DEFAULT_PORT);
        assert_eq!(defaults.secondary_dns_server, DEFAULT_SECONDARY_DNS_SERVER);
        assert_eq!(defaults.auth_type, DEFAULT_AUTH_TYPE);
        assert_eq!(defaults.login_domain, DEFAULT_LOGIN_DOMAIN);
        assert_eq!(defaults.client_data_file, DEFAULT_CLIENT_DATA_FILE);
        assert!(defaults.eip_auto_open);
        assert!(defaults.tun_mode);
        assert_eq!(
            defaults.proxybridge_processes,
            default_proxybridge_processes()
        );
    }

    #[test]
    fn user_settings_store_load_missing_proxybridge_processes_seeds_default() {
        let tmp = tempdir().unwrap();
        let store = UserSettingsStore::new(tmp.path());
        let payload = json!({
            "username": "alice",
            "password": "p",
        });
        fs::write(tmp.path().join(SETTINGS_FILE_NAME), payload.to_string()).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(
            loaded.proxybridge_processes,
            default_proxybridge_processes()
        );
    }

    #[test]
    fn user_settings_store_load_preserves_explicitly_empty_proxybridge_processes() {
        let tmp = tempdir().unwrap();
        let store = UserSettingsStore::new(tmp.path());
        let payload = json!({
            "username": "alice",
            "password": "p",
            "proxybridgeProcesses": [],
        });
        fs::write(tmp.path().join(SETTINGS_FILE_NAME), payload.to_string()).unwrap();

        let loaded = store.load().unwrap();
        assert!(loaded.proxybridge_processes.is_empty());
    }

    #[test]
    fn user_settings_store_load_forces_fixed_port() {
        let tmp = tempdir().unwrap();
        let store = UserSettingsStore::new(tmp.path());
        let payload = json!({
            "protocol": "easyconnect",
            "server": "example.com",
            "port": 12345,
            "username": "alice",
            "password": "p",
            "socksBind": "127.0.0.1:1081",
            "httpBind": "127.0.0.1:8889",
            "secondaryDnsServer": "8.8.8.8",
            "authType": "auth/foo",
            "loginDomain": "AD2",
            "clientDataFile": "other.json",
            "tunMode": false,
        });
        fs::write(tmp.path().join(SETTINGS_FILE_NAME), payload.to_string()).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.protocol, DEFAULT_PROTOCOL);
        assert_eq!(loaded.server, DEFAULT_SERVER);
        assert_eq!(loaded.port, DEFAULT_PORT);
        assert_eq!(loaded.secondary_dns_server, DEFAULT_SECONDARY_DNS_SERVER);
        assert_eq!(loaded.auth_type, DEFAULT_AUTH_TYPE);
        assert_eq!(loaded.login_domain, DEFAULT_LOGIN_DOMAIN);
        assert_eq!(loaded.client_data_file, DEFAULT_CLIENT_DATA_FILE);
        assert_eq!(loaded.username, "alice");
        assert_eq!(loaded.socks_bind, "127.0.0.1:1081");
        assert_eq!(loaded.http_bind, "127.0.0.1:8889");
        assert!(!loaded.tun_mode);
    }

    #[test]
    fn user_settings_store_load_preserves_eip_browser_settings() {
        let tmp = tempdir().unwrap();
        let store = UserSettingsStore::new(tmp.path());
        let payload = json!({
            "username": "alice",
            "password": "p",
            "eipBrowserProgram": "/usr/bin/chromium",
            "eipBrowserArgs": ["--new-window", "--kiosk"],
            "eipAutoOpen": false,
        });
        fs::write(tmp.path().join(SETTINGS_FILE_NAME), payload.to_string()).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.eip_browser_program, "/usr/bin/chromium");
        assert_eq!(loaded.eip_browser_args, vec!["--new-window", "--kiosk"]);
        assert!(!loaded.eip_auto_open);
    }

    #[test]
    fn user_settings_store_load_missing_eip_auto_open_defaults_enabled() {
        let tmp = tempdir().unwrap();
        let store = UserSettingsStore::new(tmp.path());
        let payload = json!({
            "username": "alice",
            "password": "p",
        });
        fs::write(tmp.path().join(SETTINGS_FILE_NAME), payload.to_string()).unwrap();

        let loaded = store.load().unwrap();
        assert!(loaded.eip_auto_open);
    }

    #[test]
    fn user_settings_store_save_roundtrip_reapplies_fixed_defaults() {
        let tmp = tempdir().unwrap();
        let store = UserSettingsStore::new(tmp.path());

        let mut wild = LaunchOptions {
            username: "alice".into(),
            password: "p".into(),
            ..LaunchOptions::default()
        };
        wild.protocol = "ipsec".into();
        wild.server = "evil.example.com".into();
        wild.port = 31337;

        store.save(wild).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.protocol, DEFAULT_PROTOCOL);
        assert_eq!(loaded.server, DEFAULT_SERVER);
        assert_eq!(loaded.port, DEFAULT_PORT);
        assert_eq!(loaded.username, "alice");
    }
}
