use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const DEFAULT_PROTOCOL: &str = "atrust";
pub const DEFAULT_SERVER: &str = "sslvpn.scmcc.com.cn";
pub const DEFAULT_PORT: u16 = 443;
pub const DEFAULT_SOCKS_BIND: &str = "127.0.0.1:1080";
pub const DEFAULT_HTTP_BIND: &str = "127.0.0.1:8888";
pub const DEFAULT_SECONDARY_DNS_SERVER: &str = "223.5.5.5";
pub const DEFAULT_AUTH_TYPE: &str = "auth/psw";
pub const DEFAULT_LOGIN_DOMAIN: &str = "AD";
pub const DEFAULT_CLIENT_DATA_FILE: &str = "client_data.json";
pub const DEFAULT_EIP_AUTO_OPEN: bool = true;

pub const SUPPORTED_PROTOCOLS: &[&str] = &["atrust", "easyconnect"];

/// Initial ProxyBridge process-list default: one RDP client per desktop OS.
/// Windows uses the built-in Remote Desktop client; Linux uses the FreeRDP
/// client (`xfreerdp`). Other platforms have no bundled default.
pub fn default_proxybridge_processes() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        vec!["mstsc.exe".into()]
    }
    #[cfg(target_os = "linux")]
    {
        vec!["xfreerdp".into()]
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        Vec::new()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchOptions {
    pub protocol: String,
    pub server: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub socks_bind: String,
    pub http_bind: String,
    pub secondary_dns_server: String,
    pub auth_type: String,
    pub login_domain: String,
    pub client_data_file: String,
    pub eip_auto_open: bool,
    pub eip_browser_program: String,
    pub eip_browser_args: Vec<String>,
    pub tun_mode: bool,
    pub debug_dump: bool,
    #[serde(default)]
    pub proxybridge_enabled: bool,
    #[serde(default)]
    pub proxybridge_processes: Vec<String>,
    #[serde(default)]
    pub proxybridge_path: Option<String>,
}

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum ValidationError {
    #[error("unsupported protocol: {0}")]
    UnsupportedProtocol(String),
    #[error("server cannot be empty")]
    EmptyServer,
    #[error("port must be between 1 and 65535")]
    InvalidPort,
    #[error("username cannot be empty")]
    EmptyUsername,
    #[error("password cannot be empty")]
    EmptyPassword,
    #[error("socks-bind cannot be empty")]
    EmptySocksBind,
    #[error("http-bind cannot be empty")]
    EmptyHttpBind,
    #[error("secondary-dns-server cannot be empty")]
    EmptySecondaryDnsServer,
    #[error("auth-type cannot be empty")]
    EmptyAuthType,
    #[error("login-domain cannot be empty")]
    EmptyLoginDomain,
    #[error("client-data-file cannot be empty")]
    EmptyClientDataFile,
}

pub fn normalize_launch_options(mut options: LaunchOptions) -> LaunchOptions {
    options.protocol = options.protocol.trim().to_lowercase();
    options.server = options.server.trim().to_string();
    options.username = options.username.trim().to_string();
    options.socks_bind = options.socks_bind.trim().to_string();
    options.http_bind = options.http_bind.trim().to_string();
    options.secondary_dns_server = options.secondary_dns_server.trim().to_string();
    options.auth_type = options.auth_type.trim().to_string();
    options.login_domain = options.login_domain.trim().to_string();
    options.client_data_file = options.client_data_file.trim().to_string();
    options.eip_browser_program = options.eip_browser_program.trim().to_string();
    options.eip_browser_args = normalize_string_list(options.eip_browser_args);
    options.proxybridge_processes = normalize_string_list(options.proxybridge_processes);
    options.proxybridge_path = options
        .proxybridge_path
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());

    if options.protocol.is_empty() {
        options.protocol = DEFAULT_PROTOCOL.to_string();
    }
    if options.server.is_empty() {
        options.server = DEFAULT_SERVER.to_string();
    }
    if options.port == 0 {
        options.port = DEFAULT_PORT;
    }
    if options.socks_bind.is_empty() {
        options.socks_bind = DEFAULT_SOCKS_BIND.to_string();
    }
    if options.http_bind.is_empty() {
        options.http_bind = DEFAULT_HTTP_BIND.to_string();
    }
    if options.secondary_dns_server.is_empty() {
        options.secondary_dns_server = DEFAULT_SECONDARY_DNS_SERVER.to_string();
    }
    if options.auth_type.is_empty() {
        options.auth_type = DEFAULT_AUTH_TYPE.to_string();
    }
    if options.login_domain.is_empty() {
        options.login_domain = DEFAULT_LOGIN_DOMAIN.to_string();
    }
    if options.client_data_file.is_empty() {
        options.client_data_file = DEFAULT_CLIENT_DATA_FILE.to_string();
    }
    options
}

