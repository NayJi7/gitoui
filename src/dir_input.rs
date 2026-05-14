//! State + helpers for the `d` (change directory) overlay.
//!
//! When active, the header's pwd display is replaced with a text input and a
//! dropdown of suggestions appears below. Suggestions come from two sources,
//! shown in this order:
//!   1. Recently-visited dirs (from `crate::recents`) whose path contains the
//!      typed substring.
//!   2. Filesystem entries from the directory implied by the typed prefix —
//!      e.g. `~/work/`, `../`, `/usr/local/`. Only sub-directories are kept
//!      so the user can't accidentally `cd` into a file.
//!
//! Path resolution supports `~` for $HOME, `.` / `..` segments, and both
//! absolute and relative inputs. Resolution is purely textual — we rely on
//! the eventual `std::env::set_current_dir` call to verify the target.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How many suggestions we keep in memory — the list is still capped here
/// to bound allocation, but the dropdown only renders `MAX_VISIBLE` rows
/// at a time and scrolls within them.
const MAX_SUGGESTIONS: usize = 50;
/// How many rows the dropdown shows on screen at once. The list scrolls
/// inside this window via ↑/↓ or the mouse wheel.
pub const MAX_VISIBLE: usize = 8;
/// Idle time after the last text edit before we re-read the parent directory
/// for filesystem suggestions. Each keystroke would otherwise trigger a
/// `std::fs::read_dir()` syscall — fine on local SSDs (~1 ms) but visibly
/// laggy on NFS / network mounts / massive dirs like `~/Downloads`. 150 ms
/// is short enough that suggestions still feel "live" but long enough to
/// coalesce a burst of typing into a single read.
const SUGGESTIONS_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionKind {
    Recent,
    Filesystem,
}

#[derive(Debug, Clone)]
pub struct DirSuggestion {
    pub path: PathBuf,
    /// Tilde-relative display string (e.g. `~/work/api`).
    pub display: String,
    pub kind: SuggestionKind,
}

#[derive(Debug, Default)]
pub struct DirInputState {
    pub active: bool,
    pub text: String,
    /// Byte index inside `text` of the cursor. Always at a char boundary.
    pub cursor: usize,
    /// Index into `suggestions` (None means "no suggestion focused — Enter
    /// uses the typed text as-is").
    pub selected: Option<usize>,
    pub suggestions: Vec<DirSuggestion>,
    /// First suggestion row visible in the dropdown — drives scrolling when
    /// the suggestion list exceeds `MAX_VISIBLE`.
    pub scroll: usize,
    /// Tallest height the dropdown has ever reached during this overlay
    /// session. The render loop watches it: each time the popup grows, the
    /// commit-graph Kitty images sitting under the newly-covered rows have
    /// to be deleted again (Kitty doesn't auto-drop placements when their
    /// placeholder cell is overwritten).
    pub last_rendered_height: u16,
    /// Set whenever the input text is mutated. The App's main loop calls
    /// `flush_pending_refresh` before each render; once `SUGGESTIONS_DEBOUNCE`
    /// has elapsed since the last edit, the filesystem read finally fires.
    /// `None` means "suggestions are up to date with the current text".
    pub text_dirty_since: Option<Instant>,
}

impl DirInputState {
    pub fn open(&mut self, recents: &[PathBuf]) {
        self.active = true;
        self.text.clear();
        self.cursor = 0;
        self.selected = None;
        self.scroll = 0;
        self.last_rendered_height = 0;
        // Open is the one place where we still refresh synchronously — the
        // user expects to see *something* the moment the overlay appears.
        self.text_dirty_since = None;
        self.refresh_suggestions(recents);
    }

    pub fn close(&mut self) {
        self.active = false;
        self.text.clear();
        self.cursor = 0;
        self.selected = None;
        self.scroll = 0;
        self.last_rendered_height = 0;
        self.suggestions.clear();
        self.text_dirty_since = None;
    }

    /// Mark suggestions stale without doing the filesystem read. The App's
    /// per-frame `flush_pending_refresh` picks it up `SUGGESTIONS_DEBOUNCE`
    /// later (or immediately on Tab/Enter, via `force_refresh`).
    fn mark_dirty(&mut self) {
        self.text_dirty_since = Some(Instant::now());
    }

