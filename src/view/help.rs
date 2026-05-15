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
    view::View,
};

/// One key + action pair displayed in the help table.
struct Shortcut {
    key: &'static str,
    action: &'static str,
}

/// One section of the help page (one logical view or group of shortcuts).
struct HelpSection {
    title: &'static str,
    intro: Option<&'static str>,
    shortcuts: &'static [Shortcut],
}

#[derive(Debug)]
pub struct HelpView<'a> {
    before: View<'a>,
    /// "Committed" section — drives the right pane content + renders
    /// the leading `▶` marker. Changes on click or arrow keys.
    selected: usize,
    /// Currently-hovered section — drives the row background highlight.
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

    /// Hovering only updates `hovered` — the right pane stays on the
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
        let sections_len = sections().len();

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
        let sections = sections();
        let mut left_lines: Vec<Line> = Vec::new();
        let mut left_item_rows: Vec<usize> = Vec::with_capacity(sections.len());
        // Reserve a 2-col gutter on the left so the `▶` selected marker
        // can render in front of the title without shifting the text.
        for (i, sec) in sections.iter().enumerate() {
            let is_hovered = i == self.hovered;
            let is_selected = i == self.selected;

            // Hover bg covers the 2-col left gutter (where the `▶`
            // marker sits when selected), the title, and a 2-col right
            // pad — so the highlight band reads as one continuous chip
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

        for sh in section.shortcuts {
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
            // themes — the same token used by config's Cycle values) so
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
// Section data — sourced directly from each view's key-handling code and
// `assets/default-keybind.toml`. Each section maps to one logical view in
// the app. Sub-views inherit shortcuts from the section above them in the
// listing order (e.g. Diff inherits Navigation; Branch Detail inherits
// Refs). Only Q (Quit) is documented once, in General.
// ─────────────────────────────────────────────────────────────────────────

fn sections() -> Vec<HelpSection> {
    vec![
        HelpSection {
            title: "General",
            intro: Some("Available from anywhere in the app."),
            shortcuts: &[
                Shortcut {
                    key: "Q",
                    action: "Quit gitoui",
                },
                Shortcut {
                    key: "Ctrl+C",
                    action: "Force quit (always works)",
                },
                Shortcut {
                    key: "? / F1",
                    action: "Toggle this help",
                },
                Shortcut {
                    key: "P",
                    action: "Open configuration",
                },
                Shortcut {
                    key: "Shift+R",
                    action: "Open Pull Requests view",
                },
                Shortcut {
                    key: "Shift+I",
                    action: "Open Issues view",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Terminal",
                    action: "",
                },
                Shortcut {
                    key: "Shift+drag",
                    action: "Select text with the mouse",
                },
                Shortcut {
                    key: "Ctrl+Shift+C",
                    action: "Copy the selected text",
                },
                Shortcut {
                    key: "Ctrl+Shift+V",
                    action: "Paste into the focused text input",
                },
            ],
        },
        HelpSection {
            title: "Navigation",
            intro: Some(
                "Shared across every list-like view (commit list, refs, files, comments…).",
            ),
            shortcuts: &[
                Shortcut {
                    key: "j / Down",
                    action: "Move down one row",
                },
                Shortcut {
                    key: "k / Up",
                    action: "Move up one row",
                },
                Shortcut {
                    key: "h / Left",
                    action: "Move left / close node",
                },
                Shortcut {
                    key: "l / Right",
                    action: "Move right / open node",
                },
                Shortcut {
                    key: "Shift+J",
                    action: "Older commit (in detail / drilldowns)",
                },
                Shortcut {
                    key: "Shift+K",
                    action: "Newer commit",
                },
                Shortcut {
                    key: "Alt+J / Alt+Down",
                    action: "Go to parent commit",
                },
                Shortcut {
                    key: "g",
                    action: "Go to top",
                },
                Shortcut {
                    key: "Shift+G",
                    action: "Go to bottom",
                },
                Shortcut {
                    key: "Shift+M",
                    action: "Select middle of viewport",
                },
                Shortcut {
                    key: "Shift+L",
                    action: "Select bottom of viewport",
                },
                Shortcut {
                    key: "Ctrl+E",
                    action: "Scroll one line down",
                },
                Shortcut {
                    key: "Ctrl+Y",
                    action: "Scroll one line up",
                },
                Shortcut {
                    key: "Ctrl+F / PageDown",
                    action: "Page down",
                },
                Shortcut {
                    key: "Ctrl+B / PageUp",
                    action: "Page up",
                },
                Shortcut {
                    key: "Ctrl+D",
                    action: "Half page down",
                },
                Shortcut {
                    key: "Ctrl+U",
                    action: "Half page up",
                },
                Shortcut {
                    key: "Enter",
                    action: "Confirm / drill into selection",
                },
                Shortcut {
                    key: "Esc",
                    action: "Cancel / back",
                },
            ],
        },
        HelpSection {
            title: "Commit List",
            intro: Some("The main view — `gitoui`'s home screen."),
            shortcuts: &[
                Shortcut {
                    key: "Search",
                    action: "",
                },
                Shortcut {
                    key: "f",
                    action: "Start a search",
                },
                Shortcut {
                    key: "Enter",
                    action: "Apply search query",
                },
                Shortcut {
                    key: "Esc",
                    action: "Cancel search / clear compare mark",
                },
                Shortcut {
                    key: "← / →",
                    action: "Cycle to previous / next match",
                },
                Shortcut {
                    key: "s",
                    action: "Toggle case-sensitive matching",
                },
                Shortcut {
                    key: "z",
                    action: "Toggle fuzzy matching",
                },
                Shortcut {
                    key: "x",
                    action: "Toggle regex matching (while search applied)",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Actions",
                    action: "",
                },
                Shortcut {
                    key: "Enter",
                    action: "Open commit detail (or Uncommitted on the top row)",
                },
                Shortcut {
                    key: "Tab",
                    action: "Open refs panel",
                },
                Shortcut {
                    key: "Space",
                    action: "Mark / unmark a commit for 2-commit compare",
                },
                Shortcut {
                    key: "r",
                    action: "Refetch from remote",
                },
                Shortcut {
                    key: "]",
                    action: "Load older commits",
                },
                Shortcut {
                    key: "Shift+P",
                    action: "Push current branch",
                },
                Shortcut {
                    key: "Shift+U",
                    action: "Pull current branch",
                },
                Shortcut {
                    key: "Ctrl+A",
                    action: "Abort in-progress rebase / merge / cherry-pick",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Copy",
                    action: "",
                },
                Shortcut {
                    key: "c",
                    action: "Copy commit message",
                },
                Shortcut {
                    key: "Shift+C",
                    action: "Copy commit hash",
                },
            ],
        },
        HelpSection {
            title: "Commit Detail",
            intro: Some("Opens with Enter on a commit. All Navigation shortcuts apply."),
            shortcuts: &[
                Shortcut {
                    key: "Esc / Enter",
                    action: "Close detail",
                },
                Shortcut {
                    key: "b",
                    action: "Open blame on the focused file",
                },
                Shortcut {
                    key: "Shift+H",
                    action: "Open file history on the focused file",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Commit actions",
                    action: "",
                },
                Shortcut {
                    key: "t",
                    action: "Add tag",
                },
                Shortcut {
                    key: "Shift+B",
                    action: "Create branch from this commit",
                },
                Shortcut {
                    key: "o",
                    action: "Checkout this commit",
                },
                Shortcut {
                    key: "Shift+O",
                    action: "Cherry-pick",
                },
                Shortcut {
                    key: "d",
                    action: "Drop commit (uses interactive rebase)",
                },
                Shortcut {
                    key: "m",
                    action: "Merge",
                },
                Shortcut {
                    key: "e",
                    action: "Open interactive rebase from here",
                },
                Shortcut {
                    key: "Shift+S",
                    action: "Reset",
                },
                Shortcut {
                    key: "Ctrl+S",
                    action: "Squash with previous commit",
                },
                Shortcut {
                    key: "Ctrl+M",
                    action: "Amend HEAD message (HEAD only)",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Stash actions",
                    action: "(only on stash nodes)",
                },
                Shortcut {
                    key: "y",
                    action: "Apply stash (keep)",
                },
                Shortcut {
                    key: "Ctrl+P",
                    action: "Pop stash (apply + drop)",
                },
                Shortcut {
                    key: "Ctrl+X",
                    action: "Drop stash",
                },
                Shortcut {
                    key: "Ctrl+N",
                    action: "Create branch from stash",
                },
                Shortcut {
                    key: "Ctrl+I",
                    action: "Copy stash name",
                },
                Shortcut {
                    key: "Ctrl+O",
                    action: "Copy stash hash",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Copy",
                    action: "",
                },
                Shortcut {
                    key: "c",
                    action: "Copy commit message",
                },
                Shortcut {
                    key: "Shift+C",
                    action: "Copy commit hash",
                },
            ],
        },
        HelpSection {
            title: "Refs",
            intro: Some(
                "Opens with Tab from the commit list. Lists branches, tags, stashes, remotes.",
            ),
            shortcuts: &[
                Shortcut {
                    key: "Tab / Esc",
                    action: "Close refs panel",
                },
                Shortcut {
                    key: "Right",
                    action: "Open / expand node",
                },
                Shortcut {
                    key: "Left",
                    action: "Close / collapse node",
                },
                Shortcut {
                    key: "Enter",
                    action: "Open Branch Detail / Tag Detail / jump to ref",
                },
                Shortcut {
                    key: "r",
                    action: "Refetch",
                },
                Shortcut {
                    key: "Shift+C",
                    action: "Copy ref name",
                },
            ],
        },
        HelpSection {
            title: "Branch Detail",
            intro: Some("Opens with Enter on a branch in the Refs panel."),
            shortcuts: &[
                Shortcut {
                    key: "Esc",
                    action: "Close",
                },
                Shortcut {
                    key: "o",
                    action: "Checkout branch",
                },
                Shortcut {
                    key: "Ctrl+R",
                    action: "Rename branch",
                },
                Shortcut {
                    key: "Shift+D",
                    action: "Delete branch",
                },
                Shortcut {
                    key: "m",
                    action: "Merge into current branch",
                },
                Shortcut {
                    key: "e",
                    action: "Rebase onto this branch",
                },
                Shortcut {
                    key: "Shift+Q",
                    action: "Push branch",
                },
                Shortcut {
                    key: "Shift+U",
                    action: "Pull branch",
                },
                Shortcut {
                    key: "Ctrl+T",
                    action: "Set upstream",
                },
                Shortcut {
                    key: "Shift+E",
                    action: "Create archive (.tar.gz)",
                },
                Shortcut {
                    key: "Shift+T",
                    action: "Unselect branch (detach HEAD)",
                },
                Shortcut {
                    key: "Shift+V",
                    action: "Copy branch name",
                },
                Shortcut {
                    key: "r",
                    action: "Refetch",
                },
            ],
        },
        HelpSection {
            title: "Tag Detail",
            intro: Some("Opens with Enter on a tag in the Refs panel."),
            shortcuts: &[
                Shortcut {
                    key: "Esc",
                    action: "Close",
                },
                Shortcut {
                    key: "Shift+W",
                    action: "Push tag",
                },
                Shortcut {
                    key: "Shift+F",
                    action: "Delete tag",
                },
                Shortcut {
                    key: "Shift+Y",
                    action: "Copy tag name",
                },
                Shortcut {
                    key: "r",
                    action: "Refetch",
                },
            ],
        },
        HelpSection {
            title: "Uncommitted",
            intro: Some("Opens with Enter on the top row of the commit list (`(uncommitted)`)."),
            shortcuts: &[
                Shortcut {
                    key: "Esc",
                    action: "Back to commit list",
                },
                Shortcut {
                    key: "Up / Down",
                    action: "Navigate files within the active section",
                },
                Shortcut {
                    key: "Left / Right",
                    action: "Switch section (Unstaged / Staged / Untracked)",
                },
                Shortcut {
                    key: "Enter",
                    action: "Open diff of the focused file",
                },
                Shortcut {
                    key: "r",
                    action: "Refresh file lists",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Staging",
                    action: "",
                },
                Shortcut {
                    key: "a",
                    action: "Stage selected file",
                },
                Shortcut {
                    key: "Shift+A",
                    action: "Stage all unstaged + untracked",
                },
                Shortcut {
                    key: "u",
                    action: "Unstage selected file (Staged section)",
                },
                Shortcut {
                    key: "Shift+U",
                    action: "Unstage all staged files",
                },
                Shortcut {
                    key: "x",
                    action: "Discard selected unstaged change",
                },
                Shortcut {
                    key: "Shift+X",
                    action: "Discard all unstaged + staged changes",
                },
                Shortcut {
                    key: "v",
                    action: "Clean (delete) untracked file",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Save state",
                    action: "",
                },
                Shortcut {
                    key: "w",
                    action: "Commit (opens commit message editor)",
                },
                Shortcut {
                    key: "i",
                    action: "Stash",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Inspect",
                    action: "",
                },
                Shortcut {
                    key: "b",
                    action: "Blame the focused file",
                },
                Shortcut {
                    key: "Shift+H",
                    action: "File history of the focused file",
                },
            ],
        },
        HelpSection {
            title: "Diff",
            intro: Some("Opens with Enter on a file row, from commit Detail or Uncommitted."),
            shortcuts: &[
                Shortcut {
                    key: "Esc / Enter",
                    action: "Close diff",
                },
                Shortcut {
                    key: "j / k / arrows",
                    action: "Scroll",
                },
                Shortcut {
                    key: "Left / Right",
                    action: "Move focus across hunks / action buttons",
                },
                Shortcut {
                    key: "+",
                    action: "Cycle to the next file in the commit",
                },
                Shortcut {
                    key: "-",
                    action: "Cycle to the previous file",
                },
                Shortcut {
                    key: "r",
                    action: "Refresh",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Search",
                    action: "",
                },
                Shortcut {
                    key: "f",
                    action: "Toggle in-diff search",
                },
                Shortcut {
                    key: "n",
                    action: "Next match",
                },
                Shortcut {
                    key: "Shift+N",
                    action: "Previous match",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Inspect",
                    action: "",
                },
                Shortcut {
                    key: "b",
                    action: "Blame the current file",
                },
                Shortcut {
                    key: "Shift+H",
                    action: "Open file history",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Copy",
                    action: "",
                },
                Shortcut {
                    key: "c",
                    action: "Copy the file path",
                },
                Shortcut {
                    key: "Shift+C",
                    action: "Copy the commit hash",
                },
            ],
        },
        HelpSection {
            title: "Blame",
            intro: Some("Opens with `b` on a file."),
            shortcuts: &[
                Shortcut {
                    key: "Esc",
                    action: "Close blame",
                },
                Shortcut {
                    key: "j / k",
                    action: "Move line by line",
                },
                Shortcut {
                    key: "Left / Right",
                    action: "Jump to previous / next blame block",
                },
                Shortcut {
                    key: "PageUp / PageDown",
                    action: "Scroll a page",
                },
                Shortcut {
                    key: "g / Shift+G",
                    action: "Top / bottom",
                },
                Shortcut {
                    key: "Enter",
                    action: "Open the commit that authored the focused line",
                },
                Shortcut {
                    key: "Shift+H",
                    action: "Open file history (same file)",
                },
            ],
        },
        HelpSection {
            title: "File History",
            intro: Some("Opens with `Shift+H` on a file — equivalent of `git log --follow`."),
            shortcuts: &[
                Shortcut {
                    key: "Esc / Enter",
                    action: "Close history",
                },
                Shortcut {
                    key: "j / k / arrows",
                    action: "Navigate revisions",
                },
                Shortcut {
                    key: "PageUp / PageDown",
                    action: "Scroll a page",
                },
                Shortcut {
                    key: "g / Shift+G",
                    action: "Top / bottom",
                },
                Shortcut {
                    key: "Enter",
                    action: "Open commit detail for the focused revision",
                },
                Shortcut {
                    key: "b",
                    action: "Blame the file at this revision",
                },
                Shortcut {
                    key: "c",
                    action: "Copy commit message",
                },
                Shortcut {
                    key: "Shift+C",
                    action: "Copy commit hash",
                },
            ],
        },
        HelpSection {
            title: "Interactive Rebase",
            intro: Some("Opens with `e` on a commit. Edit the rebase plan, then Enter to apply."),
            shortcuts: &[
                Shortcut {
                    key: "Enter",
                    action: "Apply rebase plan",
                },
                Shortcut {
                    key: "Esc",
                    action: "Cancel (releases grab first if any)",
                },
                Shortcut {
                    key: "Up / Down",
                    action: "Move selection (or reorder when grabbed)",
                },
                Shortcut {
                    key: "Left / Right",
                    action: "Cycle action for the selected row",
                },
                Shortcut {
                    key: "Space",
                    action: "Grab / release the focused row",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Per-row action",
                    action: "",
                },
                Shortcut {
                    key: "p",
                    action: "Pick",
                },
                Shortcut {
                    key: "r",
                    action: "Reword (opens inline editor)",
                },
                Shortcut {
                    key: "e",
                    action: "Edit",
                },
                Shortcut {
                    key: "s",
                    action: "Squash into previous",
                },
                Shortcut {
                    key: "f",
                    action: "Fixup into previous",
                },
                Shortcut {
                    key: "d",
                    action: "Drop",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Stale rebase",
                    action: "(when resume banner shown)",
                },
                Shortcut {
                    key: "Shift+A",
                    action: "Abort the previously-stuck rebase",
                },
                Shortcut {
                    key: "c / Shift+C",
                    action: "Continue stuck rebase",
                },
                Shortcut {
                    key: "s / Shift+S",
                    action: "Skip current commit",
                },
                Shortcut {
                    key: "a / Shift+A",
                    action: "Abort and bail",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Reword editor",
                    action: "(while inline-editing a subject)",
                },
                Shortcut {
                    key: "Enter",
                    action: "Save reworded message",
                },
                Shortcut {
                    key: "Esc",
                    action: "Cancel reword",
                },
                Shortcut {
                    key: "Ctrl+H / Ctrl+W",
                    action: "Delete word to the left",
                },
                Shortcut {
                    key: "Ctrl+Backspace",
                    action: "Delete word to the left",
                },
                Shortcut {
                    key: "Ctrl+Delete",
                    action: "Delete word to the right",
                },
                Shortcut {
                    key: "Ctrl+Left / Right",
                    action: "Jump by word",
                },
            ],
        },
        HelpSection {
            title: "Conflict Editor",
            intro: Some("Opens automatically during a conflicted merge / rebase / cherry-pick."),
            shortcuts: &[
                Shortcut {
                    key: "Enter",
                    action: "Save resolution and mark as resolved",
                },
                Shortcut {
                    key: "Esc",
                    action: "Cancel and leave conflict in place",
                },
                Shortcut {
                    key: "Left / p",
                    action: "Previous conflict hunk",
                },
                Shortcut {
                    key: "Right / n",
                    action: "Next conflict hunk",
                },
                Shortcut {
                    key: "o",
                    action: "Pick OURS for the focused hunk",
                },
                Shortcut {
                    key: "t",
                    action: "Pick THEIRS",
                },
                Shortcut {
                    key: "b",
                    action: "Keep BOTH (ours first)",
                },
                Shortcut {
                    key: "Shift+B",
                    action: "Keep BOTH (theirs first)",
                },
                Shortcut {
                    key: "j / k / arrows",
                    action: "Scroll",
                },
            ],
        },
        HelpSection {
            title: "Compare (2-commit)",
            intro: Some("Opens after marking two commits with Space — shows the cumulative diff."),
            shortcuts: &[
                Shortcut {
                    key: "Esc",
                    action: "Close compare",
                },
                Shortcut {
                    key: "j / k / arrows",
                    action: "Navigate files / scroll",
                },
                Shortcut {
                    key: "Left / Right",
                    action: "Switch between file list and diff panel",
                },
                Shortcut {
                    key: "Enter",
                    action: "Open the focused file's diff",
                },
            ],
        },
        HelpSection {
            title: "Pull Requests",
            intro: Some("Opens with `Shift+R` (needs a GitHub remote + auth)."),
            shortcuts: &[
                Shortcut {
                    key: "List",
                    action: "",
                },
                Shortcut {
                    key: "Up / Down",
                    action: "Navigate PRs",
                },
                Shortcut {
                    key: "Left / Right",
                    action: "Cycle filter (Open / Merged / Closed / All)",
                },
                Shortcut {
                    key: "Enter",
                    action: "Open PR detail",
                },
                Shortcut {
                    key: "n",
                    action: "Compose a new PR",
                },
                Shortcut {
                    key: "r",
                    action: "Reload",
                },
                Shortcut {
                    key: "Esc",
                    action: "Close PR view",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Detail",
                    action: "",
                },
                Shortcut {
                    key: "Left / Right",
                    action: "Switch tab (Conversation / Files / Commits / References)",
                },
                Shortcut {
                    key: "Enter",
                    action: "Drill into focused file or commit row",
                },
                Shortcut {
                    key: "Esc",
                    action: "Back (drilldown → list → close)",
                },
                Shortcut {
                    key: "r",
                    action: "Reload PR",
                },
                Shortcut {
                    key: "o",
                    action: "Open PR in browser",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "PR actions",
                    action: "",
                },
                Shortcut {
                    key: "a",
                    action: "Approve review",
                },
                Shortcut {
                    key: "x",
                    action: "Request changes",
                },
                Shortcut {
                    key: "m",
                    action: "Merge PR",
                },
                Shortcut {
                    key: "l",
                    action: "Labels picker",
                },
                Shortcut {
                    key: "v",
                    action: "Reviewers picker",
                },
                Shortcut {
                    key: "Ctrl+X",
                    action: "Close PR",
                },
                Shortcut {
                    key: "Ctrl+O",
                    action: "Reopen PR",
                },
                Shortcut {
                    key: "Ctrl+D",
                    action: "Toggle draft state",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Conversation",
                    action: "(tab actions)",
                },
                Shortcut {
                    key: "c",
                    action: "New comment",
                },
                Shortcut {
                    key: "Shift+R",
                    action: "Quote-reply to focused comment",
                },
                Shortcut {
                    key: "e",
                    action: "Edit own comment",
                },
                Shortcut {
                    key: "d",
                    action: "Delete own comment",
                },
                Shortcut {
                    key: "+",
                    action: "React (emoji picker)",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Compose",
                    action: "(new PR / new comment / edit / reply)",
                },
                Shortcut {
                    key: "Tab",
                    action: "Move to the next field",
                },
                Shortcut {
                    key: "Ctrl+S",
                    action: "Submit",
                },
                Shortcut {
                    key: "Esc",
                    action: "Cancel",
                },
                Shortcut {
                    key: "#",
                    action: "Open the #N issue/PR mention popup",
                },
                Shortcut {
                    key: "Ctrl+H / Ctrl+W",
                    action: "Delete word to the left",
                },
                Shortcut {
                    key: "Ctrl+Left / Right",
                    action: "Jump by word",
                },
            ],
        },
        HelpSection {
            title: "Issues",
            intro: Some("Opens with `Shift+I` (needs a GitHub remote + auth)."),
            shortcuts: &[
                Shortcut {
                    key: "List",
                    action: "",
                },
                Shortcut {
                    key: "j / k / arrows",
                    action: "Navigate issues",
                },
                Shortcut {
                    key: "g / Shift+G",
                    action: "Top / bottom",
                },
                Shortcut {
                    key: "Left / Right",
                    action: "Cycle filter (Open / Closed / All)",
                },
                Shortcut {
                    key: "Enter",
                    action: "Open issue detail",
                },
                Shortcut {
                    key: "n",
                    action: "Compose a new issue",
                },
                Shortcut {
                    key: "r",
                    action: "Reload",
                },
                Shortcut {
                    key: "Esc",
                    action: "Close Issues view",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Detail",
                    action: "",
                },
                Shortcut {
                    key: "Left / Right",
                    action: "Switch tab (Conversation / Timeline / References)",
                },
                Shortcut {
                    key: "j / k",
                    action: "Move comment cursor (Conversation tab)",
                },
                Shortcut {
                    key: "g / Shift+G",
                    action: "First / last comment",
                },
                Shortcut {
                    key: "PageUp / PageDown",
                    action: "Scroll",
                },
                Shortcut {
                    key: "Ctrl+U / Ctrl+D",
                    action: "Half-page scroll",
                },
                Shortcut {
                    key: "Enter",
                    action: "Follow first #N ref (Conversation) / open ref (References)",
                },
                Shortcut {
                    key: "Esc",
                    action: "Back to list",
                },
                Shortcut {
                    key: "r",
                    action: "Reload issue",
                },
                Shortcut {
                    key: "o",
                    action: "Open issue in browser",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Issue actions",
                    action: "",
                },
                Shortcut {
                    key: "l",
                    action: "Labels picker",
                },
                Shortcut {
                    key: "a",
                    action: "Assignees picker",
                },
                Shortcut {
                    key: "m",
                    action: "Milestone picker",
                },
                Shortcut {
                    key: "x",
                    action: "Close or reopen issue",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Conversation",
                    action: "(tab actions)",
                },
                Shortcut {
                    key: "c",
                    action: "New comment",
                },
                Shortcut {
                    key: "Shift+R",
                    action: "Quote-reply to focused comment",
                },
                Shortcut {
                    key: "Shift+N",
                    action: "New issue with a back-reference to this one",
                },
                Shortcut {
                    key: "e",
                    action: "Edit own comment",
                },
                Shortcut {
                    key: "d",
                    action: "Delete own comment",
                },
                Shortcut {
                    key: "+",
                    action: "React (emoji picker)",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Compose",
                    action: "(new issue / new comment / edit / reply)",
                },
                Shortcut {
                    key: "Tab",
                    action: "Move to the next field",
                },
                Shortcut {
                    key: "Ctrl+S",
                    action: "Submit",
                },
                Shortcut {
                    key: "Esc",
                    action: "Cancel",
                },
                Shortcut {
                    key: "#",
                    action: "Open the #N issue/PR mention popup",
                },
                Shortcut {
                    key: "Ctrl+H / Ctrl+W",
                    action: "Delete word to the left",
                },
                Shortcut {
                    key: "Ctrl+Left / Right",
                    action: "Jump by word",
                },
            ],
        },
        HelpSection {
            title: "Configuration",
            intro: Some("Opens with `p` from the commit list. Edit themes, modes, and identity."),
            shortcuts: &[
                Shortcut {
                    key: "Esc / p",
                    action: "Close configuration",
                },
                Shortcut {
                    key: "j / k",
                    action: "Move between items",
                },
                Shortcut {
                    key: "h / l",
                    action: "Cycle value left / right (for cycle items)",
                },
                Shortcut {
                    key: "Enter",
                    action: "Edit text input or trigger button (GitHub auth)",
                },
                Shortcut {
                    key: "",
                    action: "",
                },
                Shortcut {
                    key: "Text edit mode",
                    action: "",
                },
                Shortcut {
                    key: "Enter",
                    action: "Save edit",
                },
                Shortcut {
                    key: "Esc",
                    action: "Cancel edit",
                },
                Shortcut {
                    key: "Ctrl+Backspace",
                    action: "Delete word to the left",
                },
                Shortcut {
                    key: "Backspace",
                    action: "Delete char to the left",
                },
            ],
        },
        HelpSection {
            title: "Help",
            intro: Some("This page!"),
            shortcuts: &[
                Shortcut {
                    key: "Esc / ? / F1",
                    action: "Close help",
                },
                Shortcut {
                    key: "j / k / arrows",
                    action: "Change section",
                },
                Shortcut {
                    key: "g / Shift+G",
                    action: "First / last section",
                },
                Shortcut {
                    key: "PageDown / PageUp",
                    action: "Scroll the right pane",
                },
                Shortcut {
                    key: "Ctrl+D / Ctrl+U",
                    action: "Half-page scroll the right pane",
                },
            ],
        },
    ]
}
