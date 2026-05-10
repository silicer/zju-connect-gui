use thiserror::Error;

pub const RESUME_PENDING_CONNECT_ARG: &str = "--resume-pending-connect";
pub const WAIT_PARENT_PID_ARG_PREFIX: &str = "--wait-parent-pid=";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ElevatedRelaunchArgs {
    pub resume_pending_connect: bool,
    pub wait_parent_pid: u32,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RelaunchArgsError {
    #[error("invalid wait-parent pid {0:?}")]
    InvalidParentPid(String),
}

pub fn build_elevated_relaunch_args(parent_pid: u32) -> Vec<String> {
    let mut args = vec![RESUME_PENDING_CONNECT_ARG.to_string()];
    if parent_pid > 0 {
        args.push(format!("{WAIT_PARENT_PID_ARG_PREFIX}{parent_pid}"));
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
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_elevated_relaunch_args_includes_marker_and_pid() {
        let args = build_elevated_relaunch_args(4242);
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
        let args = build_elevated_relaunch_args(0);
        assert_eq!(args, vec!["--resume-pending-connect".to_string()]);
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
