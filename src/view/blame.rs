//! Git blame view — GitKraken-style annotation of a file at HEAD.
//!
//! Layout (single column, full terminal width):
//! ```text
//! ─── Blame: src/app.rs @ master ─────────────────────────────────── (N lines)
//! ▎ 9c44072 alice       2d ago  chore: prune unused files… │   42 │ pub fn open(
//! ▎                                                        │   43 │     ctx: &Ctx,
//! ▎ 5d8c91a alice       3w ago  feat: error handling       │   44 │ ) -> Result {
//! ```
//! Each commit gets a cycled color on the leftmost bar `▎` so contiguous
//! authorship blocks pop visually. The hash + author + relative time +
//! commit subject are only emitted on the FIRST line of a block; the bar
//! continues alone underneath. Enter on the cursor row opens the commit's
//! Detail view via `AppEvent::OpenDetailByHash`.

use std::{rc::Rc, time::Duration};

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use rustc_hash::FxHashMap;

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    git::blame::{relative_time, BlameLine},
    highlight::SyntaxHighlighter,
    widget::commit_list::CommitListState,
};

/// 8-color cycle used to colour the per-commit bar. Picked to be visible on
/// both dark and light backgrounds and visually distinct between adjacent
/// commits in the typical blame pattern.
const COMMIT_PALETTE: &[Color] = &[
    Color::Rgb(0xF0, 0x51, 0x33), // gitoui primary orange
    Color::Rgb(0x4F, 0xC1, 0xFF), // bright cyan
    Color::Rgb(0xA8, 0xF0, 0x6B), // lime
    Color::Rgb(0xFF, 0xCB, 0x6B), // amber
    Color::Rgb(0xC5, 0x8A, 0xFF), // violet
    Color::Rgb(0xFF, 0x8A, 0xC5), // pink
    Color::Rgb(0x82, 0xE6, 0xC8), // teal
    Color::Rgb(0xE6, 0xC8, 0x82), // sand
];

/// Contiguous run of `BlameLine`s that share the same commit. Built once at
/// construction so block navigation (↑↓ → next/previous block) is O(1) and
/// the renderer can resolve "which block does row N belong to" in O(log n).
#[derive(Debug, Clone)]
struct BlameBlock {
    /// First line index in `lines` (inclusive).
    start: usize,
    /// One past the last line index.
    end: usize,
}

#[derive(Debug)]
pub struct BlameView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    file_path: String,
    lines: Vec<BlameLine>,
    /// Grouped commit blocks — see `BlameBlock`.
    blocks: Vec<BlameBlock>,
    /// Index into `blocks` of the keyboard-focused commit group. Drives the
    /// row-wide grey highlight and `Enter` → open-commit target.
    focused_block: Option<usize>,
    /// Index into `blocks` of the block the mouse is over. Same visual as
    /// `focused_block`; takes precedence when set so the highlight follows
    /// the mouse.
    hovered_block: Option<usize>,
    scroll_offset: usize,
    view_height: usize,
    content_area: Option<Rect>,
    /// Deterministic per-SHA palette index.
    commit_colors: FxHashMap<String, usize>,
    ctx: Rc<AppContext>,
    tx: Sender,
    /// Cache: unix-timestamp (s) → (rendered string, computed_at). Avoids
    /// calling `Local::now()` + integer arithmetic for every visible row on
    /// every frame; entries are recomputed at most once per minute.
    rel_time_cache: FxHashMap<i64, (String, std::time::Instant)>,
    /// Scroll position from the last avatar render pass. Changing only
    /// causes a full re-render; selection changes use the lighter selective
    /// path instead (mirrors commit_list's 3-path approach).
    avatar_stable_key: Option<(usize, usize)>,
    /// Which block was active during the last avatar render pass. Used to
    /// detect selection-only changes so we only re-render the two affected
    /// block heads instead of every visible row.
    avatar_prev_active_block: Option<Option<usize>>,
}

/// How long a cached relative-time string stays valid. One minute matches
/// the coarsest sub-hour bucket ("Xm ago"), so strings never go stale by
/// more than one display unit.
const RELTIME_TTL: Duration = Duration::from_secs(60);

