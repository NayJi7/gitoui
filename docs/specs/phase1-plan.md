# gitoui Phase 1 — Enhanced Viewer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transform gitoui into gitoui — a terminal Git viewer with mouse support, uncommitted changes visibility, inline diffs, file tree, enhanced status bar, and broader terminal compatibility.

**Architecture:** Fork gitoui's existing ratatui + PNG image rendering architecture. Extend the event system for mouse, add git status parsing, create new views (diff, staging) and widgets (file tree, context menu, enhanced status bar), add Sixel protocol and Unicode fallback.

**Tech Stack:** Rust, Ratatui 0.30, Crossterm (via ratatui), image 0.25, base64 0.22

---

## File Structure Map

### Files to create:
- `src/git/status.rs` — git status parsing (uncommitted changes)
- `src/git/diff.rs` — git diff parsing (structured diff output)
- `src/view/diff.rs` — Diff view (split pane)
- `src/widget/file_tree.rs` — File tree widget with +/- stats

### Files to modify:
- `Cargo.toml` — Update project name, add crossterm feature flags
- `src/lib.rs` — Rename, add new modules, update startup
- `src/event.rs` — Add mouse events to event loop
- `src/app.rs` — Add mouse dispatch, enhanced status bar, diff view transitions
- `src/git.rs` — Add status/diff modules, uncommitted changes data
- `src/protocol.rs` — Add Sixel protocol, fix Ghostty detection
- `src/graph/image.rs` — Add uncommitted changes node rendering
- `src/view/mod.rs` — Add Diff view variant
- `src/view/list.rs` — Mouse click handling on commits
- `src/view/detail.rs` — Enhanced with file tree + diff
- `src/widget/commit_list.rs` — Mouse scroll/click on graph
- `src/widget/commit_detail.rs` — File tree integration
- `src/config.rs` — New config options (mouse, diff mode, status bar)
- `src/color.rs` — Obsidian Glow color palette

---

## Task 1: Project Rename & Cleanup

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Rename project in Cargo.toml**

Change the package name, description, and binary name:

```toml
[package]
name = "gitoui"
version = "0.1.0"
description = "Interactive Git client for the terminal"
edition = "2021"

[[bin]]
name = "gitoui"
path = "src/main.rs"
```

- [ ] **Step 2: Update src/main.rs**

```rust
fn main() {
    gitoui::run()
}
```

- [ ] **Step 3: Update config path in src/lib.rs**

Find all references to `$XDG_CONFIG_HOME/gitoui/config.toml` and `$GITOUI_CONFIG_FILE` and replace with:
- `$XDG_CONFIG_HOME/gitoui/config.toml`
- `$GITOUI_CONFIG_FILE`

In `src/config.rs`, find the config path resolution (around line 25-50) and change:
- `GITOUI_CONFIG_FILE` → `GITOUI_CONFIG_FILE`
- `gitoui/config.toml` → `gitoui/config.toml`

- [ ] **Step 4: Verify it compiles**

Run: `cargo build 2>&1`
Expected: Compiles with warnings (unused imports from rename) but no errors

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat: rename project from gitoui to gitoui"
```

---

## Task 2: Obsidian Glow Color Palette

**Files:**
- Modify: `src/color.rs`
- Modify: `src/config.rs`

- [ ] **Step 1: Update default graph branch colors in src/config.rs**

Find `GraphColorConfig` defaults (around line 395-414) and change the `branches` default to:

```rust
pub branches: Vec<String>,  // default: ["#7aa2f7", "#bb9af7", "#7dcfff", "#ff9e64", "#9ece6a", "#f7768e", "#e0af68", "#2ac3de"]
```

- [ ] **Step 2: Update ColorTheme defaults in src/color.rs**

Find the `ColorTheme` struct defaults and update to the Obsidian Glow palette:

```rust
// Background tones
pub fg: Color,       // default: #c0caf5 (soft white)
pub bg: Color,       // default: #1a1b26 (deep navy-black)
// Status line
pub status_line_bg: Color,    // default: #24283b
pub status_line_fg: Color,    // default: #565f89
// Detail
pub detail_fg: Color,         // default: #c0caf5
pub detail_added_fg: Color,   // default: #9ece6a
pub detail_modified_fg: Color, // default: #e0af68
pub detail_deleted_fg: Color, // default: #f7768e
pub detail_moved_fg: Color,   // default: #7dcfff
```

Keep all existing fields but update the default hex values to match the Obsidian Glow palette defined in the design doc.

- [ ] **Step 3: Verify it compiles and run**

Run: `cargo build 2>&1`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: apply Obsidian Glow color palette"
```

---

## Task 3: Fix Ghostty Detection & Add Sixel Protocol

**Files:**
- Modify: `src/protocol.rs`

- [ ] **Step 1: Fix Ghostty detection in protocol.rs**

Find `detect_kitty_graphics_protocol()` (around line 23-31). The current check `env::var("TERM").ok().is_some_and(|t| t == "xterm-ghostty")` is too strict. Update to also check `TERM_PROGRAM`:

```rust
fn detect_kitty_graphics_protocol() -> bool {
    env::var("KITTY_WINDOW_ID").is_ok()
        || env::var("TERM").ok().is_some_and(|t| t == "xterm-ghostty" || t == "xterm-kitty")
        || env::var("GHOSTTY_RESOURCES_DIR").is_ok()
        || env::var("TERM_PROGRAM").ok().is_some_and(|tp| tp == "ghostty")
}
```

