//! Where gitoui keeps its files, resolved the same way everywhere.
//!
//! Linux and macOS: `$XDG_*_HOME`, otherwise `~/.config` and `~/.cache`
//! (macOS too, to keep existing installs where they are).
//! Windows: `%APPDATA%` and `%LOCALAPPDATA%`, which `dirs` resolves, since
//! `HOME` is normally not set there.

use std::env;
use std::path::PathBuf;

/// Windows gives back paths in their verbatim form (`\\?\C:\dir`) after
/// `canonicalize`: shown as is, they look broken. `C:\dir` is the same place.
/// UNC shares (`\\?\UNC\server\share`) and every other path are left alone.
pub fn simplify(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") && rest.chars().nth(1) == Some(':') => {
            PathBuf::from(rest)
        }
        _ => path,
    }
}

fn var_dir(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The user's home directory: `$HOME` when set (it wins, tests and
/// Unix-like shells on Windows rely on it), otherwise what the OS says.
pub fn home_dir() -> Option<PathBuf> {
    var_dir("HOME").or_else(dirs::home_dir)
}

/// Base directory of the configuration files (gitoui adds its own folder).
pub fn config_base() -> Option<PathBuf> {
    base("XDG_CONFIG_HOME", ".config", dirs::config_dir)
}

/// Base directory of the caches (gitoui adds its own folder).
pub fn cache_base() -> Option<PathBuf> {
    base("XDG_CACHE_HOME", ".cache", dirs::cache_dir)
}

fn base(xdg: &str, unix_default: &str, platform: fn() -> Option<PathBuf>) -> Option<PathBuf> {
    if let Some(dir) = var_dir(xdg) {
        return Some(dir);
    }
    if cfg!(windows) {
        return platform().or_else(|| home_dir().map(|h| h.join(unix_default)));
    }
    home_dir().map(|h| h.join(unix_default))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_variable_counts_as_unset() {
        env::set_var("GITOUI_TEST_EMPTY_DIR", "");
        assert_eq!(var_dir("GITOUI_TEST_EMPTY_DIR"), None);
        env::set_var("GITOUI_TEST_EMPTY_DIR", "/x");
        assert_eq!(var_dir("GITOUI_TEST_EMPTY_DIR"), Some(PathBuf::from("/x")));
        env::remove_var("GITOUI_TEST_EMPTY_DIR");
    }

    #[test]
    fn verbatim_drive_paths_lose_their_prefix() {
        let p = |s: &str| simplify(PathBuf::from(s));
        assert_eq!(p(r"\\?\C:\Users\a"), PathBuf::from(r"C:\Users\a"));
        assert_eq!(p(r"\\?\UNC\srv\share"), PathBuf::from(r"\\?\UNC\srv\share"));
        assert_eq!(p("/home/a"), PathBuf::from("/home/a"));
        assert_eq!(p(r"C:\Users\a"), PathBuf::from(r"C:\Users\a"));
    }

    #[test]
    fn the_xdg_variable_wins() {
        env::set_var("GITOUI_TEST_XDG", "/xdg");
        assert_eq!(
            base("GITOUI_TEST_XDG", ".cfg", || None),
            Some(PathBuf::from("/xdg"))
        );
        env::remove_var("GITOUI_TEST_XDG");
    }
}
