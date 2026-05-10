use std::process::Command;

pub const EIP_URL: &str = "http://eip.scmcc.com.cn/";

#[derive(Debug, thiserror::Error)]
pub enum OpenEipError {
    #[error("failed to spawn browser process: {0}")]
    Spawn(std::io::Error),
}

pub fn open_eip(program: &str, args: &[String]) -> Result<(), OpenEipError> {
    let mut cmd = if program.is_empty() {
        default_open_command()
    } else {
        let mut c = Command::new(program);
        c.args(args).arg(EIP_URL);
        c
    };
    cmd.spawn().map_err(OpenEipError::Spawn)?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn default_open_command() -> Command {
    let mut cmd = Command::new("rundll32");
    cmd.arg("url.dll,FileProtocolHandler").arg(EIP_URL);
    cmd
}

#[cfg(target_os = "macos")]
fn default_open_command() -> Command {
    let mut cmd = Command::new("open");
    cmd.arg(EIP_URL);
    cmd
}

#[cfg(all(unix, not(target_os = "macos")))]
fn default_open_command() -> Command {
    let mut cmd = Command::new("xdg-open");
    cmd.arg(EIP_URL);
    cmd
}