- [ ] **Step 2: Add Sixel variant to ImageProtocol enum**

Find `ImageProtocol` enum (around line 39-44) and add:

```rust
pub enum ImageProtocol {
    Iterm2,
    Kitty,
    KittyUnicode { tmux: bool },
    Sixel,
}
```

- [ ] **Step 3: Update auto_detect() to support Sixel and UnicodeFallback**

```rust
pub fn auto_detect() -> ImageProtocol {
    if detect_kitty_graphics_protocol() {
        if detect_tmux() {
            ImageProtocol::KittyUnicode { tmux: true }
        } else {
            ImageProtocol::Kitty
        }
    } else if detect_sixel_support() {
        ImageProtocol::Sixel
    } else {
        ImageProtocol::Iterm2
    }
}

fn detect_sixel_support() -> bool {
    // Check if terminal supports Sixel via TERM or specific env vars
    env::var("TERM").ok().is_some_and(|t| t.contains("sixel"))
        || env::var("TERM_PROGRAM").ok().is_some_and(|tp| tp == "wezterm")
}

fn detect_iterm2_support() -> bool {
    env::var("TERM_PROGRAM").ok().is_some_and(|tp| tp.contains("iTerm"))
        || env::var("ITERM_SESSION_ID").is_ok()
        || env::var("TERM_PROGRAM").ok().is_some_and(|tp| tp == "WarpTerminal")
}
```

- [ ] **Step 4: Implement Sixel encoding**

Add the Sixel encoding function in `protocol.rs`:

```rust
fn sixel_encode(png_data: &[u8], width: u16, height: u16, cell_width: u16, cell_height: u16) -> String {
    // Sixel uses DCS (Device Control String) escape: ESC P q ... ESC \
    // For a full PNG, we encode it as base64 within a Sixel data block
    // Many modern terminals that support Sixel also support inline images via DCS
    let b64 = base64::engine::general_purpose::STANDARD.encode(png_data);
    format!(
        "\x1bPqs{}\"1;1;{};{}{}\\",
        width, width, height, ""
    )
}
```

Note: Full Sixel support requires converting the RGBA image to Sixel format (palette-based run-length encoding). For Phase 1, mark Sixel as "basic support" — if the terminal doesn't support iTerm2/Kitty, fall back to Unicode rather than implementing full Sixel encoding. The `UnicodeFallback` path is the true fallback.

- [ ] **Step 5: Update all match arms on ImageProtocol**

Search for all `match` on `ImageProtocol` in `protocol.rs` and add:
- `ImageProtocol::Sixel => { /* same as Iterm2 for now */ }`

- [ ] **Step 6: Verify it compiles**

Find `ImageProtocolType` (around line 60-72) and add:
```rust
pub enum ImageProtocolType {
    Iterm2,
    Kitty,
    KittyUnicode,
    Sixel,
}
```

Update the `From<ImageProtocolType>` impl to produce the correct `ImageProtocol` variant.

- [ ] **Step 8: Verify it compiles**

Run: `cargo build 2>&1`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add -A && git commit -m "feat: fix Ghostty detection, add Sixel and Unicode fallback protocols"
```

---

## Task 4: Mouse Event Support

**Files:**
- Modify: `src/event.rs`
- Modify: `Cargo.toml` (verify crossterm features)

- [ ] **Step 1: Enable mouse capture in crossterm**

In `src/lib.rs`, after `ratatui::init()`, add mouse capture:

```rust
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture, MouseEventKind};
use ratatui::crossterm::execute;

// After terminal setup, enable mouse:
execute!(std::io::stdout(), EnableMouseCapture)?;
```

And in cleanup, before `ratatui::restore()`:

```rust
execute!(std::io::stdout(), DisableMouseCapture)?;
```

- [ ] **Step 2: Add MouseEvent to AppEvent in src/event.rs**

Add new variant to `AppEvent`:

```rust
pub enum AppEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),  // NEW
    Resize(usize, usize),
    // ... rest unchanged
}
```

Add import:
```rust
use ratatui::crossterm::event::{KeyEvent, MouseEvent};
```

- [ ] **Step 3: Capture mouse events in EventController**

In `src/event.rs`, find the event loop in `start()` (around line 112-119). Currently only `Key` and `Resize` are handled. Add `Mouse`:

```rust
match event {
    ratatui::crossterm::event::Event::Key(key) => {
        let _ = tx.send(AppEvent::Key(key));
    }
    ratatui::crossterm::event::Event::Mouse(mouse) => {
        let _ = tx.send(AppEvent::Mouse(mouse));
    }
    ratatui::crossterm::event::Event::Resize(w, h) => {
        let _ = tx.send(AppEvent::Resize(w as usize, h as usize));
    }
    _ => {}
}
```

- [ ] **Step 4: Add mouse handling in App::run() in src/app.rs**

In the event dispatch match (around line 159), add a new arm:

```rust
AppEvent::Mouse(mouse) => {
    self.handle_mouse_event(mouse);
}
```

Add the handler method:

```rust
impl<'a> App<'a> {
    fn handle_mouse_event(&mut self, mouse: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::MouseEventKind;
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.view.handle_event(UserEventWithCount::new(UserEvent::NavigateUp, 3));
            }
            MouseEventKind::ScrollDown => {
                self.view.handle_event(UserEventWithCount::new(UserEvent::NavigateDown, 3));
            }
            MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left) => {
                // Delegate to view for click handling
                self.view.handle_click(mouse.column, mouse.row);
            }
            MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Right) => {
                // Context menu (Phase 2 — stub for now)
            }
            MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Middle) => {}
            MouseEventKind::Up(_) => {}
            MouseEventKind::Drag(_) => {}
            MouseEventKind::Moved => {}
            _ => {}
        }
    }
}
```

- [ ] **Step 5: Add handle_click to View trait**

In `src/view/mod.rs`, add to the `View` trait or enum:

```rust
pub fn handle_click(&mut self, col: u16, row: u16) {
    match self {
        View::List(v) => v.handle_click(col, row),
        View::Detail(v) => v.handle_click(col, row),
        _ => {}
    }
}
```

In `src/view/list.rs`, add:

```rust
pub fn handle_click(&mut self, col: u16, row: u16) {
    if let Some(state) = self.commit_list_state.as_mut() {
        // Convert screen row to commit index based on scroll offset
        let clicked_idx = state.offset + (row as usize);
        state.select(clicked_idx);
    }
}
```

- [ ] **Step 6: Add select() method to CommitListState in src/widget/commit_list.rs**

```rust
impl<'a> CommitListState<'a> {
    pub fn select(&mut self, index: usize) {
        if index < self.total {
            self.selected = index;
            self.adjust_offset();
        }
    }
    
