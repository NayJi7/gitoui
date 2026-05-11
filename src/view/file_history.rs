use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize, Modifier},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    git::actions::FileHistoryEntry,
    widget::commit_list::CommitListState,
};

/// Per-row layout numbers — pulled into constants so the click/hover
/// detection and the renderer agree on column boundaries.
const PREFIX_W: usize = 2; // "▶ " / "  "
const HASH_W: usize = 7;
const COL_GAP: usize = 2; // breathing room between hash / subject / author / date
const AUTHOR_W: usize = 16;
const DATE_W: usize = 14;

#[derive(Debug)]
pub struct FileHistoryView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    file_path: String,
    entries: Vec<FileHistoryEntry>,
    selected: usize,
    /// Row under the mouse cursor (None when mouse is outside the content
    /// area). Drives the transient row highlight.
    hovered: Option<usize>,
    scroll_offset: usize,
    view_height: usize,
    /// Cached so `handle_click` / `handle_mouse_move` can translate absolute
    /// terminal rows into entry indices. Populated by `render`.
    content_area: Option<Rect>,
    ctx: Rc<AppContext>,
    tx: Sender,
    /// Scroll position from the last avatar render pass.
    avatar_stable_key: Option<(usize, usize)>,
    /// Active row (hovered ?? selected) from the last avatar render pass.
    /// Used for the selective path: only the two rows that gained/lost active
    /// state are re-rendered; everything else is skipped.
    avatar_prev_active_row: Option<usize>,
}

