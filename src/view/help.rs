use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph},
    Frame,
};

use crate::{
    app::AppContext,
    color::ColorTheme,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    keybind::KeyBinds,
    view::View,
};

/// One key + action pair displayed in the help table. `key` is owned
/// because it's resolved per-render against the user's `KeyBinds`,
/// so the help page mirrors any rebinding from the config file.
struct Shortcut {
    key: String,
    action: &'static str,
}

/// One section of the help page (one logical view or group of shortcuts).
struct HelpSection {
    title: &'static str,
    intro: Option<&'static str>,
    shortcuts: Vec<Shortcut>,
}

/// Build a Shortcut for a global UserEvent. The displayed key tracks
/// `[keybind]` overrides automatically. Falls back to the action label
/// alone (no key prefix) when the user unbound the event.
fn sg(kb: &KeyBinds, event: UserEvent, action: &'static str) -> Shortcut {
    Shortcut {
        key: kb.primary_global_key(event),
        action,
    }
}

/// Build a Shortcut for a scope-local action (`[scope.<path>] action`).
/// Same dynamic-key story as `sg`.
fn ss(kb: &KeyBinds, scope: &[&str], action_name: &str, action: &'static str) -> Shortcut {
    Shortcut {
        key: kb.primary_scoped_key(scope, action_name),
        action,
    }
}

/// Build a Shortcut with a fixed key string, for symbolic keys
/// (`Esc`, `↑↓`, `Enter`, etc.) and for terminal-level shortcuts
/// (`Ctrl+Shift+C` copy/paste) that aren't bound through gitoui.
fn lit(key: &str, action: &'static str) -> Shortcut {
    Shortcut {
        key: key.to_string(),
        action,
    }
}

#[derive(Debug)]
pub struct HelpView<'a> {
    before: View<'a>,
    /// "Committed" section, drives the right pane content + renders
    /// the leading `▶` marker. Changes on click or arrow keys.
    selected: usize,
    /// Currently-hovered section, drives the row background highlight.
    /// Tracks mouse-move and arrow keys but NOT clicks alone (a click
    /// pulls it in sync with `selected` so the two never disagree
    /// without a follow-up mouse move).
    hovered: usize,
    scroll: usize,
    height: u16,
    /// Last rendered bounds of the section-list (left) column. Used by
    /// `handle_click` / `handle_mouse_move` to map a (col, row) screen
    /// coordinate back to a section index.
    left_area: Rect,
    /// Row → section index mapping for the left column, refreshed each
    /// render. Same shape as config view's `left_item_rows`.
    left_item_rows: Vec<usize>,
    tx: Sender,
    ctx: Rc<AppContext>,
}