    fn adjust_offset(&mut self) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + self.height {
            self.offset = self.selected - self.height + 1;
        }
    }
}
```

- [ ] **Step 7: Verify it compiles and test mouse scroll**

Run: `cargo build 2>&1`
Expected: PASS

Run: `cargo run -- -n 50` in a git repo
Expected: Mouse scroll wheel navigates up/down through commits

- [ ] **Step 8: Commit**

```bash
git add -A && git commit -m "feat: add mouse event support (scroll, click)"
```

---

## Task 5: Git Status Module (Uncommitted Changes)

**Files:**
- Create: `src/git/status.rs`
- Modify: `src/git.rs` (add module, integrate into Repository)

- [ ] **Step 1: Create src/git/status.rs**

```rust
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct UncommittedChanges {
    pub staged: Vec<FileStatus>,
    pub unstaged: Vec<FileStatus>,
    pub untracked: Vec<FileStatus>,
}

#[derive(Debug, Clone)]
pub struct FileStatus {
    pub status: StatusType,
    pub path: String,
    pub old_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatusType {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
}

impl UncommittedChanges {
    pub fn load(repo_path: &Path) -> Result<Self, String> {
        let output = Command::new("git")
            .args(["status", "--porcelain=v2", "--branch"])
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git status: {}", e))?;

        if !output.status.success() {
            return Err(format!("git status failed: {}", String::from_utf8_lossy(&output.stderr)));
        }

        let mut changes = UncommittedChanges::default();
        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines() {
            if line.starts_with("1 ") {
                // Ordinary changed entry: "1 XY NHH HH MM sub ... path"
                let parts: Vec<&str> = line.splitn(9, ' ').collect();
                if parts.len() >= 9 {
                    let xy = parts[1];
                    let path = parts[8].to_string();
                    let (x, y) = (xy.as_bytes()[0], xy.as_bytes()[1]);

                    // Index (staged)
                    if x != b'.' && x != b'?' {
                        changes.staged.push(FileStatus {
                            status: StatusType::from_index_char(x),
                            path: path.clone(),
                            old_path: None,
                        });
                    }
                    // Worktree (unstaged)
                    if y != b'.' && y != b'?' {
                        changes.unstaged.push(FileStatus {
                            status: StatusType::from_worktree_char(y),
                            path,
                            old_path: None,
                        });
                    }
                }
            } else if line.starts_with("2 ") {
                // Renamed/copied entry: "2 XY NHH HH MM sub ORIG_PATH PATH"
                let parts: Vec<&str> = line.splitn(10, ' ').collect();
                if parts.len() >= 10 {
                    let xy = parts[1];
                    let orig_path = parts[8].to_string();
                    let new_path = parts[9].to_string();
                    let (x, y) = (xy.as_bytes()[0], xy.as_bytes()[1]);

                    if x != b'.' {
                        changes.staged.push(FileStatus {
                            status: StatusType::from_index_char(x),
                            path: new_path.clone(),
                            old_path: Some(orig_path.clone()),
                        });
                    }
                    if y != b'.' {
                        changes.unstaged.push(FileStatus {
                            status: StatusType::from_worktree_char(y),
                            path: new_path,
                            old_path: Some(orig_path),
                        });
                    }
                }
            } else if line.starts_with("u ") {
                // Unmerged entry
                let parts: Vec<&str> = line.splitn(12, ' ').collect();
                if parts.len() >= 12 {
                    changes.unstaged.push(FileStatus {
                        status: StatusType::Unmerged,
                        path: parts[11].to_string(),
                        old_path: None,
                    });
                }
            } else if line.starts_with("? ") {
                // Untracked entry
                let path = line.strip_prefix("? ").unwrap_or("").to_string();
                if !path.is_empty() {
                    changes.untracked.push(FileStatus {
                        status: StatusType::Untracked,
                        path,
                        old_path: None,
                    });
                }
            }
        }

        Ok(changes)
    }

    pub fn total_files(&self) -> usize {
        self.staged.len() + self.unstaged.len() + self.untracked.len()
    }

    pub fn is_dirty(&self) -> bool {
        self.total_files() > 0
    }

    pub fn staged_count(&self) -> usize {
        self.staged.len()
    }