impl<'a> BlameView<'a> {
    pub fn new(
        commit_list_state: Option<CommitListState<'a>>,
        file_path: String,
        lines: Vec<BlameLine>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        // Pre-assign palette indices in first-appearance order so adjacent
        // distinct commits land on distinct colours.
        let mut commit_colors: FxHashMap<String, usize> = FxHashMap::default();
        let mut next_idx = 0usize;
        for l in &lines {
            commit_colors.entry(l.hash.clone()).or_insert_with(|| {
                let i = next_idx;
                next_idx = next_idx.wrapping_add(1);
                i
            });
        }

        // Walk lines and bucket consecutive same-hash rows into BlameBlocks.
        let mut blocks: Vec<BlameBlock> = Vec::new();
        let mut i = 0usize;
        while i < lines.len() {
            let start = i;
            let hash = lines[i].hash.clone();
            i += 1;
            while i < lines.len() && lines[i].hash == hash {
                i += 1;
            }
            blocks.push(BlameBlock { start, end: i });
        }

        let focused_block = if blocks.is_empty() { None } else { Some(0) };

        Self {
            commit_list_state,
            file_path,
            lines,
            blocks,
            focused_block,
            hovered_block: None,
            scroll_offset: 0,
            view_height: 0,
            content_area: None,
            commit_colors,
            ctx,
            tx,
            rel_time_cache: FxHashMap::default(),
            avatar_stable_key: None,
            avatar_prev_active_block: None,
        }
    }

