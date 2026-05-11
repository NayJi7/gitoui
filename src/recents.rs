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
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("gitoui").join("recents.json"))
}

pub fn load() -> Vec<PathBuf> {
    let Some(path) = recents_file() else { return Vec::new() };
    let Ok(text) = fs::read_to_string(&path) else { return Vec::new() };
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

pub fn save(entries: &[PathBuf]) {
    let Some(path) = recents_file() else { return };
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
    if let Err(e) = fs::write(&path, out) {
        eprintln!("gitoui: failed to write recents.json: {}", e);
    }
}

/// Move `path` to the front of the recents list, dedup, cap at `MAX_RECENTS`,
/// then persist. `path` is canonicalised so the same physical dir entered via
/// different relative routes maps to a single entry.
pub fn push(path: &Path) {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut recents = load();
    recents.retain(|p| p != &canonical);
    recents.insert(0, canonical);
    recents.truncate(MAX_RECENTS);
    save(&recents);
}