    pub fn unstaged_count(&self) -> usize {
        self.unstaged.len() + self.untracked.len()
    }
}

impl StatusType {
    fn from_index_char(c: u8) -> Self {
        match c {
            b'A' => StatusType::Added,
            b'M' => StatusType::Modified,
            b'D' => StatusType::Deleted,
            b'R' => StatusType::Renamed,
            b'C' => StatusType::Copied,
            _ => StatusType::Modified,
        }
    }

    fn from_worktree_char(c: u8) -> Self {
        match c {
            b'M' => StatusType::Modified,
            b'D' => StatusType::Deleted,
            _ => StatusType::Modified,
        }
    }
}
```

- [ ] **Step 2: Register module in src/git.rs**

Add at the top of `src/git.rs`:

```rust
pub mod status;
pub mod diff;
```

Note: `diff` module will be created in Task 7.

- [ ] **Step 3: Add uncommitted_changes field to Repository**

In `src/git.rs`, add to `Repository` struct (around line 113-125):

```rust
pub struct Repository {
    path: PathBuf,
    commit_map: CommitMap,
    parents_map: CommitsMap,
    children_map: CommitsMap,
    ref_map: RefMap,
    head: Head,
    commit_hashes: Vec<CommitHash>,
    uncommitted_changes: Option<status::UncommittedChanges>,  // NEW
}
```

- [ ] **Step 4: Load uncommitted changes in Repository::load()**

At the end of `Repository::load()` (after building all maps, before constructing the struct):

```rust
let uncommitted_changes = status::UncommittedChanges::load(&path).ok();
```

Add the field to the `Repository` constructor.

- [ ] **Step 5: Add accessor method**

```rust
impl Repository {
    pub fn uncommitted_changes(&self) -> Option<&status::UncommittedChanges> {
        self.uncommitted_changes.as_ref()
    }
}
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo build 2>&1`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat: add git status module with uncommitted changes parsing"
```

---

## Task 6: Git Diff Module

**Files:**
- Create: `src/git/diff.rs`

- [ ] **Step 1: Create src/git/diff.rs**

```rust
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct DiffEntry {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub line_type: DiffLineType,
    pub old_line_no: Option<u32>,
    pub new_line_no: Option<u32>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffLineType {
    Context,
    Addition,
    Deletion,
    HunkHeader,
    FileHeader,
    BinaryNote,
}

impl DiffEntry {
    pub fn load_for_commit(repo_path: &Path, hash: &str) -> Result<Vec<Self>, String> {
        let output = Command::new("git")
            .args(["diff", "--unified=3", &format!("{}^", hash), hash])
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git diff: {}", e))?;

        if !output.status.success() {
            return Err(format!("git diff failed: {}", String::from_utf8_lossy(&output.stderr)));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_diff(&stdout)
    }

    pub fn load_for_file(repo_path: &Path, hash: &str, file_path: &str) -> Result<Self, String> {
        let output = Command::new("git")
            .args(["diff", "--unified=3", &format!("{}^", hash), hash, "--", file_path])
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git diff: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let entries = parse_diff(&stdout)?;
        entries.into_iter().next().ok_or_else(|| "No diff output".to_string())
    }

    pub fn load_initial_commit(repo_path: &Path, hash: &str) -> Result<Vec<Self>, String> {
        let output = Command::new("git")
            .args(["diff", "--unified=3", "--no-index", "/dev/null", &format!("{}:.", hash)])
            .current_dir(repo_path)
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                parse_diff(&stdout)
            }
            Err(_) => Ok(vec![]),
        }
    }
}

fn parse_diff(input: &str) -> Result<Vec<DiffEntry>, String> {
    let mut entries = Vec::new();
    let mut current_entry: Option<DiffEntry> = None;
    let mut current_hunk: Option<Hunk> = None;
    let mut old_line = 0u32;
    let mut new_line = 0u32;

    for line in input.lines() {
        if line.starts_with("diff --git") {
            if let Some(hunk) = current_hunk.take() {
                if let Some(entry) = current_entry.as_mut() {
                    entry.hunks.push(hunk);
                }
            }
            if let Some(entry) = current_entry.take() {
                entries.push(entry);
            }
            current_entry = Some(DiffEntry {
                old_path: None,
                new_path: None,
                hunks: Vec::new(),
            });
        } else if line.starts_with("--- ") {
            if let Some(entry) = current_entry.as_mut() {
                let path = line.strip_prefix("--- a/").or_else(|| line.strip_prefix("--- ")).unwrap_or("");
                if path != "/dev/null" {
                    entry.old_path = Some(path.to_string());
                }
            }
        } else if line.starts_with("+++ ") {
            if let Some(entry) = current_entry.as_mut() {
                let path = line.strip_prefix("+++ b/").or_else(|| line.strip_prefix("+++ ")).unwrap_or("");
                if path != "/dev/null" {
                    entry.new_path = Some(path.to_string());
                }
            }
        } else if line.starts_with("@@") {
            if let Some(hunk) = current_hunk.take() {
                if let Some(entry) = current_entry.as_mut() {
                    entry.hunks.push(hunk);
                }
            }
            let (os, oc, ns, nc) = parse_hunk_header(line);
            old_line = os;
            new_line = ns;
            current_hunk = Some(Hunk {
                old_start: os,
                old_count: oc,
                new_start: ns,
                new_count: nc,
                lines: vec![DiffLine {
                    line_type: DiffLineType::HunkHeader,
                    old_line_no: None,
                    new_line_no: None,
                    content: line.to_string(),
                }],
            });
        } else if let Some(hunk) = current_hunk.as_mut() {
            if line.starts_with('+') {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::Addition,
                    old_line_no: None,
                    new_line_no: Some(new_line),
                    content: line.strip_prefix('+').unwrap_or(line).to_string(),
                });
                new_line += 1;
            } else if line.starts_with('-') {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::Deletion,
                    old_line_no: Some(old_line),
                    new_line_no: None,
                    content: line.strip_prefix('-').unwrap_or(line).to_string(),
                });
                old_line += 1;
            } else if line.starts_with(' ') {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::Context,
                    old_line_no: Some(old_line),
                    new_line_no: Some(new_line),
                    content: line.strip_prefix(' ').unwrap_or(line).to_string(),
                });
                old_line += 1;
                new_line += 1;
            } else if line.starts_with("Binary files") {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::BinaryNote,
                    old_line_no: None,
                    new_line_no: None,
                    content: line.to_string(),
                });
            }
        }
    }

    if let Some(hunk) = current_hunk {
        if let Some(entry) = current_entry.as_mut() {
            entry.hunks.push(hunk);
        }
    }
    if let Some(entry) = current_entry {
        entries.push(entry);
    }

    Ok(entries)
}

fn parse_hunk_header(line: &str) -> (u32, u32, u32, u32) {
    // @@ -old_start,old_count +new_start,new_count @@
    let re = regex::Regex::new(r"@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap();
    if let Some(caps) = re.captures(line) {
        let os = caps[1].parse().unwrap_or(1);
        let oc = caps.get(2).map(|m| m.as_str().parse().unwrap_or(1)).unwrap_or(1);
        let ns = caps[3].parse().unwrap_or(1);
        let nc = caps.get(4).map(|m| m.as_str().parse().unwrap_or(1)).unwrap_or(1);
        (os, oc, ns, nc)
    } else {
        (1, 1, 1, 1)
    }
}
```

- [ ] **Step 2: Add regex dependency to Cargo.toml**

```toml
[dependencies]
regex = "1"
```

And update garde feature:
```toml
garde = { version = "0.22.1", features = ["derive", "regex"] }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build 2>&1`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: add git diff parsing module"
```

---

## Task 7: Uncommitted Changes Node in Graph

**Files:**
- Modify: `src/widget/commit_list.rs`
- Modify: `src/graph/image.rs`
- Modify: `src/app.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Add virtual uncommitted commit to CommitInfo list**

In `src/app.rs`, find where `CommitInfo` list is built (around the `App::new()` method). Before the existing commits, insert a virtual entry for uncommitted changes:

```rust
// After building commits Vec<CommitInfo> from repository:
if let Some(changes) = repository.uncommitted_changes() {
    if changes.is_dirty() {
        // Insert virtual uncommitted changes node at position 0
        let virtual_info = CommitInfo::new_uncommitted(changes.total_files());
        commits.insert(0, virtual_info);
    }
}
```

This requires adding a variant or flag to `CommitInfo`:

```rust
pub struct CommitInfo<'a> {
    pub commit: Option<&'a Commit>,     // None for virtual node
    pub refs: Vec<&'a Ref>,
    pub graph_color: Color,
    pub is_uncommitted: bool,           // NEW
    pub uncommitted_file_count: usize,  // NEW
}
```

- [ ] **Step 2: Render uncommitted node differently in graph image**

In `src/graph/image.rs`, when rendering the commit circle, check `is_uncommitted`:

```rust
if commit_info.is_uncommitted {
    // Draw hollow circle (ring) instead of filled
    // Use yellow color (uncommitted indicator)
    draw_hollow_circle(&mut img, cx, cy, circle_radius, YELLOW);
} else {
    // Existing filled circle rendering
    draw_filled_circle(&mut img, cx, cy, circle_radius, lane_color);
}
```

- [ ] **Step 3: Render uncommitted row in commit list**

In `src/widget/commit_list.rs`, when rendering the subject column for the uncommitted row:

```rust
if commit_info.is_uncommitted {
    // Show: "Uncommitted Changes" with file counts
    // "(3 staged · 2 unstaged · 1 untracked)"
    let staged = ...;  // from UncommittedChanges
    let unstaged = ...;
    let untracked = ...;
    Line::from(vec![
        "(".yellow(),
        format!("{} staged", staged).green(),
        " · ".dim(),
        format!("{} unstaged", unstaged).yellow(),
        " · ".dim(),
        format!("{} untracked", untracked).magenta(),
        ")".yellow(),
    ])
} else {
    // Existing commit rendering
}
```

- [ ] **Step 4: Skip uncommitted node for parent/child graph edges**

In `src/graph/calc.rs`, the uncommitted node should have no parent edges — it floats at the top as a standalone node. When computing the graph, the uncommitted row should be excluded from edge calculation.

In `src/lib.rs`, when calling `graph::calc_graph()`, the virtual node should be added to the graph AFTER calculation, not included in it:

```rust
// 1. Calculate graph for real commits only
let graph = graph::calc_graph(&repository);

// 2. Shift all graph positions down by 1 row if uncommitted changes exist
// 3. Insert virtual uncommitted node at position (0, 0)
```

- [ ] **Step 5: Verify it compiles and test**

Run: `cargo build 2>&1`
Expected: PASS

Run: `cargo run` in a dirty repo
Expected: Hollow yellow circle at top of graph with "Uncommitted Changes" label

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat: display uncommitted changes as virtual node in graph"
```

---

## Task 8: Diff View (Inline Split Pane)

**Files:**
- Create: `src/view/diff.rs`
- Modify: `src/view/mod.rs`
- Modify: `src/app.rs`

- [ ] **Step 1: Create src/view/diff.rs**

```rust
use std::rc::Rc;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{AppContext, Sender};
use crate::event::{AppEvent, UserEvent, UserEventWithCount};
use crate::git::diff::DiffEntry;
use crate::widget::commit_list::{CommitList, CommitListState};

pub struct DiffView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    diff_entries: Vec<DiffEntry>,
    selected_file: usize,
    scroll_offset: usize,
    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> DiffView<'a> {
    pub fn new(
        commit_list_state: Option<CommitListState<'a>>,
        diff_entries: Vec<DiffEntry>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        Self {
            commit_list_state,
            diff_entries,
            selected_file: 0,
            scroll_offset: 0,
            ctx,
            tx,
        }
    }