impl<'a> FileHistoryView<'a> {
    pub fn new(
        commit_list_state: Option<CommitListState<'a>>,
        file_path: String,
        entries: Vec<FileHistoryEntry>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        Self {
            commit_list_state,
            file_path,
            entries,
            selected: 0,
            hovered: None,
            scroll_offset: 0,
            view_height: 0,
            content_area: None,
            ctx,
            tx,
            avatar_stable_key: None,
            avatar_prev_active_row: None,
        }
    }

    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
    }

    pub fn file_path(&self) -> &str {
        &self.file_path
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        Rc::make_mut(&mut self.ctx).color_theme = theme;
    }

    pub fn refresh(&self) {
        self.tx.send(AppEvent::OpenFileHistory {
            file_path: self.file_path.clone(),
        });
    }

    pub fn prepare_graph_uploads(&mut self) {
        if let Some(ref mut state) = self.commit_list_state {
            state.ensure_visible_graph_uploaded();
            state.ensure_visible_avatars_uploaded(
                &mut self.ctx.avatar_manager.lock().unwrap(),
                self.ctx.color_theme.bg,
                self.ctx.color_theme.list_selected_bg,
            );
        }
        self.ensure_visible_file_history_avatars();
    }

    fn ensure_visible_file_history_avatars(&mut self) {
        let bg = self.ctx.color_theme.bg;
        let start = self.scroll_offset;
        let end = (start + self.view_height).min(self.entries.len());
        if start >= end {
            return;
        }

        let hashes: Vec<String> = self.entries[start..end]
            .iter()
            .filter(|e| !e.author_email.is_empty())
            .map(|e| e.hash.clone())
            .collect();

        let mut avatar_manager = self.ctx.avatar_manager.lock().unwrap();
        if !avatar_manager.is_enabled() {
            return;
        }
        for entry in self.entries.iter().skip(start).take(end - start) {
            if entry.author_email.is_empty() {
                continue;
            }
            let email = &entry.author_email;
            if avatar_manager.prepared_image(email, 1, false).is_none() {
                if avatar_manager.cached_avatar_exists(email) {
                    avatar_manager.ensure_uploaded(email, 1, false, bg);
                } else {
                    avatar_manager.prefetch(hashes.clone(), email);
                }
            }
        }
    }

    pub fn clear_graph_images(&mut self) {
        if let Some(ref mut state) = self.commit_list_state {
            state.clear_graph_images();
        }
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        self.commit_list_state
            .as_mut()
            .map(|s| s.drain_pending_graph_uploads())
            .unwrap_or_default()
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        self.commit_list_state
            .as_ref()
            .map(|s| s.graph_image_ids_sorted())
            .unwrap_or_default()
    }

    /// Click anywhere on a row: select it AND open the commit in one gesture.
    /// Mirrors the Blame view's click flow — no two-step "select then confirm"
    /// dance.
    pub fn handle_click(&mut self, _col: u16, row: u16) {
        let Some(area) = self.content_area else { return };
        if row < area.y || row >= area.y + area.height {
            return;
        }
        let idx = self.scroll_offset + (row - area.y) as usize;
        if idx >= self.entries.len() {
            return;
        }
        self.selected = idx;
        self.hovered = None;
        self.open_selected();
    }

    pub fn handle_mouse_move(&mut self, _col: u16, row: u16) {
        let Some(area) = self.content_area else { return };
        if row < area.y || row >= area.y + area.height {
            if self.hovered.is_some() {
                self.hovered = None;
            }
            return;
        }
        let idx = self.scroll_offset + (row - area.y) as usize;
        let new_hover = if idx < self.entries.len() { Some(idx) } else { None };
        if new_hover != self.hovered {
            self.hovered = new_hover;
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _key: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;
        match event {
            UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::CloseFileHistory);
            }
            UserEvent::Confirm => {
                self.open_selected();
            }
            UserEvent::NavigateDown | UserEvent::ScrollDown => {
                for _ in 0..count {
                    self.move_down();
                }
            }
            UserEvent::NavigateUp | UserEvent::ScrollUp => {
                for _ in 0..count {
                    self.move_up();
                }
            }
            UserEvent::PageDown => {
                let n = self.view_height.max(1);
                for _ in 0..count {
                    for _ in 0..n {
                        self.move_down();
                    }
                }
            }
            UserEvent::PageUp => {
                let n = self.view_height.max(1);
                for _ in 0..count {
                    for _ in 0..n {
                        self.move_up();
                    }
                }
            }
            UserEvent::GoToTop => {
                self.selected = 0;
                self.scroll_offset = 0;
            }
            UserEvent::GoToBottom => {
                if !self.entries.is_empty() {
                    self.selected = self.entries.len() - 1;
                    self.scroll_to_selected();
                }
            }
            UserEvent::HelpToggle => {
                self.tx.send(AppEvent::OpenHelp);
            }
            UserEvent::Blame => {
                self.tx.send(AppEvent::OpenBlame {
                    file_path: self.file_path.clone(),
                });
            }
            UserEvent::ShortCopy => {
                if let Some(entry) = self.entries.get(self.selected) {
                    self.tx.send(AppEvent::CopyToClipboard {
                        name: "Commit SHA".into(),
                        value: entry.hash.clone(),
                    });
                }
            }
            UserEvent::FullCopy => {
                if let Some(entry) = self.entries.get(self.selected) {
                    self.tx.send(AppEvent::CopyToClipboard {
                        name: "Commit message".into(),
                        value: entry.subject.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let [sep_area, title_area, _spacer, content_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area);

        self.view_height = content_area.height as usize;
        self.content_area = Some(content_area);

        // ── Separator ──────────────────────────────────────────────────
        let separator = Line::from(
            "─".repeat(area.width as usize)
                .fg(self.ctx.color_theme.divider_fg),
        );
        f.render_widget(Paragraph::new(separator), sep_area);

        // ── Title ──────────────────────────────────────────────────────
        let title = Line::from(vec![
            Span::styled(
                format!("─── File History: {} ", self.file_path),
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "({} {})",
                    self.entries.len(),
                    if self.entries.len() == 1 { "commit" } else { "commits" }
                ),
                Style::default().fg(self.ctx.color_theme.list_hash_fg),
            ),
        ]);
        f.render_widget(Paragraph::new(title), title_area);

        self.scroll_to_selected();

        if self.entries.is_empty() {
            let placeholder = Line::from(Span::styled(
                " No history found for this file.",
                Style::default().fg(self.ctx.color_theme.status_warn_fg),
            ));
            f.render_widget(Paragraph::new(placeholder), content_area);
            return;
        }

        // ── Subject column gets whatever's left after the fixed columns ──
        let total_w = content_area.width as usize;
        let avatars_enabled = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        // 2 image cells + 1 space before the author name.
        let avatar_col_w: usize = if avatars_enabled { 3 } else { 0 };
        let fixed_w = PREFIX_W + HASH_W + COL_GAP + COL_GAP + avatar_col_w + AUTHOR_W + COL_GAP + DATE_W;
        let subject_w = total_w.saturating_sub(fixed_w).max(10);

        let theme = &self.ctx.color_theme;
        let head_fg = theme.list_head_fg;
        let hash_fg = theme.list_hash_fg;
        let subject_fg = theme.list_commit_message_fg;
        let author_fg = theme.list_name_fg;
        let date_fg = theme.list_date_fg;
        let selected_bg = theme.list_selected_bg;
        let normal_bg = theme.bg;

        let lines: Vec<Line> = self
            .entries
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(self.view_height)
            .map(|(i, entry)| {
                // Single active row — mouse hover wins over keyboard
                // selection so the highlight always follows the mouse when
                // present. Matches the Blame view exactly so the two
                // file-centric views feel identical.
                let active_row = self.hovered.unwrap_or(self.selected);
                let is_active = i == active_row;
                let bg = if is_active { selected_bg } else { normal_bg };

                // Selection arrow in head-fg colour so it visually echoes
                // the commit-list "current HEAD" marker.
                let prefix_span = if is_active {
                    Span::styled(
                        "▶ ",
                        Style::default()
                            .fg(head_fg)
                            .bg(bg)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    Span::styled("  ".to_string(), Style::default().bg(bg))
                };

                let subject = truncate_to_width(&entry.subject, subject_w);
                let author = truncate_to_width(&entry.author, AUTHOR_W);
                let date = truncate_to_width(&entry.date, DATE_W);

                let row_w_used = PREFIX_W
                    + HASH_W
                    + COL_GAP
                    + subject_w
                    + COL_GAP
                    + avatar_col_w
                    + AUTHOR_W
                    + COL_GAP
                    + DATE_W;
                let trailing_pad = total_w.saturating_sub(row_w_used);

                // Avatar placeholder (2 image cells + 1 space). The actual
                // Kitty image bytes are written into the buffer after the
                // Paragraph renders — these spaces just reserve the columns.
                let avatar_span = if avatars_enabled {
                    Span::styled("   ".to_string(), Style::default().bg(bg))
                } else {
                    Span::raw("")
                };

                Line::from(vec![
                    prefix_span,
                    Span::styled(
                        format!("{:width$}", entry.short_hash, width = HASH_W),
                        Style::default().fg(hash_fg).bg(bg),
                    ),
                    Span::styled(" ".repeat(COL_GAP), Style::default().bg(bg)),
                    Span::styled(
                        pad_right(&subject, subject_w),
                        Style::default().fg(subject_fg).bg(bg),
                    ),
                    Span::styled(" ".repeat(COL_GAP), Style::default().bg(bg)),
                    avatar_span,
                    Span::styled(
                        pad_right(&author, AUTHOR_W),
                        Style::default().fg(author_fg).bg(bg),
                    ),
                    Span::styled(" ".repeat(COL_GAP), Style::default().bg(bg)),
                    Span::styled(
                        pad_right(&date, DATE_W),
                        Style::default().fg(date_fg).bg(bg),
                    ),
                    Span::styled(" ".repeat(trailing_pad), Style::default().bg(bg)),
                ])
            })
            .collect();

        f.render_widget(Paragraph::new(lines), content_area);

        // ── Avatar image pass (3-path like commit_list) ─────────────────────
        if avatars_enabled {
            let active_row = self.hovered.unwrap_or(self.selected);
            let scroll_key = (self.scroll_offset, self.view_height);

            let scroll_stable = self.avatar_stable_key == Some(scroll_key);
            let select_stable = self.avatar_prev_active_row == Some(active_row);

            let avatar_col_start = (PREFIX_W + HASH_W + COL_GAP + subject_w + COL_GAP) as u16;
            let avatar_x = content_area.left() + avatar_col_start;

            let buf: &mut Buffer = f.buffer_mut();
            let clear_cell = self.ctx.image_protocol.clear_cell();
            let avatar_manager = self.ctx.avatar_manager.lock().unwrap();
            let normal_bg = self.ctx.color_theme.bg;
            let sel_bg = self.ctx.color_theme.list_selected_bg;

            // Helper: write avatar or clear for one row.
            macro_rules! write_row {
                ($j:expr, $entry:expr, $is_now_active:expr) => {{
                    let y = content_area.top() + ($j - self.scroll_offset) as u16;
                    let bg = if $is_now_active { sel_bg } else { normal_bg };
                    let prepared = avatar_manager.prepared_image(&$entry.author_email, 1, false);
                    if let Some(p) = prepared {
                        for (x, c) in p.cells().iter().enumerate() {
                            let cell = &mut buf[(avatar_x + x as u16, y)];
                            cell.set_symbol(c.symbol());
                            cell.set_style(c.style().bg(bg));
                            cell.set_skip(c.skip());
                        }
                    } else {
                        for x in 0..2u16 {
                            let cell = &mut buf[(avatar_x + x, y)];
                            cell.set_symbol(clear_cell.symbol());
                            cell.set_style(clear_cell.style().bg(bg));
                            cell.set_skip(clear_cell.skip());
                        }
                    }
                }};
            }

            if scroll_stable && select_stable {
                // ── Path 1: nothing changed ──────────────────────────────────
                for j in 0..self.view_height.min(self.entries.len().saturating_sub(self.scroll_offset)) {
                    let y = content_area.top() + j as u16;
                    for x in 0..2u16 {
                        buf[(avatar_x + x, y)].set_skip(true);
                    }
                }
            } else if scroll_stable {
                // ── Path 2: only selection changed ───────────────────────────
                let old_active = self.avatar_prev_active_row;

                for (j, entry) in self
                    .entries
                    .iter()
                    .enumerate()
                    .skip(self.scroll_offset)
                    .take(self.view_height)
                {
                    let y = content_area.top() + (j - self.scroll_offset) as u16;
                    let was_active = old_active == Some(j);
                    let is_now_active = j == active_row;

                    if !was_active && !is_now_active {
                        for x in 0..2u16 {
                            buf[(avatar_x + x, y)].set_skip(true);
                        }
                    } else if !entry.author_email.is_empty() {
                        write_row!(j, entry, is_now_active);
                    } else {
                        let bg = if is_now_active { sel_bg } else { normal_bg };
                        for x in 0..2u16 {
                            let cell = &mut buf[(avatar_x + x, y)];
                            cell.set_symbol(clear_cell.symbol());
                            cell.set_style(clear_cell.style().bg(bg));
                            cell.set_skip(clear_cell.skip());
                        }
                    }
                }
            } else {
                // ── Path 3: scroll changed — full render ─────────────────────
                for (j, entry) in self
                    .entries
                    .iter()
                    .enumerate()
                    .skip(self.scroll_offset)
                    .take(self.view_height)
                {
                    let y = content_area.top() + (j - self.scroll_offset) as u16;
                    let is_now_active = j == active_row;
                    let bg = if is_now_active { sel_bg } else { normal_bg };

                    if !entry.author_email.is_empty() {
                        write_row!(j, entry, is_now_active);
                    } else {
                        for x in 0..2u16 {
                            let cell = &mut buf[(avatar_x + x, y)];
                            cell.set_symbol(clear_cell.symbol());
                            cell.set_style(clear_cell.style().bg(bg));
                            cell.set_skip(clear_cell.skip());
                        }
                    }
                }
                self.avatar_stable_key = Some(scroll_key);
            }

            self.avatar_prev_active_row = Some(active_row);
        }
    }

    pub fn update_layout(&mut self, _area: Rect) {}

    pub fn footer_hint(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.entries.len() > 1 {
            parts.push("↑↓:navigate");
        }
        if !self.entries.is_empty() {
            parts.push("Enter:open commit");
            parts.push("c:msg");
            parts.push("C:hash");
        }
        parts.push("b:blame");
        parts.push("r:refresh");
        format!("⌘ {}", parts.join("▕▏"))
    }

    fn move_down(&mut self) {
        // Mirrors the Blame view's hover-aware nav: arrow keys anchor at the
        // mouse-hovered row when set, otherwise at the current selection.
        // After the move, hover is cleared so the highlight follows the
        // keyboard until the mouse moves again.
        let anchor = self.hovered.unwrap_or(self.selected);
        if anchor + 1 < self.entries.len() {
            self.selected = anchor + 1;
            self.hovered = None;
            self.scroll_to_selected();
        }
    }

    fn move_up(&mut self) {
        let anchor = self.hovered.unwrap_or(self.selected);
        if anchor > 0 {
            self.selected = anchor - 1;
            self.hovered = None;
            self.scroll_to_selected();
        }
    }

    fn scroll_to_selected(&mut self) {
        if self.view_height == 0 {
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + self.view_height {
            self.scroll_offset = self.selected + 1 - self.view_height;
        }
    }

    fn open_selected(&mut self) {
        if let Some(entry) = self.entries.get(self.selected) {
            self.tx.send(AppEvent::OpenDetailByHash {
                hash: entry.hash.clone(),
            });
        }
    }
}

fn truncate_to_width(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= w {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(w.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn pad_right(s: &str, w: usize) -> String {
    let count = s.chars().count();
    if count >= w {
        s.chars().take(w).collect()
    } else {
        let mut out = String::with_capacity(s.len() + (w - count));
        out.push_str(s);
        for _ in 0..(w - count) {
            out.push(' ');
        }
        out
    }
}
