//! 2-commit comparison view.
//!
//! Two-pane layout inspired by GitKraken / lazygit:
//! - **Left**: list of files that differ between the two commits, with status
//!   indicators (M / A / D / R) and per-file +N -M counters.
//! - **Right**: full diff of the currently-selected file. Internally this is
//!   a regular `DiffView` rebuilt on every selection change — it inherits all
//!   the existing diff features (Enhanced / Raw / SideBySide rendering modes,
//!   gap navigation with show-more, syntax highlighting, search-within-diff).
//!
//! Tab switches focus between panes; ↑↓ navigates whichever pane has focus;
//! Esc closes back to the commit list.
//!
//! The view OWNS the `CommitListState` (returned via `take_list_state()`
//! when the user closes) so the caller can restore the commit list as it
//! was when the comparison was opened.

use std::path::PathBuf;
use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    git::diff::{DiffEntry, DiffLineType},
    view::diff::DiffView,
    widget::commit_list::CommitListState,
};

/// Which "cursor" the user moved last. Both cursors (the highlighted file row
/// in the left pane and the focused show-more button in the right pane) are
/// always visible — `ActiveCursor` only tracks which one Enter should
/// activate. Updated by every navigation key:
/// - ↑↓ → `File`
/// - ←→ → `Button`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveCursor {
    File,
    Button,
}

/// Per-file summary line shown in the left pane.
#[derive(Debug, Clone)]
struct FileSummary {
    /// Display path (the new path if available, else the old).
    path: String,
    /// `M` modified, `A` added, `D` deleted, `R` renamed.
    status: char,
    add_count: usize,
    del_count: usize,
}

impl FileSummary {
    fn from_entry(entry: &DiffEntry) -> Self {
        let (status, path) = match (&entry.old_path, &entry.new_path) {
            (None, Some(np)) => ('A', np.clone()),
            (Some(op), None) => ('D', op.clone()),
            (Some(op), Some(np)) if op != np => ('R', np.clone()),
            (Some(_), Some(np)) => ('M', np.clone()),
            (None, None) => ('?', String::new()),
        };
        let mut add_count = 0;
        let mut del_count = 0;
        for hunk in &entry.hunks {
            for line in &hunk.lines {
                match line.line_type {
                    DiffLineType::Addition => add_count += 1,
                    DiffLineType::Deletion => del_count += 1,
                    _ => {}
                }
            }
        }
        FileSummary {
            path,
            status,
            add_count,
            del_count,
        }
    }
}

