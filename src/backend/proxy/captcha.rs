use base64::Engine;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tokio::fs;
use tokio::time;

pub const CAPTCHA_POLL_INTERVAL: Duration = Duration::from_millis(500);
pub const CAPTCHA_MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(500);
pub const CAPTCHA_POLL_TIMEOUT: Duration = Duration::from_secs(60);

pub fn encode_captcha(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Block until the captcha file at `path` exists, has non-zero size, and that size is
/// stable across at least one polling interval (so the producer has finished writing).
/// Returns `Ok(Some(bytes))` when ready, `Ok(None)` if `CAPTCHA_POLL_TIMEOUT` elapses.
pub async fn poll_for_stable_captcha(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    let deadline = tokio::time::Instant::now() + CAPTCHA_POLL_TIMEOUT;
    let mut last_size: u64 = 0;
    let mut stable_count: u32 = 0;
    while tokio::time::Instant::now() < deadline {
        time::sleep(CAPTCHA_POLL_INTERVAL).await;
        let metadata = match fs::metadata(path).await {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        let size = metadata.len();
        if size == 0 {
            continue;
        }
        if size == last_size {
            stable_count += 1;
        } else {
            stable_count = 0;
        }
        last_size = size;
        if stable_count < 1 {
            continue;
        }
        match fs::read(path).await {
            Ok(bytes) if !bytes.is_empty() => return Ok(Some(bytes)),
            _ => continue,
        }
    }
    Ok(None)
}

/// Stream of captcha file modifications (size or mtime change with non-empty file).
/// Returns the path each time a fresh write is observed. Cancellation is the caller's
/// responsibility — drop the future to stop monitoring.
pub async fn monitor_captcha_file<F: FnMut(PathBuf)>(path: PathBuf, mut on_update: F) {
    let mut last_mtime: Option<SystemTime> = None;
    let mut last_size: u64 = 0;
    loop {
        time::sleep(CAPTCHA_MONITOR_POLL_INTERVAL).await;
        let Ok(metadata) = fs::metadata(&path).await else {
            continue;
        };
        if metadata.len() == 0 {
            continue;
        }
        let current_mtime = metadata.modified().ok();
        let current_size = metadata.len();
        if current_mtime == last_mtime && current_size == last_size {
            continue;
        }
        last_mtime = current_mtime;
        last_size = current_size;
        on_update(path.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn encode_captcha_uses_standard_base64() {
        assert_eq!(encode_captcha(b"hi"), "aGk=");
        assert_eq!(encode_captcha(&[]), "");
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn poll_for_stable_captcha_times_out_when_file_missing() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("never");
        let result = poll_for_stable_captcha(&path).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn poll_for_stable_captcha_returns_bytes_when_size_stabilizes() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("captcha.png");
        let path_clone = path.clone();
        let writer = tokio::spawn(async move {
            // First write: smaller, transient
            let mut f = std::fs::File::create(&path_clone).unwrap();
            f.write_all(b"abc").unwrap();
            f.sync_all().unwrap();
            drop(f);
            tokio::time::sleep(Duration::from_millis(600)).await;
            // Second write: final size, will stabilize after one more poll
            let mut f = std::fs::File::create(&path_clone).unwrap();
            f.write_all(b"finalbytes").unwrap();
            f.sync_all().unwrap();
        });

        let bytes = poll_for_stable_captcha(&path).await.unwrap();
        writer.await.unwrap();
        assert_eq!(bytes.as_deref(), Some(&b"finalbytes"[..]));
    }
}
