//! How the user's shell is launched: kind detection, a cleaned environment and
//! the integration directory holding the injected scripts.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use super::platform;

pub(crate) type EnvVars = Vec<(OsString, OsString)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShellKind {
    Bash,
    Zsh,
    Fish,
    /// Any other shell: each command runs alone through its own `-c`.
    Posix,
}

pub(crate) fn shell_kind(shell: &Path) -> ShellKind {
    match shell.file_name().and_then(OsStr::to_str) {
        Some("bash") => ShellKind::Bash,
        Some("zsh") => ShellKind::Zsh,
        Some("fish") => ShellKind::Fish,
        _ => ShellKind::Posix,
    }
}

pub(crate) fn env_value<'e>(env: &'e EnvVars, name: &str) -> Option<&'e OsStr> {
    env.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_os_str())
}

fn set_var(env: &mut EnvVars, name: &str, value: impl Into<OsString>) {
    env.retain(|(k, _)| k != name);
    env.push((OsString::from(name), value.into()));
}

const TERMINAL_PREFIXES: &[&str] = &[
    "KITTY_",
    "WEZTERM_",
    "ITERM_",
    "GHOSTTY_",
    "ALACRITTY_",
    "KONSOLE_",
    "VTE_",
    "WT_",
];
const TERMINAL_VARS: &[&str] = &[
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "TERMINFO",
    "GITOUI_NONCE",
    "GITOUI_USER_ZDOTDIR",
    "GITOUI_LOGIN",
];

/// The environment commands run in: the parent terminal's identity is dropped
/// so programs do not believe they run inside Kitty, WezTerm, etc.
pub(crate) fn sanitized_env(vars: EnvVars) -> EnvVars {
    let mut env: EnvVars = vars
        .into_iter()
        .filter(|(k, _)| {
            let k = k.to_string_lossy();
            !TERMINAL_VARS.contains(&k.as_ref())
                && !TERMINAL_PREFIXES.iter().any(|p| k.starts_with(p))
        })
        .collect();
    set_var(&mut env, "TERM", "xterm-256color");
    set_var(&mut env, "COLORTERM", "truecolor");
    env
}

/// Scripts the shell loads after the user's own configuration, in a private
/// temporary directory that is removed on drop.
pub(crate) struct Integration {
    dir: PathBuf,
    nonce: String,
}

impl Integration {
    pub(crate) fn create() -> io::Result<Self> {
        use std::hash::BuildHasher;
        let now = std::time::SystemTime::now();
        let raw =
            std::collections::hash_map::RandomState::new().hash_one((std::process::id(), now));
        let nonce = format!("{raw:016x}");
        let dir = std::env::temp_dir().join(format!("gitoui-shell-{}-{nonce}", std::process::id()));
        platform::create_private_dir(&dir)?;
        let integration = Self { dir, nonce };
        std::fs::create_dir(integration.dir.join("zsh"))?;
        let files: [(&str, &str); 7] = [
            ("zsh/.zshenv", include_str!("scripts/zshenv.zsh")),
            ("zsh/.zprofile", include_str!("scripts/zprofile.zsh")),
            ("zsh/.zshrc", include_str!("scripts/zshrc.zsh")),
            ("zsh/.zlogin", include_str!("scripts/zlogin.zsh")),
            ("zsh/gitoui.zsh", include_str!("scripts/gitoui.zsh")),
            ("gitoui.bash", include_str!("scripts/gitoui.bash")),
            ("gitoui.fish", include_str!("scripts/gitoui.fish")),
        ];
        for (name, body) in files {
            std::fs::write(integration.dir.join(name), body)?;
        }
        Ok(integration)
    }

    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    pub(crate) fn nonce(&self) -> &str {
        &self.nonce
    }
}

impl Drop for Integration {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub(crate) struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: EnvVars,
    pub cwd: PathBuf,
}

pub(crate) fn fish_single_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// A shell we know nothing about (nushell, xonsh, tcsh...): started as is,
/// without integration, so nothing but its own startup files runs.
pub(crate) fn plain_spec(shell: &Path, env: EnvVars, cwd: &Path) -> LaunchSpec {
    LaunchSpec {
        program: shell.to_path_buf(),
        args: Vec::new(),
        env: sanitized_env(env),
        cwd: cwd.to_path_buf(),
    }
}