#[derive(Debug)]
pub struct CompareView<'a> {
    /// Returned via `take_list_state()` when the user closes the view, so
    /// the caller can rebuild `View::List` with the commit list intact.
    commit_list_state: Option<CommitListState<'a>>,

    /// All files that differ between the two endpoints. Index into this and
    /// `file_summaries` is the sole "file selection" state.
    diff_entries: Vec<DiffEntry>,
    file_summaries: Vec<FileSummary>,

    /// Index of the file currently shown in the right pane (committed by
    /// click or by ↑↓ navigation — NOT by hover).
    selected_file_idx: usize,
    /// Index of the file currently under the mouse, if any. Pure visual
    /// state — does not change the displayed diff. Cleared when the mouse
    /// leaves the file pane.
    hovered_file_idx: Option<usize>,
    /// First visible row of the file list (for scrolling when the list is
    /// taller than the available height).
    files_offset: usize,
    /// Cached visible height of the file list (updated by `update_layout`).
    files_height: usize,

    active_cursor: ActiveCursor,

    /// The diff pane is delegated to a regular `DiffView`. Rebuilt every time
    /// the file selection changes — DiffView::new is cheap (no I/O), so this
    /// stays snappy.
    diff_pane: Option<DiffView<'a>>,

    /// Endpoints — older on the left/red, newer on the right/green.
    older_hash: String,
    newer_hash: String,

    repo_path: PathBuf,

    /// Layout rectangles cached from `update_layout` so `handle_click` can
    /// route mouse coordinates to the right pane.
    files_area: Rect,
    diff_area: Rect,

    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> CompareView<'a> {
    pub fn new(
        commit_list_state: CommitListState<'a>,
        diff_entries: Vec<DiffEntry>,
        older_hash: String,
        newer_hash: String,
        repo_path: PathBuf,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> CompareView<'a> {
        let file_summaries: Vec<FileSummary> =
            diff_entries.iter().map(FileSummary::from_entry).collect();
        let mut view = CompareView {
            commit_list_state: Some(commit_list_state),
            diff_entries,
            file_summaries,
            selected_file_idx: 0,
            hovered_file_idx: None,
            files_offset: 0,
            files_height: 0,
            active_cursor: ActiveCursor::File,
            diff_pane: None,
            older_hash,
            newer_hash,
            repo_path,
            files_area: Rect::default(),
            diff_area: Rect::default(),
            ctx,
            tx,
        };
        view.rebuild_diff_pane();
        view
    }

    /// Rebuild the inner DiffView around the currently-selected file. Called
    /// at construction and on every file selection change.
    fn rebuild_diff_pane(&mut self) {
        let Some(entry) = self.diff_entries.get(self.selected_file_idx).cloned() else {
            self.diff_pane = None;
            return;
        };
        let short = |h: &str| h.chars().take(7).collect::<String>();
        // Title format mirrors the standalone compare-diff title — the
        // status-line / header rendering picks this up to colorize the SHAs.
        let title = format!("Compare {}..{}", short(&self.older_hash), short(&self.newer_hash));
        self.diff_pane = Some(DiffView::new(
            None, // commit list is owned by the CompareView, not the inner DiffView
            vec![entry],
            self.ctx.clone(),
            self.tx.clone(),
            title,
            self.newer_hash.clone(),
            Vec::new(), // no file cycling at the inner level — files navigate through the left pane
            self.repo_path.clone(),
        ));
    }

    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        std::rc::Rc::make_mut(&mut self.ctx).color_theme = theme.clone();
        if let Some(pane) = &mut self.diff_pane {
            pane.update_color_theme(theme);
        }
    }

    pub fn refresh(&self) {
        // No-op for now — gitoui auto-refresh fires on file system changes,
        // and the compare view is bound to two specific commit SHAs that
        // can't change underneath us. If we later support comparing a
        // commit to HEAD, refresh would re-load the diff.
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count.max(1);

        // Cancel always closes the view (highest priority).
        if matches!(event, UserEvent::Cancel) {
            self.tx.send(AppEvent::CloseDiff);
            return;
        }

        // Global passthroughs.
        match event {
            UserEvent::Quit => {
                self.tx.send(AppEvent::Quit);
                return;
            }
            UserEvent::HelpToggle => {
                self.tx.send(AppEvent::OpenHelp);
                return;
            }
            _ => {}
        }

        // Vertical navigation always targets the FILE list. Updates
        // `active_cursor` so the next Enter triggers the file action (a no-op
        // for now — the file is already shown in the right pane).
        if matches!(
            event,
            UserEvent::NavigateUp
                | UserEvent::NavigateDown
                | UserEvent::SelectUp
                | UserEvent::SelectDown
                | UserEvent::GoToTop
                | UserEvent::GoToBottom
        ) {
            self.move_file_cursor(event, count);
            self.active_cursor = ActiveCursor::File;
            return;
        }

        // Mouse wheel: route to the pane the cursor is currently over. When
        // hovering the file list the wheel scrolls its offset (no selection
        // change — the user must click or use ↑↓ to commit a new file);
        // otherwise the wheel falls through to the inner DiffView.
        if matches!(event, UserEvent::ScrollUp | UserEvent::ScrollDown) {
            if self.hovered_file_idx.is_some() {
                let delta = count as isize
                    * if matches!(event, UserEvent::ScrollUp) { -1 } else { 1 };
                self.scroll_files(delta);
                return;
            }
            // else: forward to diff pane below.
        }

        // Horizontal navigation always targets the show-more BUTTONS in the
        // diff pane. The inner DiffView already implements
        // NavigateLeft/Right to cycle its `focused_button`; we just forward.
        if matches!(event, UserEvent::NavigateLeft | UserEvent::NavigateRight) {
            if let Some(pane) = &mut self.diff_pane {
                pane.handle_event(event_with_count, key);
            }
            self.active_cursor = ActiveCursor::Button;
            return;
        }

        // Enter activates whichever cursor was used most recently.
        if matches!(event, UserEvent::Confirm) {
            if self.active_cursor == ActiveCursor::Button {
                if let Some(pane) = &mut self.diff_pane {
                    pane.handle_event(event_with_count, key);
                }
            }
            // File-cursor Enter is a no-op for v1: the diff is already
            // displayed in the right pane. (Future: open the file in
            // fullscreen DiffView for more vertical space.)
            return;
        }

        // Everything else (scroll, search, in-diff actions) goes to the
        // diff pane so its existing keyboard surface keeps working —
        // search-in-diff, copy-path, etc.
        if let Some(pane) = &mut self.diff_pane {
            pane.handle_event(event_with_count, key);
        }
    }

    fn move_file_cursor(&mut self, event: UserEvent, count: usize) {
        let total = self.file_summaries.len();
        if total == 0 {
            return;
        }
        let new_idx = match event {
            UserEvent::NavigateDown | UserEvent::SelectDown => {
                (self.selected_file_idx + count).min(total - 1)
            }
            UserEvent::NavigateUp | UserEvent::SelectUp => {
                self.selected_file_idx.saturating_sub(count)
            }
            UserEvent::GoToTop => 0,
            UserEvent::GoToBottom => total - 1,
            _ => return,
        };
        if new_idx != self.selected_file_idx {
            self.selected_file_idx = new_idx;
            self.ensure_visible();
            self.rebuild_diff_pane();
        }
    }

    /// Scroll the file list offset by `delta` rows (negative = up, positive
    /// = down). Does NOT change `selected_file_idx` — the user must hover
    /// or click to change selection. Bounds the offset so we never scroll
    /// past either end of the list.
    fn scroll_files(&mut self, delta: isize) {
        let total = self.file_summaries.len();
        if total == 0 || self.files_height == 0 {
            return;
        }
        let max_offset = total.saturating_sub(self.files_height);
        let new_offset = if delta < 0 {
            self.files_offset.saturating_sub((-delta) as usize)
        } else {
            (self.files_offset + delta as usize).min(max_offset)
        };
        self.files_offset = new_offset;
    }

    fn ensure_visible(&mut self) {
        if self.files_height == 0 {
            return;
        }
        if self.selected_file_idx < self.files_offset {
            self.files_offset = self.selected_file_idx;
        } else if self.selected_file_idx >= self.files_offset + self.files_height {
            self.files_offset = self.selected_file_idx + 1 - self.files_height;
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Header (full width, 1 row) + body (rest).
        let [header_area, body_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area);

        self.render_header(f, header_area);

        // 30/70 horizontal split. On terminals narrower than 80 cols, fall
        // back to a wider files pane (40%) since absolute width matters more
        // than ratio there.
        let files_pct = if body_area.width < 80 { 40 } else { 30 };
        let [files_area, diff_area] = Layout::horizontal([
            Constraint::Percentage(files_pct),
            Constraint::Percentage(100 - files_pct),
        ])
        .areas(body_area);

        self.files_area = files_area;
        self.diff_area = diff_area;

        self.render_files(f, files_area);

        if let Some(pane) = &mut self.diff_pane {
            pane.render(f, diff_area);
        }
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let short = |h: &str| h.chars().take(7).collect::<String>();
        let total_add: usize = self.file_summaries.iter().map(|f| f.add_count).sum();
        let total_del: usize = self.file_summaries.iter().map(|f| f.del_count).sum();
        let n_files = self.file_summaries.len();

        let mut spans: Vec<Span<'static>> = vec![
            Span::styled(
                "── Compare ".to_string(),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                short(&self.older_hash),
                Style::default()
                    .fg(theme.detail_file_change_delete_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " → ".to_string(),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                short(&self.newer_hash),
                Style::default()
                    .fg(theme.detail_file_change_add_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ─ {} file{} ", n_files, if n_files == 1 { "" } else { "s" }),
                Style::default().fg(theme.fg),
            ),
        ];
        if total_add > 0 {
            spans.push(Span::styled(
                format!("+{total_add} "),
                Style::default()
                    .fg(theme.detail_file_change_add_fg)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if total_del > 0 {
            spans.push(Span::styled(
                format!("-{total_del} "),
                Style::default()
                    .fg(theme.detail_file_change_delete_fg)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        spans.push(Span::styled(
            "──".to_string(),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_files(&mut self, f: &mut Frame, area: Rect) {
        // Snapshot only the colors we need from the theme so the rest of the
        // function can take a `&mut self` (for ensure_visible) without holding
        // an immutable borrow.
        let theme_fg = self.ctx.color_theme.fg;
        let theme_divider = self.ctx.color_theme.divider_fg;
        // Status letter colors mirror the commit-detail file tree exactly so
        // M/A/D/R read identically across the two views.
        let theme_add = self.ctx.color_theme.detail_file_change_add_fg;
        let theme_modify = self.ctx.color_theme.detail_file_change_modify_fg;
        let theme_del = self.ctx.color_theme.detail_file_change_delete_fg;
        let theme_move = self.ctx.color_theme.detail_file_change_move_fg;
        let theme_head = self.ctx.color_theme.list_head_fg;
        let theme_sel_bg = self.ctx.color_theme.list_selected_bg;
        let theme_sel_fg = self.ctx.color_theme.list_selected_fg;

        // Bordered block on the left pane — border brightens when the file
        // cursor is the most recently used (signals "Enter would land here").
        let border_style = if self.active_cursor == ActiveCursor::File {
            Style::default().fg(theme_fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme_divider)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(format!(" Files ({}) ", self.file_summaries.len()));
        let inner = block.inner(area);
        f.render_widget(block, area);

        self.files_height = inner.height as usize;
        // NOTE: do NOT call `ensure_visible()` here. It's only meaningful
        // after a SELECTION change (↑↓ navigation, click) — not on every
        // render. Calling it at render-time would clamp `files_offset` back
        // toward `selected_file_idx`, instantly undoing any wheel-scroll
        // the user just performed. ensure_visible is invoked from the
        // selection-changing code paths (`move_file_cursor`, `handle_click`
        // on the files pane, `rebuild_diff_pane` callers).

        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, summary) in self
            .file_summaries
            .iter()
            .enumerate()
            .skip(self.files_offset)
            .take(self.files_height)
        {
            let status_color = match summary.status {
                'A' => theme_add,
                'M' => theme_modify,
                'D' => theme_del,
                'R' => theme_move,
                _ => theme_fg,
            };
            let is_selected = i == self.selected_file_idx;
            let is_hovered = self.hovered_file_idx == Some(i);

            // Selection indicator: `▶` in the theme's accent color
            // (`list_head_fg` — typically cyan/teal/blue, the same colour
            // used for HEAD elsewhere). Same glyph as the FileHistory view
            // so the "selected row" cue feels uniform across the app.
            let leading = if is_selected { "▶" } else { " " };
            let leading_style = if is_selected {
                Style::default().fg(theme_head).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme_fg)
            };

            let mut spans: Vec<Span<'static>> = vec![
                Span::styled(leading.to_string(), leading_style),
                Span::styled(
                    format!(" {} ", summary.status),
                    Style::default().fg(status_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(" {}", summary.path),
                    Style::default().fg(theme_fg),
                ),
            ];
            if summary.add_count > 0 {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(
                    format!("+{}", summary.add_count),
                    Style::default().fg(theme_add).add_modifier(Modifier::BOLD),
                ));
            }
            if summary.del_count > 0 {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    format!("-{}", summary.del_count),
                    Style::default().fg(theme_del).add_modifier(Modifier::BOLD),
                ));
            }

            let mut line = Line::from(spans);
            // Hover indicator: full-row background highlight — strong cue
            // that "a click here would commit this row". When the hovered
            // row is also the selected row, both signals stack: bg shows
            // hover, leading `▸` keeps its accent color via per-span style.
            if is_hovered {
                line = line.bg(theme_sel_bg).fg(theme_sel_fg);
            }
            lines.push(line);
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    pub fn update_layout(&mut self, area: Rect) {
        // The header takes 1 row; the rest is the body. We don't actually
        // need the precise files area height ahead of time — render_files
        // computes it from `inner.height`. But forwarding the area to the
        // inner DiffView lets it precompute viewport-dependent state.
        let body_area = if area.height >= 1 {
            Rect::new(area.x, area.y + 1, area.width, area.height - 1)
        } else {
            area
        };
        let files_pct = if body_area.width < 80 { 40 } else { 30 };
        let [_files_area, diff_area] = Layout::horizontal([
            Constraint::Percentage(files_pct),
            Constraint::Percentage(100 - files_pct),
        ])
        .areas(body_area);
        if let Some(pane) = &mut self.diff_pane {
            pane.update_layout(diff_area);
        }
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        // Click in the files pane → select that file and mark the file
        // cursor as active.
        if self.files_area.contains(ratatui::layout::Position { x: col, y: row }) {
            if row > self.files_area.y {
                let inner_row = (row - self.files_area.y - 1) as usize;
                let target = self.files_offset + inner_row;
                if target < self.file_summaries.len() {
                    if target != self.selected_file_idx {
                        self.selected_file_idx = target;
                        self.ensure_visible();
                        self.rebuild_diff_pane();
                    }
                    self.active_cursor = ActiveCursor::File;
                }
            }
            return;
        }
        // Click in the diff pane → forward (the inner DiffView handles
        // show-more buttons + scroll-on-click). Mark the button cursor as
        // active so the next Enter targets the diff pane too.
        if self.diff_area.contains(ratatui::layout::Position { x: col, y: row }) {
            if let Some(pane) = &mut self.diff_pane {
                pane.handle_click(col, row);
            }
            self.active_cursor = ActiveCursor::Button;
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) -> bool {
        // File pane hover: pure visual preview — sets `hovered_file_idx`
        // without touching `selected_file_idx` or the displayed diff. The
        // user must click to actually swap the right pane.
        let in_files = self
            .files_area
            .contains(ratatui::layout::Position { x: col, y: row });
        if in_files && row > self.files_area.y {
            let inner_row = (row - self.files_area.y - 1) as usize;
            let target = self.files_offset + inner_row;
            self.hovered_file_idx = if target < self.file_summaries.len() {
                Some(target)
            } else {
                None
            };
        } else {
            // Mouse out of the file pane → clear hover.
            self.hovered_file_idx = None;
        }

        // Always forward to the diff pane: even when the mouse is in the
        // files area, DiffView needs the event to clear its own
        // `focused_button` (mirrors the contract of plain DiffView hover).
        if let Some(pane) = &mut self.diff_pane {
            pane.handle_mouse_move(col, row);
        }
        true
    }

    pub fn footer_hint(&self) -> String {
        // Two-cursor model: the file row in the left pane and the focused
        // show-more button in the right pane move independently. Enter
        // activates whichever cursor was used last.
        "⌘ ↑↓:files▕▏←→:show-more▕▏Enter:expand▕▏Esc:close".to_string()
    }

    pub fn prepare_graph_uploads(&mut self) {
        if let Some(pane) = &mut self.diff_pane {
            pane.prepare_graph_uploads();
        }
    }

    pub fn clear_graph_images(&mut self) {
        if let Some(pane) = &mut self.diff_pane {
            pane.clear_graph_images();
        }
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        if let Some(pane) = &mut self.diff_pane {
            pane.drain_pending_graph_uploads()
        } else {
            Vec::new()
        }
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        if let Some(pane) = &self.diff_pane {
            pane.graph_image_ids_sorted()
        } else {
            Vec::new()
        }
    }

    pub fn as_list_state(&self) -> Option<&CommitListState<'a>> {
        self.diff_pane
            .as_ref()
            .and_then(|p| p.as_list_state())
            .or(self.commit_list_state.as_ref())
    }
}
