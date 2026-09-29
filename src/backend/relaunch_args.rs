use thiserror::Error;

use crate::backend::elevation::{ElevationHandshake, ELEVATE_HANDSHAKE_ARG_PREFIX};

pub const RESUME_PENDING_CONNECT_ARG: &str = "--resume-pending-connect";
pub const WAIT_PARENT_PID_ARG_PREFIX: &str = "--wait-parent-pid=";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ElevatedRelaunchArgs {
    pub resume_pending_connect: bool,
    pub wait_parent_pid: u32,
    /// Readiness token for this elevation attempt (see
    /// [`crate::backend::elevation`]). Absent for a plain relaunch.
    pub handshake: Option<ElevationHandshake>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RelaunchArgsError {
    #[error("invalid wait-parent pid {0:?}")]
    InvalidParentPid(String),
    #[error("invalid elevation handshake token {0:?}")]
    InvalidHandshake(String),
}

pub fn build_elevated_relaunch_args(
    parent_pid: u32,
    handshake: Option<&ElevationHandshake>,
) -> Vec<String> {
    let mut args = vec![RESUME_PENDING_CONNECT_ARG.to_string()];
    if parent_pid > 0 {
        args.push(format!("{WAIT_PARENT_PID_ARG_PREFIX}{parent_pid}"));
    }
    if let Some(handshake) = handshake {
        args.push(handshake.arg());
    }
    args
}

pub fn parse_elevated_relaunch_args(
    args: &[String],
) -> Result<ElevatedRelaunchArgs, RelaunchArgsError> {
    let mut parsed = ElevatedRelaunchArgs::default();
    for arg in args {
        if arg == RESUME_PENDING_CONNECT_ARG {
            parsed.resume_pending_connect = true;
        } else if let Some(value) = arg.strip_prefix(WAIT_PARENT_PID_ARG_PREFIX) {
            let pid: u32 = value
                .parse()
                .map_err(|_| RelaunchArgsError::InvalidParentPid(value.to_string()))?;
            if pid == 0 {
                return Err(RelaunchArgsError::InvalidParentPid(value.to_string()));
            }
            parsed.wait_parent_pid = pid;
            parsed.resume_pending_connect = true;
        } else if let Some(value) = arg.strip_prefix(ELEVATE_HANDSHAKE_ARG_PREFIX) {
            parsed.handshake = Some(
                ElevationHandshake::parse(value)
                    .ok_or_else(|| RelaunchArgsError::InvalidHandshake(value.to_string()))?,
            );
            parsed.resume_pending_connect = true;
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_elevated_relaunch_args_includes_marker_and_pid() {
        let args = build_elevated_relaunch_args(4242, None);
        assert_eq!(
            args,
            vec![
                "--resume-pending-connect".to_string(),
                "--wait-parent-pid=4242".to_string(),
            ]
        );
    }

    #[test]
    fn build_elevated_relaunch_args_omits_zero_pid() {
        let args = build_elevated_relaunch_args(0, None);
        assert_eq!(args, vec!["--resume-pending-connect".to_string()]);
    }

    #[test]
    fn build_elevated_relaunch_args_carries_the_handshake_token() {
        let handshake = ElevationHandshake::new();
        let args = build_elevated_relaunch_args(4242, Some(&handshake));
        assert_eq!(args.len(), 3);
        assert_eq!(args[2], handshake.arg());

        // ...and the child parses back exactly the same token, which is what
        // makes the readiness marker trustworthy.
        let parsed = parse_elevated_relaunch_args(&args).unwrap();
        assert!(parsed.resume_pending_connect);
        assert_eq!(parsed.wait_parent_pid, 4242);
        assert_eq!(parsed.handshake, Some(handshake));
    }

    #[test]
    fn parse_elevated_relaunch_args_rejects_a_malformed_handshake() {
        for bad in ["", "not-hex", "../../etc/passwd"] {
            let arg = format!("{ELEVATE_HANDSHAKE_ARG_PREFIX}{bad}");
            assert_eq!(
                parse_elevated_relaunch_args(&[arg]),
                Err(RelaunchArgsError::InvalidHandshake(bad.to_string()))
            );
        }
    }

    #[test]
    fn parse_elevated_relaunch_args_marker_alone() {
        let parsed =
            parse_elevated_relaunch_args(&["--resume-pending-connect".to_string()]).unwrap();
        assert!(parsed.resume_pending_connect);
        assert_eq!(parsed.wait_parent_pid, 0);
    }

    #[test]
    fn parse_elevated_relaunch_args_pid_implies_resume() {
        let parsed =
            parse_elevated_relaunch_args(&["--wait-parent-pid=12345".to_string()]).unwrap();
        assert!(parsed.resume_pending_connect);
        assert_eq!(parsed.wait_parent_pid, 12345);
    }

    #[test]
    fn parse_elevated_relaunch_args_invalid_parent_pid() {
        assert_eq!(
            parse_elevated_relaunch_args(&["--wait-parent-pid=abc".to_string()]),
            Err(RelaunchArgsError::InvalidParentPid("abc".into()))
        );
        assert_eq!(
            parse_elevated_relaunch_args(&["--wait-parent-pid=0".to_string()]),
            Err(RelaunchArgsError::InvalidParentPid("0".into()))
        );
        assert_eq!(
            parse_elevated_relaunch_args(&["--wait-parent-pid=-5".to_string()]),
            Err(RelaunchArgsError::InvalidParentPid("-5".into()))
        );
    }
}
