use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

/// Upper bound on buffered partial-line data. A pathological child that emits
/// endless data without newlines must not grow the buffer without limit; once
/// the cap is hit we keep only the tail (an unterminated multi-hundred-KB
/// "line" is not a prompt anyway).
const MAX_PENDING_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedPrompt {
    Sms,
    Callback,
    Captcha,
}

pub fn classify_prompt(line: &str) -> Option<DetectedPrompt> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_lowercase();
    if lower.contains("sms verification code")
        || lower.contains("sms code")
        || lower.contains("sms check code")
        || trimmed.contains("短信验证码")
    {
        return Some(DetectedPrompt::Sms);
    }
    if lower.contains("callback url") {
        return Some(DetectedPrompt::Callback);
    }
    if lower.contains("graph check code")
        || lower.contains("graph code json")
        || lower.contains("rand code")
        || (lower.contains("captcha") && !lower.contains("sms"))
        || trimmed.contains("图形验证码")
    {
        return Some(DetectedPrompt::Captcha);
    }
    None
}

pub fn is_vpn_started(line: &str) -> bool {
    line.trim().contains("VPN client started")
}

pub fn is_route_added(line: &str) -> bool {
    line.trim().contains("Add route to ")
}

pub async fn consume_stream<R, F, P>(
    mut reader: R,
    mut on_line: F,
    mut on_partial: P,
) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    F: FnMut(&str),
    P: FnMut(&str),
{
    let mut buf = vec![0u8; 32 * 1024];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => {
                let pending_str = String::from_utf8_lossy(&pending);
                if !pending_str.trim().is_empty() {
                    on_line(&pending_str);
                }
                return Ok(());
            }
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                loop {
                    let pos = pending.iter().position(|&b| b == b'\r' || b == b'\n');
                    let Some(idx) = pos else { break };
                    let line_bytes = &pending[..idx];
                    let line = String::from_utf8_lossy(line_bytes).into_owned();
                    let mut consume = idx + 1;
                    while consume < pending.len()
                        && (pending[consume] == b'\r' || pending[consume] == b'\n')
                    {
                        consume += 1;
                    }
                    pending.drain(..consume);
                    on_line(&line);
                }
                if pending.len() > MAX_PENDING_BYTES {
                    let excess = pending.len() - MAX_PENDING_BYTES;
                    pending.drain(..excess);
                }
                let pending_str = String::from_utf8_lossy(&pending);
                if !pending_str.trim().is_empty() {
                    on_partial(&pending_str);
                }
            }
            Err(err) => return Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_sms_prompts() {
        assert_eq!(
            classify_prompt("Please enter SMS verification code"),
            Some(DetectedPrompt::Sms)
        );
        assert_eq!(
            classify_prompt("Enter sms code:"),
            Some(DetectedPrompt::Sms)
        );
        assert_eq!(
            classify_prompt("请输入短信验证码"),
            Some(DetectedPrompt::Sms)
        );
    }

    #[test]
    fn classify_callback_prompt() {
        assert_eq!(
            classify_prompt("please paste the callback URL"),
            Some(DetectedPrompt::Callback)
        );
    }

    #[test]
    fn classify_captcha_prompts() {
        assert_eq!(
            classify_prompt("graph check code please"),
            Some(DetectedPrompt::Captcha)
        );
        assert_eq!(
            classify_prompt("captcha required"),
            Some(DetectedPrompt::Captcha)
        );
        assert_eq!(
            classify_prompt("请输入图形验证码"),
            Some(DetectedPrompt::Captcha)
        );
    }

    #[test]
    fn classify_returns_none_for_irrelevant_lines() {
        assert_eq!(classify_prompt("VPN client started"), None);
        assert_eq!(classify_prompt(""), None);
    }

    #[test]
    fn is_vpn_started_matches_substring() {
        assert!(is_vpn_started("[INFO] VPN client started successfully"));
        assert!(!is_vpn_started("starting VPN"));
    }

    #[test]
    fn is_route_added_matches_substring() {
        assert!(is_route_added("[INFO] Add route to 10.0.0.0/8"));
        assert!(!is_route_added("removing route"));
    }

    #[tokio::test]
    async fn consume_stream_splits_lines_and_flushes_tail() {
        let input = b"first\nsecond\r\nthird".to_vec();
        let mut lines = Vec::new();
        consume_stream(
            input.as_slice(),
            |line| lines.push(line.to_string()),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(lines, vec!["first", "second", "third"]);
    }

    #[tokio::test]
    async fn consume_stream_emits_partial_line_for_prompts() {
        let mut partial_seen = Vec::new();
        let mut lines = Vec::new();
        // a single chunk without newline triggers on_partial but not on_line
        consume_stream(
            &b"please enter sms code: "[..],
            |line| lines.push(line.to_string()),
            |partial| partial_seen.push(partial.to_string()),
        )
        .await
        .unwrap();
        // No newline → only on_partial fires after the read; on EOF it flushes via on_line
        assert_eq!(lines, vec!["please enter sms code: "]);
        assert!(!partial_seen.is_empty());
    }

    #[tokio::test]
    async fn consume_stream_caps_unbounded_partial_buffer() {
        // A 256 KiB chunk with no newlines must not grow the pending buffer
        // without bound; the tail is capped at MAX_PENDING_BYTES.
        let input = vec![b'x'; MAX_PENDING_BYTES * 4];
        let mut line_lengths = Vec::new();
        consume_stream(
            input.as_slice(),
            |line| line_lengths.push(line.len()),
            |_| {},
        )
        .await
        .unwrap();
        // Complete lines: none. EOF flush: exactly one capped line.
        assert_eq!(line_lengths, vec![MAX_PENDING_BYTES]);
    }
}
