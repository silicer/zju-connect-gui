use std::net::ToSocketAddrs;

pub fn readiness_dial_address(bind: &str) -> String {
    let trimmed = bind.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some((host, port)) = split_host_port(trimmed) {
        let host = match host {
            "" | "0.0.0.0" => "127.0.0.1",
            "::" => "::1",
            other => other,
        };
        if host.contains(':') {
            return format!("[{host}]:{port}");
        }
        return format!("{host}:{port}");
    }
    trimmed.to_string()
}

fn split_host_port(s: &str) -> Option<(&str, &str)> {
    if let Some(rest) = s.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        let after = &rest[end + 1..];
        let port = after.strip_prefix(':')?;
        return Some((host, port));
    }
    let idx = s.rfind(':')?;
    let host = &s[..idx];
    let port = &s[idx + 1..];
    if host.contains(':') {
        return None;
    }
    Some((host, port))
}

pub async fn check_tcp_connect(target: &str, timeout: std::time::Duration) -> bool {
    let resolved: Vec<_> = match target.to_socket_addrs() {
        Ok(addrs) => addrs.collect(),
        Err(_) => return false,
    };
    for addr in resolved {
        let conn = tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr));
        if let Ok(Ok(_)) = conn.await {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_dial_address_localhost_for_wildcard() {
        assert_eq!(readiness_dial_address("0.0.0.0:1080"), "127.0.0.1:1080");
        assert_eq!(readiness_dial_address("127.0.0.1:1080"), "127.0.0.1:1080");
    }

    #[test]
    fn readiness_dial_address_loopback_for_v6_wildcard() {
        assert_eq!(readiness_dial_address("[::]:8888"), "[::1]:8888");
    }

    #[test]
    fn readiness_dial_address_passthrough_for_named_host() {
        assert_eq!(readiness_dial_address("proxy.local:443"), "proxy.local:443");
    }

    #[test]
    fn readiness_dial_address_returns_input_for_malformed() {
        assert_eq!(readiness_dial_address("not_a_bind"), "not_a_bind");
    }
}
