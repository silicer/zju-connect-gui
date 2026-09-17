//! Readiness probe for the core's tunnel-backed DNS server.
//!
//! `zju-connect -dns-server-bind <addr>` starts a UDP DNS server that resolves
//! through the tunnel *even in proxy-only mode*, which is what makes a
//! per-process DNS hijack possible at all (the SOCKS5 UDP relay dials
//! unmatched destinations on the local machine, so proxying a DNS query through
//! it would just re-reach the local stub resolver).
//!
//! Redirecting a process to that server before it actually works is worse than
//! not redirecting at all: the core logs "Starting DNS server at ..." *before*
//! it binds, it keeps running when the bind fails, and while the tunnel is not
//! usable it answers NOERROR with an empty answer section (never SERVFAIL). A
//! stub resolver that caches such an answer treats the name as nonexistent. So
//! ProxyBridge only starts redirecting once this probe has seen a real answer.
//!
//! Everything here is UDP: the core's DNS listener is UDP-only
//! (`dns.Server{Net: "udp"}`), and a redirect that also captured TCP port 53
//! would break a stub resolver's retry after a truncated response.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Resend interval while waiting for the DNS server to become usable.
const RESEND_INTERVAL: Duration = Duration::from_millis(500);
/// How long a single `recv` may block before we look at the deadline again.
const READ_TIMEOUT: Duration = Duration::from_millis(200);

/// Sends an `A` query for `name` to `resolver`, resending it until `budget`
/// runs out, and reports whether the server ever answered with at least one
/// record.
///
/// An empty NOERROR/NXDOMAIN answer is deliberately *not* good enough: that is
/// what the core returns while the tunnel is still coming up.
pub fn wait_for_answer(resolver: SocketAddr, name: &str, budget: Duration) -> bool {
    let query = match build_query(name, query_id()) {
        Some(query) => query,
        None => return false,
    };

    let socket = match UdpSocket::bind(("0.0.0.0", 0)) {
        Ok(socket) => socket,
        Err(_) => return false,
    };
    if socket.set_read_timeout(Some(READ_TIMEOUT)).is_err() {
        return false;
    }

    let deadline = Instant::now() + budget;
    let mut next_send = Instant::now();
    let mut buf = [0u8; 512];

    loop {
        let now = Instant::now();
        if now >= deadline {
            return false;
        }

        if now >= next_send {
            if socket.send_to(&query, resolver).is_err() {
                return false;
            }
            next_send = now + RESEND_INTERVAL;
        }

        // A timeout, or a stray ICMP error surfaced as an error, just means we
        // keep resending until the budget is gone.
        if let Ok((len, _)) = socket.recv_from(&mut buf) {
            if answer_count_matches(&buf[..len], &query[..2]) {
                return true;
            }
        }
    }
}

/// Fallback probe name for a bare-IP server address.
///
/// The probe needs a name that resolves through the tunnel; this application
/// exists for the ZJU VPN, so its public site is a reasonable stand-in. A probe
/// that does not resolve only costs the hijack, never correctness.
pub const FALLBACK_PROBE_NAME: &str = "www.zju.edu.cn";

/// Name the readiness probe asks for: the configured VPN server, unless that is
/// empty or a bare IP address.
pub fn probe_name(server: &str) -> &str {
    let server = server.trim();
    if server.is_empty() || server.parse::<std::net::IpAddr>().is_ok() {
        FALLBACK_PROBE_NAME
    } else {
        server
    }
}

/// Random-ish query id; the socket is fresh, so a collision is irrelevant and
/// only used to discard replies that clearly are not ours.
fn query_id() -> u16 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos as u16) | 1
}

/// True when this packet is a response to our query and carries at least one
/// answer record.
fn answer_count_matches(response: &[u8], query_id: &[u8]) -> bool {
    // Header: id(2) flags(2) qdcount(2) ancount(2) ...
    if response.len() < 12 || response[..2] != *query_id {
        return false;
    }
    let is_response = response[2] & 0x80 != 0;
    let answer_count = u16::from_be_bytes([response[6], response[7]]);
    is_response && answer_count > 0
}

