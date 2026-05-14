//! Merge-conflict editor — VS Code / GitKraken inspired three-way merge UI.
//!
//! Opens on a single conflicted file (status = `Unmerged` in the uncommitted
//! view). The user navigates between conflict hunks with `n`/`p`, picks a
//! resolution for each (`o`/`t`/`b`/`B`), then saves with `Enter` which writes
//! the resolved file and stages it via `git add`.
//!
//! Layout modes — selectable via `[ui.common] conflict_view`:
//! - `three-pane`: Ours | Base | Theirs row + Result preview row below
//! - `two-pane`  : Ours | Theirs row + Result preview row below
//! - `inline`    : full-width Ours / Theirs / Result stacked vertically
//!
//! All three share the same keyboard map and colour palette — only the
//! spatial arrangement of the source panes changes. The Result preview is
//! always shown so the user can see the file they will end up committing.

use std::{path::PathBuf, rc::Rc};

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::{
    app::AppContext,
    config::ConflictViewMode,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    git::conflict::{ConflictFile, FileSegment, HunkResolution},
    highlight::SyntaxHighlighter,
    widget::commit_list::CommitListState,
};

/// Internal abstraction over the three source panes. The Result preview has
/// its own dedicated render path, so it is not part of this enum.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Ours,
    Base,
    Theirs,
}

/// Tagged origin used to colour annotation badges in the Result preview.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineOrigin {
    Context,
    FromOurs,
    FromTheirs,
    Unresolved,
}

/// Visual state of a hunk row — drives the row's background colour, the
/// side-bar glyph, and the bold/normal weight of the syntax-coloured text.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HunkState {
    /// Not part of a hunk — neutral context line.
    Context,
    /// Hunk that is neither focused nor under the mouse.
    Idle,
    /// Mouse is hovering this hunk.
    Hovered,
    /// Hunk the user is currently resolving (keyboard focus / clicked).
    Current,
}

/// Per-pane rect + hunk-index lookup table built each frame and consumed
/// by the mouse handlers to map (col, row) back to a hunk.
#[derive(Debug)]
struct ResultCache {
    resolutions: Vec<HunkResolution>,
    inner_width: u16,
    /// Current hunk at the time the cache was built. Must be part of the
    /// key because the "YOU ARE HERE" marker on Unresolved placeholders
    /// targets the currently-focused hunk — without this, the marker
    /// freezes on the first hunk forever.
    current_hunk: usize,
    lines: Vec<RenderedLine>,
}

#[derive(Clone, Debug)]
struct PaneHitMap {
    /// Inner area of the pane (excluding the border).
    inner: Rect,
    /// For each visible row in `inner`, the hunk_idx it belongs to (or None
    /// for context / padding rows).
    rows: Vec<Option<usize>>,
}