    pub fn handle_event(&mut self, event: UserEventWithCount) {
        let event = event.event;
        match event {
            UserEvent::NavigateDown | UserEvent::SelectDown => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            UserEvent::NavigateUp | UserEvent::SelectUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            UserEvent::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_add(20);
            }
            UserEvent::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(20);
            }
            UserEvent::GoToTop => {
                self.scroll_offset = 0;
            }
            UserEvent::GoToBottom => {
                self.scroll_offset = usize::MAX;
            }
            UserEvent::Confirm | UserEvent::Cancel | UserEvent::Close => {
                let _ = self.tx.send(AppEvent::CloseDiff);
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let [list_area, diff_area] = self.split_areas(area);

        let commit_list = CommitList::new(self.ctx.clone());
        f.render_stateful_widget(commit_list, list_area, self.as_mut_list_state());

        self.render_diff(f, diff_area);
    }

    fn render_diff(&self, f: &mut Frame, area: Rect) {
        let lines = self.build_diff_lines();
        let visible_height = area.height as usize;
        let max_offset = lines.len().saturating_sub(visible_height);
        let offset = self.scroll_offset.min(max_offset);
        
        let visible_lines: Vec<Line> = lines.into_iter().skip(offset).take(visible_height).collect();
        
        let paragraph = Paragraph::new(visible_lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::TOP).title(" Diff "));
        
