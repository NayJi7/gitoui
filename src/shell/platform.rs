//! The only module with operating-system specific code for the integrated terminal.

use std::io;
use std::path::{Path, PathBuf};

use super::launch::{env_value, EnvVars};

/// macOS terminals open login shells, and much of the configuration lives in
/// the login files there.
pub(crate) fn login_shell() -> bool {
    cfg!(target_os = "macos")
}

/// Used when no shell is configured or the configured one cannot be started.
#[cfg(not(windows))]
pub(crate) const FALLBACK_SHELL: &str = "/bin/sh";
#[cfg(windows)]
pub(crate) const FALLBACK_SHELL: &str = "cmd.exe";

/// `$SHELL` (Git Bash and friends set it), otherwise the platform's default.
pub(crate) fn user_shell(env: &EnvVars) -> PathBuf {
    match env_value(env, "SHELL") {
        Some(shell) if !shell.is_empty() => PathBuf::from(shell),
        _ => default_shell(env),
    }
}

#[cfg(not(windows))]
fn default_shell(_env: &EnvVars) -> PathBuf {
    PathBuf::from(FALLBACK_SHELL)
}

/// PowerShell 7, then Windows PowerShell, then `%ComSpec%`.
#[cfg(windows)]
fn default_shell(env: &EnvVars) -> PathBuf {
    for name in ["pwsh.exe", "powershell.exe"] {
        if let Some(found) = find_in_path(env, name) {
            return found;
        }
    }
    match env_value(env, "ComSpec") {
        Some(comspec) if !comspec.is_empty() => PathBuf::from(comspec),
        _ => PathBuf::from(FALLBACK_SHELL),
    }
}

/// The first `PATH` directory that holds `name`.
#[cfg(any(windows, test))]
pub(crate) fn find_in_path(env: &EnvVars, name: &str) -> Option<PathBuf> {
    let path = env_value(env, "PATH")?;
    std::env::split_paths(path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn env(pairs: &[(&str, OsString)]) -> EnvVars {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(*k), v.clone()))
            .collect()
    }

    #[test]
    fn the_shell_variable_wins() {
        let e = env(&[("SHELL", "/usr/bin/fish".into())]);
        assert_eq!(user_shell(&e), PathBuf::from("/usr/bin/fish"));
    }

    #[test]
    fn find_in_path_returns_the_first_directory_holding_the_file() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::write(second.path().join("tool"), "").unwrap();
        let joined = std::env::join_paths([first.path(), second.path()]).unwrap();
        let e = env(&[("PATH", joined)]);
        assert_eq!(find_in_path(&e, "tool"), Some(second.path().join("tool")));
        assert_eq!(find_in_path(&e, "absent"), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_prefers_powershell_then_comspec() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("powershell.exe"), "").unwrap();
        let path = std::env::join_paths([dir.path()]).unwrap();
        let e = env(&[
            ("Path", path),
            ("ComSpec", r"C:\Windows\System32\cmd.exe".into()),
        ]);
        assert_eq!(user_shell(&e), dir.path().join("powershell.exe"));
        let none = env(&[("ComSpec", r"C:\x\cmd.exe".into())]);
        assert_eq!(user_shell(&none), PathBuf::from(r"C:\x\cmd.exe"));
    }
}
