//! The elevation handshake: how the original (unprivileged) instance learns
//! that the elevated instance it asked for is actually up.
//!
//! Relaunching ourselves with administrator rights is asynchronous and
//! user-paced: on Windows `ShellExecuteW("runas")` waits on the UAC consent
//! prompt, on Linux `pkexec` waits on the polkit authentication dialog. The
//! parent must therefore neither release the single-instance lock too early
//! (the elevated child polls for it) nor keep it forever when the user
//! dismisses the dialog — otherwise the GUI sits at "waiting for
//! authorization" with nothing on screen.
//!
//! The protocol is deliberately simple and file-based:
//!
//! 1. the parent generates a random token and passes it to the child as
//!    `--elevate-handshake=<token>`;
//! 2. the child, as one of its very first actions, creates
//!    `<app_dir>/.elevate-ready-<token>`;
//! 3. the parent polls for exactly that path (and for the death of the
//!    elevation helper) until it appears, and only then drops the instance
//!    lock and exits.
//!
//! Because the token is random per attempt, a marker left behind by an earlier
//! attempt can never be mistaken for this one; because the token is validated
//! to be lowercase hex, it cannot smuggle a path separator into `app_dir`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rand::Rng;

/// Command-line flag carrying the handshake token to the elevated instance.
pub const ELEVATE_HANDSHAKE_ARG_PREFIX: &str = "--elevate-handshake=";

/// Marker file prefix inside the application directory.
const MARKER_PREFIX: &str = ".elevate-ready-";

/// Number of hex characters in a token (128 random bits).
const TOKEN_LEN: usize = 32;

/// A per-attempt readiness token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElevationHandshake {
    token: String,
}

impl ElevationHandshake {
    /// Generate a fresh token for one elevation attempt.
    pub fn new() -> Self {
        let bits: u128 = rand::thread_rng().gen();
        Self {
            token: format!("{bits:0width$x}", width = TOKEN_LEN),
        }
    }

    /// Parse a token received on the command line. Returns `None` for anything
    /// that is not exactly [`TOKEN_LEN`] lowercase hex characters, so a
    /// malformed (or hostile) value can never escape `app_dir`.
    pub fn parse(token: &str) -> Option<Self> {
        if token.len() != TOKEN_LEN {
            return None;
        }
        if !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        Some(Self {
            token: token.to_string(),
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// The `--elevate-handshake=<token>` argument for the elevated instance.
    pub fn arg(&self) -> String {
        format!("{ELEVATE_HANDSHAKE_ARG_PREFIX}{}", self.token)
    }

    /// Absolute path of this attempt's readiness marker.
    pub fn marker_path(&self, app_dir: &Path) -> PathBuf {
        app_dir.join(format!("{MARKER_PREFIX}{}", self.token))
    }

    /// Child side: publish readiness. Written *before* the instance lock is
    /// touched — the parent still holds it and only releases it once this file
    /// exists.
    pub fn mark_ready(&self, app_dir: &Path) -> io::Result<()> {
        fs::write(
            self.marker_path(app_dir),
            format!("pid={}\n", std::process::id()),
        )
    }

    /// Parent side: has the elevated instance come up?
    pub fn is_ready(&self, app_dir: &Path) -> bool {
        self.marker_path(app_dir).exists()
    }

    /// Best-effort removal of the marker (both sides call this once it is
    /// obsolete).
    pub fn clear(&self, app_dir: &Path) {
        match fs::remove_file(self.marker_path(app_dir)) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => log::debug!("failed to remove elevation marker: {err}"),
        }
    }
}

impl Default for ElevationHandshake {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn tokens_are_hex_and_unique() {
        let a = ElevationHandshake::new();
        let b = ElevationHandshake::new();
        assert_eq!(a.token().len(), TOKEN_LEN);
        assert!(a.token().bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a.token(), b.token());
    }

    #[test]
    fn arg_round_trips_through_parse() {
        let handshake = ElevationHandshake::new();
        let arg = handshake.arg();
        let value = arg
            .strip_prefix(ELEVATE_HANDSHAKE_ARG_PREFIX)
            .expect("arg carries the documented prefix");
        assert_eq!(
            ElevationHandshake::parse(value),
            Some(handshake.clone()),
            "the child must reconstruct exactly the parent's token"
        );
    }

    #[test]
    fn parse_rejects_malformed_tokens() {
        // Wrong length.
        assert!(ElevationHandshake::parse("").is_none());
        assert!(ElevationHandshake::parse("abc").is_none());
        assert!(ElevationHandshake::parse(&"a".repeat(TOKEN_LEN + 1)).is_none());
        // Uppercase hex is not what `new()` emits.
        assert!(ElevationHandshake::parse(&"A".repeat(TOKEN_LEN)).is_none());
        // Path traversal / separators must never reach `marker_path`, not even
        // at the right length.
        assert!(ElevationHandshake::parse(&format!("{}..", "a".repeat(TOKEN_LEN - 2))).is_none());
        assert!(ElevationHandshake::parse(&"../".repeat(TOKEN_LEN)[..TOKEN_LEN]).is_none());
    }

    #[test]
    fn marker_path_stays_inside_app_dir() {
        let handshake = ElevationHandshake::new();
        let dir = Path::new("/tmp/app");
        let marker = handshake.marker_path(dir);
        assert_eq!(marker.parent(), Some(dir));
        assert!(marker
            .file_name()
            .expect("marker has a name")
            .to_string_lossy()
            .starts_with(MARKER_PREFIX));
    }

    #[test]
    fn mark_ready_is_visible_to_the_parent_and_clears() {
        let tmp = tempdir().unwrap();
        let handshake = ElevationHandshake::new();

        assert!(!handshake.is_ready(tmp.path()));
        handshake.mark_ready(tmp.path()).unwrap();
        assert!(handshake.is_ready(tmp.path()));

        // A different attempt's token must not see this marker.
        let other = ElevationHandshake::new();
        assert!(!other.is_ready(tmp.path()));

        handshake.clear(tmp.path());
        assert!(!handshake.is_ready(tmp.path()));
        // Clearing twice is fine (the parent and the child both do it).
        handshake.clear(tmp.path());
    }
}
