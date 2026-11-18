//! The only module with operating-system specific code for the integrated terminal.

use std::io;
use std::path::{Path, PathBuf};

use super::launch::{env_value, EnvVars};

/// macOS terminals open login shells, and much of the configuration lives in
/// the login files there.
pub(crate) fn login_shell() -> bool {
    cfg!(target_os = "macos")
}

/// Used when `$SHELL` is missing, empty or cannot be started.
pub(crate) const FALLBACK_SHELL: &str = "/bin/sh";

/// `$SHELL`, or the fallback shell when it is missing or empty.
pub(crate) fn user_shell(env: &EnvVars) -> PathBuf {
    match env_value(env, "SHELL") {
        Some(shell) if !shell.is_empty() => PathBuf::from(shell),
        _ => PathBuf::from(FALLBACK_SHELL),
    }
}

#[cfg(unix)]
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
pub(crate) fn create_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir(path)
}

/// SIGKILL to a whole process group. `false` when it could not be sent.
#[cfg(unix)]
pub(crate) fn kill_process_group(pgid: i32) -> bool {
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;
    killpg(Pid::from_raw(pgid), Signal::SIGKILL).is_ok()
}

#[cfg(not(unix))]
pub(crate) fn kill_process_group(_pgid: i32) -> bool {
    false
}

#[cfg(all(unix, test))]
pub(crate) fn process_exists(pid: u32) -> bool {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    kill(Pid::from_raw(pid as i32), None).is_ok()
}

#[cfg(all(not(unix), test))]
pub(crate) fn process_exists(_pid: u32) -> bool {
    false
}