fn normalize_string_list(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(values.len());
    for v in values {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
    }
    out
}

impl LaunchOptions {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !SUPPORTED_PROTOCOLS.contains(&self.protocol.as_str()) {
            return Err(ValidationError::UnsupportedProtocol(self.protocol.clone()));
        }
        if self.server.is_empty() {
            return Err(ValidationError::EmptyServer);
        }
        if self.port == 0 {
            return Err(ValidationError::InvalidPort);
        }
        if self.username.is_empty() {
            return Err(ValidationError::EmptyUsername);
        }
        if self.password.is_empty() {
            return Err(ValidationError::EmptyPassword);
        }
        if self.socks_bind.is_empty() {
            return Err(ValidationError::EmptySocksBind);
        }
        if self.http_bind.is_empty() {
            return Err(ValidationError::EmptyHttpBind);
        }
        if self.secondary_dns_server.is_empty() {
            return Err(ValidationError::EmptySecondaryDnsServer);
        }
        if self.auth_type.is_empty() {
            return Err(ValidationError::EmptyAuthType);
        }
        if self.login_domain.is_empty() {
            return Err(ValidationError::EmptyLoginDomain);
        }
        if self.client_data_file.is_empty() {
            return Err(ValidationError::EmptyClientDataFile);
        }
        Ok(())
    }

    pub fn build_args(&self, captcha_path: &str) -> Vec<String> {
        let mut args = vec![
            "-protocol".into(),
            self.protocol.clone(),
            "-server".into(),
            self.server.clone(),
            "-port".into(),
            self.port.to_string(),
            "-username".into(),
            self.username.clone(),
            "-password".into(),
            self.password.clone(),
            "-disable-zju-config".into(),
            "-socks-bind".into(),
            self.socks_bind.clone(),
            "-http-bind".into(),
            self.http_bind.clone(),
            "-secondary-dns-server".into(),
            self.secondary_dns_server.clone(),
            "-auth-type".into(),
            self.auth_type.clone(),
            "-login-domain".into(),
            self.login_domain.clone(),
            "-client-data-file".into(),
            self.client_data_file.clone(),
        ];
        if !captcha_path.is_empty() {
            args.push("-graph-code-file".into());
            args.push(captcha_path.to_string());
        }
        if self.tun_mode {
            args.extend([
                "-tun-mode".into(),
                "-add-route".into(),
                "-dns-hijack".into(),
                "-fake-ip".into(),
            ]);
        }
        if self.debug_dump {
            args.push("-debug-dump".into());
        }
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn populated_options() -> LaunchOptions {
        LaunchOptions {
            username: "alice".into(),
            password: "secret".into(),
            ..normalize_launch_options(LaunchOptions::default())
        }
    }

    #[test]
    fn normalize_launch_options_defaults() {
        let normalized = normalize_launch_options(LaunchOptions::default());
        assert_eq!(normalized.protocol, DEFAULT_PROTOCOL);
        assert_eq!(normalized.server, DEFAULT_SERVER);
        assert_eq!(normalized.port, DEFAULT_PORT);
        assert_eq!(normalized.socks_bind, DEFAULT_SOCKS_BIND);
        assert_eq!(normalized.http_bind, DEFAULT_HTTP_BIND);
        assert_eq!(
            normalized.secondary_dns_server,
            DEFAULT_SECONDARY_DNS_SERVER
        );
        assert_eq!(normalized.auth_type, DEFAULT_AUTH_TYPE);
        assert_eq!(normalized.login_domain, DEFAULT_LOGIN_DOMAIN);
        assert_eq!(normalized.client_data_file, DEFAULT_CLIENT_DATA_FILE);
        assert!(!normalized.tun_mode);
        assert!(!normalized.debug_dump);
        assert!(!normalized.eip_auto_open);
        assert_eq!(normalized.eip_browser_program, "");
        assert!(normalized.eip_browser_args.is_empty());
    }

    #[test]
    fn normalize_launch_options_eip_browser_fields() {
        let raw = LaunchOptions {
            eip_browser_program: "  /usr/bin/firefox  ".into(),
            eip_browser_args: vec![
                "--new-window".into(),
                "  ".into(),
                " --kiosk ".into(),
                String::new(),
            ],
            ..LaunchOptions::default()
        };
        let normalized = normalize_launch_options(raw);
        assert_eq!(normalized.eip_browser_program, "/usr/bin/firefox");
        assert_eq!(normalized.eip_browser_args, vec!["--new-window", "--kiosk"]);
    }

    #[test]
    fn launch_options_validate() {
        let mut opts = populated_options();
        assert!(opts.validate().is_ok());

        opts.protocol = "ipsec".into();
        assert_eq!(
            opts.validate(),
            Err(ValidationError::UnsupportedProtocol("ipsec".into()))
        );

        let mut opts = populated_options();
        opts.username.clear();
        assert_eq!(opts.validate(), Err(ValidationError::EmptyUsername));

        let mut opts = populated_options();
        opts.password.clear();
        assert_eq!(opts.validate(), Err(ValidationError::EmptyPassword));

        let mut opts = populated_options();
        opts.port = 0;
        assert_eq!(opts.validate(), Err(ValidationError::InvalidPort));

        let mut opts = populated_options();
        opts.socks_bind.clear();
        assert_eq!(opts.validate(), Err(ValidationError::EmptySocksBind));

        let mut opts = populated_options();
        opts.http_bind.clear();
        assert_eq!(opts.validate(), Err(ValidationError::EmptyHttpBind));
    }

    #[test]
    fn build_args() {
        let opts = LaunchOptions {
            username: "alice".into(),
            password: "p@ss".into(),
            tun_mode: true,
            debug_dump: true,
            ..normalize_launch_options(LaunchOptions::default())
        };
        let args = opts.build_args("/tmp/captcha.png");

        let head = vec![
            "-protocol",
            "atrust",
            "-server",
            "sslvpn.scmcc.com.cn",
            "-port",
            "443",
            "-username",
            "alice",
            "-password",
            "p@ss",
            "-disable-zju-config",
            "-socks-bind",
            "127.0.0.1:1080",
            "-http-bind",
            "127.0.0.1:8888",
            "-secondary-dns-server",
            "223.5.5.5",
            "-auth-type",
            "auth/psw",
            "-login-domain",
            "AD",
            "-client-data-file",
            "client_data.json",
            "-graph-code-file",
            "/tmp/captcha.png",
            "-tun-mode",
            "-add-route",
            "-dns-hijack",
            "-fake-ip",
            "-debug-dump",
        ];
        assert_eq!(args, head);
    }

    #[test]
    fn build_args_without_tun_debug() {
        let opts = LaunchOptions {
            username: "alice".into(),
            password: "p@ss".into(),
            ..normalize_launch_options(LaunchOptions::default())
        };
        let args = opts.build_args("");
        assert!(!args.iter().any(|a| a == "-tun-mode"));
        assert!(!args.iter().any(|a| a == "-debug-dump"));
        assert!(!args.iter().any(|a| a == "-graph-code-file"));
        assert_eq!(args.first().map(String::as_str), Some("-protocol"));
    }
}