#[derive(Debug)]
pub struct ConflictView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    repo_path: PathBuf,
    file: ConflictFile,
    /// Index into `file.hunk_count()` of the currently focused conflict.
    current_hunk: usize,
    /// Hunk currently under the mouse cursor — drives the "Hovered" visual
    /// state. Updated on every `handle_mouse_move`.
    hovered_hunk: Option<usize>,
    /// Effective rendering mode — downgrades ThreePane → TwoPane on narrow
    /// terminals.
    mode: ConflictViewMode,
    /// Vertical scroll DELTA, relative to the auto-anchor that puts the
    /// current hunk at ~25 % from the top of every pane. `0` means
    /// "stay anchored on the current hunk"; positive values push the view
    /// down, negative pull it up. Reset to 0 whenever the current hunk
    /// changes so each new hunk gets re-centred. Per-pane auto-anchors
    /// keep the three views synced even when the result-pane line count
    /// differs from the source panes.
    scroll_delta: i32,
    /// Per-pane hit-test map populated every render. Used by `handle_click`
    /// and `handle_mouse_move` to map cursor positions to hunk indices.
    pane_hit_maps: Vec<PaneHitMap>,
    /// Lazily-built syntax-highlight caches. Each entry maps to one line
    /// of the corresponding pane, in document order. Building them costs
    /// O(N · syntect) which is the dominant render expense in debug
    /// builds; once cached they get reused on every hover / scroll / pick
    /// since the source content never changes during the editor's life.
    syntax_cache_ours: Option<Vec<Option<Vec<Span<'static>>>>>,
    syntax_cache_theirs: Option<Vec<Option<Vec<Span<'static>>>>>,
    syntax_cache_base: Option<Vec<Option<Vec<Span<'static>>>>>,
    /// Cached Result-preview build, keyed on the current resolutions +
    /// inner width. The expensive bit is the per-line syntect pass through
    /// the picked content; on hover / scroll the cache hits and we skip it
    /// entirely. Invalidated only when the user picks a different
    /// resolution or the pane resizes.
    result_cache: Option<ResultCache>,
    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> ConflictView<'a> {
    pub fn new(
        commit_list_state: Option<CommitListState<'a>>,
        repo_path: PathBuf,
        file: ConflictFile,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        let mode = ctx.ui_config.common.conflict_view;
        Self {
            commit_list_state,
            repo_path,
            file,
            current_hunk: 0,
            hovered_hunk: None,
            mode,
            scroll_delta: 0,
            pane_hit_maps: Vec::new(),
            syntax_cache_ours: None,
            syntax_cache_theirs: None,
            syntax_cache_base: None,
            result_cache: None,
            ctx,
            tx,
        }
    }

    /// Compute (and cache on first use) the syntax-highlighted spans for
    /// every line of a source pane, in document order. Called once per
    /// pane's lifetime — subsequent renders reuse the cached vector and
    /// skip the expensive syntect pass entirely, which is the dominant
    /// cost of a debug-build render (300+ highlight_line calls per frame
    /// otherwise).
    fn ensure_syntax_cache(&mut self, side: Pane) {
        let already = match side {
            Pane::Ours => self.syntax_cache_ours.is_some(),
            Pane::Theirs => self.syntax_cache_theirs.is_some(),
            Pane::Base => self.syntax_cache_base.is_some(),
        };
        if already {
            return;
        }
        let mut highlighter = SyntaxHighlighter::new(&self.file.path);
        let theme = &self.ctx.color_theme;
        let context_fg = theme.detail_label_fg;
        let accent = match side {
            Pane::Ours => theme.list_ref_branch_fg,
            Pane::Theirs => theme.list_ref_remote_branch_fg,
            Pane::Base => theme.detail_label_fg,
        };
        let mut out: Vec<Option<Vec<Span<'static>>>> = Vec::new();
        for segment in &self.file.segments {
            match segment {
                FileSegment::Context(lines) => {
                    for s in lines {
                        out.push(highlight_or_plain(&mut highlighter, s, context_fg));
                    }
                }
                FileSegment::Hunk(h) => {
                    let pane_lines: &[String] = match side {
                        Pane::Ours => &h.ours,
                        Pane::Theirs => &h.theirs,
                        Pane::Base => h.base.as_deref().unwrap_or(&[]),
                    };
                    if matches!(side, Pane::Base) && h.base.is_none() {
                        out.push(None);
                    } else {
                        for s in pane_lines {
                            out.push(highlight_or_plain(&mut highlighter, s, accent));
                        }
                    }
                }
            }
        }
        match side {
            Pane::Ours => self.syntax_cache_ours = Some(out),
            Pane::Theirs => self.syntax_cache_theirs = Some(out),
            Pane::Base => self.syntax_cache_base = Some(out),
        }
    }

    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
    }

    pub fn footer_hint(&self) -> String {
        let parts = vec![
            "o:ours".to_string(),
            "t:theirs".to_string(),
            "b:both".to_string(),
            "B:both (reversed)".to_string(),
            "⇆:hunks".to_string(),
            "↑↓:scroll".to_string(),
        ];
        format!("⌘ {}", parts.join("▕▏"))
    }

    pub fn update_layout(&mut self, _area: Rect) {}
    pub fn prepare_graph_uploads(&mut self) {}
    pub fn clear_graph_images(&mut self) {}
    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        Vec::new()
    }
    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        Vec::new()
    }
    pub fn refresh(&mut self) {}
    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        Rc::make_mut(&mut self.ctx).color_theme = theme;
    }

    /// Apply a resolution to the current hunk WITHOUT auto-advancing.
    /// The user explicitly steps to the next hunk with ⇆ / n / p / click —
    /// staying put lets them tweak the same pick (e.g. flip from `b` to
    /// `B`) before moving on.
    fn pick(&mut self, res: HunkResolution) {
        if let Some(h) = self.file.hunk_mut(self.current_hunk) {
            h.resolution = res;
        }
    }

    fn goto_hunk(&mut self, delta: isize) {
        let total = self.file.hunk_count() as isize;
        if total == 0 {
            return;
        }
        let mut next = self.current_hunk as isize + delta;
        if next < 0 {
            next += total;
        }
        next %= total;
        self.current_hunk = next as usize;
        self.scroll_delta = 0;
    }

    fn save_and_resolve(&mut self) {
        if !self.file.is_fully_resolved() {
            let n = self.file.unresolved_count();
            let noun = if n == 1 { "conflict" } else { "conflicts" };
            self.tx.send(AppEvent::NotifyError(format!(
                "{} {} still unresolved",
                n, noun
            )));
            return;
        }
        let content = self.file.render_resolved();
        let full = self.repo_path.join(&self.file.path);
        if let Err(e) = std::fs::write(&full, content) {
            self.tx
                .send(AppEvent::NotifyError(format!("Write failed: {}", e)));
            return;
        }
        match crate::git::actions::stage_file(&self.repo_path, &self.file.path) {
            Ok(_) => {
                self.tx.send(AppEvent::NotifySuccess(format!(
                    "Resolved {}",
                    self.file.path
                )));
                self.tx.send(AppEvent::CloseConflictEditor);
            }
            Err(e) => {
                self.tx
                    .send(AppEvent::NotifyError(format!("git add failed: {}", e)));
            }
        }
    }

    fn cancel(&mut self) {
        self.tx.send(AppEvent::CloseConflictEditor);
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        use ratatui::crossterm::event::KeyCode;

        match key.code {
            KeyCode::Char('o') => {
                self.pick(HunkResolution::Ours);
                return;
            }
            KeyCode::Char('t') => {
                self.pick(HunkResolution::Theirs);
                return;
            }
            KeyCode::Char('b') => {
                self.pick(HunkResolution::BothOursFirst);
                return;
            }
            KeyCode::Char('B') => {
                self.pick(HunkResolution::BothTheirsFirst);
                return;
            }
            KeyCode::Char('n') => {
                self.goto_hunk(1);
                return;
            }
            KeyCode::Char('p') => {
                self.goto_hunk(-1);
                return;
            }
            _ => {}
        }

        match event_with_count.event {
            UserEvent::Confirm => self.save_and_resolve(),
            UserEvent::Cancel | UserEvent::Close => self.cancel(),
            UserEvent::NavigateLeft => self.goto_hunk(-1),
            UserEvent::NavigateRight => self.goto_hunk(1),
            UserEvent::NavigateUp | UserEvent::ScrollUp => {
                self.scroll_delta = self.scroll_delta.saturating_sub(1);
            }
            UserEvent::NavigateDown | UserEvent::ScrollDown => {
                self.scroll_delta = self.scroll_delta.saturating_add(1);
            }
            UserEvent::PageUp => {
                self.scroll_delta = self.scroll_delta.saturating_sub(10);
            }
            UserEvent::PageDown => {
                self.scroll_delta = self.scroll_delta.saturating_add(10);
            }
            UserEvent::GoToTop => {
                self.scroll_delta = i32::MIN / 2;
            }
            UserEvent::GoToBottom => {
                self.scroll_delta = i32::MAX / 2;
            }
            _ => {}
        }
    }

    /// Mouse click — if the cursor lands on a hunk row in one of the source
    /// panes, focus that hunk so the next o/t/b key picks for it.
    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(hunk_idx) = self.hunk_at(col, row) {
            self.current_hunk = hunk_idx;
            self.scroll_delta = 0; // re-centre on the picked hunk
        }
    }

    /// Mouse hover — track which hunk (if any) is under the cursor so the
    /// next render can paint it with the Hovered visual state.
    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        self.hovered_hunk = self.hunk_at(col, row);
    }

    /// Walk the last frame's pane hit-maps and return the hunk index at the
    /// given screen position, or None when the cursor isn't over a hunk row.
    fn hunk_at(&self, col: u16, row: u16) -> Option<usize> {
        for map in &self.pane_hit_maps {
            if col >= map.inner.x
                && col < map.inner.x + map.inner.width
                && row >= map.inner.y
                && row < map.inner.y + map.inner.height
            {
                let local = (row - map.inner.y) as usize;
                return map.rows.get(local).copied().flatten();
            }
        }
        None
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let effective_mode = match self.mode {
            ConflictViewMode::ThreePane if area.width < 110 => ConflictViewMode::TwoPane,
            m => m,
        };

        // Reset the mouse hit-map — render_source_pane() repopulates it
        // with the rects/rows of every pane it draws this frame.
        self.pane_hit_maps.clear();

        let [header_area, body_area] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
        self.render_header(f, header_area);

        // Body is split into a "source panes" row (top, 60-65%) and a
        // "Result preview" row (bottom, 35-40%). The user can always see
        // the resolved file shaping up as they pick.
        let [sources_area, result_area] = match effective_mode {
            ConflictViewMode::Inline => {
                // Inline: vertical stack — Ours, Theirs, Result (each ~33%).
                let [ours, theirs, result] = Layout::vertical([
                    Constraint::Percentage(33),
                    Constraint::Percentage(33),
                    Constraint::Percentage(34),
                ])
                .areas(body_area);
                self.render_source_pane(f, ours, Pane::Ours);
                self.render_source_pane(f, theirs, Pane::Theirs);
                self.render_result_pane(f, result);
                return;
            }
            _ => Layout::vertical([Constraint::Percentage(62), Constraint::Percentage(38)])
                .areas(body_area),
        };

        match effective_mode {
            ConflictViewMode::ThreePane => {
                let [ours, base, theirs] = Layout::horizontal([
                    Constraint::Percentage(34),
                    Constraint::Percentage(33),
                    Constraint::Percentage(33),
                ])
                .areas(sources_area);
                self.render_source_pane(f, ours, Pane::Ours);
                self.render_source_pane(f, base, Pane::Base);
                self.render_source_pane(f, theirs, Pane::Theirs);
            }
            ConflictViewMode::TwoPane => {
                let [ours, theirs] =
                    Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .areas(sources_area);
                self.render_source_pane(f, ours, Pane::Ours);
                self.render_source_pane(f, theirs, Pane::Theirs);
            }
            ConflictViewMode::Inline => unreachable!(),
        }
        self.render_result_pane(f, result_area);
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let total = self.file.hunk_count();
        let unresolved = self.file.unresolved_count();
        let progress_color = if unresolved == 0 {
            theme.status_success_fg
        } else {
            theme.status_error_fg
        };
        let title = Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "⚠ ",
                Style::default()
                    .fg(theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Resolve: ",
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                self.file.path.clone(),
                Style::default().fg(theme.list_hash_fg),
            ),
            Span::raw("  "),
            Span::styled(
                format!(
                    "hunk {}/{}",
                    (self.current_hunk + 1).min(total.max(1)),
                    total
                ),
                Style::default().fg(theme.detail_label_fg),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} unresolved", unresolved),
                Style::default()
                    .fg(progress_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        let divider = Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(theme.divider_fg),
        ));
        f.render_widget(Paragraph::new(vec![title, divider]), area);
    }

    /// Render one of the three source panes. Layout matches VS Code: a left
    /// gutter with line numbers, conflict regions painted with a full-row
    /// background colour, neutral context lines plain. The pane's inner
    /// rect + per-row hunk indices are pushed into `pane_hit_maps` so the
    /// mouse handlers can resolve clicks back to a hunk.
    fn render_source_pane(&mut self, f: &mut Frame, area: Rect, side: Pane) {
        let theme = &self.ctx.color_theme;
        let dark_bg = is_dark_bg(theme.bg);

        // Theme-aware accents instead of hard green / red:
        // - Ours uses the local-branch colour (semantically "the branch you
        //   are on") — every theme already picks a distinct hue for it.
        // - Theirs uses the remote-branch colour ("the branch coming in").
        // - Base falls back to a dim label colour.
        let (title, accent) = match side {
            Pane::Ours => (self.pane_title(Pane::Ours), theme.list_ref_branch_fg),
            Pane::Base => ("Base".to_string(), theme.detail_label_fg),
            Pane::Theirs => (
                self.pane_title(Pane::Theirs),
                theme.list_ref_remote_branch_fg,
            ),
        };

        let resolution = self
            .file
            .hunk(self.current_hunk)
            .map(|h| h.resolution)
            .unwrap_or(HunkResolution::Unresolved);
        let is_picked = matches!(
            (side, resolution),
            (Pane::Ours, HunkResolution::Ours)
                | (Pane::Ours, HunkResolution::BothOursFirst)
                | (Pane::Ours, HunkResolution::BothTheirsFirst)
                | (Pane::Theirs, HunkResolution::Theirs)
                | (Pane::Theirs, HunkResolution::BothOursFirst)
                | (Pane::Theirs, HunkResolution::BothTheirsFirst)
        );

        let border_color = if is_picked { accent } else { theme.divider_fg };
        let title_style = if is_picked {
            Style::default().fg(accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(accent)
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Line::from(vec![
                Span::raw(" "),
                if is_picked {
                    Span::styled("✓ ", Style::default().fg(theme.status_success_fg))
                } else {
                    Span::raw("  ")
                },
                Span::styled(title, title_style),
                Span::raw(" "),
            ]));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let _ = dark_bg; // retained for build_result_lines below
        let lines = self.build_source_lines(side, accent, theme.bg, inner.width);
        let current_hunk_line = self.current_hunk_line(&lines);
        let (visible, hit_rows) =
            self.apply_scroll(&lines, inner.height as usize, current_hunk_line);
        // Remember this pane so handle_click / handle_mouse_move can resolve
        // (col, row) → hunk_idx on the next mouse event.
        self.pane_hit_maps.push(PaneHitMap {
            inner,
            rows: hit_rows,
        });
        f.render_widget(Paragraph::new(visible), inner);
    }

    fn render_result_pane(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let dark_bg = is_dark_bg(theme.bg);

        let unresolved = self.file.unresolved_count();
        let (title_text, title_style) = if unresolved == 0 {
            (
                "Result preview (ready to save)".to_string(),
                Style::default()
                    .fg(theme.status_success_fg)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            (
                format!("Result preview ({} unresolved)", unresolved),
                Style::default()
                    .fg(theme.status_warn_fg)
                    .add_modifier(Modifier::BOLD),
            )
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(vec![
                Span::raw(" "),
                Span::styled(title_text, title_style),
                Span::raw(" "),
            ]));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Check cache: if resolutions + width are unchanged, skip the
        // (expensive) syntax-highlight pass and reuse the previous build.
        let current_resolutions: Vec<HunkResolution> = self
            .file
            .segments
            .iter()
            .filter_map(|s| match s {
                FileSegment::Hunk(h) => Some(h.resolution),
                _ => None,
            })
            .collect();
        let cache_hit = self
            .result_cache
            .as_ref()
            .map(|c| {
                c.resolutions == current_resolutions
                    && c.inner_width == inner.width
                    && c.current_hunk == self.current_hunk
            })
            .unwrap_or(false);
        if !cache_hit {
            let lines = self.build_result_lines(theme.bg, dark_bg, inner.width);
            self.result_cache = Some(ResultCache {
                resolutions: current_resolutions,
                inner_width: inner.width,
                current_hunk: self.current_hunk,
                lines,
            });
        }
        let cached = self.result_cache.as_ref().unwrap();
        // Result preview shares the same scroll cadence as the source panes:
        // it anchors on the current hunk's first line so all three views
        // stay aligned and a click on the Theirs pane's hunk 2 brings
        // hunk 2 into view in the Result preview too.
        let result_current_line = self.current_hunk_line(&cached.lines);
        let (visible, _hunks) =
            self.apply_scroll(&cached.lines, inner.height as usize, result_current_line);
        f.render_widget(Paragraph::new(visible), inner);
    }

    fn pane_title(&self, side: Pane) -> String {
        let hunk = self.file.hunk(self.current_hunk);
        match side {
            Pane::Ours => format!(
                "Ours ({})",
                hunk.map(|h| non_empty(&h.ours_label, "HEAD"))
                    .unwrap_or("HEAD")
            ),
            Pane::Theirs => format!(
                "Theirs ({})",
                hunk.map(|h| non_empty(&h.theirs_label, "incoming"))
                    .unwrap_or("incoming")
            ),
            Pane::Base => "Base".to_string(),
        }
    }

    /// Build the rendered lines for a source pane. Reuses the cached
    /// syntax-highlighted spans so each render does only the cheap
    /// state-styling work (HunkState → bg + bold + bar), not the
    /// expensive syntect pass.
    fn build_source_lines(
        &mut self,
        side: Pane,
        accent: Color,
        theme_bg: Color,
        inner_width: u16,
    ) -> Vec<RenderedLine> {
        self.ensure_syntax_cache(side);
        let theme = &self.ctx.color_theme;
        let cache: &[Option<Vec<Span<'static>>>] = match side {
            Pane::Ours => self.syntax_cache_ours.as_deref().unwrap(),
            Pane::Theirs => self.syntax_cache_theirs.as_deref().unwrap(),
            Pane::Base => self.syntax_cache_base.as_deref().unwrap(),
        };

        let mut out: Vec<RenderedLine> = Vec::new();
        let mut line_number = 1u32;
        let mut hunk_idx = 0usize;
        let mut cache_idx = 0usize;

        for segment in &self.file.segments {
            match segment {
                FileSegment::Context(ctx_lines) => {
                    for s in ctx_lines {
                        let spans = cache.get(cache_idx).and_then(|c| c.clone());
                        cache_idx += 1;
                        out.push(make_line(
                            Some(line_number),
                            " ",
                            s,
                            theme.detail_label_fg,
                            theme.detail_label_fg,
                            HunkState::Context,
                            theme_bg,
                            spans,
                            inner_width,
                            None,
                        ));
                        line_number += 1;
                    }
                }
                FileSegment::Hunk(h) => {
                    let state = if hunk_idx == self.current_hunk {
                        HunkState::Current
                    } else if self.hovered_hunk == Some(hunk_idx) {
                        HunkState::Hovered
                    } else {
                        HunkState::Idle
                    };
                    let prefix = match side {
                        Pane::Ours => "+",
                        Pane::Theirs => "-",
                        Pane::Base => "·",
                    };
                    let pane_lines: &[String] = match side {
                        Pane::Ours => &h.ours,
                        Pane::Theirs => &h.theirs,
                        Pane::Base => h.base.as_deref().unwrap_or(&[]),
                    };
                    if matches!(side, Pane::Base) && h.base.is_none() {
                        cache_idx += 1; // placeholder entry kept in cache for sync
                        out.push(make_line(
                            None,
                            "·",
                            "(no diff3 base — `git config merge.conflictStyle diff3`)",
                            theme.divider_fg,
                            theme.divider_fg,
                            state,
                            theme_bg,
                            None,
                            inner_width,
                            Some(hunk_idx),
                        ));
                    } else {
                        for s in pane_lines {
                            let spans = cache.get(cache_idx).and_then(|c| c.clone());
                            cache_idx += 1;
                            out.push(make_line(
                                Some(line_number),
                                prefix,
                                s,
                                accent,
                                accent,
                                state,
                                theme_bg,
                                spans,
                                inner_width,
                                Some(hunk_idx),
                            ));
                            line_number += 1;
                        }
                    }
                    hunk_idx += 1;
                }
            }
        }
        out
    }

    /// Result preview content: walk every segment and emit either the
    /// surviving context lines, the picked side(s) of each hunk, or a bright
    /// "UNRESOLVED" placeholder block when the user has not chosen yet.
    fn build_result_lines(
        &self,
        theme_bg: Color,
        dark_bg: bool,
        inner_width: u16,
    ) -> Vec<RenderedLine> {
        let theme = &self.ctx.color_theme;
        let mut highlighter = SyntaxHighlighter::new(&self.file.path);
        let mut out: Vec<RenderedLine> = Vec::new();
        let mut line_number = 1u32;
        let mut hunk_idx = 0usize;

        // Reuse the same theme-aware accents as the source panes so the
        // result preview is consistent with what's above. Idle intensity
        // because no row here is "focused" — it's a passive preview.
        let ours_accent = theme.list_ref_branch_fg;
        let theirs_accent = theme.list_ref_remote_branch_fg;
        let ours_bg = hunk_bg(ours_accent, theme_bg, HunkState::Idle).unwrap();
        let theirs_bg = hunk_bg(theirs_accent, theme_bg, HunkState::Idle).unwrap();
        // Unresolved stays red — it's an actual error state, not a side choice.
        let unres_bg = if dark_bg {
            Color::Rgb(0x55, 0x20, 0x20)
        } else {
            Color::Rgb(0xFF, 0xCC, 0xCC)
        };

        for segment in &self.file.segments {
            match segment {
                FileSegment::Context(ctx_lines) => {
                    for s in ctx_lines {
                        out.push(make_annotated_line(
                            Some(line_number),
                            s,
                            theme.detail_label_fg,
                            None,
                            None,
                            LineOrigin::Context,
                            highlight_or_plain(&mut highlighter, s, theme.detail_label_fg),
                            inner_width,
                            theme,
                            None,
                        ));
                        line_number += 1;
                    }
                }
                FileSegment::Hunk(h) => {
                    let is_current = hunk_idx == self.current_hunk;
                    match h.resolution {
                        HunkResolution::Unresolved => {
                            let header = if is_current {
                                format!(
                                    "    ⚠  UNRESOLVED  hunk {}  ←  YOU ARE HERE — press [o] / [t] / [b] / [B]",
                                    hunk_idx + 1
                                )
                            } else {
                                format!("    ⚠  UNRESOLVED  hunk {}", hunk_idx + 1)
                            };
                            out.push(make_annotated_line(
                                None,
                                &header,
                                theme.status_error_fg,
                                Some(unres_bg),
                                None,
                                LineOrigin::Unresolved,
                                None,
                                inner_width,
                                theme,
                                Some(hunk_idx),
                            ));
                        }
                        HunkResolution::Ours => {
                            for s in &h.ours {
                                out.push(make_annotated_line(
                                    Some(line_number),
                                    s,
                                    theme.fg,
                                    Some(ours_bg),
                                    Some("← Ours"),
                                    LineOrigin::FromOurs,
                                    highlight_or_plain(&mut highlighter, s, theme.fg),
                                    inner_width,
                                    theme,
                                    Some(hunk_idx),
                                ));
                                line_number += 1;
                            }
                        }
                        HunkResolution::Theirs => {
                            for s in &h.theirs {
                                out.push(make_annotated_line(
                                    Some(line_number),
                                    s,
                                    theme.fg,
                                    Some(theirs_bg),
                                    Some("← Theirs"),
                                    LineOrigin::FromTheirs,
                                    highlight_or_plain(&mut highlighter, s, theme.fg),
                                    inner_width,
                                    theme,
                                    Some(hunk_idx),
                                ));
                                line_number += 1;
                            }
                        }
                        HunkResolution::BothOursFirst => {
                            // Each line is just labelled by its origin — the order
                            // in the file (ours first here) tells the rest.
                            for s in &h.ours {
                                out.push(make_annotated_line(
                                    Some(line_number),
                                    s,
                                    theme.fg,
                                    Some(ours_bg),
                                    Some("← Ours"),
                                    LineOrigin::FromOurs,
                                    highlight_or_plain(&mut highlighter, s, theme.fg),
                                    inner_width,
                                    theme,
                                    Some(hunk_idx),
                                ));
                                line_number += 1;
                            }
                            for s in &h.theirs {
                                out.push(make_annotated_line(
                                    Some(line_number),
                                    s,
                                    theme.fg,
                                    Some(theirs_bg),
                                    Some("← Theirs"),
                                    LineOrigin::FromTheirs,
                                    highlight_or_plain(&mut highlighter, s, theme.fg),
                                    inner_width,
                                    theme,
                                    Some(hunk_idx),
                                ));
                                line_number += 1;
                            }
                        }
                        HunkResolution::BothTheirsFirst => {
                            // Reversed both — theirs lines come first in the file.
                            for s in &h.theirs {
                                out.push(make_annotated_line(
                                    Some(line_number),
                                    s,
                                    theme.fg,
                                    Some(theirs_bg),
                                    Some("← Theirs"),
                                    LineOrigin::FromTheirs,
                                    highlight_or_plain(&mut highlighter, s, theme.fg),
                                    inner_width,
                                    theme,
                                    Some(hunk_idx),
                                ));
                                line_number += 1;
                            }
                            for s in &h.ours {
                                out.push(make_annotated_line(
                                    Some(line_number),
                                    s,
                                    theme.fg,
                                    Some(ours_bg),
                                    Some("← Ours"),
                                    LineOrigin::FromOurs,
                                    highlight_or_plain(&mut highlighter, s, theme.fg),
                                    inner_width,
                                    theme,
                                    Some(hunk_idx),
                                ));
                                line_number += 1;
                            }
                        }
                    }
                    hunk_idx += 1;
                }
            }
        }
        out
    }

    /// Returns the rendered-line index where the current hunk's first line
    /// appears, so the auto-scroll positions it near the top in both the
    /// source panes AND the Result preview (which has different line counts
    /// per hunk depending on the picked resolution).
    fn current_hunk_line(&self, lines: &[RenderedLine]) -> Option<usize> {
        lines
            .iter()
            .position(|l| l.hunk_idx == Some(self.current_hunk))
    }

    /// Apply scroll offset to a buffer of rendered lines and pad to height.
    /// Returns the visible window plus the hunk_idx for every visible row
    /// (None for context / padding rows) so the caller can wire mouse
    /// hit-testing.
    ///
    /// Scroll model: each pane has its own AUTO anchor (puts the current
    /// hunk at ~25 % from the top), and we then apply the shared
    /// `scroll_delta` on top of that anchor. With delta=0 every pane shows
    /// the current hunk near the top (auto-sync). When the user presses ↓
    /// once, delta becomes 1, so every pane's start advances by 1 from
    /// wherever its anchor was — no more "scroll-jumps-to-line-0" bug.
    fn apply_scroll(
        &self,
        lines: &[RenderedLine],
        visible_height: usize,
        current_hunk_line: Option<usize>,
    ) -> (Vec<Line<'static>>, Vec<Option<usize>>) {
        let total = lines.len();
        let auto_anchor: i32 = current_hunk_line
            .map(|h| {
                let target_offset_in_view = (visible_height / 4) as i32;
                (h as i32) - target_offset_in_view
            })
            .unwrap_or(0);
        let max_start: i32 = (total as i32) - (visible_height as i32);
        let raw_start = auto_anchor.saturating_add(self.scroll_delta);
        let start = raw_start.max(0).min(max_start.max(0)) as usize;

        let end = (start + visible_height).min(total);
        let slice = &lines[start..end];
        let mut visible: Vec<Line<'static>> = slice.iter().map(|l| l.line.clone()).collect();
        let mut hunks: Vec<Option<usize>> = slice.iter().map(|l| l.hunk_idx).collect();
        while visible.len() < visible_height {
            visible.push(Line::raw(""));
            hunks.push(None);
        }
        (visible, hunks)
    }
}

/// Pre-rendered line carrying both the styled Ratatui `Line` and the metadata
/// the scroll/mouse logic needs (current-hunk position, owning hunk index).
#[derive(Clone, Debug)]
struct RenderedLine {
    line: Line<'static>,
    is_current_hunk: bool,
    /// Hunk this line belongs to (None for context / placeholders).
    hunk_idx: Option<usize>,
}

/// Build a single source-pane line. Picks the side-bar glyph, the row
/// background colour and the font weight from the `HunkState`, then pads
/// the trailing background so the hunk colour fills the entire row width.
///
/// Layout: `▌|gutter|prefix|content|<pad>`
/// where `▌` is `▎` for Current, `▏` for Hovered, no bar for Idle/Context.
#[allow(clippy::too_many_arguments)]
fn make_line(
    line_number: Option<u32>,
    prefix: &str,
    raw_content: &str,
    fg: Color,
    accent: Color,
    state: HunkState,
    theme_bg: Color,
    syntax_spans: Option<Vec<Span<'static>>>,
    inner_width: u16,
    hunk_idx: Option<usize>,
) -> RenderedLine {
    let bg = hunk_bg(accent, theme_bg, state);
    let bold = matches!(state, HunkState::Current);
    let mut style = Style::default().fg(fg);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if let Some(b) = bg {
        style = style.bg(b);
    }

    // Side bar glyph: ▎ for current (thick), ▏ for hovered (thin), space
    // for idle/context. Painted in the pane's accent colour so the user
    // can see which hunk is focused at a glance.
    let (bar_ch, bar_color, bar_bold) = match state {
        HunkState::Current => ("▎", accent, true),
        HunkState::Hovered => ("▏", accent, false),
        HunkState::Idle | HunkState::Context => (" ", fg, false),
    };
    let mut bar_style = Style::default().fg(bar_color);
    if bar_bold {
        bar_style = bar_style.add_modifier(Modifier::BOLD);
    }
    if let Some(b) = bg {
        bar_style = bar_style.bg(b);
    }

    let gutter_style = Style::default().fg(Color::Rgb(0x66, 0x66, 0x66));
    let gutter_style = if let Some(b) = bg {
        gutter_style.bg(b)
    } else {
        gutter_style
    };
    let gutter = match line_number {
        Some(n) => format!("{:>4} ", n),
        None => "     ".to_string(),
    };

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(8);
    spans.push(Span::styled(bar_ch.to_string(), bar_style));
    spans.push(Span::styled(gutter.clone(), gutter_style));
    spans.push(Span::styled(format!("{} ", prefix), style));
    let content_spans: Vec<Span<'static>> = match syntax_spans {
        Some(s) if bg.is_some() => s
            .into_iter()
            .map(|sp| {
                let mut st = sp.style;
                st.bg = bg;
                if bold {
                    st = st.add_modifier(Modifier::BOLD);
                }
                Span::styled(sp.content.into_owned(), st)
            })
            .collect(),
        Some(s) => s,
        None => vec![Span::styled(raw_content.to_string(), style)],
    };
    let content_width: usize = content_spans
        .iter()
        .map(|s| s.content.chars().count())
        .sum();
    let used = 1 + gutter.chars().count() + 2 + content_width;
    let pad = (inner_width as usize).saturating_sub(used);
    spans.extend(content_spans);
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), style));
    }
    RenderedLine {
        line: Line::from(spans),
        is_current_hunk: matches!(state, HunkState::Current),
        hunk_idx,
    }
}