    /// Return a cached relative-time string for `dt`. The cache entry is
    /// recomputed at most once per `RELTIME_TTL` so `Local::now()` is not
    /// called for every visible row on every frame.
    fn cached_relative_time(&mut self, dt: Option<&chrono::DateTime<chrono::Local>>) -> String {
        let Some(dt) = dt else {
            return "—".to_string();
        };
        let key = dt.timestamp();
        if let Some((s, computed_at)) = self.rel_time_cache.get(&key) {
            if computed_at.elapsed() < RELTIME_TTL {
                return s.clone();
            }
        }
        let s = relative_time(Some(dt));
        self.rel_time_cache
            .insert(key, (s.clone(), std::time::Instant::now()));
        s
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
        self.tx.send(AppEvent::OpenBlame {
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
        self.ensure_visible_blame_avatars();
    }

    /// Upload/prefetch avatars for the blame annotation column. Only the
    /// normal (non-selected) variant is needed since blame doesn't use a
    /// per-row selected-bg on the annotation side.
    fn ensure_visible_blame_avatars(&mut self) {
        let bg = self.ctx.color_theme.bg;
        let start = self.scroll_offset;
        let end = (start + self.view_height).min(self.lines.len());
        if start >= end {
            return;
        }

        // Collect commit hashes for GitHub prefetch (needed by avatar API).
        let hashes: Vec<String> = self.lines[start..end]
            .iter()
            .filter(|bl| !bl.author_mail.is_empty())
            .map(|bl| bl.hash.clone())
            .collect();

        let mut avatar_manager = self.ctx.avatar_manager.lock().unwrap();
        if !avatar_manager.is_enabled() {
            return;
        }
        for i in start..end {
            let bl = &self.lines[i];
            if bl.author_mail.is_empty() || !self.is_block_head(i) {
                continue;
            }
            let email = &bl.author_mail;
            // Only the normal (non-selected) variant. Preparing the selected
            // variant on-demand would require a synchronous PNG decode+composite
            // (~50–200 ms) the first time each block gains focus, causing
            // visible freezes when navigating through many distinct commits.
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

    pub fn handle_event(&mut self, event: UserEventWithCount, _key: KeyEvent) {
        let count = event.count;
        match event.event {
            UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::CloseBlame);
            }
            UserEvent::Confirm => self.open_focused_commit(),
            // ↑/↓ scroll the viewport line by line (no focus change).
            UserEvent::NavigateDown | UserEvent::ScrollDown => {
                for _ in 0..count {
                    self.scroll_lines(1);
                }
            }
            UserEvent::NavigateUp | UserEvent::ScrollUp => {
                for _ in 0..count {
                    self.scroll_lines(-1);
                }
            }
            // ←/→ jump between commit blocks; the block's first line pins to
            // the top of the viewport on each jump.
            UserEvent::NavigateRight => {
                for _ in 0..count {
                    self.focus_next_block();
                }
            }
            UserEvent::NavigateLeft => {
                for _ in 0..count {
                    self.focus_prev_block();
                }
            }
            UserEvent::PageDown => {
                let n = self.view_height.max(1) as isize;
                for _ in 0..count {
                    self.scroll_lines(n);
                }
            }
            UserEvent::PageUp => {
                let n = self.view_height.max(1) as isize;
                for _ in 0..count {
                    self.scroll_lines(-n);
                }
            }
            UserEvent::GoToTop => {
                self.scroll_offset = 0;
                if !self.blocks.is_empty() {
                    self.focused_block = Some(0);
                }
            }
            UserEvent::GoToBottom if !self.blocks.is_empty() => {
                self.focused_block = Some(self.blocks.len() - 1);
                self.scroll_to_focused();
            }
            UserEvent::HelpToggle => self.tx.send(AppEvent::OpenHelp),
            UserEvent::FileHistory => {
                self.tx.send(AppEvent::OpenFileHistory {
                    file_path: self.file_path.clone(),
                });
            }
            _ => {}
        }
    }

    pub fn handle_click(&mut self, _col: u16, row: u16) {
        let Some(area) = self.content_area else {
            return;
        };
        if row < area.y || row >= area.y + area.height {
            return;
        }
        let line_idx = self.scroll_offset + (row - area.y) as usize;
        if line_idx >= self.lines.len() {
            return;
        }
        // Click jumps straight into the commit — no two-step "select then
        // confirm" dance. Update the focused block first so the UI reflects
        // which line was actually clicked while the Detail view loads.
        if let Some(block_idx) = self.block_at_line(line_idx) {
            self.focused_block = Some(block_idx);
        }
        let hash = self.lines[line_idx].hash.clone();
        self.tx.send(AppEvent::OpenDetailByHash { hash });
    }

    pub fn handle_mouse_move(&mut self, _col: u16, row: u16) {
        let Some(area) = self.content_area else {
            return;
        };
        if row < area.y || row >= area.y + area.height {
            if self.hovered_block.is_some() {
                self.hovered_block = None;
            }
            return;
        }
        let line_idx = self.scroll_offset + (row - area.y) as usize;
        let block = if line_idx < self.lines.len() {
            self.block_at_line(line_idx)
        } else {
            None
        };
        if block != self.hovered_block {
            self.hovered_block = block;
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Unified header layout: title line (2-space indent + bold name +
        // semantic-coloured badges), divider line on the next row.
        let [title, sep, _spacer, content] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area);

        let theme = &self.ctx.color_theme;
        let title_line = Line::from(vec![
            Span::raw("  "),
            // `▤` (square with horizontal lines) reads as "stacked
            // rows of annotation" — the blame view's whole purpose.
            // Blue keeps it visually distinct from the warm-toned
            // PR / Issues icons.
            Span::styled(
                "▤ ",
                Style::default()
                    .fg(theme.status_info_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Blame ",
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                self.file_path.clone(),
                Style::default().fg(theme.list_hash_fg),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} lines", self.lines.len()),
                Style::default().fg(theme.detail_label_fg),
            ),
        ]);
        f.render_widget(Paragraph::new(title_line), title);

        let sep_line = Line::from("─".repeat(area.width as usize).fg(theme.divider_fg));
        f.render_widget(Paragraph::new(sep_line), sep);

        self.view_height = content.height as usize;
        self.content_area = Some(content);
        // Clamp scroll if the terminal shrank (or this is the first render
        // after view_height became known) so the focused block stays anchored.
        let max_scroll = self.lines.len().saturating_sub(self.view_height.max(1));
        if self.scroll_offset > max_scroll {
            self.scroll_offset = max_scroll;
        }

        if self.lines.is_empty() {
            let placeholder = Line::from(Span::styled(
                " No blame data available for this file.",
                Style::default().fg(self.ctx.color_theme.status_warn_fg),
            ));
            f.render_widget(Paragraph::new(placeholder), content);
            return;
        }

        let total_w = content.width as usize;
        // The left annotation column is sized to roughly 1/3 of the viewport.
        // Inside that budget, hash + relative time + line number columns are
        // fixed; author + subject grow with available space and disappear on
        // very narrow terminals.
        let bar_w: usize = 2; // "▎ "
        let hash_w: usize = 7;
        let reltime_w: usize = 8;
        let lineno_w: usize = 5;
        let sep_mid: usize = 1; // space between chrome columns
        let sep_border: usize = 3; // " │ "

        let avatars_enabled = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        // 2 image cells + 1 space separator before the hash column.
        let avatar_col_w: usize = if avatars_enabled { 3 } else { 0 };

        let target_left = total_w / 3;
        // Fixed part of the left column when neither author nor subject is shown.
        let fixed_left = bar_w
            + avatar_col_w
            + hash_w
            + sep_mid
            + reltime_w
            + sep_border
            + lineno_w
            + sep_border;
        let extra = target_left.saturating_sub(fixed_left);
        let (author_w, subject_w) = if extra >= 14 {
            // Roomy: author + subject share the leftover, author capped at 10.
            let aw = 10usize.min(extra);
            let sw = extra.saturating_sub(aw + sep_mid + sep_mid);
            (aw, sw)
        } else if extra >= 6 {
            // Medium: author only.
            (extra, 0)
        } else {
            (0, 0)
        };
        let show_author = author_w > 0;
        let show_subject = subject_w > 0;
        // Recompute actual left width with the variable columns.
        let actual_left = fixed_left
            + author_w
            + if show_author { sep_mid } else { 0 }
            + subject_w
            + if show_subject { sep_mid } else { 0 };
        let code_w = total_w.saturating_sub(actual_left).max(1);

        // Initialise a syntax highlighter once for this file — `highlight_line`
        // is stateful (multi-line context).
        let mut highlighter = SyntaxHighlighter::new_with_theme(
            &self.file_path,
            &self.ctx.core_config.option.syntax_theme,
        );

        let visible_range =
            self.scroll_offset..(self.scroll_offset + self.view_height).min(self.lines.len());

        // Extract timestamps into an owned Vec so `&mut self` is free for the
        // cache update below (avoids an aliasing conflict with self.lines).
        let timestamps: Vec<Option<chrono::DateTime<chrono::Local>>> = visible_range
            .clone()
            .map(|i| self.lines[i].author_time)
            .collect();
        let rel_times: Vec<String> = timestamps
            .iter()
            .map(|t| self.cached_relative_time(t.as_ref()))
            .collect();

        let visible_lines: Vec<Line<'static>> = visible_range
            .clone()
            .enumerate()
            .map(|(j, i)| {
                let bl = &self.lines[i];
                self.render_line(
                    bl,
                    i,
                    show_author,
                    show_subject,
                    avatars_enabled,
                    hash_w,
                    author_w,
                    &rel_times[j],
                    reltime_w,
                    subject_w,
                    lineno_w,
                    code_w,
                    highlighter.as_mut(),
                )
            })
            .collect();

        f.render_widget(Paragraph::new(visible_lines), content);

        // ── Avatar image pass (3-path like commit_list) ─────────────────────
        // Kitty images live in a separate terminal layer — written directly into
        // the buffer AFTER the Paragraph so text layout is already finalised.
        //
        // Path 1 — fully stable (scroll + selection unchanged): skip all cells.
        // Path 2 — selective (only selection changed): update only the two block
        //           heads that gained/lost active state; skip everything else.
        // Path 3 — full (scroll changed): re-render every visible row.
        //
        // Paths 1 and 2 emit O(1) Kitty APCs even for large files; only path 3
        // scales with row count (and it only fires on actual scrolls).
        if avatars_enabled {
            let scroll_key = (self.scroll_offset, self.view_height);
            let active_block = self.hovered_block.or(self.focused_block);

            let scroll_stable = self.avatar_stable_key == Some(scroll_key);
            let select_stable = self.avatar_prev_active_block == Some(active_block);

            let buf = f.buffer_mut();
            let avatar_x = content.left() + bar_w as u16;
            let avatar_manager = self.ctx.avatar_manager.lock().unwrap();
            let clear_cell = self.ctx.image_protocol.clear_cell();
            let normal_bg = self.ctx.color_theme.bg;
            let sel_bg = self.ctx.color_theme.list_selected_bg;

            // Helper: write avatar or clear for a block head row.
            macro_rules! write_head {
                ($j:expr, $i:expr, $is_now_active:expr) => {{
                    let y = content.top() + $j as u16;
                    let bl = &self.lines[$i];
                    let row_bg = if $is_now_active { sel_bg } else { normal_bg };
                    let prepared = avatar_manager.prepared_image(&bl.author_mail, 1, false);
                    if let Some(p) = prepared {
                        for (x, c) in p.cells().iter().enumerate() {
                            let cell = &mut buf[(avatar_x + x as u16, y)];
                            cell.set_symbol(c.symbol());
                            cell.set_style(c.style().bg(row_bg));
                            cell.set_skip(c.skip());
                        }
                    } else {
                        for x in 0..2u16 {
                            let cell = &mut buf[(avatar_x + x, y)];
                            cell.set_symbol(clear_cell.symbol());
                            cell.set_style(clear_cell.style().bg(row_bg));
                            cell.set_skip(clear_cell.skip());
                        }
                    }
                }};
            }

            if scroll_stable && select_stable {
                // ── Path 1: nothing changed ──────────────────────────────────
                for j in 0..self
                    .view_height
                    .min(self.lines.len().saturating_sub(self.scroll_offset))
                {
                    let y = content.top() + j as u16;
                    for x in 0..2u16 {
                        buf[(avatar_x + x, y)].set_skip(true);
                    }
                }
            } else if scroll_stable {
                // ── Path 2: selection changed, scroll same ───────────────────
                let old_active = self.avatar_prev_active_block.flatten();

                for (j, i) in visible_range.enumerate() {
                    let y = content.top() + j as u16;
                    let block_idx = self.block_at_line(i);
                    let is_head = self.is_block_head(i);

                    let was_active = old_active.is_some() && block_idx == old_active;
                    let is_now_active = active_block.is_some() && block_idx == active_block;

                    if !was_active && !is_now_active {
                        // Block state unchanged → preserve image or skip.
                        for x in 0..2u16 {
                            buf[(avatar_x + x, y)].set_skip(true);
                        }
                    } else if is_head {
                        write_head!(j, i, is_now_active);
                    } else {
                        // Continuation of a block that changed active state:
                        // bg needs updating. No image to delete — plain space.
                        let row_bg = if is_now_active { sel_bg } else { normal_bg };
                        for x in 0..2u16 {
                            let cell = &mut buf[(avatar_x + x, y)];
                            cell.set_symbol(" ");
                            cell.set_style(ratatui::style::Style::default().bg(row_bg));
                            cell.set_skip(false);
                        }
                    }
                }
            } else {
                // ── Path 3: scroll changed — full render ─────────────────────
                for (j, i) in visible_range.enumerate() {
                    let y = content.top() + j as u16;
                    let is_head = self.is_block_head(i);
                    let block_idx = self.block_at_line(i);
                    let is_now_active = active_block.is_some() && block_idx == active_block;
                    let row_bg = if is_now_active { sel_bg } else { normal_bg };

                    if is_head && !self.lines[i].author_mail.is_empty() {
                        write_head!(j, i, is_now_active);
                    } else {
                        // Continuation or missing email — clear any stale image.
                        for x in 0..2u16 {
                            let cell = &mut buf[(avatar_x + x, y)];
                            cell.set_symbol(clear_cell.symbol());
                            cell.set_style(clear_cell.style().bg(row_bg));
                            cell.set_skip(clear_cell.skip());
                        }
                    }
                }
                self.avatar_stable_key = Some(scroll_key);
            }

            self.avatar_prev_active_block = Some(active_block);
        }
    }

    pub fn update_layout(&mut self, _area: Rect) {}

    pub fn footer_hint(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        parts.push("↑↓:scroll");
        if self.blocks.len() > 1 {
            parts.push("←→:prev/next block");
        }
        if !self.blocks.is_empty() {
            parts.push("Enter:open commit");
        }
        parts.push("H:history");
        parts.push("r:refresh");
        format!("⌘ {}", parts.join("▕▏"))
    }

    fn focus_next_block(&mut self) {
        if self.blocks.is_empty() {
            return;
        }
        // Anchor the nav at whatever the user is visually seeing: mouse hover
        // wins (since that's what the row-bg highlights), keyboard focus next,
        // first block as the empty-state fallback. Then clear `hovered_block`
        // so the visual follows the keyboard until the mouse moves again.
        let anchor = self.hovered_block.or(self.focused_block);
        let next = match anchor {
            Some(i) => (i + 1).min(self.blocks.len() - 1),
            None => 0,
        };
        self.focused_block = Some(next);
        self.hovered_block = None;
        self.scroll_to_focused();
    }

    fn focus_prev_block(&mut self) {
        if self.blocks.is_empty() {
            return;
        }
        let anchor = self.hovered_block.or(self.focused_block);
        let prev = match anchor {
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        self.focused_block = Some(prev);
        self.hovered_block = None;
        self.scroll_to_focused();
    }

    /// Scroll-into-view (no pin): if the focused block's first line is
    /// already visible, leave the viewport alone. Otherwise scroll just
    /// enough to bring it back into the viewport — at the top if it was
    /// above, at the bottom if it was below. This avoids the "every ← /
    /// → press resets the scroll" feel.
    fn scroll_to_focused(&mut self) {
        if self.view_height == 0 {
            return;
        }
        if let Some(idx) = self.focused_block {
            if let Some(block) = self.blocks.get(idx) {
                let max_scroll = self.lines.len().saturating_sub(self.view_height);
                if block.start < self.scroll_offset {
                    self.scroll_offset = block.start;
                } else if block.start >= self.scroll_offset + self.view_height {
                    // Place the block's first line at the LAST visible row so
                    // it just barely enters the viewport from the bottom.
                    self.scroll_offset = (block.start + 1)
                        .saturating_sub(self.view_height)
                        .min(max_scroll);
                }
                // Otherwise the block is already visible — no scroll.
            }
        }
    }

    /// True viewport scroll — doesn't touch focus. Used by the mouse wheel
    /// and page-up/down so the user can survey context around the focused
    /// block without losing it.
    fn scroll_lines(&mut self, delta: isize) {
        if self.view_height == 0 {
            return;
        }
        let max_scroll = self.lines.len().saturating_sub(self.view_height);
        let new_off = self.scroll_offset as isize + delta;
        let clamped = new_off.max(0).min(max_scroll as isize);
        self.scroll_offset = clamped as usize;
    }

    fn block_at_line(&self, line: usize) -> Option<usize> {
        // Binary search would be more elegant but blocks lists are short
        // enough (a few hundred at most) that linear scan keeps the code
        // tight without measurable cost.
        self.blocks
            .iter()
            .position(|b| line >= b.start && line < b.end)
    }

    fn open_focused_commit(&self) {
        let Some(idx) = self.focused_block else {
            return;
        };
        if let Some(block) = self.blocks.get(idx) {
            if let Some(bl) = self.lines.get(block.start) {
                self.tx.send(AppEvent::OpenDetailByHash {
                    hash: bl.hash.clone(),
                });
            }
        }
    }

    fn commit_color(&self, hash: &str) -> Color {
        let idx = self.commit_colors.get(hash).copied().unwrap_or(0);
        COMMIT_PALETTE[idx % COMMIT_PALETTE.len()]
    }

    /// True if the previous row belongs to a different commit — drives the
    /// "show full annotation on the first line of a block, blank on the rest"
    /// pattern.
    fn is_block_head(&self, idx: usize) -> bool {
        if idx == 0 {
            return true;
        }
        self.lines[idx].hash != self.lines[idx - 1].hash
    }

    #[allow(clippy::too_many_arguments)]
    fn render_line(
        &self,
        bl: &BlameLine,
        idx: usize,
        show_author: bool,
        show_subject: bool,
        avatars_enabled: bool,
        hash_w: usize,
        author_w: usize,
        rel_time: &str,
        reltime_w: usize,
        subject_w: usize,
        lineno_w: usize,
        code_w: usize,
        highlighter: Option<&mut SyntaxHighlighter>,
    ) -> Line<'static> {
        let bar_color = self.commit_color(&bl.hash);
        // Same grey highlight for both keyboard focus and mouse hover — hover
        // wins when set so the highlight follows the mouse, otherwise focus
        // stays put. Matches the hunk row highlight in the Diff view exactly.
        let active_block = self.hovered_block.or(self.focused_block);
        let is_active = active_block
            .and_then(|i| self.blocks.get(i))
            .is_some_and(|b| idx >= b.start && idx < b.end);
        let row_bg = if is_active {
            self.ctx.color_theme.list_selected_bg
        } else {
            self.ctx.color_theme.bg
        };

        // Foreground palette aligned with the commit list — same token per
        // column so the user's eye treats them identically. `divider_fg` is
        // the same grey the Diff view uses for line numbers; we promote it
        // to the main `fg` on active rows so the dim grey doesn't drown in
        // the `list_selected_bg` highlight.
        let hash_fg = self.ctx.color_theme.list_hash_fg;
        let author_fg = self.ctx.color_theme.list_name_fg;
        let date_fg = self.ctx.color_theme.list_date_fg;
        let subject_fg = self.ctx.color_theme.list_commit_message_fg;
        // Line-number & separator chrome both use `divider_fg` (dim grey) at
        // rest, promoted to the theme's main `fg` on the active row so the
        // grey doesn't fade into the `list_selected_bg` highlight.
        let chrome_fg = if is_active {
            self.ctx.color_theme.fg
        } else {
            self.ctx.color_theme.divider_fg
        };
        let lineno_fg = chrome_fg;
        let sep_fg = chrome_fg;
        let code_base_style = Style::default().fg(self.ctx.color_theme.fg).bg(row_bg);

        let mut spans: Vec<Span<'static>> = Vec::new();
        // ▎ + space, coloured by commit.
        spans.push(Span::styled(
            "▎".to_string(),
            Style::default().fg(bar_color).bg(row_bg),
        ));
        spans.push(Span::styled(" ".to_string(), Style::default().bg(row_bg)));

        // Avatar placeholder — 2 image cells + 1 separator space. The actual
        // Kitty image bytes are written directly to the buffer AFTER the
        // Paragraph renders (see the avatar image pass in render()). Here we
        // just reserve the space so the following text columns don't overlap.
        if avatars_enabled {
            spans.push(Span::styled("   ".to_string(), Style::default().bg(row_bg)));
        }

        if self.is_block_head(idx) {
            // Hash
            spans.push(Span::styled(
                pad(&bl.short_hash, hash_w),
                Style::default()
                    .fg(hash_fg)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(" ".to_string(), Style::default().bg(row_bg)));

            if show_author {
                spans.push(Span::styled(
                    pad(&truncate(&bl.author, author_w), author_w),
                    Style::default().fg(author_fg).bg(row_bg),
                ));
                spans.push(Span::styled(" ".to_string(), Style::default().bg(row_bg)));
            }

            spans.push(Span::styled(
                pad(&truncate(rel_time, reltime_w), reltime_w),
                Style::default().fg(date_fg).bg(row_bg),
            ));
            spans.push(Span::styled(" ".to_string(), Style::default().bg(row_bg)));

            if show_subject {
                spans.push(Span::styled(
                    pad(&truncate(&bl.summary, subject_w), subject_w),
                    Style::default().fg(subject_fg).bg(row_bg),
                ));
                spans.push(Span::styled(" ".to_string(), Style::default().bg(row_bg)));
            }
        } else {
            // Continuation of a block — pad the annotation columns blank.
            let blank_w = hash_w
                + 1
                + author_w
                + if show_author { 1 } else { 0 }
                + reltime_w
                + 1
                + subject_w
                + if show_subject { 1 } else { 0 };
            spans.push(Span::styled(
                " ".repeat(blank_w),
                Style::default().bg(row_bg),
            ));
        }

        // Separator → lineno → separator → code
        spans.push(Span::styled(
            " │ ".to_string(),
            Style::default().fg(sep_fg).bg(row_bg),
        ));
        spans.push(Span::styled(
            format!("{:>w$}", bl.line_no, w = lineno_w),
            Style::default().fg(lineno_fg).bg(row_bg),
        ));
        spans.push(Span::styled(
            " │ ".to_string(),
            Style::default().fg(sep_fg).bg(row_bg),
        ));

        // Code with optional syntax highlighting, truncated to code_w cells.
        let mut code = bl.content.clone();
        // Replace tabs with 4 spaces so width math is correct.
        code = code.replace('\t', "    ");
        let truncated_code = truncate(&code, code_w);
        if let Some(h) = highlighter {
            let mut hl_spans = h.highlight_line(&truncated_code, code_base_style, None);
            // Apply row_bg overlay (syntect doesn't know about row_bg).
            for sp in &mut hl_spans {
                sp.style.bg = Some(row_bg);
            }
            spans.extend(hl_spans);
        } else {
            spans.push(Span::styled(truncated_code, code_base_style));
        }

        Line::from(spans)
    }
}

/// Pad a string with spaces on the right to exactly `w` cells. Truncates with
/// `…` if longer (truncate is done by `truncate` separately).
fn pad(s: &str, w: usize) -> String {
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

fn truncate(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= w {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(w - 1).collect();
        out.push('…');
        out
    }
}