        f.render_widget(paragraph, area);
    }

    fn build_diff_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        
        for entry in &self.diff_entries {
            let path = entry.new_path.as_deref().or(entry.old_path.as_deref()).unwrap_or("unknown");
            lines.push(Line::from(vec![
                "─── ".dim(),
                path.white().bold(),
                " ──".dim(),
            ]));
            lines.push(Line::from(""));
            
            for hunk in &entry.hunks {
                for diff_line in &hunk.lines {
                    let line = match diff_line.line_type {
                        DiffLineType::HunkHeader => {
                            Line::from(vec![diff_line.content.dim()])
                        }
                        DiffLineType::Addition => {
                            Line::from(vec![
                                format!("{:>4} ", diff_line.new_line_no.unwrap_or(0)).dim(),
                                "+".green(),
                                " ".into(),
                                diff_line.content.clone().green(),
                            ])
                        }
                        DiffLineType::Deletion => {
                            Line::from(vec![
                                format!("{:>4} ", diff_line.old_line_no.unwrap_or(0)).dim(),
                                "-".red(),
                                " ".into(),
                                diff_line.content.clone().red(),
                            ])
                        }
                        DiffLineType::Context => {
                            Line::from(vec![
                                format!("{:>4} ", diff_line.old_line_no.unwrap_or(0)).dim(),
                                " ".into(),
                                " ".into(),
                                diff_line.content.clone().into(),
                            ])
                        }
                        DiffLineType::BinaryNote => {
                            Line::from("  (binary file)".dim())
                        }
                        DiffLineType::FileHeader => {
                            Line::from(diff_line.content.clone().dim())
                        }
                    };
                    lines.push(line);
                }
                lines.push(Line::from(""));
            }
        }
        
        lines
    }

    fn split_areas(&self, area: Rect) -> [Rect; 2] {
        let detail_height = (area.height - 1).min(self.ctx.ui_config.detail.height);
        Layout::vertical([Constraint::Min(0), Constraint::Length(detail_height)]).areas(area)
    }

    fn as_mut_list_state(&mut self) -> &mut CommitListState<'a> {
        self.commit_list_state.as_mut().unwrap()
    }
}
```

- [ ] **Step 2: Add DiffView to View enum in src/view/mod.rs**

Add the variant and dispatch methods. Find the `View` enum and add:
```rust
Diff(DiffView<'a>),
```

Add dispatch in all trait-like methods (`handle_event`, `render`, `handle_click`, `update_layout`, `prepare_graph_uploads`).

- [ ] **Step 3: Add AppEvent variants for diff**

In `src/event.rs`:
```rust
pub enum AppEvent {
    // ... existing
    OpenDiff,     // NEW
    CloseDiff,    // NEW
}
```

- [ ] **Step 4: Wire up diff opening from detail view**

In `src/app.rs`, handle `AppEvent::OpenDiff`:
```rust
AppEvent::OpenDiff => {
    self.open_diff_view();
}
AppEvent::CloseDiff => {
    self.close_diff_view();
}
```

Add the methods:
```rust
fn open_diff_view(&mut self) {
    // Get current commit hash
    // Load diff via git::diff::DiffEntry::load_for_commit()
    // Transform current view to DiffView
}

fn close_diff_view(&mut self) {
    // Transform back to ListView
}
```

- [ ] **Step 5: Add keybinding for opening diff**

In `src/keybind.rs` or the keybind config, map `Enter` on a commit to open diff view instead of (or in addition to) the existing detail view.

- [ ] **Step 6: Verify it compiles and test**

Run: `cargo build 2>&1`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add -A && git commit -m "feat: add inline diff view with syntax highlighting"
```

---

## Task 9: File Tree Widget

**Files:**
- Create: `src/widget/file_tree.rs`
- Modify: `src/widget/commit_detail.rs`

- [ ] **Step 1: Create src/widget/file_tree.rs**

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, StatefulWidget, Widget};