/// Build a Result-preview line with optional right-aligned origin badge.
#[allow(clippy::too_many_arguments)]
fn make_annotated_line(
    line_number: Option<u32>,
    raw_content: &str,
    fg: Color,
    bg: Option<Color>,
    badge: Option<&'static str>,
    origin: LineOrigin,
    syntax_spans: Option<Vec<Span<'static>>>,
    inner_width: u16,
    theme: &crate::color::ColorTheme,
    hunk_idx: Option<usize>,
) -> RenderedLine {
    let mut style = Style::default().fg(fg);
    if let Some(b) = bg {
        style = style.bg(b);
    }
    let gutter_style = Style::default()
        .fg(Color::Rgb(0x66, 0x66, 0x66))
        .bg(bg.unwrap_or(Color::Reset));
    let gutter = match line_number {
        Some(n) => format!(" {:>4} ", n),
        None => "      ".to_string(),
    };

    let mut spans: Vec<Span<'static>> = Vec::with_capacity(8);
    spans.push(Span::styled(gutter.clone(), gutter_style));
    let content_spans: Vec<Span<'static>> = match syntax_spans {
        Some(s) if bg.is_some() => s
            .into_iter()
            .map(|sp| {
                let mut st = sp.style;
                st.bg = bg;
                Span::styled(sp.content.into_owned(), st)
            })
            .collect(),
        Some(s) => s,
        None => vec![Span::styled(raw_content.to_string(), style)],
    };
    let content_width: usize = content_spans
        .iter()
        .map(|s| s.content.chars().count())
        .sum();
    spans.extend(content_spans);

    let badge_text = badge.unwrap_or("");
    let badge_width = badge_text.chars().count();
    let used = gutter.chars().count() + content_width;
    let avail = (inner_width as usize).saturating_sub(used);
    if badge_width > 0 && badge_width + 2 <= avail {
        let pad_before_badge = avail - badge_width - 1;
        spans.push(Span::styled(" ".repeat(pad_before_badge), style));
        let badge_color = match origin {
            LineOrigin::FromOurs => theme.list_ref_branch_fg,
            LineOrigin::FromTheirs => theme.list_ref_remote_branch_fg,
            LineOrigin::Unresolved => theme.status_error_fg,
            LineOrigin::Context => theme.divider_fg,
        };
        let mut badge_style = Style::default().fg(badge_color);
        if let Some(b) = bg {
            badge_style = badge_style.bg(b);
        }
        spans.push(Span::styled(format!("{} ", badge_text), badge_style));
    } else if avail > 0 {
        spans.push(Span::styled(" ".repeat(avail), style));
    }
    RenderedLine {
        line: Line::from(spans),
        is_current_hunk: matches!(origin, LineOrigin::Unresolved),
        hunk_idx,
    }
}