/// Minimal DNS query: recursion desired, one `A`/`IN` question.
fn build_query(name: &str, id: u16) -> Option<Vec<u8>> {
    let mut query = Vec::with_capacity(32);
    query.extend_from_slice(&id.to_be_bytes());
    query.extend_from_slice(&0x0100u16.to_be_bytes()); // RD
    query.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    query.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    query.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    query.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT

    let name = name.trim().trim_end_matches('.');
    if name.is_empty() {
        return None;
    }
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.push(0);
    query.extend_from_slice(&1u16.to_be_bytes()); // QTYPE A
    query.extend_from_slice(&1u16.to_be_bytes()); // QCLASS IN
    Some(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::thread;

    /// A throwaway DNS server that answers every query with the given answer
    /// count and hands back the address it bound to.
    fn fake_dns_server(answer_count: u16) -> (SocketAddr, thread::JoinHandle<()>) {
        let socket = UdpSocket::bind(("127.0.0.1", 0)).expect("bind fake dns server");
        let addr = socket.local_addr().expect("fake dns server addr");
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("set read timeout");

        let handle = thread::spawn(move || {
            let mut buf = [0u8; 512];
            // Serve a handful of queries, then exit so the test never hangs.
            for _ in 0..8 {
                let Ok((len, peer)) = socket.recv_from(&mut buf) else {
                    return;
                };
                if len < 12 {
                    continue;
                }
                let mut reply = buf[..len].to_vec();
                reply[2] |= 0x80; // QR: this is a response
                reply[3] = 0; // rcode success, no truncation
                reply[6..8].copy_from_slice(&answer_count.to_be_bytes());
                if answer_count > 0 {
                    reply.extend_from_slice(&[
                        0xC0, 0x0C, // pointer to the question name
                        0x00, 0x01, // A
                        0x00, 0x01, // IN
                        0x00, 0x00, 0x00, 0x05, // TTL
                        0x00, 0x04, // RDLENGTH
                        0x01, 0x02, 0x03, 0x04,
                    ]);
                }
                let _ = socket.send_to(&reply, peer);
            }
        });

        (addr, handle)
    }

    #[test]
    fn build_query_encodes_labels() {
        let query = build_query("a.bc", 0x1234).expect("query");
        assert_eq!(
            query,
            vec![
                0x12, 0x34, // id
                0x01, 0x00, // flags: RD
                0x00, 0x01, // QDCOUNT
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // AN/NS/AR
                0x01, b'a', // "a"
                0x02, b'b', b'c', // "bc"
                0x00, // root
                0x00, 0x01, // QTYPE A
                0x00, 0x01, // QCLASS IN
            ]
        );

        assert!(build_query("", 1).is_none());
        assert!(build_query("a..b", 1).is_none());
        assert!(build_query(&"x".repeat(64), 1).is_none());
    }

    #[test]
    fn probe_name_prefers_a_hostname() {
        assert_eq!(probe_name("sslvpn.scmcc.com.cn"), "sslvpn.scmcc.com.cn");
        assert_eq!(probe_name(" sslvpn.scmcc.com.cn "), "sslvpn.scmcc.com.cn");
        assert_eq!(probe_name("10.0.0.1"), FALLBACK_PROBE_NAME);
        assert_eq!(probe_name("::1"), FALLBACK_PROBE_NAME);
        assert_eq!(probe_name(""), FALLBACK_PROBE_NAME);
    }

    #[test]
    fn probe_succeeds_on_a_real_answer() {
        let (addr, handle) = fake_dns_server(1);
        assert!(wait_for_answer(
            addr,
            "sslvpn.scmcc.com.cn",
            Duration::from_secs(2)
        ));
        drop(handle);
    }

    #[test]
    fn probe_rejects_empty_answers() {
        // This is the pre-tunnel behaviour of the core (NOERROR, no records):
        // redirecting on it would poison the client's negative cache.
        let (addr, handle) = fake_dns_server(0);
        assert!(!wait_for_answer(
            addr,
            "example.com",
            Duration::from_millis(600)
        ));
        drop(handle);
    }

    #[test]
    fn probe_fails_when_nothing_listens() {
        // Bind and immediately drop to obtain a port that is free again.
        let dead = UdpSocket::bind(("127.0.0.1", 0))
            .expect("bind")
            .local_addr()
            .expect("addr");
        assert!(!wait_for_answer(
            dead,
            "example.com",
            Duration::from_millis(300)
        ));
    }

    #[test]
    fn probe_ignores_replies_with_a_foreign_id() {
        let socket = UdpSocket::bind(("127.0.0.1", 0)).expect("bind");
        let addr = socket.local_addr().expect("addr");
        let handle = thread::spawn(move || {
            let mut buf = [0u8; 512];
            if let Ok((len, peer)) = socket.recv_from(&mut buf) {
                let mut reply = buf[..len].to_vec();
                reply[2] |= 0x80;
                reply[6..8].copy_from_slice(&1u16.to_be_bytes());
                reply[0] ^= 0xFF; // wrong id
                let _ = socket.send_to(&reply, peer);
            }
        });
        assert!(!wait_for_answer(
            addr,
            "example.com",
            Duration::from_millis(400)
        ));
        drop(handle);
    }
}
