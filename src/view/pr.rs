//! Pull Requests view — list + detail panels for the active repo's
//! GitHub PRs. Read-only in this iteration; review/merge actions live
//! in a follow-up.
//!
//! Data is fetched up-front (list) and lazily on selection change
//! (detail). Both calls block; the view shows a loading marker while
//! the request is in flight. Errors surface in a persistent banner
//! similar to the rebase apply error path.

use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use rustc_hash::FxHashMap;

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    github::{
        pr::{
            FileStatus, PullRequest, PullRequestDetail, PullState,
        },
        RepoCoords,
    },
};

#[derive(Debug)]
pub struct PullRequestsView<'a> {
    // Re-anchored on close so we can return to the commit list state we
    // came from. `None` if opened from a view without a commit list.
    commit_list_state: Option<crate::widget::commit_list::CommitListState<'a>>,
    coords: RepoCoords,
    token: String,
    /// Open PRs returned by `list_pull_requests`. May be empty.
    items: Vec<PullRequest>,
    /// Per-PR detail cache. First open of a PR spawns a background fetch;
    /// subsequent visits hit this cache and render instantly.
    detail_cache: FxHashMap<u64, PullRequestDetail>,
    /// PR number whose fetch is currently in flight, if any. Drives the
    /// "Loading…" indicator in the detail panel.
    loading_for: Option<u64>,
    /// Index of the row under the keyboard cursor / mouse hover. Drives
    /// the row highlight only — opening the PR (loading its detail)
    /// requires an explicit Enter or click.
    hovered: usize,
    /// Index of the PR currently displayed in the detail panel, if any.
    /// Marked with a leading triangle in the list. `None` until the user
    /// clicks or presses Enter on a row.
    opened: Option<usize>,
    /// Which panel has keyboard focus: the list (default) or the detail.
    focus: Focus,
    /// Vertical scroll in the detail panel. Reset when selection changes.
    detail_scroll: usize,
    last_error: Option<String>,
    /// Rects captured each frame for mouse hit-testing — `None` until the
    /// first render. Cleared at the top of each render pass.
    list_area: Option<Rect>,
    detail_area: Option<Rect>,
    /// First inner row Y of the list panel (rendering coordinate) — used to
    /// translate a click row into a selection index. Captured during render.
    list_inner_y: u16,
    /// Top-of-list scroll offset, kept in sync with what we passed to ratatui
    /// so click hit-testing stays accurate when the list scrolls.
    list_scroll_offset: usize,
    ctx: Rc<AppContext>,
    tx: Sender,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Detail,
}

impl<'a> PullRequestsView<'a> {
    pub fn new(
        commit_list_state: Option<crate::widget::commit_list::CommitListState<'a>>,
        coords: RepoCoords,
        token: String,
        items: Vec<PullRequest>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        let view = Self {
            commit_list_state,
            coords,
            token,
            items,
            detail_cache: FxHashMap::default(),
            loading_for: None,
            hovered: 0,
            opened: None,
            focus: Focus::List,
            detail_scroll: 0,
            last_error: None,
            list_area: None,
            detail_area: None,
            list_inner_y: 0,
            list_scroll_offset: 0,
            ctx,
            tx,
        };
        // Do NOT auto-fetch detail on open — the user has to click or
        // press Enter on a row to load it. Keeps the initial open snappy
        // and avoids burning a request the user doesn't want.
        view
    }

    /// Receive a background fetch result and update the cache. The view's
    /// next render picks up the new data automatically.
    pub fn on_detail_fetched(
        &mut self,
        number: u64,
        result: Result<PullRequestDetail, String>,
    ) {
        if self.loading_for == Some(number) {
            self.loading_for = None;
        }
        match result {
            Ok(d) => {
                self.detail_cache.insert(number, d);
                self.last_error = None;
            }
            Err(e) => {
                self.last_error = Some(format!("Fetch PR #{}: {}", number, e));
            }
        }
    }