/// Try to syntax-highlight a single line; return None when the file has no
/// supported syntax (falls back to plain styled text in the caller).
fn highlight_or_plain(
    highlighter: &mut Option<SyntaxHighlighter>,
    content: &str,
    fallback_fg: Color,
) -> Option<Vec<Span<'static>>> {
    let h = highlighter.as_mut()?;
    let spans = h.highlight_line(content, Style::default().fg(fallback_fg), None);
    if spans.is_empty() {
        None
    } else {
        Some(spans)
    }
}

/// Pick the row background colour for hunks by blending the pane's accent
/// (taken from theme.list_ref_branch_fg / list_ref_remote_branch_fg, so
/// it follows the active theme) with the theme background at three
/// intensities — vivid for the focused hunk, medium for hover, subtle for
/// idle. Context lines return None so they inherit the terminal bg.
fn hunk_bg(accent: Color, theme_bg: Color, state: HunkState) -> Option<Color> {
    let alpha = match state {
        HunkState::Context => return None,
        HunkState::Current => 0.30,
        HunkState::Hovered => 0.18,
        HunkState::Idle => 0.10,
    };
    Some(blend(accent, theme_bg, alpha))
}

/// Linear-blend two RGB colours. Non-RGB ratatui colours (Indexed, named)
/// fall back to the foreground untouched — themes ship full RGB triples
/// so this branch is rarely hit in practice.
fn blend(fg: Color, bg: Color, alpha: f32) -> Color {
    let (fr, fg_g, fb) = match fg {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return fg,
    };
    let (br, bg_g, bb) = match bg {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (0, 0, 0),
    };
    let mix = |f: u8, b: u8| (f as f32 * alpha + b as f32 * (1.0 - alpha)) as u8;
    Color::Rgb(mix(fr, br), mix(fg_g, bg_g), mix(fb, bb))
}

fn is_dark_bg(bg: Color) -> bool {
    match bg {
        Color::Rgb(r, g, b) => (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) <= 128.0,
        _ => true,
    }
}

fn non_empty<'a>(s: &'a str, fallback: &'a str) -> &'a str {
    if s.is_empty() {
        fallback
    } else {
        s
    }
}
