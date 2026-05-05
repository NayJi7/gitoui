# gitui — Design Document

**Date:** 2026-05-02  
**Status:** Draft  
**Base:** Fork of [serie](https://github.com/lusingander/serie) v0.8.0  
**Goal:** Terminal-based Git client with full Git Graph parity, pixel-perfect rendering

---

## 1. Vision

**gitui** is a fully interactive Git client for the terminal. It provides the same level of functionality as the Git Graph VS Code extension — visual commit graph, diffs, staging, branch/tag/stash operations, push/pull — rendered with pixel-perfect PNG graphics (Bezier curves, anti-aliased nodes, specular highlights) via Kitty/iTerm2/Sixel image protocols.

**Core principle:** Every phase ships a complete, usable tool. No half-features.

---

## 2. Architecture

```
src/
├── main.rs                  # CLI entry (clap)
├── lib.rs                   # Startup orchestration
├── app.rs                   # Main loop, event dispatch, view transitions
├── event.rs                 # Keyboard + mouse event system (crossterm)
├── git/
│   ├── mod.rs               # Repository abstraction
│   ├── read.rs              # git CLI wrappers (log, show, diff, status — read-only)
│   ├── write.rs             # git CLI wrappers (merge, rebase, checkout, push, etc.)
│   ├── diff.rs              # Diff parsing & structured representation
│   └── status.rs            # git status, staging area state
├── graph/
│   ├── calc.rs              # Commit graph layout algorithm (from serie)
│   ├── image.rs             # PNG generation per-row (from serie)
│   ├── protocol.rs          # Kitty / iTerm2 / Sixel encoding
│   └── fallback.rs          # Unicode fallback for unsupported terminals
├── view/
│   ├── mod.rs               # View enum, state machine, transitions
│   ├── graph.rs             # Main commit graph list (serie's list.rs)
│   ├── detail.rs            # Commit detail split pane (enhanced)
│   ├── diff.rs              # Diff viewer (unified + side-by-side)
│   ├── staging.rs           # Staging area view
│   ├── refs.rs              # Branches/tags/remotes browser (from serie)
│   ├── help.rs              # Keybinding reference overlay (from serie)
│   └── dialog.rs            # Confirmation & input dialogs
├── widget/
│   ├── commit_list.rs       # Graph image + commit list (from serie)
│   ├── commit_detail.rs     # Commit info panel (enhanced)
│   ├── diff_view.rs         # Unified/side-by-side diff rendering
│   ├── file_tree.rs         # File tree with +/- stats per file
│   ├── staging_list.rs      # Staged / unstaged / untracked file lists
│   ├── context_menu.rs      # Right-click action menu
│   ├── find.rs              # Search/filter widget
│   └── status_bar.rs        # Bottom status bar
├── config.rs                # TOML config with defaults + validation
├── keybind.rs               # TOML keybinding system (from serie)
├── color.rs                 # Theme + color palette (from serie)
└── external.rs              # Clipboard, editor launch, shell commands
```

### Key architectural decisions

1. **Git via CLI subprocess** — Same approach as serie. No libgit2 binding. Simpler, no C dependency, works everywhere git is installed. Performance is adequate for repos up to ~50k commits.

2. **Image rendering for graph** — PNG per commit row, displayed via terminal image protocols. Gives pixel-perfect Bezier curves, anti-aliased circles, gradient highlights. Cannot be achieved with Unicode box-drawing characters.

3. **View state machine** — Same pattern as serie: `View` enum with variants for each screen. Transitions are explicit. `CommitListState` shared between views.

4. **Mouse via crossterm** — Crossterm natively captures `Event::Mouse` (scroll, click, drag). Add mouse event handling in `event.rs` alongside existing keyboard events.

5. **Protocol support matrix:**

   | Protocol | Terminals | Status |
   |----------|-----------|--------|
   | Kitty | Kitty, Ghostty | Primary |
   | iTerm2 | iTerm2, Warp | Supported |
   | Sixel | mlterm, xterm -ti 340 | New |
   | Unicode fallback | All others | Fallback |

---

## 3. Phases

### Phase 1 — Enhanced Viewer

*Ship a better serie with mouse support, diffs, and uncommitted changes visibility.*

| Feature | Detail |
|---------|--------|
| Mouse support | Scroll, click to select commit, click to resize panes, double-click to open detail |
| Uncommitted changes | Virtual commit node at top of graph showing staged/unstaged/untracked file counts |
| Inline diff view | Split pane: select commit → show diff (unified format) with scroll |
| File tree widget | Files changed per commit with add/delete stats, click to see diff |
| Status bar | Current branch, file counts (staged/unstaged/untracked), repo dirty/clean |
| Sixel protocol | Support Sixel for broader terminal coverage |
| Ghostty detection | Auto-detect Ghostty (check TERM, GHOSTTY_RESOURCES_DIR, fallback to Kitty protocol) |
| Unicode fallback | When no image protocol is available, fall back to box-drawing characters |

### Phase 2 — Basic Git Actions

*Act on the repository: branches, tags, stash, reset.*

| Feature | Detail |
|---------|--------|
| Context menu | Right-click (or `x` key) on commit/branch/tag → action menu |
| Checkout | Checkout branch, checkout commit (detached HEAD with confirmation) |
| Branch CRUD | Create branch at commit (optional checkout), delete (optional force), rename |
| Tag CRUD | Create annotated/lightweight, delete, push to remote |
| Stash CRUD | Stash (with --include-untracked option), apply, pop, drop, create branch from stash |
| Reset | Soft / Mixed / Hard reset to selected commit |
| Dialog system | Confirmation modals for destructive operations, input fields for branch/tag names |
| Refresh | Full reload after write operations, preserve view state |

### Phase 3 — Advanced Git Actions

*Full Git Graph parity for git operations.*

| Feature | Detail |
|---------|--------|
| Merge | Into current branch, options: --no-ff, --squash, --no-commit |
| Rebase | On branch/commit, --interactive launches $EDITOR in subshell |
| Cherry-pick | With --no-commit and record origin (-x) options |
| Push | To remote(s), --set-upstream, --force, --force-with-lease |
| Pull | From remote, --no-ff, --squash options |
| Fetch | From all or specific remote, --prune, --prune-tags |
| Remote management | Add, edit, delete, prune remotes |
| Commit comparison | Select 2 commits (Shift+click) → diff between them |
| Enhanced search | Search commit messages, hashes, authors, branch/tag names; regex support |
| Branch filter | Filter graph to show only selected branches |
| Auto-refresh | inotify/file watcher on .git directory, debounced refresh |

### Phase 4 — Polish

*Product-quality finish.*

| Feature | Detail |
|---------|--------|
| Side-by-side diff | Two-column diff view alongside unified |
| Code review tracking | Mark files as reviewed per commit range, persisted to disk |
| Issue linking | Configurable regex to turn issue references into highlighted text |
| Multi-repo | Detect repos in workspace, dropdown to switch |
| Config export | Share config in repo (.gitui/config.toml) |
| Performance | Lazy-load commits (initial batch + load-more on scroll) for large repos |

---

## 4. Input System

### Mouse

| Action | Effect |
|--------|--------|
| Scroll wheel | Navigate graph vertically |
| Left click | Select commit / select file / focus pane |
| Right click | Open context menu |
| Double click | Open detail view / open diff |
| Drag pane border | Resize split panes |
| Shift + click | Select second commit for comparison |

### Keyboard

All of serie's existing bindings preserved, plus:

| Key | Action |
|-----|--------|
| `Tab` | Cycle focus between panes (graph → detail → staging) |
| `Shift+Tab` | Cycle focus backwards |
| `s` | Stage selected file |
| `S` | Stage all files |
| `u` | Unstage selected file |
| `U` | Unstage all files |
| `c` | Commit (open inline editor or $EDITOR) |
| `x` | Open context menu on selected item |
| `Enter` | Open diff of selected file / confirm dialog |
| `Escape` | Close current pane / menu / dialog |
| `f` | Open find/filter widget |
| `b` | Open branch filter dropdown |
| `1-9` | Numeric prefix (vim-style, from serie) |

All bindings fully customizable via TOML config (from serie's keybind system).

---

## 5. UI Layout

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ gitui — my-project (main)                                       ? help  q quit │
├──────────────────────────────┬───────────────────────────────────────────────┤
│                              │ ● a1b2c3d (HEAD -> main, origin/main)        │
│   [PNG pixel-perfect         │ Alice • 2026-05-01                            │
│    graph rendering]          │                                               │
│       ●───●                  │ Merge pull request #42: Dashboard feature     │
│       │   ╰──●               │                                               │
│    ●──╯      │               │ Files (12):  +340  −28                        │
│    │         │               │ ├── src/app.rs           +89 −12             │
│    │         ●               │ ├── src/main.rs          +45 −3              │
│    │         │               │ └── Cargo.toml           +2 −1               │
│    ●─────────╯               │                                               │
│    │                         │ ─── diff: src/app.rs ──────────────────────  │
│    ●  tag: v0.1.0            │  42 │ fn old_function() {                    │
│                              │  42 │ -   return None;                       │
│                              │  43 │ +   let x = Some(42);                  │
│                              │  44 │ +   Some(x)                            │
│                              │  45 │ }                                       │
├──────────────────────────────┴───────────────────────────────────────────────┤
│  main ↑3 │ 3 staged │ 2 unstaged │ 1 untracked │ mouse:on │ Kitty protocol  │
└──────────────────────────────────────────────────────────────────────────────┘
```

### Pane structure

- **Left pane** (resizable): Commit graph + commit list. Always visible.
- **Right pane** (resizable): Contextual — shows one of:
  - Commit detail + file tree (default)
  - Diff view (when file selected)
  - Staging area (toggle with Tab)
  - Commit comparison (when 2 commits selected)
- **Status bar** (fixed, bottom): Branch, file counts, protocol, mouse state.
- **Overlays**: Help (`?`), Context menu (right-click/`x`), Find (`f`), Dialogs.

### Pane resizing

Vertical divider between left/right panes. Drag with mouse or use `H`/`L` keys to resize. Minimum widths enforced. Persisted in config.

---

## 6. Context Menu Structure

Right-clicking (or pressing `x`) shows different menus depending on what's selected:

### On a commit

```
┌─ Commit ─────────────┐
│ Checkout              │
│ Create Branch...      │
│ Add Tag...            │
│ Merge into current    │
│ Rebase on this commit │
│ Cherry-pick           │
│ Revert                │
│ Drop commit           │
│ Reset to here  →  ┌──┤
│                    │So│
│ Copy hash           │Mi│
│ Copy subject        │Ha│
│                    │rd│
│ View diff            └──┤
│ Compare with...        │
└────────────────────────┘
```

### On a branch ref

```
┌─ Branch: feature/auth ──┐
│ Checkout                  │
│ Merge into current        │
│ Rebase current on this    │
│ Push to remote...         │
│ Pull from remote...       │
│ Delete                    │
│ Rename                    │
└───────────────────────────┘
```

### On a tag ref

```
┌─ Tag: v1.0.0 ────────┐
│ View details           │
│ Push to remote         │
│ Delete                 │
│ Copy name              │
└────────────────────────┘
```

### On uncommitted changes node

```
┌─ Uncommitted Changes ─┐
│ Stage all               │
│ Unstage all             │
│ Discard all             │
│ Stash...                │
│ Commit                  │
└─────────────────────────┘
```

---

## 7. Staging Area View

Activated by pressing `Tab` to switch focus from graph to staging pane.

```
┌─ Staging Area ──────────────────────────────────┐
│                                                   │
│  Staged (3)                     s:stage  u:unstage│
│  ├── modified   src/app.rs         +89 −12       │
│  ├── modified   src/main.rs        +45 −3        │
│  └── new file   Cargo.lock         +823 −0       │
│                                                   │
│  Unstaged (2)                                     │
│  ├── modified   src/lib.rs          +12 −5       │
│  └── deleted    old_module.rs        +0 −67      │
│                                                   │
│  Untracked (1)                                    │
│  └── new        notes.txt                         │
│                                                   │
│  c:commit  S:stage-all  U:unstage-all  a:amend    │
└───────────────────────────────────────────────────┘
```

- `s` stages the selected file (`git add`)
- `u` unstages the selected file (`git restore --staged`)
- `c` opens inline commit editor (`git commit`)
- `Enter` on a file shows its diff
- Staged files shown with green indicator, unstaged with yellow, untracked with red

---

## 8. Configuration

TOML config at `$XDG_CONFIG_HOME/gitui/config.toml` or `$GITUI_CONFIG_FILE`.

All of serie's existing config options are preserved. New options:

```toml
[general]
# Show uncommitted changes as virtual node in graph
show_uncommitted_changes = true

# Auto-refresh on .git changes (inotify/polling)
auto_refresh = true
auto_refresh_debounce_ms = 500

# Default commit count to load (lazy loading)
initial_load_count = 500
load_more_count = 200

[ui]
# Enable mouse support
mouse_enabled = true

# Show status bar
status_bar = true

# Default pane split ratio (0.0-1.0, left pane proportion)
split_ratio = 0.35

# Diff view mode: "unified" or "side-by-side"
diff_mode = "unified"

# Show file tree in detail view (vs flat list)
file_tree_view = true

[dialogs]
# Pre-fill defaults for git operations
reset_mode = "mixed"           # soft / mixed / hard
merge_no_ff = true
stash_include_untracked = true
cherry_pick_no_commit = false
tag_type = "annotated"         # annotated / lightweight

[protocols]
# Force a specific image protocol: "auto", "kitty", "iterm2", "sixel", "unicode"
force_protocol = "auto"

[branch_colors]
# Customizable branch lane colors (up to 12) — Tokyo Night palette
colors = ["#7aa2f7", "#bb9af7", "#7dcfff", "#ff9e64", "#9ece6a", "#f7768e", "#e0af68", "#2ac3de"]

[theme]
# Built-in themes: "obsidian" (default), "dracula", "nord", "onedark", "catppuccin"
theme = "obsidian"

# Or override individual colors:
# bg = "#1a1b26"
# surface = "#24283b"
# border = "#3b4261"
# text_primary = "#c0caf5"
# text_secondary = "#565f89"

[keybind]
# All keybindings customizable (inherits serie's system)
# New defaults:
toggle_staging = "tab"
stage_file = "s"
stage_all = "S"
unstage_file = "u"
unstage_all = "U"
commit = "c"
context_menu = "x"
find = "f"
branch_filter = "b"
```

---

## 9. Visual Design Language

**Inspirations:** GitKraken (polish, dark theme, graph aesthetics), Git Graph (clean layout, ref labels), Lazygit (terminal-native UX patterns).

### 9.1 Color Palette — "Obsidian Glow"

Dark-first theme. The terminal background is assumed dark. Every color is chosen for maximum contrast and readability on dark backgrounds.

```
Background:     #1a1b26  (deep navy-black, Tokyo Night inspired)
Surface:        #24283b  (raised panels, dialogs)
Surface dim:    #1f2335  (inactive pane backgrounds)
Border:         #3b4261  (subtle, not aggressive)
Border active:  #7aa2f7  (blue glow on focused pane)

Text primary:   #c0caf5  (soft white, never pure #fff)
Text secondary: #565f89  (dimmed text, hashes, dates)
Text muted:     #3b4261  (truly muted, barely visible)

Accents (branch lanes):
  Lane 0:  #7aa2f7  (blue — main branch)
  Lane 1:  #bb9af7  (mauve)
  Lane 2:  #7dcfff  (cyan)
  Lane 3:  #ff9e64  (orange)
  Lane 4:  #9ece6a  (green)
  Lane 5:  #f7768e  (red/pink)
  Lane 6:  #e0af68  (gold)
  Lane 7:  #2ac3de  (teal)

Semantic colors:
  Added:     #9ece6a  (green — diff additions, staged)
  Deleted:   #f7768e  (red — diff deletions, conflicts)
  Modified:  #e0af68  (yellow — unstaged changes)
  Untracked: #bb9af7  (purple — new files)
  HEAD:      #7dcfff  (bright cyan)
  Tag:       #e0af68  (gold)
  Stash:     #ff9e64  (orange)
  Remote:    #565f89  (dimmed blue-gray)
```

### 9.2 Graph Rendering — Pixel-Perfect Details

The graph is the centerpiece. Every pixel matters.

**Commit nodes:**
- Size: 7px radius for normal commits, 9px for HEAD commit
- Fill: Solid color matching the branch lane
- Specular highlight: Brighter spot offset top-left (1.5x brighter, 2px radius), gives a 3D "orb" look like GitKraken
- Shadow: 1px darker ring around the node for depth
- HEAD node: Pulsing outer glow ring (3px, semi-transparent lane color)
- Uncommitted changes: Hollow circle (ring only, no fill), pulsing yellow, with a lightning bolt or `~` indicator
- Merge commits: Slightly larger (8px), with a subtle double-ring effect

**Branch lines (edges):**
- Width: 2px (not 1px — thicker = more visible, like GitKraken)
- Style: Smooth Bezier curves for merges (quadratic bezier with control point), never straight diagonal lines
- Color: Same as the branch lane color, full opacity
- Vertical lines: Perfectly straight, 2px wide, connecting nodes
- Merge curves: Elegant S-curve from child to parent lane, never sharp corners

**Edge case handling:**
- Lane crossings: One lane passes "behind" the other (interrupted line with 2px gap)
- Overlapping merges: Detour routing (like serie already does)

### 9.3 Ref Labels — Pill Badges

Ref labels (branch names, tags, HEAD) are drawn as colored pill-shaped badges next to the commit node, inspired by Git Graph's label styling.

**Rendering approach:**
- Badges are rendered as part of the PNG image (pixel-perfect rounded rectangles)
- Each badge: rounded rectangle with 3px corner radius, filled with a tinted background, text rendered in the lane color
- Badges are positioned to the right of the node, stacked vertically if multiple

**Badge styles:**
```
┌──────────────────────────────────────────────────┐
│                                                  │
│  ●─── [HEAD] [main] [origin/main]                │
│       ▀▀▀▀  ▀▀▀▀  ▀▀▀▀▀▀▀▀▀▀                   │
│       cyan   green  dim blue                      │
│       bold   bold   dim                           │
│                                                  │
│  ●─── [tag: v1.0.0] [tag: v0.9.0]               │
│       ▀▀▀▀▀▀▀▀▀▀    ▀▀▀▀▀▀▀▀▀▀                  │
│       gold           gold                         │
│                                                  │
│  ●─── [feature/auth] [origin/feature/auth]       │
│       ▀▀▀▀▀▀▀▀▀▀▀▀▀  ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀   │
│       mauve             dim                       │
│                                                  │
└──────────────────────────────────────────────────┘
```

- HEAD: Bright cyan pill, bold text, slight glow
- Current branch (main/master): Green pill, bold
- Other local branches: Lane color pill, semi-bold
- Remote branches: Dimmed, no pill fill (just outlined, muted text)
- Tags: Gold pill with a small tag icon (🏷 or ▲ marker)
- Stashes: Orange pill with a small stash icon

### 9.4 Commit List Row Design

Each commit row in the list has consistent visual structure:

```
  [graph PNG]  ● a1b2c3d  Merge pull request #42    Alice     2026-05-01
               │
               ├── Selected row: inverted background (#7aa2f7 at 15% opacity)
               ├── Hover row: subtle highlight (#7aa2f7 at 8% opacity)
               ├── Normal row: transparent background
               └── Merge commit: slightly indented subject, dimmed
```

**Columns (left to right):**
1. Graph image (PNG, fixed width ~140px)
2. Ref pills (variable width, rendered in PNG)
3. Commit hash (7 chars, dimmed monospace)
4. Subject (takes remaining space, primary text color)
5. Author (fixed width, blue, truncated if needed)
6. Date (fixed width, dimmed, configurable format)

**Selected commit styling:**
- Background highlight on the entire row
- Commit hash changes from dim to bright white
- Subject text becomes bold
- A subtle left-border accent (2px, lane color) appears on the left edge

### 9.5 Detail Panel — Card-Based Layout

The detail panel uses a card/section approach with clear visual hierarchy:

```
┌─────────────────────────────────────────────┐
│  ● a1b2c3d  ───────────────────────────     │
│  │ Merge pull request #42: Dashboard         │
│  │                                           │
│  │ Alice <alice@dev.com>                     │
│  │ 2026-05-01 14:32:07 +0200                 │
│  │                                           │
│  │ Parents: d5c4b3a  f7e8d9c                │
│  │ Refs: HEAD -> main, origin/main           │
│  │                                           │
│  │ Detailed commit message body goes here.   │
│  │ Multiple paragraphs are supported.        │
│  └───────────────────────────────────────────│
│                                              │
│  Files changed (12)  +340 −28                │
│  ┌──────────────────────────────────────────┐│
│  │ M  src/app.rs              +89 −12       ││
│  │ M  src/main.rs             +45 −3        ││
│  │ A  src/new_module.rs       +234 −0       ││
│  │ D  src/old.rs               +0 −67       ││
│  │ R  src/utils.rs → src/helpers.rs  +12 −5 ││
│  └──────────────────────────────────────────┘│
│                                              │
│  ─── diff: src/app.rs ────────────────────   │
│  42 │ fn old_function() {                    │
│  42 │ -   return None;                       │
│  43 │ +   let x = Some(42);                  │
│  44 │ +   Some(x)                            │
│  45 │ }                                      │
└──────────────────────────────────────────────┘
```

**Visual details:**
- Card has a rounded border (unicode ╭╮╯╰), `Surface` background color
- File status icons are color-coded: `M` yellow, `A` green, `D` red, `R` blue
- Additions/deletions stats use mini inline bar graph (colored proportional bar)
- Diff section uses standard diff coloring: red background for deletions, green for additions
- Line numbers in dim text, gutter separator in border color

### 9.6 Diff View Styling

```
─── a/src/app.rs ────────────────────────────────────────
   40 │                                         │
   41 │   pub fn process(data: &str) -> Result  │
   42 │ -   let parsed = old_parser(data)?;     │
     │ -   if parsed.is_empty() {               │
     │ -     return Err(Error::Empty);          │
     │ -   }                                    │
   42 │ +   let parsed = new_parser(data)?;     │
   43 │ +   let validated = validate(&parsed)?; │
   44 │ +   Ok(validated)                       │
   45 │   }                                      │
   46 │                                         │
─── a/src/main.rs ───────────────────────────────────────
```

**Diff coloring:**
- Removed lines: Background `#3b1d2b` (dark red tint), text in `#f7768e`
- Added lines: Background `#1d2b3b` (dark green tint), text in `#9ece6a`
- Context lines: Default text color on transparent background
- Line numbers: `Text muted` color, right-aligned, with a vertical separator line
- File header: Bold, with file path in primary text color, separator line in border color
- Hunk headers (`@@ -1,4 +1,5 @@`): `Text secondary`, italic

### 9.7 Context Menu Styling

Context menus use a floating overlay with a subtle shadow effect:

```
     ┌─────────────────────────────┐
  ╲  │  ▸ Checkout              ↵  │
   ╲ │    Create Branch...      b  │
    ╲│    Add Tag...            t  │
     │  ─────────────────────────  │
     │    Merge into current    m  │
     │    Rebase on this       r  │
     │    Cherry-pick          c  │
     │  ─────────────────────────  │
     │    Reset to here        ▸──│──► Soft
     │    Copy hash            ⇧c │──► Mixed
     │    Copy subject         ⇧s │──► Hard
     │  ─────────────────────────  │
     │    View diff            ↵  │
     └─────────────────────────────┘
```

**Visual details:**
- Background: `Surface` color with a 1px border in `Border active` color
- Top-left corner has a subtle drop shadow (darker character)
- Selected item: Inverted colors (bright background, dark text), rounded highlight
- Disabled items (unavailable actions): `Text muted` color, no highlight
- Keyboard shortcuts shown right-aligned in `Text secondary` color
- Submenus indicated with `▸`, expand on hover/right arrow
- Dividers between action groups: thin line in `Border` color

### 9.8 Dialog / Confirmation Modal

```
          ┌─────────────────────────────────────────┐
          │                                          │
          │   ⚠  Reset to a1b2c3d?                  │
          │                                          │
          │   This will reset your current HEAD,     │
          │   index, and working tree to this commit. │
          │   Uncommitted changes will be lost.      │
          │                                          │
          │   Mode:  ○ Soft  ● Mixed  ○ Hard        │
          │                                          │
          │        [ Cancel ]    [ Confirm ]         │
          │        ─────────    ──────────           │
          │         dim white    red bg, white text   │
          └─────────────────────────────────────────┘
```

**Visual details:**
- Centered on screen with a semi-transparent overlay behind (dim the rest of the UI)
- Warning icon `⚠` in orange for destructive operations
- Radio buttons: Filled circle `●` for selected, empty `○` for others, in accent color
- Cancel button: Dim border, subtle text
- Confirm button: Filled red background for destructive, filled blue for safe operations
- Focus indicator on buttons: Brighter border, slightly raised

### 9.9 Status Bar

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  ● main ↑3  │  ✓ 3 staged  │  ⚡ 2 unstaged  │  ? 1 untracked  │  Kitty  ⊙  │
└──────────────────────────────────────────────────────────────────────────────┘
```

- Single line, fixed at bottom
- Background: `Surface` color, top border in `Border` color
- Segments separated by `│` in `Border` color
- Branch name in branch color with `●` dot
- Arrow `↑3` means 3 commits ahead of remote (bright green)
- Staged count in green with `✓`
- Unstaged count in yellow with `⚡`
- Untracked count in purple with `?`
- Protocol name in dim text on the right
- Mouse icon `⊙` shows mouse is enabled (dim if disabled)
- Clean repo: `✓ clean` in green instead of file counts

### 9.10 Find/Search Widget

```
┌──────────────────────────────────────────────────────────────────┐
│  🔍 [commit message, hash, author, branch...    ]  Aa  .*  ↓3  │
│                                                    │   │   │     │
│                                              case regex match#  │
└──────────────────────────────────────────────────────────────────┘
```

- Floats at top of the graph pane
- Inline text input with search icon
- Toggle buttons: Case sensitive (`Aa`), Regex (`.*`)
- Match count: `↓3` means 3rd of N matches
- Matches highlighted in the commit list with a bright background band

### 9.11 Staging Area

```
┌─ Staging Area ────────────────────────────────────────────┐
│                                                            │
│  Staged (3)                                s:stage u:unstage│
│  ┌──────────────────────────────────────────────────────┐ │
│  │ ✓ M  src/app.rs                    +89 −12          │ │
│  │ ✓ M  src/main.rs                   +45 −3           │ │
│  │ ✓ A  src/new_module.rs             +823 −0          │ │
│  └──────────────────────────────────────────────────────┘ │
│                                                            │
│  Unstaged (2)                                              │
│  ┌──────────────────────────────────────────────────────┐ │
│  │   M  src/lib.rs                     +12 −5          │ │
│  │   D  old_module.rs                   +0 −67          │ │
│  └──────────────────────────────────────────────────────┘ │
│                                                            │
│  Untracked (1)                                             │
│  ┌──────────────────────────────────────────────────────┐ │
│  │   ?  notes.txt                                       │ │
│  └──────────────────────────────────────────────────────┘ │
│                                                            │
│  c:commit  S:stage all  U:unstage all  a:amend  d:discard │
└────────────────────────────────────────────────────────────┘
```

**Visual details:**
- Staged section: Green `✓` prefix, green border on card
- Unstaged: Yellow `M`/`D` prefix, yellow border
- Untracked: Purple `?` prefix, purple border
- Selected file: Inverted background
- Mini bar graph per file: green proportional bar for additions, red for deletions
- Action hints at bottom in `Text secondary` color, keyboard shortcuts in accent color

### 9.12 Animations & Transitions

Subtle, purposeful animations. Terminal cannot do 60fps — keep it tasteful.

- **Pane open/close:** Instant (no animation needed in terminal)
- **Context menu:** Instant appear
- **Selection change:** Cursor movement is immediate (no slide animation)
- **Scroll:** Jump scroll (no smooth scroll — terminals don't support it well)
- **Loading indicator:** Spinning braille pattern `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏` in status bar when git commands run
- **Uncommitted changes node:** Slow pulse (toggle visibility every 2s) to draw attention

### 9.13 Accessibility

- All interactive elements navigable by keyboard alone
- Color-blind safe: Never rely on color alone — always pair with text labels or icons (✓, ⚡, ?, M, A, D, R)
- High contrast mode: Config option to use brighter colors throughout
- Screen reader: Not applicable (TUI is inherently text-based)

---

## 11. Technical Notes

### Git write operations safety

All destructive operations (reset --hard, delete branch, force push) require explicit confirmation via dialog. Non-destructive operations (checkout, merge --no-ff, fetch) execute immediately.

### Error handling

Git command failures are captured (stderr) and displayed in a dialog overlay. The app never crashes on git errors.

### Performance considerations

- Graph images are generated lazily (only for visible rows), same as serie
- For repos >10k commits, load first `initial_load_count` commits, load more on scroll to bottom
- Diff rendering uses a streaming parser — doesn't load the full diff into memory
- File tree is computed once per commit selection, cached until selection changes

### Terminal compatibility

- Kitty protocol: Kitty, Ghostty (confirmed working)
- iTerm2 protocol: iTerm2, Warp
- Sixel: xterm (with -ti 340), mlterm, other Sixel-capable terminals
- Unicode fallback: All terminals — uses box-drawing characters with 16-color ANSI

---

## 12. Comparison: gitui vs Git Graph vs serie

| Feature | serie | gitui (target) | Git Graph |
|---------|-------|---------------|-----------|
| Pixel-perfect graph | PNG (Kitty/iTerm2) | PNG (Kitty/iTerm2/Sixel) + Unicode fallback | SVG/Canvas (webview) |
| Mouse support | No | Yes (scroll, click, drag) | Yes |
| Uncommitted changes | No | Yes (virtual node) | Yes |
| Inline diff view | No (via user command) | Yes (built-in) | Yes |
| File tree | No | Yes | Yes |
| Staging area | No | Yes | No (uses VS Code SCM) |
| Branch operations | No | Yes | Yes |
| Tag operations | No | Yes | Yes |
| Stash operations | View only | Yes (full CRUD) | Yes |
| Merge/Rebase/Cherry-pick | No | Yes | Yes |
| Push/Pull/Fetch | No | Yes | Yes |
| Context menus | No | Yes (right-click) | Yes (right-click) |
| Commit comparison | No | Yes (Shift+click) | Yes (Ctrl+click) |
| Search/Filter | Basic search | Enhanced (regex, filter by branch) | Yes (regex) |
| Auto-refresh | Manual only | Yes (file watcher) | Yes (file watcher) |
| Code review | No | Phase 4 | Yes |
| Multi-repo | No | Phase 4 | Yes |
| Config system | TOML | TOML (extended) | VS Code settings |
| Terminal compatibility | Kitty/iTerm2 only | Kitty/iTerm2/Sixel/Unicode | N/A (VS Code) |
| Performance | Fast (Rust) | Fast (Rust) | Medium (webview) |