    pub fn take_list_state(
        &mut self,
    ) -> Option<crate::widget::commit_list::CommitListState<'a>> {
        self.commit_list_state.take()
    }

    pub fn footer_hint(&self) -> String {
        // Adapt the hint to where the cursor lives so the user always sees
        // the verbs that apply to their current focus.
        let parts: Vec<&str> = match self.focus {
            Focus::List => vec!["↑↓:hover", "↵:open PR", "Tab:focus detail", "r:reload"],
            Focus::Detail => vec!["Tab:focus list", "r:reload"],
        };
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
    pub fn refresh(&mut self) {
        self.reload();
    }
    pub fn update_color_theme(&mut self, _theme: crate::color::ColorTheme) {}

    // ---------- data ----------

    /// Open the currently-hovered PR — sets `opened`, resets detail
    /// scroll, and (if needed) fires a background fetch. Cache hits
    /// render instantly; cache misses show "Fetching from GitHub…" until
    /// the worker thread completes.
    fn open_hovered(&mut self) {
        let Some(pr) = self.items.get(self.hovered) else {
            return;
        };
        let number = pr.number;
        self.opened = Some(self.hovered);
        self.detail_scroll = 0;
        if self.detail_cache.contains_key(&number) {
            return;
        }
        if self.loading_for == Some(number) {
            return;
        }
        self.spawn_detail_fetch(number);
    }

    fn spawn_detail_fetch(&mut self, number: u64) {
        self.loading_for = Some(number);
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = crate::github::pr::fetch_pull_request_detail(
                &token, &coords, number,
            );
            tx.send(AppEvent::PullRequestDetailFetched { number, result });
        });
    }

    /// Reload the list — list fetch stays synchronous (fast, single call).
    /// Drops the cache, clears the "opened" marker so the user has to
    /// re-pick a PR after a refresh.
    fn reload(&mut self) {
        match crate::github::pr::list_pull_requests(&self.token, &self.coords) {
            Ok(items) => {
                self.items = items;
                self.hovered = self.hovered.min(self.items.len().saturating_sub(1));
                self.opened = None;
                self.detail_cache.clear();
                self.loading_for = None;
                self.last_error = None;
            }
            Err(e) => {
                self.last_error = Some(format!("Reload PRs: {}", e));
            }
        }
    }

    // ---------- events ----------

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};

        // Lower-case `r` reloads — independent of the global revert/refresh
        // bindings since this view has no per-commit actions.
        if key.code == KeyCode::Char('r') && key.modifiers == KeyModifiers::NONE {
            self.reload();
            return;
        }
        // Tab swaps focus between the list and the detail panel.
        if key.code == KeyCode::Tab {
            self.focus = match self.focus {
                Focus::List => Focus::Detail,
                Focus::Detail => Focus::List,
            };
            return;
        }

        match event_with_count.event {
            UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::ClosePullRequests);
            }
            // Enter opens the hovered PR (lazy fetch, sets `opened`).
            UserEvent::Confirm => {
                if matches!(self.focus, Focus::List) {
                    self.open_hovered();
                }
            }
            UserEvent::NavigateUp => match self.focus {
                Focus::List => self.move_hovered(-1),
                Focus::Detail => self.detail_scroll = self.detail_scroll.saturating_sub(1),
            },
            UserEvent::NavigateDown => match self.focus {
                Focus::List => self.move_hovered(1),
                Focus::Detail => self.detail_scroll = self.detail_scroll.saturating_add(1),
            },
            UserEvent::PageUp => match self.focus {
                Focus::List => self.move_hovered(-10),
                Focus::Detail => self.detail_scroll = self.detail_scroll.saturating_sub(10),
            },
            UserEvent::PageDown => match self.focus {
                Focus::List => self.move_hovered(10),
                Focus::Detail => self.detail_scroll = self.detail_scroll.saturating_add(10),
            },
            UserEvent::GoToTop => match self.focus {
                Focus::List => self.hovered = 0,
                Focus::Detail => self.detail_scroll = 0,
            },
            UserEvent::GoToBottom => match self.focus {
                Focus::List => self.hovered = self.items.len().saturating_sub(1),
                Focus::Detail => self.detail_scroll = self.detail_scroll.saturating_add(50),
            },
            // Mouse wheel — scroll the focused panel's viewport without
            // moving the keyboard cursor. 1 line per event matches the
            // rest of the app (commit list, user-command, etc.).
            UserEvent::ScrollUp => match self.focus {
                Focus::List => self.scroll_list(-1),
                Focus::Detail => self.detail_scroll = self.detail_scroll.saturating_sub(1),
            },
            UserEvent::ScrollDown => match self.focus {
                Focus::List => self.scroll_list(1),
                Focus::Detail => self.detail_scroll = self.detail_scroll.saturating_add(1),
            },
            _ => {}
        }
    }

    /// Click handler:
    /// - On a list row → focus list + open the PR (fetches detail, marks
    ///   it with the triangle indicator).
    /// - On the detail block → focus detail (subsequent ↑↓ scroll body).
    pub fn handle_click(&mut self, col: u16, row: u16) {
        if rect_contains(self.list_area, col, row) {
            self.focus = Focus::List;
            if let Some(idx) = self.row_at(row) {
                if idx < self.items.len() {
                    self.hovered = idx;
                    self.open_hovered();
                }
            }
            return;
        }
        if rect_contains(self.detail_area, col, row) {
            self.focus = Focus::Detail;
        }
    }

    /// Hover handler:
    /// - Over the list → set `hovered` (visual cursor only; no fetch).
    /// - Over the detail panel → auto-tab focus to it.
    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        if rect_contains(self.list_area, col, row) {
            self.focus = Focus::List;
            if let Some(idx) = self.row_at(row) {
                if idx < self.items.len() && idx != self.hovered {
                    self.hovered = idx;
                }
            }
            return;
        }
        if rect_contains(self.detail_area, col, row) {
            self.focus = Focus::Detail;
        }
    }

    /// Translate a screen row into a `selected` index, accounting for the
    /// current scroll offset. Returns None when the click is outside the
    /// list's content rows (e.g. on the border).
    fn row_at(&self, row: u16) -> Option<usize> {
        if row < self.list_inner_y {
            return None;
        }
        let visible_row = row.saturating_sub(self.list_inner_y) as usize;
        let idx = self.list_scroll_offset + visible_row;
        if idx < self.items.len() {
            Some(idx)
        } else {
            None
        }
    }

    fn move_hovered(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let max = self.items.len() as i32 - 1;
        self.hovered = (self.hovered as i32 + delta).clamp(0, max) as usize;
    }

    /// Pan the visible list window by `delta` rows without touching the
    /// hovered cursor. Mouse wheel calls this.
    fn scroll_list(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let max = self.items.len().saturating_sub(1) as i32;
        let new = (self.list_scroll_offset as i32 + delta).clamp(0, max) as usize;
        self.list_scroll_offset = new;
    }

    // ---------- rendering ----------

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Clear hit-test rects from the previous frame so a stale rect
        // from a different layout never matches a click.
        self.list_area = None;
        self.detail_area = None;

        let banner_height: u16 = if self.last_error.is_some() && area.height > 6 {
            3
        } else {
            0
        };
        let [header_area, banner_area, body_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(banner_height),
            Constraint::Min(0),
        ])
        .areas(area);
        self.render_header(f, header_area);
        if banner_height > 0 {
            self.render_error_banner(f, banner_area);
        }

        // List on top (40% height), detail below.
        let [list_area, detail_area] = Layout::vertical([
            Constraint::Percentage(40),
            Constraint::Min(0),
        ])
        .areas(body_area);
        self.list_area = Some(list_area);
        self.detail_area = Some(detail_area);
        self.render_list(f, list_area);
        self.render_detail(f, detail_area);
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let title = Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "⊙ ",
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Pull Requests ",
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            // Repo coords are an identifier, not a branch — use the
            // hash/identifier token so the name reads as "named ref" and
            // doesn't compete with the green/red branch convention below.
            Span::styled(
                format!("{}/{}", self.coords.owner, self.coords.repo),
                Style::default()
                    .fg(theme.list_hash_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} open", self.items.len()),
                Style::default().fg(theme.detail_label_fg),
            ),
        ]);
        let divider = Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(theme.divider_fg),
        ));
        f.render_widget(Paragraph::new(vec![title, divider]), area);
    }

    fn render_error_banner(&self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let Some(msg) = self.last_error.as_ref() else {
            return;
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.status_error_fg))
            .title(Line::from(Span::styled(
                " ✗ GitHub fetch failed ",
                Style::default()
                    .fg(theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let p = Paragraph::new(Span::styled(
            msg.clone(),
            Style::default().fg(theme.status_error_fg),
        ));
        f.render_widget(p, inner);
    }

    fn render_list(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let title_color = if self.focus == Focus::List {
            theme.fg
        } else {
            theme.detail_label_fg
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(Span::styled(
                " Open PRs ",
                Style::default()
                    .fg(title_color)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.list_inner_y = inner.y;
        if self.items.is_empty() {
            self.list_scroll_offset = 0;
            let empty = Paragraph::new(Span::styled(
                "No open pull requests.",
                Style::default().fg(theme.detail_label_fg),
            ))
            .alignment(Alignment::Center);
            f.render_widget(empty, inner);
            return;
        }
        // Keyboard nav (↑↓ etc.) re-anchors the scroll so the hovered
        // row stays on-screen. Mouse-wheel scroll, on the other hand,
        // moves the viewport independently and may leave `hovered`
        // outside the visible window — that's by design.
        let visible = inner.height as usize;
        if self.hovered >= self.list_scroll_offset + visible {
            self.list_scroll_offset = self.hovered + 1 - visible;
        } else if self.hovered < self.list_scroll_offset {
            self.list_scroll_offset = self.hovered;
        }
        let max_offset = self.items.len().saturating_sub(visible);
        if self.list_scroll_offset > max_offset {
            self.list_scroll_offset = max_offset;
        }

        let items: Vec<ListItem<'static>> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, pr)| {
                let is_opened = self.opened == Some(i);
                ListItem::new(self.format_pr_row(pr, is_opened))
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.hovered));
        *state.offset_mut() = self.list_scroll_offset;
        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme.list_selected_bg)
                .fg(theme.list_selected_fg)
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, inner, &mut state);
    }

    fn format_pr_row(&self, pr: &PullRequest, is_opened: bool) -> Line<'static> {
        let theme = &self.ctx.color_theme;
        // State word — colored text instead of a filled chip so it sits
        // alongside the commit-list style of plain coloured spans.
        let state_span = match (pr.state, pr.draft) {
            (PullState::Open, true) => Span::styled(
                "DRAFT",
                Style::default()
                    .fg(theme.detail_label_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            (PullState::Open, false) => Span::styled(
                "OPEN",
                Style::default()
                    .fg(theme.status_success_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            (PullState::Merged, _) => Span::styled(
                "MERGED",
                Style::default()
                    .fg(theme.list_ref_branch_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            (PullState::Closed, _) => Span::styled(
                "CLOSED",
                Style::default()
                    .fg(theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            ),
        };
        // Base ref always renders as a REMOTE branch (it's the GitHub
        // target — the local repo may not even have it checked out, so
        // `list_ref_remote_branch_fg` matches what the graph would use).
        // Leading `▶` flags the PR whose detail is currently shown.
        let marker = if is_opened {
            Span::styled(
                "▶ ",
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("  ")
        };
        Line::from(vec![
            marker,
            state_span,
            Span::raw("  "),
            Span::styled(
                format!("#{}", pr.number),
                Style::default()
                    .fg(theme.list_hash_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                pr.title.clone(),
                Style::default().fg(theme.list_commit_message_fg),
            ),
            Span::raw("  "),
            Span::styled(pr.author.clone(), Style::default().fg(theme.list_name_fg)),
            Span::raw("  → "),
            Span::styled(
                pr.base_ref.clone(),
                Style::default()
                    .fg(theme.list_ref_remote_branch_fg)
                    .add_modifier(Modifier::BOLD),
            ),
        ])
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let title_color = if self.focus == Focus::Detail {
            theme.fg
        } else {
            theme.detail_label_fg
        };
        // Detail panel shows the OPENED PR (set by click / Enter), not
        // the hovered one. That way a user can hover over rows without
        // wrecking the detail they're reading.
        let current_number = self.opened.and_then(|i| self.items.get(i)).map(|p| p.number);
        let cached_detail =
            current_number.and_then(|n| self.detail_cache.get(&n)).cloned();
        let title = match (&cached_detail, current_number) {
            (Some(d), _) => format!(" #{} {} ", d.number, truncate(&d.title, 60)),
            (None, Some(n)) if self.loading_for == Some(n) => {
                format!(" #{} loading… ", n)
            }
            (None, _) => " Detail ".to_string(),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(Span::styled(
                title,
                Style::default()
                    .fg(title_color)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let Some(detail) = cached_detail.as_ref() else {
            // Show a placeholder while the background fetch is in flight.
            let label = if current_number.is_some()
                && self.loading_for == current_number
            {
                "  Fetching from GitHub…"
            } else {
                "  Press a PR to open it."
            };
            let p = Paragraph::new(Span::styled(
                label,
                Style::default().fg(theme.detail_label_fg),
            ));
            f.render_widget(p, inner);
            return;
        };

        // Detail panel follows the same convention as the commit detail
        // widget: labels in `detail_label_fg`, semantically-typed values
        // (name, hash, branch, file changes) using their dedicated tokens
        // from the theme so palette swaps stay consistent.
        let mut lines: Vec<Line<'static>> = Vec::new();
        // Inline labels keep the dimmed look so values pop. Section
        // headers ("Description", "Files (N)") use the head accent
        // instead so the panel reads as several colored blocks rather
        // than a wall of grey.
        let label = Style::default().fg(theme.detail_label_fg);
        let section = Style::default()
            .fg(theme.list_head_fg)
            .add_modifier(Modifier::BOLD);
        let name_style = Style::default().fg(theme.list_name_fg);
        let head_branch_style = Style::default()
            .fg(theme.list_ref_branch_fg)
            .add_modifier(Modifier::BOLD);
        let base_branch_style = Style::default()
            .fg(theme.list_ref_remote_branch_fg)
            .add_modifier(Modifier::BOLD);
        let value = Style::default().fg(theme.fg);
        let hash_style = Style::default().fg(theme.list_hash_fg);
        let date_style = Style::default().fg(theme.list_date_fg);
        let add_style = Style::default().fg(theme.detail_file_change_add_fg);
        let del_style = Style::default().fg(theme.detail_file_change_delete_fg);

        lines.push(Line::from(vec![
            Span::styled("  Author: ", label),
            Span::styled(detail.author.clone(), name_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Branch: ", label),
            Span::styled(detail.head_label.clone(), head_branch_style),
            Span::styled("  →  ", label),
            Span::styled(detail.base_ref.clone(), base_branch_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("   Stats: ", label),
            Span::styled(format!("{} commits", detail.commits), hash_style),
            Span::raw("   "),
            Span::styled(format!("+{}", detail.additions), add_style),
            Span::raw(" "),
            Span::styled(format!("-{}", detail.deletions), del_style),
            Span::raw("   "),
            Span::styled(format!("{} files", detail.changed_files), date_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled(" Reviews: ", label),
            Span::styled(
                format!("✓ {}", detail.reviews.approved),
                Style::default()
                    .fg(theme.status_success_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(
                format!("✗ {}", detail.reviews.changes_requested),
                Style::default()
                    .fg(theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(
                format!("💬 {}", detail.reviews.commented),
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("      CI: ", label),
            ci_summary_spans(theme, &detail.ci),
        ]));
        if !detail.reviewers.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(" Pending: ", label),
                Span::styled(detail.reviewers.join(", "), name_style),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  Description", section)));
        if detail.body.is_empty() {
            lines.push(Line::from(Span::styled("    (no description)", label)));
        } else {
            for raw in detail.body.lines().take(20) {
                lines.push(Line::from(Span::styled(
                    format!("    {}", truncate(raw, 200)),
                    value,
                )));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  Files ({})", detail.changed_files),
            section,
        )));
        for f in detail.files.iter().take(30) {
            lines.push(file_line(theme, f));
        }

        let scroll = self.detail_scroll.min(lines.len().saturating_sub(1));
        let p = Paragraph::new(lines).scroll((scroll as u16, 0));
        f.render_widget(p, inner);
    }
}

fn ci_summary_spans(theme: &crate::color::ColorTheme, ci: &crate::github::pr::CiSummary) -> Span<'static> {
    if ci.total == 0 {
        return Span::styled(
            "— no checks".to_string(),
            Style::default().fg(theme.detail_label_fg),
        );
    }
    let (label, color) = if ci.failure > 0 {
        ("✗ failing", theme.status_error_fg)
    } else if ci.pending > 0 {
        ("⏳ running", theme.status_warn_fg)
    } else {
        ("✓ passing", theme.status_success_fg)
    };
    Span::styled(
        format!("{} {}/{}", label, ci.success, ci.total),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn file_line(theme: &crate::color::ColorTheme, f: &crate::github::pr::PullFile) -> Line<'static> {
    // Re-use the commit-detail palette for file-change tags so a PR's
    // file list reads the same way as the local commit's diff list.
    let (tag, tag_color) = match f.status {
        FileStatus::Added => ("A", theme.detail_file_change_add_fg),
        FileStatus::Modified => ("M", theme.detail_file_change_modify_fg),
        FileStatus::Removed => ("D", theme.detail_file_change_delete_fg),
        FileStatus::Renamed => ("R", theme.detail_file_change_move_fg),
        FileStatus::Other => ("?", theme.detail_label_fg),
    };
    Line::from(vec![
        Span::raw("    "),
        Span::styled(
            tag.to_string(),
            Style::default().fg(tag_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(f.filename.clone(), Style::default().fg(theme.fg)),
        Span::raw("  "),
        Span::styled(
            format!("+{}", f.additions),
            Style::default().fg(theme.detail_file_change_add_fg),
        ),
        Span::raw(" "),
        Span::styled(
            format!("-{}", f.deletions),
            Style::default().fg(theme.detail_file_change_delete_fg),
        ),
    ])
}

fn rect_contains(rect: Option<Rect>, col: u16, row: u16) -> bool {
    let Some(r) = rect else {
        return false;
    };
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut)
    }
}