    pub fn insert_char(&mut self, c: char, _recents: &[PathBuf]) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.selected = None;
        self.mark_dirty();
    }

    pub fn backspace(&mut self, _recents: &[PathBuf]) {
        if self.cursor == 0 {
            return;
        }
        // Step back one char (handles multi-byte UTF-8).
        let new_cursor = self.text[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        self.text.replace_range(new_cursor..self.cursor, "");
        self.cursor = new_cursor;
        self.selected = None;
        self.mark_dirty();
    }

    pub fn move_cursor_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor = self.text[..self.cursor]
            .char_indices()
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }

    pub fn move_cursor_right(&mut self) {
        if self.cursor >= self.text.len() {
            return;
        }
        let rest = &self.text[self.cursor..];
        if let Some(c) = rest.chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    /// Jump cursor one word back. Skips a run of word boundary chars
    /// (`/`, `.`, whitespace) then a run of word chars.
    pub fn move_cursor_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        // Walk backwards skipping boundary chars first.
        let bytes = self.text.as_bytes();
        let mut i = self.cursor;
        while i > 0 {
            let prev = prev_char_boundary(&self.text, i);
            let c = self.text[prev..i].chars().next().unwrap_or(' ');
            if is_word_boundary(c) {
                i = prev;
            } else {
                break;
            }
        }
        while i > 0 {
            let prev = prev_char_boundary(&self.text, i);
            let c = self.text[prev..i].chars().next().unwrap_or(' ');
            if is_word_boundary(c) {
                break;
            } else {
                i = prev;
            }
        }
        let _ = bytes;
        self.cursor = i;
    }

    /// Jump cursor one word forward.
    pub fn move_cursor_word_right(&mut self) {
        let len = self.text.len();
        if self.cursor >= len {
            return;
        }
        let mut i = self.cursor;
        // Skip current word chars.
        while i < len {
            let c = self.text[i..].chars().next().unwrap_or(' ');
            if is_word_boundary(c) {
                break;
            }
            i += c.len_utf8();
        }
        // Skip boundary run that follows.
        while i < len {
            let c = self.text[i..].chars().next().unwrap_or(' ');
            if !is_word_boundary(c) {
                break;
            }
            i += c.len_utf8();
        }
        self.cursor = i;
    }

    /// Delete the previous word (and any trailing boundary chars before it).
    pub fn delete_word_left(&mut self, _recents: &[PathBuf]) {
        if self.cursor == 0 {
            return;
        }
        let old_cursor = self.cursor;
        self.move_cursor_word_left();
        self.text.replace_range(self.cursor..old_cursor, "");
        self.selected = None;
        self.mark_dirty();
    }

    /// Delete the character to the right of the cursor (forward Delete key).
    pub fn delete_right(&mut self, _recents: &[PathBuf]) {
        if self.cursor >= self.text.len() {
            return;
        }
        let next = next_char_boundary(&self.text, self.cursor);
        self.text.replace_range(self.cursor..next, "");
        self.selected = None;
        self.mark_dirty();
    }

    /// Delete the next word starting at the cursor (Ctrl+Delete).
    pub fn delete_word_right(&mut self, _recents: &[PathBuf]) {
        if self.cursor >= self.text.len() {
            return;
        }
        let old_cursor = self.cursor;
        self.move_cursor_word_right();
        self.text.replace_range(old_cursor..self.cursor, "");
        self.cursor = old_cursor;
        self.selected = None;
        self.mark_dirty();
    }

    pub fn select_next(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        let new_sel = match self.selected {
            Some(i) => (i + 1).min(self.suggestions.len() - 1),
            None => 0,
        };
        self.selected = Some(new_sel);
        self.scroll_into_view();
    }

    pub fn select_prev(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        let new_sel = match self.selected {
            Some(0) | None => 0,
            Some(i) => i - 1,
        };
        self.selected = Some(new_sel);
        self.scroll_into_view();
    }

    /// Scroll the visible window by `delta` rows (positive = down). Clamps
    /// to the valid range so we don't scroll past the suggestion list.
    /// Used by the mouse wheel; selection is left untouched so the user can
    /// preview without committing.
    pub fn scroll_by(&mut self, delta: isize) {
        let max_scroll = self.suggestions.len().saturating_sub(MAX_VISIBLE);
        let new_scroll = (self.scroll as isize + delta)
            .max(0)
            .min(max_scroll as isize);
        self.scroll = new_scroll as usize;
    }

    fn scroll_into_view(&mut self) {
        if let Some(i) = self.selected {
            if i < self.scroll {
                self.scroll = i;
            } else if i >= self.scroll + MAX_VISIBLE {
                self.scroll = i + 1 - MAX_VISIBLE;
            }
        }
    }

    /// Resolve the input to a concrete path: prefer the focused suggestion,
    /// otherwise expand `~` / `..` / relative segments against `cwd`.
    pub fn resolve(&self, cwd: &Path) -> Option<PathBuf> {
        if let Some(i) = self.selected {
            if let Some(s) = self.suggestions.get(i) {
                return Some(s.path.clone());
            }
        }
        let text = self.text.trim();
        if text.is_empty() {
            return None;
        }
        Some(resolve_path(text, cwd))
    }

    pub fn refresh_suggestions_from(&mut self, recents: &[PathBuf]) {
        self.refresh_suggestions(recents);
        self.text_dirty_since = None;
    }

    /// Called by the App's main loop before each render: if the input was
    /// mutated more than `SUGGESTIONS_DEBOUNCE` ago, run the filesystem
    /// read now. Returns `true` if a refresh actually happened — callers
    /// use it to flag `needs_draw` so the new suggestions show on screen.
    pub fn flush_pending_refresh(&mut self, recents: &[PathBuf]) -> bool {
        let Some(dirty_at) = self.text_dirty_since else {
            return false;
        };
        if dirty_at.elapsed() < SUGGESTIONS_DEBOUNCE {
            return false;
        }
        self.refresh_suggestions(recents);
        self.text_dirty_since = None;
        true
    }

    /// Synchronous refresh for actions that need *current* suggestions
    /// regardless of debounce state — Tab (complete) and Enter (resolve).
    /// Without this, a fast typist + Tab would complete against a stale
    /// suggestion list.
    pub fn force_refresh(&mut self, recents: &[PathBuf]) {
        if self.text_dirty_since.is_some() {
            self.refresh_suggestions(recents);
            self.text_dirty_since = None;
        }
    }

    fn refresh_suggestions(&mut self, recents: &[PathBuf]) {
        let text = self.text.trim();
        let mut out: Vec<DirSuggestion> = Vec::new();
        let home = home_dir();

        // 1. Recents matching the substring. When the input is empty we
        //    surface every recent, which is the "VS Code recent workspaces"
        //    feel the user asked for.
        let needle = text.to_lowercase();
        for p in recents {
            if !needle.is_empty() {
                let hay = p.to_string_lossy().to_lowercase();
                if !hay.contains(&needle) {
                    continue;
                }
            }
            out.push(DirSuggestion {
                display: tilde_path(p, home.as_deref()),
                path: p.clone(),
                kind: SuggestionKind::Recent,
            });
            if out.len() >= MAX_SUGGESTIONS {
                break;
            }
        }

        // 2. Filesystem entries — only when the user explicitly typed a
        //    path-like prefix. Otherwise the dropdown stays focused on
        //    recents and isn't polluted with the current dir's children.
        if !text.is_empty() && looks_like_path(text) && out.len() < MAX_SUGGESTIONS {
            let (parent, prefix) = split_for_completion(text, home.as_deref());
            if let Ok(entries) = std::fs::read_dir(&parent) {
                let mut matches: Vec<PathBuf> = entries
                    .filter_map(|e| e.ok())
                    .filter(|e| {
                        // Sub-directories only — we can't cd into a file.
                        e.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    })
                    .filter(|e| {
                        let name = e.file_name();
                        let s = name.to_string_lossy();
                        prefix.is_empty() || s.starts_with(prefix.as_str())
                    })
                    .map(|e| e.path())
                    .collect();
                matches.sort();
                for p in matches {
                    // Skip already-listed recents to avoid duplicates.
                    if out.iter().any(|s| s.path == p) {
                        continue;
                    }
                    out.push(DirSuggestion {
                        display: tilde_path(&p, home.as_deref()),
                        path: p,
                        kind: SuggestionKind::Filesystem,
                    });
                    if out.len() >= MAX_SUGGESTIONS {
                        break;
                    }
                }
            }
        }

        self.suggestions = out;
        // Clamp the previous selection so it stays valid after the list shrinks.
        if let Some(i) = self.selected {
            if i >= self.suggestions.len() {
                self.selected = if self.suggestions.is_empty() {
                    None
                } else {
                    Some(self.suggestions.len() - 1)
                };
            }
        }
        // Re-clamp scroll in case the filtered list got shorter.
        let max_scroll = self.suggestions.len().saturating_sub(MAX_VISIBLE);
        if self.scroll > max_scroll {
            self.scroll = max_scroll;
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Word-boundary classifier for Ctrl+Left/Right and Ctrl+Backspace.
/// Path separators count as boundaries so jumping through `~/work/api/src`
/// hops one segment at a time instead of treating the whole string as a
/// single word.
fn is_word_boundary(c: char) -> bool {
    c.is_whitespace() || matches!(c, '/' | '.' | '-' | '_' | '~')
}

/// Byte offset of the start of the char ending at `byte_idx`. Handles
/// multi-byte UTF-8 by walking from the cursor backwards.
fn prev_char_boundary(s: &str, byte_idx: usize) -> usize {
    s[..byte_idx]
        .char_indices()
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Byte offset just after the char starting at `byte_idx`. Returns
/// `s.len()` when already at the end. Mirrors `prev_char_boundary` for
/// forward edits (Delete key, word-right delete).
fn next_char_boundary(s: &str, byte_idx: usize) -> usize {
    s[byte_idx..]
        .chars()
        .next()
        .map(|c| byte_idx + c.len_utf8())
        .unwrap_or(byte_idx)
}

fn looks_like_path(s: &str) -> bool {
    s.starts_with('/')
        || s.starts_with('~')
        || s.starts_with("./")
        || s.starts_with("../")
        || s == "."
        || s == ".."
        || s.contains('/')
}

/// Tilde-relative display: `~/foo` instead of `/home/user/foo`.
pub fn tilde_path(p: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home {
        if let Ok(rel) = p.strip_prefix(home) {
            if rel.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rel.to_string_lossy());
        }
    }
    p.to_string_lossy().to_string()
}

/// Split the typed text into (parent directory to scan, prefix to match).
/// `~/work/api` → (`~/work`, `api`).
/// `../foo` → (`<cwd>/..`, `foo`).
fn split_for_completion(text: &str, home: Option<&Path>) -> (PathBuf, String) {
    // Expand the parent portion via `resolve_path`. Anything after the last
    // `/` is treated as the in-progress prefix and not resolved.
    if let Some(slash) = text.rfind('/') {
        let parent_raw = &text[..=slash];
        let prefix = text[slash + 1..].to_string();
        let parent = if parent_raw == "~" || parent_raw == "~/" {
            home.map(|h| h.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("/"))
        } else if let Some(rest) = parent_raw.strip_prefix("~/") {
            home.map(|h| h.join(rest))
                .unwrap_or_else(|| PathBuf::from(parent_raw))
        } else {
            PathBuf::from(parent_raw)
        };
        (parent, prefix)
    } else {
        // No slash → match in the current directory.
        (PathBuf::from("."), text.to_string())
    }
}

/// Resolve `text` to an absolute path:
///  - `~` / `~/...`        → $HOME / $HOME/...
///  - `./...` / `../...`   → relative to `cwd`
///  - `/...`               → kept as-is
///  - bare name (`foo`)    → `cwd/foo`
///
/// `..` and `.` segments inside are collapsed lexically — we don't call
/// `canonicalize` because the target might not exist yet.
pub fn resolve_path(text: &str, cwd: &Path) -> PathBuf {
    let home = home_dir();
    let base = if text == "~" {
        return home.unwrap_or_else(|| PathBuf::from(text));
    } else if let Some(rest) = text.strip_prefix("~/") {
        home.map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(text))
    } else if text.starts_with('/') {
        PathBuf::from(text)
    } else {
        cwd.join(text)
    };

    let mut out = PathBuf::new();
    for comp in base.components() {
        use std::path::Component;
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_handles_tilde_and_relatives() {
        std::env::set_var("HOME", "/home/user");
        assert_eq!(
            resolve_path("~", Path::new("/cwd")),
            PathBuf::from("/home/user")
        );
        assert_eq!(
            resolve_path("~/foo", Path::new("/cwd")),
            PathBuf::from("/home/user/foo")
        );
        assert_eq!(
            resolve_path("/abs/path", Path::new("/cwd")),
            PathBuf::from("/abs/path")
        );
        assert_eq!(
            resolve_path("./foo", Path::new("/cwd")),
            PathBuf::from("/cwd/foo")
        );
        assert_eq!(
            resolve_path("../foo", Path::new("/cwd/sub")),
            PathBuf::from("/cwd/foo")
        );
        assert_eq!(
            resolve_path("foo", Path::new("/cwd")),
            PathBuf::from("/cwd/foo")
        );
    }

    #[test]
    fn looks_like_path_detects_path_inputs() {
        assert!(looks_like_path("~/foo"));
        assert!(looks_like_path("/abs"));
        assert!(looks_like_path("./rel"));
        assert!(looks_like_path("../rel"));
        assert!(looks_like_path("a/b"));
        assert!(!looks_like_path("project"));
        assert!(!looks_like_path(""));
    }

    #[test]
    fn split_for_completion_handles_tilde() {
        std::env::set_var("HOME", "/home/user");
        let home = home_dir();
        let (parent, prefix) = split_for_completion("~/work/ap", home.as_deref());
        assert_eq!(parent, PathBuf::from("/home/user/work"));
        assert_eq!(prefix, "ap");

        let (parent, prefix) = split_for_completion("foo", home.as_deref());
        assert_eq!(parent, PathBuf::from("."));
        assert_eq!(prefix, "foo");
    }

    #[test]
    fn insert_and_backspace_keep_cursor_consistent() {
        let mut s = DirInputState::default();
        s.open(&[]);
        s.insert_char('h', &[]);
        s.insert_char('e', &[]);
        s.insert_char('l', &[]);
        assert_eq!(s.text, "hel");
        assert_eq!(s.cursor, 3);
        s.backspace(&[]);
        assert_eq!(s.text, "he");
        assert_eq!(s.cursor, 2);
    }
}
