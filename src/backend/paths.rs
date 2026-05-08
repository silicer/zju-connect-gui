use std::io;
use std::path::PathBuf;

pub fn resolve_app_dir() -> io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    exe.parent()
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("executable has no parent directory"))
}
