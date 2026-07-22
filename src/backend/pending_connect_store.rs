use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use thiserror::Error;

const PENDING_CONNECT_FILE_NAME: &str = "gui_pending_connect.json";
const PENDING_CONNECT_MAX_AGE: Duration = Duration::minutes(5);

#[derive(Debug, Error)]
pub enum PendingConnectError {
    #[error("failed to read pending connect state: {0}")]
    Read(io::Error),
    #[error("failed to persist pending connect state: {0}")]
    Write(io::Error),
    #[error("failed to clear pending connect state: {0}")]
    Clear(io::Error),
    #[error("failed to encode pending connect state: {0}")]
    Encode(serde_json::Error),
    #[error("failed to parse pending connect state: {0}")]
    Decode(serde_json::Error),
}

#[derive(Debug, Serialize, Deserialize)]
struct PendingConnectState {
    #[serde(rename = "resumeConnect")]
    resume_connect: bool,
    #[serde(rename = "createdAt")]
    created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct PendingConnectStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl PendingConnectStore {
    pub fn new(app_dir: impl AsRef<Path>) -> Self {
        Self {
            path: app_dir.as_ref().join(PENDING_CONNECT_FILE_NAME),
            lock: Mutex::new(()),
        }
    }

    pub fn mark_resume_connect(&self) -> Result<(), PendingConnectError> {
        let _guard = self.lock.lock().expect("pending connect mutex poisoned");
        let state = PendingConnectState {
            resume_connect: true,
            created_at: Utc::now(),
        };
        let payload = serde_json::to_vec(&state).map_err(PendingConnectError::Encode)?;
        write_private(&self.path, &payload).map_err(PendingConnectError::Write)
    }

    pub fn has_resume_connect(&self) -> Result<bool, PendingConnectError> {
        let _guard = self.lock.lock().expect("pending connect mutex poisoned");
        let data = match fs::read(&self.path) {
            Ok(d) => d,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(PendingConnectError::Read(err)),
        };
        let state: PendingConnectState =
            serde_json::from_slice(&data).map_err(PendingConnectError::Decode)?;
        if Utc::now().signed_duration_since(state.created_at) > PENDING_CONNECT_MAX_AGE {
            match fs::remove_file(&self.path) {
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) => return Err(PendingConnectError::Clear(err)),
            }
            return Ok(false);
        }
        Ok(state.resume_connect)
    }

    pub fn clear(&self) -> Result<(), PendingConnectError> {
        let _guard = self.lock.lock().expect("pending connect mutex poisoned");
        match fs::remove_file(&self.path) {
            Ok(_) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(PendingConnectError::Clear(err)),
        }
    }
}

#[cfg(unix)]
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    use std::io::Write;
    file.write_all(data)
}

#[cfg(not(unix))]
fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    fs::write(path, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn pending_connect_store_roundtrip() {
        let tmp = tempdir().unwrap();
        let store = PendingConnectStore::new(tmp.path());

        assert!(!store.has_resume_connect().unwrap());
        store.mark_resume_connect().unwrap();
        assert!(store.has_resume_connect().unwrap());
        store.clear().unwrap();
        assert!(!store.has_resume_connect().unwrap());
    }

    #[test]
    fn pending_connect_store_has_resume_connect_clears_stale_marker() {
        let tmp = tempdir().unwrap();
        let store = PendingConnectStore::new(tmp.path());

        let stale = PendingConnectState {
            resume_connect: true,
            created_at: Utc::now() - Duration::minutes(10),
        };
        fs::write(
            tmp.path().join(PENDING_CONNECT_FILE_NAME),
            serde_json::to_vec(&stale).unwrap(),
        )
        .unwrap();

        assert!(!store.has_resume_connect().unwrap());
        assert!(!tmp.path().join(PENDING_CONNECT_FILE_NAME).exists());
    }
}
