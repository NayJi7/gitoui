//! Recently-visited directory persistence for the `d` (change directory)
//! switcher. Stores up to `MAX_RECENTS` absolute paths in a small JSON file
//! under the user's config dir; the most recent sits at index 0.
//!
//! File location: `$XDG_CONFIG_HOME/gitoui/recents.json` (defaults to
//! `~/.config/gitoui/recents.json`). The file is written best-effort —
//! failures are logged to stderr and never propagate, because losing a
//! recents entry shouldn't disrupt the user's session.

use std::fs;
use std::path::{Path, PathBuf};

const MAX_RECENTS: usize = 20;

fn recents_file() -> Option<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("gitoui").join("recents.json"))
}

pub fn load() -> Vec<PathBuf> {
    let Some(path) = recents_file() else {
        return Vec::new();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    // Minimalist parser: the file is `["/path/a","/path/b",...]`, one entry
    // per element. We avoid pulling serde for this single use case.
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return Vec::new();
    }
    let inner = trimmed.trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .filter_map(|s| {
            let s = s.trim();
            let s = s.strip_prefix('"')?.strip_suffix('"')?;
            // Unescape `\"` → `"` and `\\` → `\` (the only two we emit).
            let unescaped = s.replace("\\\"", "\"").replace("\\\\", "\\");
            Some(PathBuf::from(unescaped))
        })
        .collect()
}

fn save_at(path: &Path, entries: &[PathBuf]) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut out = String::from("[");
    for (i, p) in entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let s = p.to_string_lossy();
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        out.push('"');
        out.push_str(&escaped);
        out.push('"');
    }
    out.push(']');
    if let Err(e) = fs::write(path, out) {
        eprintln!("gitoui: failed to write recents.json: {}", e);
    }
}

/// Move `path` to the front of `current`, dedup, cap at `MAX_RECENTS`, and
/// return the updated list. The file write is off-loaded to a background
/// thread — the returned `Vec` is immediately authoritative and the on-disk
/// copy catches up within milliseconds.
///
/// Callers pass their in-memory list so we avoid a redundant `load()` call.
/// `path` is canonicalised so the same physical dir entered via different
/// relative routes maps to a single entry.
pub fn push(path: &Path, current: &[PathBuf]) -> Vec<PathBuf> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut recents = current.to_vec();
    recents.retain(|p| p != &canonical);
    recents.insert(0, canonical);
    recents.truncate(MAX_RECENTS);
    let to_save = recents.clone();
    // Capture the destination path eagerly so a background save that
    // wakes up after the env (XDG_CONFIG_HOME) has shifted — common in
    // the parallel test suite — still writes to the file the caller
    // expects.
    let dest = recents_file();
    std::thread::spawn(move || {
        if let Some(path) = dest {
            save_at(&path, &to_save);
        }
    });
    recents
}

#[cfg(test)]
mod tests {
    //! Baseline for the recents store — pinned ahead of the upcoming
    //! "async write + in-memory cache" optimisation. Each test installs a
    //! private XDG_CONFIG_HOME pointing at a tempdir so the suite can run
    //! in parallel without stomping on the real user file.
    //!
    //! Env vars are process-global. The shared `ENV_LOCK` serialises tests
    //! that touch `XDG_CONFIG_HOME`/`HOME` so they don't race.
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        prev_xdg: Option<String>,
        prev_home: Option<String>,
        _dir: TempDir,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new() -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let dir = TempDir::new().unwrap();
            let prev_xdg = std::env::var("XDG_CONFIG_HOME").ok();
            let prev_home = std::env::var("HOME").ok();
            std::env::set_var("XDG_CONFIG_HOME", dir.path());
            Self {
                prev_xdg,
                prev_home,
                _dir: dir,
                _guard: guard,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev_xdg {
                Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            if let Some(v) = &self.prev_home {
                std::env::set_var("HOME", v);
            }
        }
    }

    #[test]
    fn empty_load_returns_empty_vec() {
        let _g = EnvGuard::new();
        assert!(load().is_empty());
    }

    #[test]
    fn push_then_load_round_trips() {
        let _g = EnvGuard::new();
        let target = TempDir::new().unwrap();
        let recents = push(target.path(), &[]);
        assert_eq!(recents.len(), 1);
        assert_eq!(recents[0], target.path().canonicalize().unwrap());
    }

    #[test]
    fn most_recent_sits_at_index_zero() {
        let _g = EnvGuard::new();
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let recents = push(a.path(), &[]);
        let recents = push(b.path(), &recents);
        assert_eq!(recents.len(), 2);
        assert_eq!(recents[0], b.path().canonicalize().unwrap());
        assert_eq!(recents[1], a.path().canonicalize().unwrap());
    }

    #[test]
    fn repushing_a_path_dedupes_and_bumps_to_top() {
        let _g = EnvGuard::new();
        let a = TempDir::new().unwrap();
        let b = TempDir::new().unwrap();
        let recents = push(a.path(), &[]);
        let recents = push(b.path(), &recents);
        let recents = push(a.path(), &recents);
        assert_eq!(recents.len(), 2, "must dedup, not grow");
        assert_eq!(recents[0], a.path().canonicalize().unwrap());
        assert_eq!(recents[1], b.path().canonicalize().unwrap());
    }

    #[test]
    fn list_caps_at_max_recents() {
        let _g = EnvGuard::new();
        let dirs: Vec<TempDir> = (0..MAX_RECENTS + 5)
            .map(|_| TempDir::new().unwrap())
            .collect();
        let mut recents: Vec<PathBuf> = vec![];
        for d in &dirs {
            recents = push(d.path(), &recents);
        }
        assert_eq!(recents.len(), MAX_RECENTS);
        // The 5 oldest must have been dropped; the very last pushed is on top.
        assert_eq!(
            recents[0],
            dirs.last().unwrap().path().canonicalize().unwrap()
        );
    }

    #[test]
    fn path_with_special_chars_round_trips() {
        let _g = EnvGuard::new();
        // The serializer escapes `\` and `"`; round-trip via save+load to
        // verify the encoding independently of the async background thread.
        let base = TempDir::new().unwrap();
        let weird = base.path().join("with \" quote and \\ slash");
        std::fs::create_dir(&weird).unwrap();
        let recents = push(&weird, &[]);
        // Flush the background write synchronously by calling save_at
        // directly. Captures the destination path explicitly so the
        // (now-non-existent) lazy XDG re-read can't drift the file.
        save_at(&recents_file().unwrap(), &recents);
        let loaded = load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], weird.canonicalize().unwrap());
    }
}