pub(crate) fn session_spec(
    shell: &Path,
    kind: ShellKind,
    integration: &Integration,
    env: EnvVars,
    cwd: &Path,
    login: bool,
) -> LaunchSpec {
    let user_zdotdir = env_value(&env, "ZDOTDIR")
        .or_else(|| env_value(&env, "HOME"))
        .map(OsStr::to_os_string)
        .unwrap_or_default();
    let mut env = sanitized_env(env);
    set_var(&mut env, "GITOUI_NONCE", integration.nonce());
    let mut args: Vec<OsString> = Vec::new();
    match kind {
        ShellKind::Zsh => {
            set_var(
                &mut env,
                "ZDOTDIR",
                integration.dir().join("zsh").into_os_string(),
            );
            set_var(&mut env, "GITOUI_USER_ZDOTDIR", user_zdotdir);
            if login {
                args.push("-l".into());
            }
            args.push("-i".into());
        }
        ShellKind::Bash => {
            args.push("--rcfile".into());
            args.push(integration.dir().join("gitoui.bash").into_os_string());
            args.push("-i".into());
            if login {
                set_var(&mut env, "GITOUI_LOGIN", "1");
            }
        }
        ShellKind::Fish => {
            if login {
                args.push("-l".into());
            }
            args.push("-i".into());
            args.push("--init-command".into());
            let script = integration.dir().join("gitoui.fish");
            args.push(format!("source {}", fish_single_quoted(&script.to_string_lossy())).into());
        }
        ShellKind::Posix => {}
    }
    LaunchSpec {
        program: shell.to_path_buf(),
        args,
        env,
        cwd: cwd.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::Path;

    fn env(pairs: &[(&str, &str)]) -> EnvVars {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    fn get<'e>(e: &'e EnvVars, name: &str) -> Option<&'e std::ffi::OsStr> {
        env_value(e, name)
    }

    #[test]
    fn shell_kind_from_path() {
        assert_eq!(shell_kind(Path::new("/bin/bash")), ShellKind::Bash);
        assert_eq!(shell_kind(Path::new("/usr/local/bin/zsh")), ShellKind::Zsh);
        assert_eq!(
            shell_kind(Path::new("/opt/homebrew/bin/fish")),
            ShellKind::Fish
        );
        assert_eq!(shell_kind(Path::new("/bin/sh")), ShellKind::Posix);
        assert_eq!(shell_kind(Path::new("/usr/bin/nu")), ShellKind::Posix);
    }

    #[test]
    fn sanitized_env_drops_the_parent_terminal() {
        let out = sanitized_env(env(&[
            ("KITTY_WINDOW_ID", "1"),
            ("TERM", "xterm-kitty"),
            ("TERM_PROGRAM", "WezTerm"),
            ("TERMINFO", "/k"),
            ("WEZTERM_PANE", "3"),
            ("PATH", "/bin"),
            ("HOME", "/h"),
        ]));
        assert_eq!(get(&out, "PATH"), Some(std::ffi::OsStr::new("/bin")));
        assert_eq!(get(&out, "HOME"), Some(std::ffi::OsStr::new("/h")));
        assert_eq!(
            get(&out, "TERM"),
            Some(std::ffi::OsStr::new("xterm-256color"))
        );
        assert_eq!(
            get(&out, "COLORTERM"),
            Some(std::ffi::OsStr::new("truecolor"))
        );
        for gone in [
            "KITTY_WINDOW_ID",
            "TERM_PROGRAM",
            "TERMINFO",
            "WEZTERM_PANE",
        ] {
            assert!(get(&out, gone).is_none(), "{gone} should be removed");
        }
        assert_eq!(out.iter().filter(|(k, _)| k == "TERM").count(), 1);
    }

    #[test]
    fn integration_dir_is_private_and_cleaned_up() {
        let integration = Integration::create().unwrap();
        let dir = integration.dir().to_path_buf();
        for f in [
            "zsh/.zshenv",
            "zsh/.zprofile",
            "zsh/.zshrc",
            "zsh/.zlogin",
            "zsh/gitoui.zsh",
            "gitoui.bash",
            "gitoui.fish",
        ] {
            assert!(dir.join(f).is_file(), "{f} missing");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        let nonce = integration.nonce();
        assert_eq!(nonce.len(), 16);
        assert!(nonce
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        drop(integration);
        assert!(!dir.exists());
    }

    #[test]
    fn zsh_spec_points_zdotdir_at_the_integration() {
        let integration = Integration::create().unwrap();
        let spec = session_spec(
            Path::new("/bin/zsh"),
            ShellKind::Zsh,
            &integration,
            env(&[("HOME", "/h")]),
            Path::new("/r"),
            false,
        );
        let zdotdir = integration.dir().join("zsh");
        assert_eq!(get(&spec.env, "ZDOTDIR"), Some(zdotdir.as_os_str()));
        assert_eq!(
            get(&spec.env, "GITOUI_USER_ZDOTDIR"),
            Some(std::ffi::OsStr::new("/h"))
        );
        assert_eq!(
            get(&spec.env, "GITOUI_NONCE"),
            Some(std::ffi::OsStr::new(integration.nonce()))
        );
        assert_eq!(spec.args, vec![OsString::from("-i")]);
        assert_eq!(spec.cwd, Path::new("/r"));
        let login = session_spec(
            Path::new("/bin/zsh"),
            ShellKind::Zsh,
            &integration,
            env(&[("HOME", "/h")]),
            Path::new("/r"),
            true,
        );
        assert_eq!(login.args, vec![OsString::from("-l"), OsString::from("-i")]);
        let custom = session_spec(
            Path::new("/bin/zsh"),
            ShellKind::Zsh,
            &integration,
            env(&[("HOME", "/h"), ("ZDOTDIR", "/z")]),
            Path::new("/r"),
            false,
        );
        assert_eq!(
            get(&custom.env, "GITOUI_USER_ZDOTDIR"),
            Some(std::ffi::OsStr::new("/z"))
        );
    }

    #[test]
    fn bash_spec_uses_the_rcfile() {
        let integration = Integration::create().unwrap();
        let rc = integration.dir().join("gitoui.bash");
        let spec = session_spec(
            Path::new("/bin/bash"),
            ShellKind::Bash,
            &integration,
            env(&[("HOME", "/h")]),
            Path::new("/r"),
            false,
        );
        assert_eq!(
            spec.args,
            vec![
                OsString::from("--rcfile"),
                rc.into_os_string(),
                OsString::from("-i")
            ]
        );
        assert!(get(&spec.env, "GITOUI_LOGIN").is_none());
        let login = session_spec(
            Path::new("/bin/bash"),
            ShellKind::Bash,
            &integration,
            env(&[("HOME", "/h")]),
            Path::new("/r"),
            true,
        );
        assert_eq!(
            get(&login.env, "GITOUI_LOGIN"),
            Some(std::ffi::OsStr::new("1"))
        );
    }

    #[test]
    fn fish_spec_sources_the_script() {
        let integration = Integration::create().unwrap();
        let spec = session_spec(
            Path::new("/usr/bin/fish"),
            ShellKind::Fish,
            &integration,
            env(&[("HOME", "/h")]),
            Path::new("/r"),
            true,
        );
        let script = integration.dir().join("gitoui.fish");
        let expected = format!("source {}", fish_single_quoted(&script.to_string_lossy()));
        assert_eq!(spec.args[0], OsString::from("-l"));
        assert_eq!(spec.args[1], OsString::from("-i"));
        assert_eq!(spec.args[2], OsString::from("--init-command"));
        assert_eq!(spec.args[3], OsString::from(expected));
    }

    #[test]
    fn plain_spec_starts_the_shell_as_is() {
        let spec = plain_spec(
            Path::new("/usr/bin/nu"),
            env(&[("TERM", "x")]),
            Path::new("/r"),
        );
        assert_eq!(spec.program, Path::new("/usr/bin/nu"));
        assert!(spec.args.is_empty());
        assert_eq!(
            get(&spec.env, "TERM"),
            Some(std::ffi::OsStr::new("xterm-256color"))
        );
        assert!(get(&spec.env, "ZDOTDIR").is_none());
    }

    #[test]
    fn fish_quoting() {
        assert_eq!(fish_single_quoted("/a'b\\c"), "'/a\\'b\\\\c'");
    }
}