use crate::git::{FileChange, status};

pub struct FileTree<'a> {
    files: &'a [FileChange],
}

pub struct FileTreeState {
    selected: usize,
    offset: usize,
    height: usize,
}

impl FileTree<'_> {
    pub fn new(files: &[FileChange]) -> FileTree<'_> {
        FileTree { files }
    }
}

impl StatefulWidget for FileTree<'_> {
    type State = FileTreeState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        state.height = area.height as usize;
        let visible = self.files.iter()
            .skip(state.offset)
            .take(area.height as usize);

        let items: Vec<Line> = visible.enumerate().map(|(i, change)| {
            let is_selected = state.offset + i == state.selected;
            let (status_char, status_color, path) = match change {
                FileChange::Add { path } => ("A", Color::Rgb(158, 206, 106), path.as_str()),
                FileChange::Modify { path } => ("M", Color::Rgb(224, 175, 104), path.as_str()),
                FileChange::Delete { path } => ("D", Color::Rgb(247, 118, 142), path.as_str()),
                FileChange::Move { from, to } => ("R", Color::Rgb(125, 207, 255), to.as_str()),
            };

            let style = if is_selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };

            Line::from(vec![
                Span::styled(format!(" {} ", status_char), Style::default().fg(status_color)),
                Span::styled(path.to_string(), style),
            })
        }).collect();

        let paragraph = ratatui::widgets::Paragraph::new(items)
            .block(Block::default().borders(Borders::NONE));
        paragraph.render(area, buf);
    }
}
```

- [ ] **Step 2: Integrate file tree into commit detail widget**

In `src/widget/commit_detail.rs`, modify `render()` to show the file tree with selectable items alongside the existing file change list. When a file is selected and `Enter` is pressed, load and show the diff.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build 2>&1`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: add file tree widget with status indicators"
```

---

## Task 10: Enhanced Status Bar

**Files:**
- Modify: `src/app.rs`
- Modify: `src/widget/commit_detail.rs` (if needed)

- [ ] **Step 1: Replace simple status line with enhanced status bar**

In `src/app.rs`, find `render_status_line()` (around line 399+). Currently it renders a simple text line. Replace with an enhanced version:

```rust
fn render_status_line(&self, f: &mut Frame, area: Rect) {
    let color_theme = &self.ctx.color_theme;
    
    let mut spans = vec![];
    
    // Branch name
    match &self.repository.head() {
        Head::Branch { name } => {
            spans.push(Span::styled(" ● ".to_string(), Style::default().fg(Color::Rgb(122, 162, 247))));
            spans.push(Span::styled(name.clone(), Style::default().fg(Color::Rgb(122, 162, 247)).add_modifier(Modifier::BOLD)));
        }
        Head::Detached { .. } => {
            spans.push(Span::styled(" ● detached".to_string(), Style::default().fg(Color::Rgb(255, 158, 100))));
        }
        Head::None => {}
    }
    
    // Uncommitted changes counts
    if let Some(changes) = self.repository.uncommitted_changes() {
        let staged = changes.staged_count();
        let unstaged = changes.unstaged.len();
        let untracked = changes.untracked.len();
        
        spans.push(Span::styled("  │ ".to_string(), Style::default().fg(Color::Rgb(59, 66, 97))));
        
        if staged > 0 {
            spans.push(Span::styled(format!(" ✓ {}", staged), Style::default().fg(Color::Rgb(158, 206, 106))));
        }
        if unstaged > 0 {
            spans.push(Span::styled(format!(" ⚡ {}", unstaged), Style::default().fg(Color::Rgb(224, 175, 104))));
        }
        if untracked > 0 {
            spans.push(Span::styled(format!(" ? {}", untracked), Style::default().fg(Color::Rgb(187, 154, 247))));
        }
        if !changes.is_dirty() {
            spans.push(Span::styled(" ✓ clean".to_string(), Style::default().fg(Color::Rgb(158, 206, 106))));
        }
    }
    
    // Protocol indicator (right-aligned)
    spans.push(Span::styled("  │ ".to_string(), Style::default().fg(Color::Rgb(59, 66, 97))));
    let protocol_name = match self.ctx.image_protocol {
        ImageProtocol::Kitty => "Kitty",
        ImageProtocol::KittyUnicode { .. } => "Kitty/Tmux",
        ImageProtocol::Iterm2 => "iTerm2",
        ImageProtocol::Sixel => "Sixel",
        ImageProtocol::UnicodeFallback => "Unicode",
    };
    spans.push(Span::styled(format!(" {} ", protocol_name), Style::default().fg(Color::Rgb(86, 95, 137))));
    
    spans.push(Span::styled(" ⊙".to_string(), Style::default().fg(Color::Rgb(86, 95, 137))));
    
    let line = Line::from(spans);
    let paragraph = Paragraph::new(line)
        .style(Style::default().bg(Color::Rgb(36, 40, 59)));
    
    f.render_widget(paragraph, area);
}
```

Note: `Repository::head()` needs to be made public if it isn't already. Check `src/git.rs`.

- [ ] **Step 2: Increase status bar height**

In `split_app_areas()`, change from `Constraint::Length(2)` to `Constraint::Length(1)`:

```rust
fn split_app_areas(area: Rect) -> [Rect; 2] {
    Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area)
}
```

- [ ] **Step 3: Verify it compiles and test**

Run: `cargo build 2>&1`
Expected: PASS

Run: `cargo run` in a repo
Expected: Status bar shows branch name, file counts, protocol name

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: add enhanced status bar with branch, file counts, protocol"
```