impl HelpView<'_> {
    pub fn new<'a>(before: View<'a>, ctx: Rc<AppContext>, tx: Sender) -> HelpView<'a> {
        HelpView {
            before,
            selected: 0,
            hovered: 0,
            scroll: 0,
            height: 0,
            left_area: Rect::default(),
            left_item_rows: Vec::new(),
            tx,
            ctx,
        }
    }

    /// Click on the section list commits the row as the new `selected`.
    /// We also re-sync `hovered` so the bg highlight stays on the row
    /// the user just clicked, instead of trailing the last mouse
    /// position.
    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(idx) = self.left_area_section_index(col, row) {
            self.selected = idx;
            self.hovered = idx;
            self.scroll = 0;
        }
    }

    /// Hovering only updates `hovered`, the right pane stays on the
    /// previously-committed `selected` section until the user clicks.
    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        if let Some(idx) = self.left_area_section_index(col, row) {
            if self.hovered != idx {
                self.hovered = idx;
            }
        }
    }

    fn left_area_section_index(&self, col: u16, row: u16) -> Option<usize> {
        if col < self.left_area.x || col >= self.left_area.x.saturating_add(self.left_area.width) {
            return None;
        }
        if row < self.left_area.y || row >= self.left_area.y.saturating_add(self.left_area.height) {
            return None;
        }
        let row_idx = (row - self.left_area.y) as usize;
        self.left_item_rows.get(row_idx).copied()
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;
        let sections_len = sections(&self.ctx.keybind).len();

        match event {
            UserEvent::Quit => self.tx.send(AppEvent::Quit),
            UserEvent::HelpToggle | UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::CloseHelp);
            }
            UserEvent::NavigateDown | UserEvent::SelectDown => {
                for _ in 0..count {
                    if self.selected + 1 < sections_len {
                        self.selected += 1;
                        self.hovered = self.selected;
                        self.scroll = 0;
                    }
                }
            }
            UserEvent::NavigateUp | UserEvent::SelectUp => {
                for _ in 0..count {
                    if self.selected > 0 {
                        self.selected -= 1;
                        self.hovered = self.selected;
                        self.scroll = 0;
                    }
                }
            }
            UserEvent::GoToTop => {
                self.selected = 0;
                self.hovered = 0;
                self.scroll = 0;
            }
            UserEvent::GoToBottom => {
                self.selected = sections_len.saturating_sub(1);
                self.hovered = self.selected;
                self.scroll = 0;
            }
            UserEvent::PageDown => {
                for _ in 0..count {
                    self.scroll = self.scroll.saturating_add(self.height as usize);
                }
            }
            UserEvent::PageUp => {
                for _ in 0..count {
                    self.scroll = self.scroll.saturating_sub(self.height as usize);
                }
            }
            UserEvent::HalfPageDown => {
                for _ in 0..count {
                    self.scroll = self.scroll.saturating_add((self.height / 2) as usize);
                }
            }
            UserEvent::HalfPageUp => {
                for _ in 0..count {
                    self.scroll = self.scroll.saturating_sub((self.height / 2) as usize);
                }
            }
            UserEvent::ScrollDown => {
                for _ in 0..count {
                    self.scroll = self.scroll.saturating_add(1);
                }
            }
            UserEvent::ScrollUp => {
                for _ in 0..count {
                    self.scroll = self.scroll.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let block = Block::default().padding(Padding::new(2, 2, 1, 1));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // ── Title row (mirrors config view) ─────────────────────────────
        let title = Line::from(vec![
            Span::styled(
                "Help",
                Style::default()
                    .fg(theme.list_ref_stash_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Keyboard reference",
                Style::default().fg(theme.divider_fg),
            ),
        ]);
        let title_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        f.render_widget(Paragraph::new(title), title_area);

        // Horizontal separator
        let sep_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: 1,
        };
        let sep_style = Style::default().fg(theme.divider_fg);
        let sep_line = Line::from("╌".repeat(inner.width as usize)).style(sep_style);
        f.render_widget(Paragraph::new(sep_line), sep_area);

        // ── Two columns ──────────────────────────────────────────────────
        let content_area = Rect {
            x: inner.x,
            y: inner.y + 3,
            width: inner.width,
            height: inner.height.saturating_sub(4),
        };
        let left_width: u16 = 26; // wide enough for "Pull Requests" etc.
        let separator_x = content_area.x + left_width;
        let left_area = Rect {
            x: content_area.x,
            y: content_area.y,
            width: left_width.saturating_sub(1),
            height: content_area.height,
        };
        let separator_area = Rect {
            x: separator_x,
            y: content_area.y,
            width: 1,
            height: content_area.height,
        };
        let right_area = Rect {
            x: separator_x + 2,
            y: content_area.y,
            width: content_area.right().saturating_sub(separator_x + 2),
            height: content_area.height,
        };
        self.height = right_area.height;

        // Vertical separator
        let vsep_style = Style::default().fg(theme.divider_fg);
        for row in 0..separator_area.height.saturating_sub(1) {
            let cell_area = Rect {
                x: separator_area.x,
                y: separator_area.y + row,
                width: 1,
                height: 1,
            };
            let line = Line::from("┊").style(vsep_style);
            f.render_widget(Paragraph::new(line), cell_area);
        }

        // ── Left: section list ───────────────────────────────────────────
        // Persist `left_area` + row→section map so mouse handlers can
        // resolve a click/hover back to a section index.
        self.left_area = left_area;
        let sections = sections(&self.ctx.keybind);
        let mut left_lines: Vec<Line> = Vec::new();
        let mut left_item_rows: Vec<usize> = Vec::with_capacity(sections.len());
        // Reserve a 2-col gutter on the left so the `▶` selected marker
        // can render in front of the title without shifting the text.
        for (i, sec) in sections.iter().enumerate() {
            let is_hovered = i == self.hovered;
            let is_selected = i == self.selected;

            // Hover bg covers the 2-col left gutter (where the `▶`
            // marker sits when selected), the title, and a 2-col right
            // pad, so the highlight band reads as one continuous chip
            // around the section name.
            let bg_style = if is_hovered {
                Style::default()
                    .fg(theme.list_selected_fg)
                    .bg(theme.list_selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            let marker_style = if is_selected {
                let mut s = Style::default()
                    .fg(theme.status_info_fg)
                    .add_modifier(Modifier::BOLD);
                if is_hovered {
                    s = s.bg(theme.list_selected_bg);
                }
                s
            } else {
                bg_style
            };
            let marker = if is_selected { "▶ " } else { "  " };
            let title_with_pad = format!("{} ", sec.title);
            left_lines.push(Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(title_with_pad, bg_style),
            ]));
            left_item_rows.push(i);
        }
        self.left_item_rows = left_item_rows;
        f.render_widget(Paragraph::new(left_lines), left_area);

        // ── Right: shortcuts for selected section ────────────────────────
        let section = &sections[self.selected];
        let mut right_lines: Vec<Line> = Vec::new();

        // Section header (matches "Details / <name>" pattern in config)
        right_lines.push(Line::from(vec![
            Span::styled(
                "Shortcuts",
                Style::default()
                    .fg(theme.list_ref_stash_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  / {}", section.title),
                Style::default().fg(theme.divider_fg),
            ),
        ]));
        right_lines.push(Line::from(""));

        if let Some(intro) = section.intro {
            for line in intro.lines() {
                right_lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(theme.fg),
                )));
            }
            right_lines.push(Line::from(""));
        }

        // Compute the widest key label so we can right-pad keys into a
        // gutter that lines up with the action column.
        let key_col_width = section
            .shortcuts
            .iter()
            .map(|s| s.key.chars().count())
            .max()
            .unwrap_or(0)
            .max(4);

        for sh in &section.shortcuts {
            if sh.key.is_empty() && sh.action.is_empty() {
                right_lines.push(Line::from(""));
                continue;
            }
            // Section header within a group (action is empty; key holds
            // a sub-section label prefixed with `▍`).
            if sh.action.is_empty() {
                right_lines.push(Line::from(vec![
                    Span::styled("▍ ", Style::default().fg(theme.list_ref_stash_fg)),
                    Span::styled(
                        sh.key.to_string(),
                        Style::default()
                            .fg(theme.list_ref_stash_fg)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                continue;
            }
            // 2-space indent on every shortcut row so items visually
            // nest under the section title + any `▍ sub-header` lines,
            // matching the config view's `CONFIG_ITEM_INDENT` style.
            // Keys use the theme's secondary accent (cyan/blue in most
            // themes, the same token used by config's Cycle values) so
            // they pop against the section header's primary accent
            // without falling into the "warn-yellow" trap of help_key_fg.
            let key_label = format!("  {:<width$}", sh.key, width = key_col_width);
            right_lines.push(Line::from(vec![
                Span::styled(
                    key_label,
                    Style::default()
                        .fg(theme.status_info_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {}", sh.action), Style::default().fg(theme.fg)),
            ]));
        }

        // Clamp scroll so we never run off the end of this section's lines.
        let total = right_lines.len();
        let viewport = right_area.height as usize;
        let max_scroll = total.saturating_sub(viewport);
        if self.scroll > max_scroll {
            self.scroll = max_scroll;
        }
        let mut visible: Vec<Line> = right_lines
            .into_iter()
            .skip(self.scroll)
            .take(viewport)
            .collect();

        // Replace the top/bottom rows with `…` to flag that more content
        // exists in that direction. Top dots appear when scrolled past
        // the first row; bottom dots when the viewport doesn't reach the
        // last row. Both vanish when the content fits.
        let dots_style = Style::default().fg(theme.divider_fg);
        let dots = Line::from(Span::styled("  …", dots_style));
        if total > viewport {
            if self.scroll > 0 && !visible.is_empty() {
                visible[0] = dots.clone();
            }
            if self.scroll + viewport < total && !visible.is_empty() {
                let last = visible.len() - 1;
                visible[last] = dots;
            }
        }
        f.render_widget(Paragraph::new(visible), right_area);
    }
}

impl<'a> HelpView<'a> {
    pub fn take_before_view(&mut self) -> View<'a> {
        std::mem::take(&mut self.before)
    }

    pub fn update_color_theme(&mut self, theme: ColorTheme) {
        self.before.update_color_theme(theme);
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        self.before.graph_image_ids_sorted()
    }

    pub fn is_search_active(&self) -> bool {
        self.before.is_search_active()
    }

    pub fn is_search_querying(&self) -> bool {
        self.before.is_search_querying()
    }

    pub fn search_case_fuzzy_regex(&self) -> Option<(bool, bool, bool)> {
        self.before.search_case_fuzzy_regex()
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Section data, sourced directly from each view's key-handling code and
// `assets/default-keybind.toml`. Each section maps to one logical view in
// the app. Sub-views inherit shortcuts from the section above them in the
// listing order (e.g. Diff inherits Navigation; Branch Detail inherits
// Refs). Only Q (Quit) is documented once, in General.
// ─────────────────────────────────────────────────────────────────────────

fn sections(kb: &KeyBinds) -> Vec<HelpSection> {
    vec![
        HelpSection {
            title: "General",
            intro: Some("Available from anywhere in the app."),
            shortcuts: vec![
                sg(kb, UserEvent::Quit, "Quit gitoui"),
                sg(kb, UserEvent::ForceQuit, "Force quit (always works)"),
                sg(kb, UserEvent::HelpToggle, "Toggle this help"),
                sg(kb, UserEvent::Config, "Open configuration"),
                sg(kb, UserEvent::PullRequests, "Open Pull Requests view"),
                sg(kb, UserEvent::Issues, "Open Issues view"),
                lit("", ""),
                lit("Terminal", ""),
                lit("Shift+drag", "Select text with the mouse"),
                lit("Ctrl+Shift+C", "Copy the selected text"),
                lit("Ctrl+Shift+V", "Paste into the focused text input"),
            ],
        },
        HelpSection {
            title: "Navigation",
            intro: Some(
                "Shared across every list-like view (commit list, refs, files, comments…).",
            ),
            shortcuts: vec![
                sg(kb, UserEvent::NavigateDown, "Move down one row"),
                sg(kb, UserEvent::NavigateUp, "Move up one row"),
                sg(kb, UserEvent::NavigateLeft, "Move left / close node"),
                sg(kb, UserEvent::NavigateRight, "Move right / open node"),
                sg(
                    kb,
                    UserEvent::SelectDown,
                    "Older commit (in detail / drilldowns)",
                ),
                sg(kb, UserEvent::SelectUp, "Newer commit"),
                sg(kb, UserEvent::GoToParent, "Go to parent commit"),
                sg(kb, UserEvent::GoToTop, "Go to top"),
                sg(kb, UserEvent::GoToBottom, "Go to bottom"),
                sg(kb, UserEvent::SelectMiddle, "Select middle of viewport"),
                sg(kb, UserEvent::SelectBottom, "Select bottom of viewport"),
                sg(kb, UserEvent::ScrollDown, "Scroll one line down"),
                sg(kb, UserEvent::ScrollUp, "Scroll one line up"),
                sg(kb, UserEvent::PageDown, "Page down"),
                sg(kb, UserEvent::PageUp, "Page up"),
                sg(kb, UserEvent::HalfPageDown, "Half page down"),
                sg(kb, UserEvent::HalfPageUp, "Half page up"),
                sg(kb, UserEvent::Confirm, "Confirm / drill into selection"),
                sg(kb, UserEvent::Cancel, "Cancel / back"),
            ],
        },
        HelpSection {
            title: "Commit List",
            intro: Some("The main view, `gitoui`'s home screen."),
            shortcuts: vec![
                lit("Search", ""),
                sg(kb, UserEvent::Search, "Start a search"),
                sg(kb, UserEvent::Confirm, "Apply search query"),
                sg(kb, UserEvent::Cancel, "Cancel search / clear compare mark"),
                lit("← / →", "Cycle to previous / next match"),
                sg(
                    kb,
                    UserEvent::IgnoreCaseToggle,
                    "Toggle case-sensitive matching",
                ),
                sg(kb, UserEvent::FuzzyToggle, "Toggle fuzzy matching"),
                sg(
                    kb,
                    UserEvent::Discard,
                    "Toggle regex matching (while search applied)",
                ),
                lit("", ""),
                lit("Actions", ""),
                sg(
                    kb,
                    UserEvent::Confirm,
                    "Open commit detail (or Uncommitted on the top row)",
                ),
                sg(kb, UserEvent::RefList, "Open refs panel"),
                sg(
                    kb,
                    UserEvent::MarkCompare,
                    "Mark / unmark a commit for 2-commit compare",
                ),
                sg(kb, UserEvent::Refresh, "Refetch from remote"),
                sg(kb, UserEvent::Push, "Push current branch"),
                sg(kb, UserEvent::Pull, "Pull current branch"),
                sg(
                    kb,
                    UserEvent::AbortOperation,
                    "Abort in-progress rebase / merge / cherry-pick",
                ),
                ss(
                    kb,
                    &["list"],
                    "select_head_commit",
                    "Scroll to HEAD (centered)",
                ),
                lit("", ""),
                lit("Copy", ""),
                sg(kb, UserEvent::FullCopy, "Copy commit message"),
                sg(kb, UserEvent::ShortCopy, "Copy commit hash"),
            ],
        },
        HelpSection {
            title: "Commit Detail",
            intro: Some("Opens with Enter on a commit. All Navigation shortcuts apply."),
            shortcuts: vec![
                lit("Esc / Enter", "Close detail"),
                sg(kb, UserEvent::Blame, "Open blame on the focused file"),
                sg(
                    kb,
                    UserEvent::FileHistory,
                    "Open file history on the focused file",
                ),
                lit("", ""),
                lit("Commit actions", ""),
                sg(kb, UserEvent::AddTag, "Add tag"),
                sg(
                    kb,
                    UserEvent::CreateBranch,
                    "Create branch from this commit",
                ),
                sg(kb, UserEvent::Checkout, "Checkout this commit"),
                sg(kb, UserEvent::CherryPick, "Cherry-pick"),
                ss(
                    kb,
                    &["detail"],
                    "revert",
                    "Revert this commit (opens confirmation dialog)",
                ),
                sg(kb, UserEvent::Drop, "Drop commit (uses interactive rebase)"),
                sg(kb, UserEvent::Merge, "Merge"),
                sg(kb, UserEvent::Rebase, "Open interactive rebase from here"),
                sg(kb, UserEvent::Reset, "Reset"),
                sg(kb, UserEvent::Squash, "Squash with previous commit"),
                sg(kb, UserEvent::AmendCommit, "Amend HEAD message (HEAD only)"),
                lit("", ""),
                lit("Stash actions", "(only on stash nodes)"),
                sg(kb, UserEvent::ApplyStash, "Apply stash (keep)"),
                sg(kb, UserEvent::PopStash, "Pop stash (apply + drop)"),
                sg(kb, UserEvent::DropStash, "Drop stash"),
                sg(
                    kb,
                    UserEvent::CreateBranchFromStash,
                    "Create branch from stash",
                ),
                sg(kb, UserEvent::CopyStashName, "Copy stash name"),
                sg(kb, UserEvent::CopyStashHash, "Copy stash hash"),
                lit("", ""),
                lit("Copy", ""),
                sg(kb, UserEvent::FullCopy, "Copy commit message"),
                sg(kb, UserEvent::ShortCopy, "Copy commit hash"),
            ],
        },
        HelpSection {
            title: "Refs",
            intro: Some(
                "Opens with Tab from the commit list. Lists branches, tags, stashes, remotes.",
            ),
            shortcuts: vec![
                lit("Tab / Esc", "Close refs panel"),
                lit("Right", "Open / expand node"),
                lit("Left", "Close / collapse node"),
                lit("Enter", "Open Branch Detail / Tag Detail / jump to ref"),
                sg(kb, UserEvent::Refresh, "Refetch"),
                sg(kb, UserEvent::ShortCopy, "Copy ref name"),
            ],
        },
        HelpSection {
            title: "Branch Detail",
            intro: Some("Opens with Enter on a branch in the Refs panel."),
            shortcuts: vec![
                lit("Esc", "Close"),
                sg(kb, UserEvent::Checkout, "Checkout branch"),
                sg(kb, UserEvent::RenameBranch, "Rename branch"),
                sg(kb, UserEvent::DeleteBranch, "Delete branch"),
                sg(kb, UserEvent::Merge, "Merge into current branch"),
                sg(kb, UserEvent::Rebase, "Rebase onto this branch"),
                sg(kb, UserEvent::PushBranch, "Push branch"),
                sg(kb, UserEvent::Pull, "Pull branch"),
                sg(kb, UserEvent::SetUpstream, "Set upstream"),
                sg(kb, UserEvent::CreateArchive, "Create archive (.tar.gz)"),
                sg(
                    kb,
                    UserEvent::UnselectBranch,
                    "Unselect branch (detach HEAD)",
                ),
                sg(kb, UserEvent::CopyBranchName, "Copy branch name"),
                sg(kb, UserEvent::Refresh, "Refetch"),
            ],
        },
        HelpSection {
            title: "Tag Detail",
            intro: Some("Opens with Enter on a tag in the Refs panel."),
            shortcuts: vec![
                lit("Esc", "Close"),
                sg(kb, UserEvent::PushTag, "Push tag"),
                sg(kb, UserEvent::DeleteTag, "Delete tag"),
                sg(kb, UserEvent::CopyTagName, "Copy tag name"),
                sg(kb, UserEvent::Refresh, "Refetch"),
            ],
        },
        HelpSection {
            title: "Uncommitted",
            intro: Some("Opens with Enter on the top row of the commit list (`(uncommitted)`)."),
            shortcuts: vec![
                lit("Esc", "Back to commit list"),
                lit("Up / Down", "Navigate files within the active section"),
                lit(
                    "Left / Right",
                    "Switch section (Unstaged / Staged / Untracked)",
                ),
                lit("Enter", "Open diff of the focused file"),
                sg(kb, UserEvent::Refresh, "Refresh file lists"),
                lit("", ""),
                lit("Staging", ""),
                sg(kb, UserEvent::Stage, "Stage selected file"),
                sg(kb, UserEvent::StageAll, "Stage all unstaged + untracked"),
                sg(
                    kb,
                    UserEvent::Unstage,
                    "Unstage selected file (Staged section)",
                ),
                sg(kb, UserEvent::Pull, "Unstage all staged files"),
                sg(kb, UserEvent::Discard, "Discard selected unstaged change"),
                sg(
                    kb,
                    UserEvent::DiscardAll,
                    "Discard all unstaged + staged changes",
                ),
                sg(
                    kb,
                    UserEvent::CleanUntracked,
                    "Clean (delete) untracked file",
                ),
                lit("", ""),
                lit("Save state", ""),
                sg(
                    kb,
                    UserEvent::Commit,
                    "Commit (opens commit message editor)",
                ),
                sg(kb, UserEvent::Stash, "Stash"),
                lit("", ""),
                lit("Inspect", ""),
                sg(kb, UserEvent::Blame, "Blame the focused file"),
                sg(
                    kb,
                    UserEvent::FileHistory,
                    "File history of the focused file",
                ),
            ],
        },
        HelpSection {
            title: "Diff",
            intro: Some("Opens with Enter on a file row, from commit Detail or Uncommitted."),
            shortcuts: vec![
                lit("Esc / Enter", "Close diff"),
                lit("j / k / arrows", "Scroll"),
                lit("Left / Right", "Move focus across hunks / action buttons"),
                sg(
                    kb,
                    UserEvent::CycleFileNext,
                    "Cycle to the next file in the commit",
                ),
                sg(kb, UserEvent::CycleFilePrev, "Cycle to the previous file"),
                sg(kb, UserEvent::Refresh, "Refresh"),
                lit("", ""),
                lit("Search", ""),
                sg(kb, UserEvent::Search, "Toggle in-diff search"),
                sg(kb, UserEvent::GoToNext, "Next match"),
                sg(kb, UserEvent::GoToPrevious, "Previous match"),
                lit("", ""),
                lit("Inspect", ""),
                sg(kb, UserEvent::Blame, "Blame the current file"),
                sg(kb, UserEvent::FileHistory, "Open file history"),
                lit("", ""),
                lit("Copy", ""),
                sg(kb, UserEvent::FullCopy, "Copy the file path"),
                sg(kb, UserEvent::ShortCopy, "Copy the commit hash"),
            ],
        },
        HelpSection {
            title: "Blame",
            intro: Some("Opens with `b` on a file."),
            shortcuts: vec![
                lit("Esc", "Close blame"),
                lit("j / k", "Move line by line"),
                lit("Left / Right", "Jump to previous / next blame block"),
                lit("PageUp / PageDown", "Scroll a page"),
                lit("g / Shift+G", "Top / bottom"),
                lit("Enter", "Open the commit that authored the focused line"),
                sg(kb, UserEvent::FileHistory, "Open file history (same file)"),
            ],
        },
        HelpSection {
            title: "File History",
            intro: Some("Opens with `Shift+H` on a file, equivalent of `git log --follow`."),
            shortcuts: vec![
                lit("Esc / Enter", "Close history"),
                lit("j / k / arrows", "Navigate revisions"),
                lit("PageUp / PageDown", "Scroll a page"),
                lit("g / Shift+G", "Top / bottom"),
                lit("Enter", "Open commit detail for the focused revision"),
                sg(kb, UserEvent::Blame, "Blame the file at this revision"),
                sg(kb, UserEvent::FullCopy, "Copy commit message"),
                sg(kb, UserEvent::ShortCopy, "Copy commit hash"),
            ],
        },
        HelpSection {
            title: "Interactive Rebase",
            intro: Some("Opens with `e` on a commit. Edit the rebase plan, then Enter to apply."),
            shortcuts: vec![
                lit("Enter", "Apply rebase plan"),
                lit("Esc", "Cancel (releases grab first if any)"),
                lit("Up / Down", "Move selection (or reorder when grabbed)"),
                lit("Left / Right", "Cycle action for the selected row"),
                ss(kb, &["rebase"], "grab", "Grab / release the focused row"),
                lit("", ""),
                lit("Per-row action", ""),
                ss(kb, &["rebase"], "pick", "Pick"),
                ss(kb, &["rebase"], "reword", "Reword (opens inline editor)"),
                ss(kb, &["rebase"], "edit", "Edit"),
                ss(kb, &["rebase"], "squash", "Squash into previous"),
                ss(kb, &["rebase"], "fixup", "Fixup into previous"),
                ss(kb, &["rebase"], "drop", "Drop"),
                lit("", ""),
                lit("Stale rebase", "(when resume banner shown)"),
                ss(
                    kb,
                    &["rebase"],
                    "abort_previous",
                    "Abort the previously-stuck rebase",
                ),
                ss(
                    kb,
                    &["rebase", "resume"],
                    "continue_rebase",
                    "Continue stuck rebase",
                ),
                ss(
                    kb,
                    &["rebase", "resume"],
                    "skip_commit",
                    "Skip current commit",
                ),
                ss(kb, &["rebase", "resume"], "abort", "Abort and bail"),
                lit("", ""),
                lit("Reword editor", "(while inline-editing a subject)"),
                lit("Enter", "Save reworded message"),
                lit("Esc", "Cancel reword"),
                ss(
                    kb,
                    &["rebase", "reword_editor"],
                    "delete_word_left",
                    "Delete word to the left",
                ),
                ss(
                    kb,
                    &["rebase", "reword_editor"],
                    "delete_word_right",
                    "Delete word to the right",
                ),
                ss(
                    kb,
                    &["rebase", "reword_editor"],
                    "word_left",
                    "Jump one word left",
                ),
                ss(
                    kb,
                    &["rebase", "reword_editor"],
                    "word_right",
                    "Jump one word right",
                ),
            ],
        },
        HelpSection {
            title: "Conflict Editor",
            intro: Some("Opens automatically during a conflicted merge / rebase / cherry-pick."),
            shortcuts: vec![
                lit("Enter", "Save resolution and mark as resolved"),
                lit("Esc", "Cancel and leave conflict in place"),
                ss(kb, &["conflict"], "prev_hunk", "Previous conflict hunk"),
                ss(kb, &["conflict"], "next_hunk", "Next conflict hunk"),
                ss(
                    kb,
                    &["conflict"],
                    "pick_ours",
                    "Pick OURS for the focused hunk",
                ),
                ss(kb, &["conflict"], "pick_theirs", "Pick THEIRS"),
                ss(
                    kb,
                    &["conflict"],
                    "pick_both_ours_first",
                    "Keep BOTH (ours first)",
                ),
                ss(
                    kb,
                    &["conflict"],
                    "pick_both_theirs_first",
                    "Keep BOTH (theirs first)",
                ),
                lit("j / k / arrows", "Scroll"),
            ],
        },
        HelpSection {
            title: "Compare (2-commit)",
            intro: Some("Opens after marking two commits with Space, shows the cumulative diff."),
            shortcuts: vec![
                lit("Esc", "Close compare"),
                lit("j / k / arrows", "Navigate files / scroll"),
                lit("Left / Right", "Switch between file list and diff panel"),
                lit("Enter", "Open the focused file's diff"),
            ],
        },
        HelpSection {
            title: "Pull Requests",
            intro: Some("Opens with `Shift+R` (needs a GitHub remote + auth)."),
            shortcuts: vec![
                lit("List", ""),
                lit("Up / Down", "Navigate PRs"),
                lit(
                    "Left / Right",
                    "Cycle filter (Open / Merged / Closed / All)",
                ),
                lit("Enter", "Open PR detail"),
                ss(kb, &["pr", "list"], "new_pr", "Compose a new PR"),
                ss(kb, &["pr"], "reload", "Reload"),
                lit("Esc", "Close PR view"),
                lit("", ""),
                lit("Detail", ""),
                lit(
                    "Left / Right",
                    "Switch tab (Conversation / Files / Commits / References)",
                ),
                lit("Enter", "Drill into focused file or commit row"),
                lit("Esc", "Back (drilldown → list → close)"),
                ss(kb, &["pr"], "reload", "Reload PR"),
                ss(kb, &["pr"], "open_in_browser", "Open PR in browser"),
                lit("", ""),
                lit("PR actions", ""),
                ss(kb, &["pr"], "approve", "Approve review"),
                ss(kb, &["pr"], "request_changes", "Request changes"),
                ss(kb, &["pr"], "merge", "Merge PR"),
                ss(kb, &["pr"], "labels_picker", "Labels picker"),
                ss(kb, &["pr"], "reviewers_picker", "Reviewers picker"),
                ss(kb, &["pr"], "close_pr", "Close PR"),
                ss(kb, &["pr"], "reopen_pr", "Reopen PR"),
                ss(kb, &["pr"], "toggle_draft", "Toggle draft state"),
                lit("", ""),
                lit("Conversation", "(tab actions)"),
                ss(kb, &["pr", "conversation"], "new_comment", "New comment"),
                ss(
                    kb,
                    &["pr", "conversation"],
                    "quote_reply",
                    "Quote-reply to focused comment",
                ),
                ss(kb, &["pr", "conversation"], "edit_own", "Edit own comment"),
                ss(
                    kb,
                    &["pr", "conversation"],
                    "delete_own",
                    "Delete own comment",
                ),
                ss(kb, &["pr", "conversation"], "react", "React (emoji picker)"),
                lit("", ""),
                lit("Compose", "(new PR / new comment / edit / reply)"),
                lit("Tab", "Move to the next field"),
                ss(kb, &["compose"], "submit", "Submit"),
                lit("Esc", "Cancel"),
                lit("#", "Open the #N issue/PR mention popup"),
                lit("Ctrl+H / Ctrl+W", "Delete word to the left"),
                lit("Ctrl+Left / Right", "Jump by word"),
            ],
        },
        HelpSection {
            title: "Issues",
            intro: Some("Opens with `Shift+I` (needs a GitHub remote + auth)."),
            shortcuts: vec![
                lit("List", ""),
                lit("j / k / arrows", "Navigate issues"),
                lit("g / Shift+G", "Top / bottom"),
                lit("Left / Right", "Cycle filter (Open / Closed / All)"),
                lit("Enter", "Open issue detail"),
                ss(kb, &["issues", "list"], "new_issue", "Compose a new issue"),
                ss(kb, &["issues"], "reload", "Reload"),
                lit("Esc", "Close Issues view"),
                lit("", ""),
                lit("Detail", ""),
                lit(
                    "Left / Right",
                    "Switch tab (Conversation / Timeline / References)",
                ),
                lit("j / k", "Move comment cursor (Conversation tab)"),
                lit("g / Shift+G", "First / last comment"),
                lit("PageUp / PageDown", "Scroll"),
                lit("Ctrl+U / Ctrl+D", "Half-page scroll"),
                lit(
                    "Enter",
                    "Follow first #N ref (Conversation) / open ref (References)",
                ),
                lit("Esc", "Back to list"),
                ss(kb, &["issues"], "reload", "Reload issue"),
                ss(kb, &["issues"], "open_in_browser", "Open issue in browser"),
                lit("", ""),
                lit("Issue actions", ""),
                ss(kb, &["issues"], "labels_picker", "Labels picker"),
                ss(kb, &["issues"], "assignees_picker", "Assignees picker"),
                ss(kb, &["issues"], "milestone_picker", "Milestone picker"),
                ss(kb, &["issues"], "close_or_reopen", "Close or reopen issue"),
                lit("", ""),
                lit("Conversation", "(tab actions)"),
                ss(kb, &["issues", "detail"], "new_comment", "New comment"),
                ss(
                    kb,
                    &["issues", "detail"],
                    "quote_reply",
                    "Quote-reply to focused comment",
                ),
                ss(
                    kb,
                    &["issues", "detail"],
                    "reference_in_new_issue",
                    "New issue with a back-reference to this one",
                ),
                ss(kb, &["issues", "detail"], "edit_own", "Edit own comment"),
                ss(
                    kb,
                    &["issues", "detail"],
                    "delete_own",
                    "Delete own comment",
                ),
                ss(kb, &["issues", "detail"], "react", "React (emoji picker)"),
                lit("", ""),
                lit("Compose", "(new issue / new comment / edit / reply)"),
                lit("Tab", "Move to the next field"),
                ss(kb, &["compose"], "submit", "Submit"),
                lit("Esc", "Cancel"),
                lit("#", "Open the #N issue/PR mention popup"),
                lit("Ctrl+H / Ctrl+W", "Delete word to the left"),
                lit("Ctrl+Left / Right", "Jump by word"),
            ],
        },
        HelpSection {
            title: "Configuration",
            intro: Some("Opens with `p` from the commit list. Edit themes, modes, and identity."),
            shortcuts: vec![
                lit("Esc / p", "Close configuration"),
                lit("j / k", "Move between items"),
                lit("h / l", "Cycle value left / right (for cycle items)"),
                lit("Enter", "Edit text input or trigger button (GitHub auth)"),
                lit("o", "Open the config file in your $EDITOR"),
                lit("", ""),
                lit("Text edit mode", ""),
                lit("Enter", "Save edit"),
                lit("Esc", "Cancel edit"),
                lit("Ctrl+Backspace", "Delete word to the left"),
                lit("Backspace", "Delete char to the left"),
            ],
        },
        HelpSection {
            title: "Help",
            intro: Some("This page!"),
            shortcuts: vec![
                lit("Esc / ? / F1", "Close help"),
                lit("j / k / arrows", "Change section"),
                lit("g / Shift+G", "First / last section"),
                lit("PageDown / PageUp", "Scroll the right pane"),
                lit("Ctrl+D / Ctrl+U", "Half-page scroll the right pane"),
            ],
        },
    ]
}