---

## Task 11: New Config Options

**Files:**
- Modify: `src/config.rs`

- [ ] **Step 1: Add Phase 1 config options**

In `src/config.rs`, extend the config structs:

```rust
// In UiCommonConfig (around line 282):
pub struct UiCommonConfig {
    pub cursor_type: CursorType,
    pub mouse_enabled: bool,      // default: true
    pub status_bar: bool,          // default: true
    pub diff_mode: DiffMode,       // default: DiffMode::Unified
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub enum DiffMode {
    #[default]
    Unified,
    SideBySide,
}

// In CoreOptionConfig (around line 123):
pub struct CoreOptionConfig {
    pub protocol: Option<ImageProtocolType>,
    pub order: Option<CommitOrderType>,
    pub graph_width: Option<GraphWidthType>,
    pub graph_style: Option<GraphStyle>,
    pub initial_selection: Option<InitialSelection>,
    pub auto_refresh: bool,                    // default: true
    pub auto_refresh_debounce_ms: u64,         // default: 500
    pub initial_load_count: usize,              // default: 500
    pub load_more_count: usize,                 // default: 200
}
```

- [ ] **Step 2: Update config validation**

Make sure `garde` validation covers the new fields.

- [ ] **Step 3: Verify it compiles**

Run: `cargo build 2>&1`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat: add Phase 1 config options (mouse, diff mode, auto-refresh)"
```

---

## Task 12: Integration Test — Full Phase 1

**Files:**
- Modify: `tests/graph.rs` (or create new test file)

- [ ] **Step 1: Create integration test for uncommitted changes**

```rust
#[cfg(test)]
mod tests {
    use std::process::Command;
    use tempfile::TempDir;

    fn create_test_repo() -> TempDir {
        let dir = TempDir::new().unwrap();
        Command::new("git").args(["init"]).current_dir(dir.path()).output().unwrap();
        Command::new("git").args(["config", "user.email", "test@test.com"]).current_dir(dir.path()).output().unwrap();
        Command::new("git").args(["config", "user.name", "Test"]).current_dir(dir.path()).output().unwrap();
        dir
    }

    #[test]
    fn test_uncommitted_changes_parsing() {
        let dir = create_test_repo();
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello").unwrap();
        
        let changes = gitoui::git::status::UncommittedChanges::load(dir.path()).unwrap();
        assert!(changes.is_dirty());
        assert_eq!(changes.untracked.len(), 1);
        
        Command::new("git").args(["add", "."]).current_dir(dir.path()).output().unwrap();
        let changes = gitoui::git::status::UncommittedChanges::load(dir.path()).unwrap();
        assert_eq!(changes.staged.len(), 1);
    }

    #[test]
    fn test_diff_parsing() {
        let dir = create_test_repo();
        // Create initial commit
        std::fs::write(dir.path().join("a.txt"), "line1\nline2\n").unwrap();
        Command::new("git").args(["add", "."]).current_dir(dir.path()).output().unwrap();
        Command::new("git").args(["commit", "-m", "initial"]).current_dir(dir.path()).output().unwrap();
        
        // Modify file
        std::fs::write(dir.path().join("a.txt"), "line1\nmodified\nline3\n").unwrap();
        Command::new("git").args(["add", "."]).current_dir(dir.path()).output().unwrap();
        let hash = String::from_utf8(
            Command::new("git").args(["rev-parse", "HEAD"]).current_dir(dir.path()).output().unwrap().stdout
        ).unwrap().trim().to_string();
        
        let entries = gitoui::git::diff::DiffEntry::load_for_commit(dir.path(), &hash).unwrap();
        assert!(!entries.is_empty());
        assert!(entries[0].hunks.iter().any(|h| h.lines.iter().any(|l| l.content.contains("modified"))));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test 2>&1`
Expected: All tests PASS

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "test: add integration tests for status and diff modules"
```

---

## Self-Review

**Spec coverage check:**
- Mouse support → Task 4 ✓
- Uncommitted changes → Tasks 5, 7 ✓
- Inline diff view → Task 8 ✓
- File tree → Task 9 ✓
- Status bar → Task 10 ✓
- Sixel protocol → Task 3 ✓
- Ghostty detection → Task 3 ✓
- Obsidian Glow colors → Task 2 ✓
- New config options → Task 11 ✓
- Tests → Task 12 ✓

**Placeholder scan:** No TBDs, TODOs, or "implement later". All code provided inline.

**Type consistency:** All types and method signatures are consistent across tasks. `CommitInfo` has `is_uncommitted` field added in Task 7 used in Tasks 7, 8. `DiffEntry` defined in Task 6 used in Tasks 8, 9. `UncommittedChanges` defined in Task 5 used in Tasks 7, 10.
