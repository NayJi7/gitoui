use std::process::Command;
use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::{
    app::AppContext,
    config::DiffMode,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    git::diff::{DiffEntry, DiffLineType, HunkOrigin},
    highlight::SyntaxHighlighter,
    widget::commit_list::{CommitList, CommitListState},
};

#[derive(Debug)]
pub struct DiffView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    diff_entries: Vec<DiffEntry>,
    scroll_offset: usize,
    content_height: usize,
    title: String,
    commit_hash: String,
    all_file_paths: Vec<(String, bool)>,

    ctx: Rc<AppContext>,
    tx: Sender,

    diff_content_area: Option<Rect>,

    // Cached lines (without hover styling) - rebuilt only when content changes
    base_lines: Vec<Line<'static>>,
    needs_rebuild: bool,

    // Expand button tracking with precise column positions
    expand_buttons: Vec<ButtonInfo>,
    focused_button: Option<usize>, // unified selection: set by both keyboard and mouse hover

    // Full file contents for directional gap expansion
    #[allow(dead_code)]
    old_file_lines: Vec<String>,
    new_file_lines: Vec<String>,
    // Per-gap state: (visible_up, visible_down, total_gap_size)
    gap_states: Vec<GapState>,

    // Last known list_area.height; used to detect when commit-list shrinks
    cached_list_height: u16,

    // Search state
    search_active: bool,
    search_query: String,
    search_cursor: usize,
    search_matches: Vec<SearchMatch>,
    search_current: usize,

    /// Maps rendered rows back to the hunk they belong to. Used by the
    /// Uncommitted view to detect which hunk the user clicked. Empty unless
    /// the diff contains hunks with `HunkOrigin::Staged` or `Unstaged`.
    hunk_spans: Vec<HunkRowSpan>,
    /// File path used when toggling a hunk's stage state, derived from the
    /// title for the Uncommitted view, empty for committed diffs.
    uncommitted_file_path: Option<String>,
    /// Index into `hunk_spans` of the hunk currently under the mouse cursor.
    /// Drives the banner emphasis used as hover feedback.
    hovered_hunk: Option<usize>,
    /// Index into `hunk_spans` of the keyboard-focused hunk. Drives the same
    /// banner emphasis as hover.
    focused_hunk: Option<usize>,
    /// When the Uncommitted view opens a combined diff, this captures which
    /// side the user clicked (Staged vs Unstaged). After the first build of
    /// `base_lines` the renderer scrolls to the first hunk of this origin
    /// and clears the field.
    initial_scroll_origin: Option<HunkOrigin>,
    /// Set by `toggle_hunk_stage` when re-opening the diff after staging or
    /// unstaging a single hunk: `(underlying hunk index in diff_entries[0],
    /// previous scroll_offset)`. The next first-build resolves the hunk and
    /// restores both focus and scroll so the user stays anchored to the
    /// hunk they just toggled instead of jumping back to row 0.
    pending_restore: Option<(usize, usize)>,
    /// True until the first build of `base_lines` completes. Lets the render
    /// loop distinguish "first time setup" (auto-focus first focusable) from
    /// "rebuild after user interaction" (preserve focus).
    first_build_pending: bool,
}

/// One concrete occurrence of the search query inside a base line.
/// `start`/`end` are byte offsets in the line's flattened text (concat of all spans).
#[derive(Debug, Clone, Copy)]
struct SearchMatch {
    line_idx: usize,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpandDirection {
    Up,
    Down,
}

/// What the unified arrow-key focus currently points at, either one of the
/// expand buttons in a gap, or one of the stage-able hunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusKind {
    Button(usize),
    Hunk(usize),
}

#[derive(Debug, Clone)]
struct ButtonInfo {
    line_idx: usize,
    direction: ExpandDirection,
    col_start: u16,
    col_end: u16,
    gap_idx: usize,
    is_edge: bool,
}

#[derive(Debug, Clone)]
struct GapState {
    visible_up: usize,
    visible_down: usize,
    total: usize,
}

/// Marks which rendered rows belong to which hunk so that click handling can
/// resolve a clicked row back to a `(file, hunk_idx)` pair. Populated by the
/// `build_*_diff_lines` functions; consumed by `handle_click`.
#[derive(Debug, Clone)]
struct HunkRowSpan {
    /// Index into `diff_entries[entry_idx].hunks`.
    hunk_idx: usize,
    /// Index into `diff_entries`. For the Uncommitted combined view this is
    /// always 0, but we keep it general so commit diffs can use the field too.
    #[allow(dead_code)]
    entry_idx: usize,
    origin: HunkOrigin,
    /// First rendered row index this hunk covers (inclusive).
    start: usize,
    /// One past the last rendered row this hunk covers.
    end: usize,
}

impl<'a> DiffView<'a> {
    pub fn all_file_paths(&self) -> &Vec<(String, bool)> {
        &self.all_file_paths
    }

    /// Append addition lines to the first hunk of the first diff entry (for progressive loading
    /// of untracked/new files). Marks base_lines for rebuild.
    pub fn append_addition_lines(&mut self, lines: Vec<crate::git::diff::DiffLine>) {
        if let Some(entry) = self.diff_entries.first_mut() {
            if let Some(hunk) = entry.hunks.first_mut() {
                let next_line_no = hunk.new_count + 1;
                let new_lines: Vec<crate::git::diff::DiffLine> = lines
                    .into_iter()
                    .enumerate()
                    .map(|(i, mut l)| {
                        l.new_line_no = Some(next_line_no + i as u32);
                        l.highlight_ranges = Vec::new();
                        l
                    })
                    .collect();
                hunk.new_count += new_lines.len() as u32;
                hunk.lines.extend(new_lines);
            }
        }
        self.needs_rebuild = true;
    }

    #[allow(dead_code)]
    pub fn has_content(&self) -> bool {
        self.diff_entries.iter().any(|e| !e.hunks.is_empty())
    }

    pub fn new(
        commit_list_state: Option<CommitListState<'a>>,
        diff_entries: Vec<DiffEntry>,
        ctx: Rc<AppContext>,
        tx: Sender,
        title: String,
        commit_hash: String,
        all_file_paths: Vec<(String, bool)>,
        repo_path: std::path::PathBuf,
    ) -> DiffView<'a> {
        let file_path = title
            .strip_prefix("Diff (staged): ")
            .or_else(|| title.strip_prefix("Diff (unstaged): "))
            .or_else(|| title.strip_prefix("Diff: "))
            .unwrap_or("");
        let (old_lines, new_lines) = Self::load_file_versions(
            &repo_path,
            &commit_hash,
            file_path,
            title.contains("(staged)"),
        );

        let file_line_count = new_lines.len() as u32;
        let gap_states = Self::compute_initial_gap_states(&diff_entries, file_line_count);

        // The uncommitted-combined view sets the title prefix to "Diff: " and
        // is the only place where hunk staging applies. Pull out the file path
        // so click-to-toggle can pipe a patch back to git.
        let uncommitted_file_path = title.strip_prefix("Diff: ").map(|p| p.to_string());

        DiffView {
            commit_list_state,
            diff_entries,
            scroll_offset: 0,
            content_height: 0,
            title,
            commit_hash,
            all_file_paths,
            ctx,
            tx,
            diff_content_area: None,
            base_lines: Vec::new(),
            needs_rebuild: true,
            expand_buttons: Vec::new(),
            focused_button: None,
            old_file_lines: old_lines,
            new_file_lines: new_lines,
            gap_states,
            cached_list_height: 0,
            search_active: false,
            search_query: String::new(),
            search_cursor: 0,
            search_matches: Vec::new(),
            search_current: 0,
            hunk_spans: Vec::new(),
            uncommitted_file_path,
            hovered_hunk: None,
            focused_hunk: None,
            initial_scroll_origin: None,
            pending_restore: None,
            first_build_pending: true,
        }
    }

    /// Tell the next render to scroll to the first hunk whose origin matches.
    /// Used by the Uncommitted view: clicking the file from the Staged list
    /// jumps to the first staged hunk, and vice versa for Unstaged.
    pub fn set_initial_scroll_origin(&mut self, origin: HunkOrigin) {
        self.initial_scroll_origin = Some(origin);
    }

    /// Tell the next render to restore the focus to the hunk whose underlying
    /// `hunk_idx` matches, and to roll back the scroll offset. Wins over
    /// `set_initial_scroll_origin`, and explicitly clears it, so we never
    /// fall back to "scroll to first hunk of origin X" right after a toggle
    /// (which is what would make focus jump to the next hunk).
    pub fn set_restore_focus(&mut self, hunk_idx: usize, scroll_offset: usize) {
        self.pending_restore = Some((hunk_idx, scroll_offset));
        self.initial_scroll_origin = None;
    }

    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    fn load_file_versions(
        repo_path: &std::path::Path,
        commit_hash: &str,
        file_path: &str,
        is_staged: bool,
    ) -> (Vec<String>, Vec<String>) {
        let old_cmd = if commit_hash.is_empty() {
            if is_staged {
                Command::new("git")
                    .args(["show", &format!("HEAD:{}", file_path)])
                    .current_dir(repo_path)
                    .output()
            } else {
                Command::new("git")
                    .args(["show", &format!(":{}", file_path)])
                    .current_dir(repo_path)
                    .output()
            }
        } else {
            Command::new("git")
                .args(["show", &format!("{}^:{}", commit_hash, file_path)])
                .current_dir(repo_path)
                .output()
        };

        let new_lines = if commit_hash.is_empty() && !is_staged {
            std::fs::read_to_string(repo_path.join(file_path))
                .unwrap_or_default()
                .lines()
                .map(|s| s.to_string())
                .collect()
        } else {
            let hash = if commit_hash.is_empty() {
                "HEAD".to_string()
            } else {
                commit_hash.to_string()
            };
            match Command::new("git")
                .args(["show", &format!("{}:{}", hash, file_path)])
                .current_dir(repo_path)
                .output()
            {
                Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(|s| s.to_string())
                    .collect(),
                _ => Vec::new(),
            }
        };

        let old_lines = match old_cmd {
            Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|s| s.to_string())
                .collect(),
            _ => Vec::new(),
        };

        (old_lines, new_lines)
    }

    fn compute_initial_gap_states(
        diff_entries: &[DiffEntry],
        file_line_count: u32,
    ) -> Vec<GapState> {
        let mut states = Vec::new();
        if let Some(entry) = diff_entries.first() {
            // Gap before first hunk, no lines shown by default, single button
            if let Some(first_new) = entry.hunks.first().and_then(|h| {
                h.lines.iter().find_map(|l| {
                    if l.line_type == DiffLineType::Context {
                        l.new_line_no
                    } else {
                        None
                    }
                })
            }) {
                if first_new > 1 {
                    let gap = (first_new - 1) as usize;
                    states.push(GapState {
                        visible_up: 0,
                        visible_down: 0,
                        total: gap,
                    });
                }
            }

            // Gaps between hunks, show some context by default
            for hunk_idx in 1..entry.hunks.len() {
                if let (Some(prev), Some(curr)) =
                    (entry.hunks.get(hunk_idx - 1), entry.hunks.get(hunk_idx))
                {
                    let prev_end = prev.lines.iter().rev().find_map(|l| {
                        if l.line_type == DiffLineType::Context {
                            l.new_line_no
                        } else {
                            None
                        }
                    });
                    let curr_start = curr.lines.iter().find_map(|l| {
                        if l.line_type == DiffLineType::Context {
                            l.new_line_no
                        } else {
                            None
                        }
                    });
                    if let (Some(pe), Some(cs)) = (prev_end, curr_start) {
                        if cs > pe + 1 {
                            let gap = (cs - pe - 1) as usize;
                            let default_visible = 3.min(gap / 2);
                            states.push(GapState {
                                visible_up: default_visible,
                                visible_down: default_visible,
                                total: gap,
                            });
                        }
                    }
                }
            }

            // Gap after last hunk, no lines shown by default, single button
            if let Some(last_new) = entry.hunks.last().and_then(|h| {
                h.lines.iter().rev().find_map(|l| {
                    if l.line_type == DiffLineType::Context {
                        l.new_line_no
                    } else {
                        None
                    }
                })
            }) {
                if last_new < file_line_count {
                    let gap = (file_line_count - last_new) as usize;
                    states.push(GapState {
                        visible_up: 0,
                        visible_down: 0,
                        total: gap,
                    });
                }
            }
        }
        states
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};

        if self.search_active {
            match key.code {
                KeyCode::Esc => {
                    self.search_active = false;
                    self.search_query.clear();
                    self.search_cursor = 0;
                    self.search_matches.clear();
                    self.clear_search_status_bar();
                    return;
                }
                // Up / Enter / Down navigate matches without leaving the input,
                // mirroring n / N behavior so search-bar editing stays available.
                KeyCode::Up => {
                    if !self.search_matches.is_empty() {
                        self.search_current = if self.search_current == 0 {
                            self.search_matches.len() - 1
                        } else {
                            self.search_current - 1
                        };
                        self.scroll_to_match(self.search_current);
                    }
                    self.update_search_status_bar();
                    return;
                }
                KeyCode::Down | KeyCode::Enter => {
                    if !self.search_matches.is_empty() {
                        self.search_current = (self.search_current + 1) % self.search_matches.len();
                        self.scroll_to_match(self.search_current);
                    }
                    self.update_search_status_bar();
                    return;
                }
                KeyCode::Char(c)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    self.search_query.insert(self.search_cursor, c);
                    self.search_cursor += c.len_utf8();
                    self.update_search_matches();
                    if !self.search_matches.is_empty() {
                        self.search_current = 0;
                        self.scroll_to_match(self.search_current);
                    }
                    self.update_search_status_bar();
                    return;
                }
                KeyCode::Backspace => {
                    if self.search_cursor > 0 {
                        let before = &self.search_query[..self.search_cursor];
                        let char_len = before.chars().last().map(|c| c.len_utf8()).unwrap_or(0);
                        self.search_cursor -= char_len;
                        self.search_query.remove(self.search_cursor);
                        self.update_search_matches();
                        if !self.search_matches.is_empty() {
                            self.search_current = 0;
                            self.scroll_to_match(self.search_current);
                        }
                    }
                    self.update_search_status_bar();
                    return;
                }
                _ => {
                    return;
                }
            }
        }

        let event = event_with_count.event;
        let count = event_with_count.count;

        match event {
            UserEvent::NavigateDown | UserEvent::ScrollDown => {
                for _ in 0..count {
                    self.scroll_down();
                }
            }
            UserEvent::NavigateUp | UserEvent::ScrollUp => {
                for _ in 0..count {
                    self.scroll_up();
                }
            }
            UserEvent::PageDown => {
                for _ in 0..count {
                    self.scroll_page_down();
                }
            }
            UserEvent::PageUp => {
                for _ in 0..count {
                    self.scroll_page_up();
                }
            }
            UserEvent::HalfPageDown => {
                for _ in 0..count {
                    self.scroll_half_page_down();
                }
            }
            UserEvent::HalfPageUp => {
                for _ in 0..count {
                    self.scroll_half_page_up();
                }
            }
            UserEvent::GoToTop => {
                self.scroll_offset = 0;
            }
            UserEvent::GoToBottom => {
                self.scroll_to_bottom();
            }
            UserEvent::ShortCopy => {
                self.copy_file_path();
            }
            UserEvent::FullCopy => {
                self.copy_commit_hash();
            }
            UserEvent::NavigateRight => {
                self.move_focus(true);
            }
            UserEvent::NavigateLeft => {
                self.move_focus(false);
            }
            UserEvent::Confirm => {
                if let Some(idx) = self.focused_hunk {
                    self.activate_focused_hunk(idx);
                } else if let Some(idx) = self.focused_button {
                    self.activate_button(idx);
                } else if self.commit_hash.is_empty() {
                    self.tx.send(AppEvent::CloseDiffToUncommitted);
                } else {
                    self.tx.send(AppEvent::CloseDiffToDetail);
                }
            }
            UserEvent::Cancel | UserEvent::Close => {
                if self.commit_hash.is_empty() {
                    self.tx.send(AppEvent::CloseDiffToUncommitted);
                } else {
                    self.tx.send(AppEvent::CloseDiffToDetail);
                }
            }
            UserEvent::CycleFileNext => {
                self.cycle_file(1);
            }
            UserEvent::CycleFilePrev => {
                self.cycle_file(-1);
            }
            UserEvent::UserCommand(n) => {
                self.tx.send(AppEvent::OpenUserCommand(n));
            }
            UserEvent::HelpToggle => {
                self.tx.send(AppEvent::OpenHelp);
            }
            UserEvent::Refresh => {
                self.refresh();
            }
            UserEvent::Search => {
                self.search_active = !self.search_active;
                if !self.search_active {
                    self.search_query.clear();
                    self.search_cursor = 0;
                    self.search_matches.clear();
                    self.clear_search_status_bar();
                } else {
                    self.update_search_status_bar();
                }
            }
            UserEvent::GoToNext if !self.search_matches.is_empty() => {
                self.search_current = (self.search_current + 1) % self.search_matches.len();
                self.scroll_to_match(self.search_current);
                self.update_search_status_bar();
            }
            UserEvent::GoToPrevious if !self.search_matches.is_empty() => {
                self.search_current = if self.search_current == 0 {
                    self.search_matches.len() - 1
                } else {
                    self.search_current - 1
                };
                self.scroll_to_match(self.search_current);
                self.update_search_status_bar();
            }
            UserEvent::FileHistory => {
                let file_path = self
                    .title
                    .strip_prefix("Diff (staged): ")
                    .or_else(|| self.title.strip_prefix("Diff (unstaged): "))
                    .or_else(|| self.title.strip_prefix("Diff: "))
                    .unwrap_or(&self.title)
                    .to_string();
                if !file_path.is_empty() {
                    self.tx.send(AppEvent::OpenFileHistory { file_path });
                }
            }
            UserEvent::Blame => {
                let file_path = self
                    .title
                    .strip_prefix("Diff (staged): ")
                    .or_else(|| self.title.strip_prefix("Diff (unstaged): "))
                    .or_else(|| self.title.strip_prefix("Diff: "))
                    .unwrap_or(&self.title)
                    .to_string();
                if !file_path.is_empty() {
                    self.tx.send(AppEvent::OpenBlame { file_path });
                }
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let [list_area, diff_area] = self.split_areas(area);

        if let Some(ref mut list_state) = self.commit_list_state {
            let commit_list = CommitList::new(self.ctx.clone());
            f.render_stateful_widget(commit_list, list_area, list_state);
            self.render_diff(f, diff_area);
        } else {
            self.render_diff(f, area);
        }
    }

    fn render_diff(&mut self, f: &mut Frame, diff_area: Rect) {
        let content_area = if !self.title.is_empty() {
            let [separator_area, title_area, content_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(diff_area);

            let separator = Line::from(
                "─"
                    .repeat(diff_area.width as usize)
                    .fg(self.ctx.color_theme.divider_fg),
            );
            f.render_widget(Paragraph::new(separator), separator_area);

            // Build title with diff stats
            let (add_count, del_count) = if let Some(entry) = self.diff_entries.first() {
                let a = entry
                    .hunks
                    .iter()
                    .flat_map(|h| h.lines.iter())
                    .filter(|l| l.line_type == DiffLineType::Addition)
                    .count();
                let d = entry
                    .hunks
                    .iter()
                    .flat_map(|h| h.lines.iter())
                    .filter(|l| l.line_type == DiffLineType::Deletion)
                    .count();
                (a, d)
            } else {
                (0, 0)
            };
            // Compare mode: split "Compare <older>..<newer>" into separate
            // colored spans so older renders in deletion-red (left, the
            // "removed-from" endpoint) and newer in addition-green (right,
            // the "added-to" endpoint). The double-dot is replaced by " → "
            // for clarity. Falls back to the plain single-span title for any
            // other diff (single-commit, file diff, stash, uncommitted).
            let mut title_spans: Vec<Span<'static>> =
                if let Some(rest) = self.title.strip_prefix("Compare ") {
                    if let Some((older, newer)) = rest.split_once("..") {
                        vec![
                            Span::styled(
                                "─── Compare ".to_string(),
                                Style::default()
                                    .fg(self.ctx.color_theme.fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                older.to_string(),
                                Style::default()
                                    .fg(self.ctx.color_theme.detail_file_change_delete_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                " → ".to_string(),
                                Style::default()
                                    .fg(self.ctx.color_theme.fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("{} ", newer),
                                Style::default()
                                    .fg(self.ctx.color_theme.detail_file_change_add_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]
                    } else {
                        vec![Span::styled(
                            format!("─── {} ", self.title),
                            Style::default()
                                .fg(self.ctx.color_theme.fg)
                                .add_modifier(Modifier::BOLD),
                        )]
                    }
                } else {
                    vec![Span::styled(
                        format!("─── {} ", self.title),
                        Style::default()
                            .fg(self.ctx.color_theme.fg)
                            .add_modifier(Modifier::BOLD),
                    )]
                };
            if add_count > 0 {
                title_spans.push(Span::styled(
                    format!("+{add_count} "),
                    Style::default()
                        .fg(self.ctx.color_theme.detail_file_change_add_fg)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if del_count > 0 {
                title_spans.push(Span::styled(
                    format!("-{del_count} "),
                    Style::default()
                        .fg(self.ctx.color_theme.detail_file_change_delete_fg)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            title_spans.push(Span::styled(
                "───",
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::BOLD),
            ));
            let title = Line::from(title_spans);
            f.render_widget(Paragraph::new(title), title_area);

            content_area
        } else {
            diff_area
        };

        // The search bar is now rendered globally via StatusLine::Input, no
        // need to carve a row out of the content area for an inline bar.
        self.diff_content_area = Some(content_area);

        // Rebuild base lines if needed (content changed, not just hover)
        if self.needs_rebuild || self.base_lines.is_empty() {
            self.base_lines = self.build_diff_lines(&content_area);
            self.needs_rebuild = false;

            if self.first_build_pending {
                // First build: honour the Uncommitted view's "open at first
                // staged/unstaged hunk" request, then fall back to auto-focusing
                // the first focusable. Subsequent builds preserve focus.
                self.first_build_pending = false;
                // Priority 1: restore the hunk the user just toggled, anchored
                // at the same scroll position so the view doesn't snap back
                // to the top after stage/unstage. Restore always wins over
                // `initial_scroll_origin`, `restored=true` is set even when
                // the underlying hunk_idx no longer maps cleanly (which can
                // happen if new_start shifted enough to reorder the hunks),
                // and we fall back to the hunk closest to the restored
                // scroll position rather than letting `initial_scroll_origin`
                // jump to the first hunk of the file's other side.
                let restored = if let Some((hunk_idx, scroll)) = self.pending_restore.take() {
                    self.scroll_offset = scroll;
                    let by_idx = self.hunk_spans.iter().position(|s| s.hunk_idx == hunk_idx);
                    let by_scroll = || {
                        // Pick the span whose start row is closest to where
                        // the user was last anchored (top of viewport).
                        let target = scroll;
                        self.hunk_spans
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, s)| {
                                let mid = s.start.midpoint(s.end);
                                mid.abs_diff(target)
                            })
                            .map(|(i, _)| i)
                    };
                    if let Some(span_idx) = by_idx.or_else(by_scroll) {
                        self.focused_hunk = Some(span_idx);
                        self.focused_button = None;
                    }
                    true
                } else {
                    false
                };
                if !restored {
                    if let Some(origin) = self.initial_scroll_origin.take() {
                        if let Some(span_idx) =
                            self.hunk_spans.iter().position(|s| s.origin == origin)
                        {
                            let span = &self.hunk_spans[span_idx];
                            self.scroll_offset = span.start;
                            self.focused_hunk = Some(span_idx);
                            self.focused_button = None;
                        } else if !self.expand_buttons.is_empty() {
                            self.focused_button = Some(0);
                        }
                    } else if !self.expand_buttons.is_empty() {
                        self.focused_button = Some(0);
                    } else if !self.hunk_spans.is_empty() {
                        self.focused_hunk = Some(0);
                    }
                }
            } else {
                // Subsequent rebuilds: just clamp focus to the new index range.
                if let Some(idx) = self.focused_button {
                    if self.expand_buttons.is_empty() {
                        self.focused_button = None;
                    } else {
                        self.focused_button = Some(idx.min(self.expand_buttons.len() - 1));
                    }
                }
                if let Some(idx) = self.focused_hunk {
                    if self.hunk_spans.is_empty() {
                        self.focused_hunk = None;
                    } else {
                        self.focused_hunk = Some(idx.min(self.hunk_spans.len() - 1));
                    }
                }
                if let Some(idx) = self.hovered_hunk {
                    if self.hunk_spans.is_empty() {
                        self.hovered_hunk = None;
                    } else {
                        self.hovered_hunk = Some(idx.min(self.hunk_spans.len() - 1));
                    }
                }
            }
            if !self.search_query.is_empty() {
                self.update_search_matches();
            }
        }
        self.content_height = self.base_lines.len();

        // Build visible lines from cache, applying hover only to button lines
        let mut visible_lines: Vec<Line> = self
            .base_lines
            .iter()
            .skip(self.scroll_offset)
            .take(content_area.height as usize)
            .cloned()
            .collect();

        // Apply highlight to the single focused/hovered button
        if let Some(btn_idx) = self.focused_button {
            if let Some(btn) = self.expand_buttons.get(btn_idx) {
                if btn.line_idx >= self.scroll_offset {
                    let visible_idx = btn.line_idx - self.scroll_offset;
                    if visible_idx < visible_lines.len() {
                        visible_lines[visible_idx] =
                            self.build_button_line(btn, true, content_area.width);
                    }
                }
            }
        }

        // Hunk hover / focus highlight, extend the row across the entire
        // width (commit-list style) AND brighten the green/red bg for changed
        // lines so the modification colour stays readable. Context lines that
        // had no bg get the neutral `list_selected_bg` grey. The padding span
        // we tack on at the end always uses the neutral grey since it sits
        // past the content.
        //
        // Hover (mouse) wins over focus (keyboard) so the mouse always paints
        // what the user is pointing at; otherwise keyboard navigation locks
        // the highlight and mouse hover appears to do nothing, especially in
        // Raw mode where there's no banner to fall back on.
        let active_hunk_span = self
            .hovered_hunk
            .or(self.focused_hunk)
            .and_then(|i| self.hunk_spans.get(i))
            .cloned();
        if let Some(span) = active_hunk_span {
            // Recompute the 4 diff bgs the same way `build_base_lines` does so
            // we can match span bgs and swap to the hover variant. The hover
            // variants sit roughly midway between the normal and the strong
            // (word-diff) bgs, bright enough to read as "selected" but dark
            // enough that the dim line-number fg stays visible on top of it.
            let bg_is_light = match self.ctx.color_theme.bg {
                Color::Rgb(r, g, b) => {
                    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) > 128.0
                }
                _ => false,
            };
            let add_bg = if bg_is_light {
                Color::Rgb(172, 242, 189)
            } else {
                Color::Rgb(32, 68, 45)
            };
            let del_bg = if bg_is_light {
                Color::Rgb(255, 186, 181)
            } else {
                Color::Rgb(68, 35, 40)
            };
            let add_hover = if bg_is_light {
                Color::Rgb(123, 218, 147)
            } else {
                Color::Rgb(46, 106, 62)
            };
            let del_hover = if bg_is_light {
                Color::Rgb(235, 130, 130)
            } else {
                Color::Rgb(111, 45, 52)
            };
            let neutral_bg = self.ctx.color_theme.list_selected_bg;
            // Line numbers (and other "chrome" spans) use `divider_fg`, a dim
            // grey that gets swallowed by the hover bg. Promote them to the
            // theme's main `fg` so they stay legible on the selected row.
            let dim_fg = self.ctx.color_theme.divider_fg;
            let bright_fg = self.ctx.color_theme.fg;
            let total_w = content_area.width as usize;
            for (vis_idx, line) in visible_lines.iter_mut().enumerate() {
                let abs_idx = self.scroll_offset + vis_idx;
                if abs_idx >= span.start && abs_idx < span.end {
                    // Pick the row's dominant bg flavour: an add line keeps a
                    // bright green, a del line a bright red, everything else
                    // a neutral grey. Detection looks at the first span that
                    // already carries a bg.
                    let row_bg = line
                        .spans
                        .iter()
                        .find_map(|s| s.style.bg)
                        .map(|b| {
                            if b == add_bg {
                                add_hover
                            } else if b == del_bg {
                                del_hover
                            } else {
                                neutral_bg
                            }
                        })
                        .unwrap_or(neutral_bg);

                    let mut used: usize = 0;
                    for s in line.spans.iter_mut() {
                        s.style.bg = Some(row_bg);
                        if s.style.fg == Some(dim_fg) {
                            s.style.fg = Some(bright_fg);
                        }
                        used = used.saturating_add(s.content.chars().count());
                    }
                    if used < total_w {
                        let pad = total_w - used;
                        line.spans
                            .push(Span::styled(" ".repeat(pad), Style::default().bg(row_bg)));
                    }
                }
            }
        }

        // Apply search highlighting to every occurrence in the visible range.
        // Each occurrence is its own SearchMatch; the current one is identified
        // by its (line_idx, start) pair and gets an UNDERLINED modifier on top.
        if !self.search_query.is_empty() && !self.search_matches.is_empty() {
            let current = self.search_matches.get(self.search_current).copied();
            let match_bg = self.ctx.color_theme.list_match_bg;
            let match_fg = self.ctx.color_theme.list_match_fg;

            for (vis_idx, line) in visible_lines.iter_mut().enumerate() {
                let abs_idx = self.scroll_offset + vis_idx;
                let line_matches: Vec<SearchMatch> = self
                    .search_matches
                    .iter()
                    .filter(|m| m.line_idx == abs_idx)
                    .copied()
                    .collect();
                if !line_matches.is_empty() {
                    let current_start_in_line =
                        current.filter(|c| c.line_idx == abs_idx).map(|c| c.start);
                    *line = highlight_search_matches(
                        line,
                        &line_matches,
                        current_start_in_line,
                        match_bg,
                        match_fg,
                    );
                }
            }
        }

        let paragraph = Paragraph::new(visible_lines);
        f.render_widget(paragraph, content_area);
    }

    pub fn update_layout(&mut self, area: Rect) {
        let [list_area, _] = self.split_areas(area);

        // When the commit-list shrinks, Kitty images that were placed in the now-gone
        // rows persist over the diff content. Delete them before the next render.
        if self.cached_list_height > list_area.height {
            let first_orphaned = area.y + list_area.height;
            let last_orphaned = area.y + self.cached_list_height;
            for y in first_orphaned..last_orphaned {
                let _ = self.ctx.image_protocol.delete_row(y);
            }
        }
        self.cached_list_height = list_area.height;

        if let Some(ref mut state) = self.commit_list_state {
            let height = if list_area.height >= 2 {
                list_area.height - 2
            } else {
                list_area.height
            };
            state.update_height(height as usize);
        }
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
    }

    pub fn clear_graph_images(&mut self) {
        if let Some(ref mut state) = self.commit_list_state {
            state.clear_graph_images();
        }
    }
}

impl<'a> DiffView<'a> {
    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
    }

    #[allow(dead_code)]
    fn as_mut_list_state(&mut self) -> Option<&mut CommitListState<'a>> {
        self.commit_list_state.as_mut()
    }

    pub fn as_list_state(&self) -> Option<&CommitListState<'a>> {
        self.commit_list_state.as_ref()
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

    fn split_areas(&mut self, area: Rect) -> [Rect; 2] {
        let available_height = area.height;
        let content_lines = self.count_diff_lines(area.width);

        let min_diff_height = (available_height * 2) / 3;
        let ideal_diff_height = if content_lines > min_diff_height as usize {
            available_height
        } else {
            min_diff_height
        };

        let diff_height = ideal_diff_height
            .min(available_height.saturating_sub(3))
            .max(8);
        let list_height = available_height - diff_height;

        Layout::vertical([
            Constraint::Length(list_height),
            Constraint::Length(diff_height),
        ])
        .areas(area)
    }

    fn count_diff_lines(&mut self, width: u16) -> usize {
        // Fast path: base_lines are already built
        if !self.base_lines.is_empty() {
            return self.base_lines.len();
        }
        // If the content hasn't changed and we have a cached count, use it
        if !self.needs_rebuild {
            return 0; // height already known to be minimal, avoid rebuild
        }
        let dummy_area = Rect::new(0, 0, width, 1);
        self.build_diff_lines(&dummy_area).len()
    }

    fn build_diff_lines(&mut self, diff_area: &Rect) -> Vec<Line<'static>> {
        // Reset hunk row tracking, populated per-mode (currently only Raw
        // tracks precisely; the other modes can be wired up later).
        self.hunk_spans.clear();
        match self.ctx.ui_config.common.diff_mode {
            DiffMode::Raw => self.build_raw_diff_lines(diff_area),
            DiffMode::Enhanced => self.build_base_lines(diff_area.width),
            DiffMode::SideBySide => self.build_sbs_diff_lines(diff_area),
            DiffMode::SideBySideEnhanced => self.build_sbs_enhanced_diff_lines(diff_area),
        }
    }

    /// Side-by-side renderer: old version on the left, new on the right,
    /// vertical bar between. Consecutive Deletion+Addition runs are zipped row
    /// by row so a "modification" shows the old and new variants on the same
    /// row. Pure deletions get an empty right cell, pure additions an empty
    /// left cell. Headers (file / hunk) span the full width on their own line.
    /// Long content is truncated with `…` rather than wrapped, wrapping each
    /// half independently would misalign the pair.
    fn build_sbs_diff_lines(&mut self, diff_area: &Rect) -> Vec<Line<'static>> {
        use crate::git::diff::DiffLineType;

        let total_width = diff_area.width as usize;
        // Layout: [half] [│] [half], 1 col reserved for the separator.
        let half = total_width.saturating_sub(1) / 2;
        if half == 0 {
            return Vec::new();
        }

        let mut lines = Vec::new();
        let dim_style = Style::default()
            .fg(self.ctx.color_theme.detail_hash_fg)
            .add_modifier(Modifier::DIM);
        let sep_style = Style::default().fg(self.ctx.color_theme.divider_fg);
        let add_style = Style::default().fg(self.ctx.color_theme.detail_file_change_add_fg);
        let del_style = Style::default().fg(self.ctx.color_theme.detail_file_change_delete_fg);
        let ctx_style = Style::default().fg(self.ctx.color_theme.fg);
        let warn_style = Style::default()
            .fg(self.ctx.color_theme.status_warn_fg)
            .add_modifier(Modifier::BOLD);

        // Push a single full-width line styled uniformly.
        let push_full = |lines: &mut Vec<Line<'static>>, content: String, style: Style| {
            for chunk in wrap_text(&content, total_width) {
                lines.push(Line::from(Span::styled(chunk.to_string(), style)));
            }
        };

        // Push a paired left/right row with the vertical separator. Each side
        // is independently truncated to `half` cells; an empty side renders as
        // pure padding so the separator stays at a fixed column.
        let push_row = |lines: &mut Vec<Line<'static>>,
                        left: Option<(String, Style)>,
                        right: Option<(String, Style)>| {
            let left_text = left
                .as_ref()
                .map(|(t, _)| truncate_to_width(t, half))
                .unwrap_or_else(|| pad_to_width("", half));
            let right_text = right
                .as_ref()
                .map(|(t, _)| truncate_to_width(t, half))
                .unwrap_or_else(|| pad_to_width("", half));
            let left_style = left.map(|(_, s)| s).unwrap_or_default();
            let right_style = right.map(|(_, s)| s).unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(left_text, left_style),
                Span::styled("│", sep_style),
                Span::styled(right_text, right_style),
            ]));
        };

        let mut sbs_hunk_spans: Vec<HunkRowSpan> = Vec::new();
        for (entry_idx, entry) in self.diff_entries.iter().enumerate() {
            // File path banner, full width.
            if let Some(path) = entry.new_path.as_ref().or(entry.old_path.as_ref()) {
                push_full(&mut lines, format!("── {} ──", path), dim_style);
            }

            for (hunk_idx, hunk) in entry.hunks.iter().enumerate() {
                let hunk_row_start = lines.len();
                // Per-hunk opening banner, only shown when the hunk has a
                // staged/unstaged origin. For SBS Raw we use indent=0 since
                // there are no line-number columns.
                if let Some(banner) =
                    self.hunk_banner_line(hunk.origin, 0, diff_area.width, false, false)
                {
                    lines.push(banner);
                }
                // Walk hunk lines, batching consecutive del / add runs so we can
                // pair them cell by cell.
                let mut i = 0;
                let h_lines = &hunk.lines;
                while i < h_lines.len() {
                    let l = &h_lines[i];
                    match l.line_type {
                        DiffLineType::HunkHeader | DiffLineType::FileHeader => {
                            push_full(&mut lines, l.content.clone(), dim_style);
                            i += 1;
                        }
                        DiffLineType::BinaryNote => {
                            push_full(&mut lines, format!(" {}", l.content), warn_style);
                            i += 1;
                        }
                        DiffLineType::Context => {
                            // Context appears identically on both sides.
                            let text = format!(" {}", l.content);
                            push_row(
                                &mut lines,
                                Some((text.clone(), ctx_style)),
                                Some((text, ctx_style)),
                            );
                            i += 1;
                        }
                        DiffLineType::Deletion => {
                            // Collect run of consecutive deletions.
                            let mut dels: Vec<String> = Vec::new();
                            while i < h_lines.len()
                                && matches!(h_lines[i].line_type, DiffLineType::Deletion)
                            {
                                dels.push(format!("-{}", h_lines[i].content));
                                i += 1;
                            }
                            // Then collect the immediately-following run of additions
                            // (treat them as paired modifications).
                            let mut adds: Vec<String> = Vec::new();
                            while i < h_lines.len()
                                && matches!(h_lines[i].line_type, DiffLineType::Addition)
                            {
                                adds.push(format!("+{}", h_lines[i].content));
                                i += 1;
                            }
                            // Zip them, padding the shorter side with a None cell.
                            let max = dels.len().max(adds.len());
                            for j in 0..max {
                                let l_cell = dels.get(j).cloned().map(|t| (t, del_style));
                                let r_cell = adds.get(j).cloned().map(|t| (t, add_style));
                                push_row(&mut lines, l_cell, r_cell);
                            }
                        }
                        DiffLineType::Addition => {
                            // Pure additions (no preceding deletion run).
                            let text = format!("+{}", l.content);
                            push_row(&mut lines, None, Some((text, add_style)));
                            i += 1;
                        }
                    }
                }
                // Per-hunk closing banner.
                if let Some(banner) =
                    self.hunk_banner_line(hunk.origin, 0, diff_area.width, true, false)
                {
                    lines.push(banner);
                }
                if matches!(hunk.origin, HunkOrigin::Staged | HunkOrigin::Unstaged)
                    && lines.len() > hunk_row_start
                {
                    sbs_hunk_spans.push(HunkRowSpan {
                        hunk_idx,
                        entry_idx,
                        origin: hunk.origin,
                        start: hunk_row_start,
                        end: lines.len(),
                    });
                }
            }
        }

        self.hunk_spans = sbs_hunk_spans;
        lines
    }

    /// Side-by-side renderer with the Enhanced styling. Mirrors
    /// `build_base_lines` exactly (skips hunk headers, computes `─── N lines
    /// unchanged ───` gap markers between hunks, displays visible_up/down
    /// context from `new_file_lines`) but emits paired left/right rows. The
    /// `show more` buttons are not yet rendered in this mode, the click
    /// area / gap-state plumbing is tightly bound to the single-column
    /// rendering. Switch to Enhanced for gap navigation.
    fn build_sbs_enhanced_diff_lines(&mut self, diff_area: &Rect) -> Vec<Line<'static>> {
        use crate::git::diff::DiffLineType;

        let total_width = diff_area.width as usize;
        let half = total_width.saturating_sub(1) / 2;
        // Each half: 7 chars for the "{:>4} │ " line-number gutter, the rest
        // for content. Bail out if the area is too narrow to fit a gutter.
        const GUTTER: usize = 7;
        if half <= GUTTER + 1 {
            return Vec::new();
        }
        let mut lines: Vec<Line<'static>> = Vec::new();

        let entry = match self.diff_entries.first() {
            Some(e) => e,
            None => return lines,
        };

        let file_path = entry
            .new_path
            .as_deref()
            .or(entry.old_path.as_deref())
            .unwrap_or("");
        let mut highlighter =
            SyntaxHighlighter::new_with_theme(file_path, &self.ctx.core_config.option.syntax_theme);

        // Theme-aware backgrounds, same logic as build_base_lines so the
        // visual feel matches Enhanced exactly.
        let bg_is_light = match self.ctx.color_theme.bg {
            ratatui::style::Color::Rgb(r, g, b) => {
                (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) > 128.0
            }
            _ => false,
        };
        let add_bg = if bg_is_light {
            Color::Rgb(172, 242, 189)
        } else {
            Color::Rgb(32, 68, 45)
        };
        let del_bg = if bg_is_light {
            Color::Rgb(255, 186, 181)
        } else {
            Color::Rgb(68, 35, 40)
        };

        let sep_style = Style::default().fg(self.ctx.color_theme.divider_fg);
        let warn_style = Style::default()
            .fg(self.ctx.color_theme.status_warn_fg)
            .add_modifier(Modifier::BOLD);
        let unchanged_label_style = Style::default()
            .fg(self.ctx.color_theme.fg)
            .add_modifier(Modifier::ITALIC);

        // ── helpers ─────────────────────────────────────────────────────
        // Render one half (gutter + syntax-highlighted content), padded to
        // exactly `half` cells with the supplied row background.
        let render_half = |line_no: Option<u32>,
                           content: &str,
                           row_bg: Option<Color>,
                           hl: &mut Option<SyntaxHighlighter>|
         -> Vec<Span<'static>> {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let gutter = match line_no {
                Some(n) => format!("{:>4} │ ", n),
                None => "     │ ".to_string(),
            };
            let mut gutter_style = Style::default().fg(sep_style.fg.unwrap_or(Color::Reset));
            if let Some(bg) = row_bg {
                gutter_style = gutter_style.bg(bg);
            }
            spans.push(Span::styled(gutter, gutter_style));

            let content_w = half - GUTTER;
            let truncated = truncate_to_width(content, content_w);
            let visible_text: String = truncated.trim_end().to_string();
            let pad_w = content_w.saturating_sub(visible_text.chars().count());
            let mut base_style = Style::default();
            if let Some(bg) = row_bg {
                base_style = base_style.bg(bg);
            }
            if let Some(h) = hl {
                let highlighted = h.highlight_line(&visible_text, base_style, None);
                for s in highlighted {
                    spans.push(s);
                }
            } else {
                spans.push(Span::styled(visible_text, base_style));
            }
            if pad_w > 0 {
                spans.push(Span::styled(" ".repeat(pad_w), base_style));
            }
            spans
        };

        // Build a paired row from optional left + right cells.
        let push_paired_row = |lines: &mut Vec<Line<'static>>,
                               left: Option<(Option<u32>, &str, Color)>,
                               right: Option<(Option<u32>, &str, Color)>,
                               hl: &mut Option<SyntaxHighlighter>| {
            let mut row: Vec<Span<'static>> = Vec::new();
            match left {
                Some((no, content, bg)) => {
                    row.extend(render_half(no, content, Some(bg), hl));
                }
                None => {
                    row.push(Span::raw(" ".repeat(half)));
                }
            }
            row.push(Span::styled("│", sep_style));
            match right {
                Some((no, content, bg)) => {
                    row.extend(render_half(no, content, Some(bg), hl));
                }
                None => {
                    row.push(Span::raw(" ".repeat(half)));
                }
            }
            lines.push(Line::from(row));
        };

        // Push a single full-width line styled uniformly (banners, gap markers).
        let push_full = |lines: &mut Vec<Line<'static>>, content: String, style: Style| {
            for chunk in wrap_text(&content, total_width) {
                lines.push(Line::from(Span::styled(chunk.to_string(), style)));
            }
        };

        // Render an unchanged context line from `new_file_lines` on both
        // sides (same line number on left and right since the line is shared
        // between old and new).
        let push_unchanged_context =
            |lines: &mut Vec<Line<'static>>,
             line_no: usize,
             new_file_lines: &[String],
             hl: &mut Option<SyntaxHighlighter>| {
                if line_no == 0 || line_no > new_file_lines.len() {
                    return;
                }
                let content = new_file_lines[line_no - 1].as_str();
                let mut row: Vec<Span<'static>> = Vec::new();
                row.extend(render_half(Some(line_no as u32), content, None, hl));
                row.push(Span::styled("│", sep_style));
                row.extend(render_half(Some(line_no as u32), content, None, hl));
                lines.push(Line::from(row));
            };

        // ── helpers for hunk gap detection (mirrors build_base_lines) ──
        fn hunk_first_new_line(hunk: &crate::git::diff::Hunk) -> Option<u32> {
            hunk.lines.iter().find_map(|l| {
                if l.line_type == DiffLineType::Context {
                    l.new_line_no
                } else {
                    None
                }
            })
        }
        fn hunk_last_new_line(hunk: &crate::git::diff::Hunk) -> Option<u32> {
            hunk.lines.iter().rev().find_map(|l| {
                if l.line_type == DiffLineType::Context {
                    l.new_line_no
                } else {
                    None
                }
            })
        }

        let gap_states = self.gap_states.clone();
        let new_file_lines = self.new_file_lines.clone();
        let visible_default = 3usize;

        // Render a gap. `edge`: None=middle, Some(true)=top, Some(false)=bottom.
        // For SBS we don't yet emit clickable buttons, the `─── N lines
        // unchanged ───` marker spans the full width, and visible_up/down
        // context lines are emitted as paired rows.
        let render_gap = |lines: &mut Vec<Line<'static>>,
                          gap_start: u32,
                          gap_end: u32,
                          gidx: usize,
                          hl: &mut Option<SyntaxHighlighter>,
                          edge: Option<bool>| {
            if gap_end <= gap_start {
                return;
            }
            let total = (gap_end - gap_start) as usize;
            let gap_state = gap_states.get(gidx).cloned().unwrap_or(GapState {
                visible_up: if edge.is_some() {
                    0
                } else {
                    visible_default.min(total)
                },
                visible_down: if edge.is_some() {
                    0
                } else {
                    visible_default.min(total)
                },
                total,
            });
            let mut visible_up = gap_state.visible_up.min(total);
            let mut visible_down = gap_state.visible_down.min(total);
            if visible_up + visible_down > total {
                visible_up = total.saturating_sub(visible_down);
                visible_down = total.saturating_sub(visible_up);
            }

            match edge {
                Some(true) => {
                    // Top edge: marker first, then visible context up to the hunk.
                    let hidden = total.saturating_sub(visible_up);
                    if hidden > 0 {
                        push_full(
                            lines,
                            format!("─── {} lines unchanged ───", hidden),
                            unchanged_label_style,
                        );
                    }
                    for i in 0..visible_up {
                        let line_no = (gap_end - visible_up as u32 + i as u32) as usize;
                        push_unchanged_context(lines, line_no, &new_file_lines, hl);
                    }
                }
                Some(false) => {
                    // Bottom edge: visible context first, then marker.
                    for i in 0..visible_down {
                        let line_no = (gap_start + i as u32) as usize;
                        push_unchanged_context(lines, line_no, &new_file_lines, hl);
                    }
                    let hidden = total.saturating_sub(visible_down);
                    if hidden > 0 {
                        push_full(
                            lines,
                            format!("─── {} lines unchanged ───", hidden),
                            unchanged_label_style,
                        );
                    }
                }
                None => {
                    // Middle gap: visible_up trailing the previous hunk, then
                    // marker, then visible_down leading the next hunk.
                    for i in 0..visible_up {
                        let line_no = (gap_start + i as u32) as usize;
                        push_unchanged_context(lines, line_no, &new_file_lines, hl);
                    }
                    let hidden = total.saturating_sub(visible_up + visible_down);
                    if hidden > 0 {
                        push_full(
                            lines,
                            format!("─── {} lines unchanged ───", hidden),
                            unchanged_label_style,
                        );
                    }
                    for i in (0..visible_down).rev() {
                        let line_no = (gap_end - i as u32 - 1) as usize;
                        push_unchanged_context(lines, line_no, &new_file_lines, hl);
                    }
                }
            }
        };

        // (No extra file banner, the diff view already renders
        // "── Diff: <path> +N -M ──" above this content. Mirroring Enhanced.)

        // ── top gap (before first hunk) ─────────────────────────────────
        let mut gap_idx = 0usize;
        if let Some(first_hunk) = entry.hunks.first() {
            if let Some(first_new) = hunk_first_new_line(first_hunk) {
                if first_new > 1 {
                    render_gap(
                        &mut lines,
                        1,
                        first_new,
                        gap_idx,
                        &mut highlighter,
                        Some(true),
                    );
                    gap_idx += 1;
                }
            }
        }

        // ── walk hunks ──────────────────────────────────────────────────
        let mut sbs_enh_hunk_spans: Vec<HunkRowSpan> = Vec::new();
        for (hunk_idx, hunk) in entry.hunks.iter().enumerate() {
            // Gap between hunks.
            if hunk_idx > 0 {
                if let Some(prev_hunk) = entry.hunks.get(hunk_idx - 1) {
                    let prev_end = hunk_last_new_line(prev_hunk);
                    let curr_start = hunk_first_new_line(hunk);
                    if let (Some(pe), Some(cs)) = (prev_end, curr_start) {
                        if cs > pe + 1 {
                            render_gap(&mut lines, pe + 1, cs, gap_idx, &mut highlighter, None);
                            gap_idx += 1;
                        }
                    }
                }
            }

            let hunk_row_start = lines.len();
            // Per-hunk opening banner, SBS Enhanced has no global indent, the
            // bg highlighting starts at column 0 on each half.
            if let Some(banner) =
                self.hunk_banner_line(hunk.origin, 0, diff_area.width, false, false)
            {
                lines.push(banner);
            }

            // Hunk content, skip(1) to drop the @@ HunkHeader line.
            let h_lines: Vec<&crate::git::diff::DiffLine> = hunk.lines.iter().skip(1).collect();
            let mut i = 0;
            while i < h_lines.len() {
                let l = h_lines[i];
                match l.line_type {
                    DiffLineType::FileHeader | DiffLineType::HunkHeader => {
                        // Defensive, should already be skipped by skip(1).
                        i += 1;
                    }
                    DiffLineType::BinaryNote => {
                        push_full(&mut lines, format!(" {}", l.content), warn_style);
                        i += 1;
                    }
                    DiffLineType::Context => {
                        let mut row: Vec<Span<'static>> = Vec::new();
                        row.extend(render_half(
                            l.old_line_no,
                            &l.content,
                            None,
                            &mut highlighter,
                        ));
                        row.push(Span::styled("│", sep_style));
                        row.extend(render_half(
                            l.new_line_no,
                            &l.content,
                            None,
                            &mut highlighter,
                        ));
                        lines.push(Line::from(row));
                        i += 1;
                    }
                    DiffLineType::Deletion => {
                        // Collect consecutive del + add runs, zip into pairs.
                        let mut dels: Vec<&crate::git::diff::DiffLine> = Vec::new();
                        while i < h_lines.len()
                            && matches!(h_lines[i].line_type, DiffLineType::Deletion)
                        {
                            dels.push(h_lines[i]);
                            i += 1;
                        }
                        let mut adds: Vec<&crate::git::diff::DiffLine> = Vec::new();
                        while i < h_lines.len()
                            && matches!(h_lines[i].line_type, DiffLineType::Addition)
                        {
                            adds.push(h_lines[i]);
                            i += 1;
                        }
                        let max = dels.len().max(adds.len());
                        for j in 0..max {
                            let l_cell = dels
                                .get(j)
                                .map(|d| (d.old_line_no, d.content.as_str(), del_bg));
                            let r_cell = adds
                                .get(j)
                                .map(|a| (a.new_line_no, a.content.as_str(), add_bg));
                            push_paired_row(&mut lines, l_cell, r_cell, &mut highlighter);
                        }
                    }
                    DiffLineType::Addition => {
                        push_paired_row(
                            &mut lines,
                            None,
                            Some((l.new_line_no, l.content.as_str(), add_bg)),
                            &mut highlighter,
                        );
                        i += 1;
                    }
                }
            }
            // Per-hunk closing banner.
            if let Some(banner) =
                self.hunk_banner_line(hunk.origin, 0, diff_area.width, true, false)
            {
                lines.push(banner);
            }
            if matches!(hunk.origin, HunkOrigin::Staged | HunkOrigin::Unstaged)
                && lines.len() > hunk_row_start
            {
                sbs_enh_hunk_spans.push(HunkRowSpan {
                    hunk_idx,
                    entry_idx: 0,
                    origin: hunk.origin,
                    start: hunk_row_start,
                    end: lines.len(),
                });
            }
        }

        // ── bottom gap (after last hunk) ────────────────────────────────
        if let Some(last_hunk) = entry.hunks.last() {
            if let Some(last_new) = hunk_last_new_line(last_hunk) {
                let file_total = new_file_lines.len() as u32;
                if last_new < file_total {
                    render_gap(
                        &mut lines,
                        last_new + 1,
                        file_total + 1,
                        gap_idx,
                        &mut highlighter,
                        Some(false),
                    );
                }
            }
        }

        self.hunk_spans = sbs_enh_hunk_spans;
        lines
    }

    /// Gutter span for a hunk in the Uncommitted view. Returns `None` for
    /// origins where staging doesn't apply (committed diffs, untracked files).
    /// Width is 2 columns (glyph + space).
    fn hunk_gutter_span(&self, origin: HunkOrigin) -> Option<Span<'static>> {
        match origin {
            HunkOrigin::Staged => Some(Span::styled(
                "┃ ",
                Style::default().fg(self.ctx.color_theme.detail_file_change_add_fg),
            )),
            HunkOrigin::Unstaged => Some(Span::styled(
                "┊ ",
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::DIM),
            )),
            _ => None,
        }
    }

    /// Indicator prefix for the hunk header in the Uncommitted view
    /// `☑ ` (staged) or `☐ ` (unstaged). Empty string otherwise.
    fn hunk_indicator(origin: HunkOrigin) -> &'static str {
        match origin {
            HunkOrigin::Staged => "☑ ",
            HunkOrigin::Unstaged => "☐ ",
            _ => "",
        }
    }

    /// Banner line shown above (or below) each hunk in the Uncommitted view.
    /// Returns `None` when the origin doesn't carry a stage state (committed
    /// diffs, untracked files).
    ///
    /// * `indent`, number of blank columns before the banner content; lets
    ///   the banner align with the hunk's coloured background instead of the
    ///   screen edge in Enhanced mode (where line numbers sit on the left).
    /// * `available_width`, number of columns the banner content (label +
    ///   waves) is allowed to occupy after `indent`.
    /// * `closing`, when true, emit waves only (no label) so the closing
    ///   banner reads as a tail delimiter.
    fn hunk_banner_line(
        &self,
        origin: HunkOrigin,
        indent: u16,
        available_width: u16,
        closing: bool,
        active: bool,
    ) -> Option<Line<'static>> {
        let (label, color) = match origin {
            HunkOrigin::Staged => (" ☑ STAGED ", self.ctx.color_theme.detail_file_change_add_fg),
            HunkOrigin::Unstaged => (" ☐ UNSTAGED ", self.ctx.color_theme.fg),
            _ => return None,
        };
        // When the hunk is hovered or keyboard-focused we crank up emphasis on
        // the entire banner, label keeps its colour but adds REVERSED so it
        // pops, and the waves drop the DIM so they stand out too.
        let (label_style, wave_style) = if active {
            (
                Style::default()
                    .fg(color)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        } else {
            (
                Style::default().fg(color).add_modifier(Modifier::BOLD),
                Style::default().fg(color).add_modifier(Modifier::DIM),
            )
        };
        let mut spans = Vec::with_capacity(3);
        if indent > 0 {
            spans.push(Span::raw(" ".repeat(indent as usize)));
        }
        let avail = available_width as usize;
        if closing {
            spans.push(Span::styled("~".repeat(avail), wave_style));
        } else {
            let label_len = label.chars().count();
            let waves = avail.saturating_sub(label_len);
            spans.push(Span::styled(label.to_string(), label_style));
            spans.push(Span::styled("~".repeat(waves), wave_style));
        }
        Some(Line::from(spans))
    }

    fn build_raw_diff_lines(&mut self, diff_area: &Rect) -> Vec<Line<'static>> {
        let width = diff_area.width as usize;
        let mut lines = Vec::new();
        let mut hunk_spans: Vec<HunkRowSpan> = Vec::new();

        for (entry_idx, entry) in self.diff_entries.iter().enumerate() {
            if let Some(path) = &entry.new_path {
                for chunk in wrap_text(&format!("--- {}", path), width) {
                    lines.push(Line::from(Span::styled(
                        chunk.to_string(),
                        Style::default()
                            .fg(self.ctx.color_theme.detail_hash_fg)
                            .add_modifier(Modifier::DIM),
                    )));
                }
            } else if let Some(path) = &entry.old_path {
                for chunk in wrap_text(&format!("--- {}", path), width) {
                    lines.push(Line::from(Span::styled(
                        chunk.to_string(),
                        Style::default()
                            .fg(self.ctx.color_theme.detail_hash_fg)
                            .add_modifier(Modifier::DIM),
                    )));
                }
            }

            if let Some(path) = &entry.new_path {
                for chunk in wrap_text(&format!("+++ {}", path), width) {
                    lines.push(Line::from(Span::styled(
                        chunk.to_string(),
                        Style::default()
                            .fg(self.ctx.color_theme.detail_hash_fg)
                            .add_modifier(Modifier::DIM),
                    )));
                }
            }

            for (hunk_idx, hunk) in entry.hunks.iter().enumerate() {
                let gutter = self.hunk_gutter_span(hunk.origin);
                let gutter_w = if gutter.is_some() { 2 } else { 0 };
                let content_width = width.saturating_sub(gutter_w).max(1);
                let hunk_start_row = lines.len();

                for diff_line in &hunk.lines {
                    match diff_line.line_type {
                        DiffLineType::HunkHeader => {
                            // Hunk header keeps the ☑/☐ indicator in place of the gutter.
                            let prefixed = format!(
                                "{}{}",
                                Self::hunk_indicator(hunk.origin),
                                diff_line.content
                            );
                            for chunk in wrap_text(&prefixed, width) {
                                lines.push(Line::from(Span::styled(
                                    chunk.to_string(),
                                    Style::default()
                                        .fg(self.ctx.color_theme.detail_hash_fg)
                                        .add_modifier(Modifier::DIM),
                                )));
                            }
                        }
                        DiffLineType::FileHeader => {
                            for chunk in wrap_text(&diff_line.content, width) {
                                lines.push(Line::from(Span::styled(
                                    chunk.to_string(),
                                    Style::default()
                                        .fg(self.ctx.color_theme.detail_hash_fg)
                                        .add_modifier(Modifier::DIM),
                                )));
                            }
                        }
                        _ => {
                            let (prefix_char, content_style) = match diff_line.line_type {
                                DiffLineType::Context => {
                                    (' ', Style::default().fg(self.ctx.color_theme.fg))
                                }
                                DiffLineType::Addition => (
                                    '+',
                                    Style::default()
                                        .fg(self.ctx.color_theme.detail_file_change_add_fg),
                                ),
                                DiffLineType::Deletion => (
                                    '-',
                                    Style::default()
                                        .fg(self.ctx.color_theme.detail_file_change_delete_fg),
                                ),
                                DiffLineType::BinaryNote => (
                                    ' ',
                                    Style::default()
                                        .fg(self.ctx.color_theme.status_warn_fg)
                                        .add_modifier(Modifier::BOLD),
                                ),
                                _ => continue,
                            };
                            let text = format!("{}{}", prefix_char, diff_line.content);
                            for chunk in wrap_text(&text, content_width) {
                                let mut spans = Vec::with_capacity(2);
                                if let Some(g) = &gutter {
                                    spans.push(g.clone());
                                }
                                spans.push(Span::styled(chunk.to_string(), content_style));
                                lines.push(Line::from(spans));
                            }
                        }
                    }
                }

                // Record this hunk's rendered row range, only when it carries
                // a stage state, which is what the Uncommitted view needs to
                // detect click-to-toggle. Skips committed / untracked hunks.
                if matches!(hunk.origin, HunkOrigin::Staged | HunkOrigin::Unstaged)
                    && lines.len() > hunk_start_row
                {
                    hunk_spans.push(HunkRowSpan {
                        hunk_idx,
                        entry_idx,
                        origin: hunk.origin,
                        start: hunk_start_row,
                        end: lines.len(),
                    });
                }
            }
        }

        self.hunk_spans = hunk_spans;
        lines
    }

    fn build_base_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let entry = match self.diff_entries.first() {
            Some(e) => e,
            None => return lines,
        };

        let file_path = entry
            .new_path
            .as_deref()
            .or(entry.old_path.as_deref())
            .unwrap_or("");
        let mut highlighter =
            SyntaxHighlighter::new_with_theme(file_path, &self.ctx.core_config.option.syntax_theme);

        // Detect if the theme is light (luminance > 128 means light background)
        let bg_is_light = match self.ctx.color_theme.bg {
            ratatui::style::Color::Rgb(r, g, b) => {
                (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) > 128.0
            }
            _ => false,
        };

        let add_bg = if bg_is_light {
            Color::Rgb(172, 242, 189) // GitHub light: soft green
        } else {
            Color::Rgb(32, 68, 45) // VS Code dark: dark green
        };
        let del_bg = if bg_is_light {
            Color::Rgb(255, 186, 181) // GitHub light: soft red
        } else {
            Color::Rgb(68, 35, 40) // VS Code dark: dark red
        };
        // Word-diff strong highlight: significantly more vivid than the line bg.
        let add_strong_bg = if bg_is_light {
            Color::Rgb(75, 195, 105) // richer green on light
        } else {
            Color::Rgb(60, 145, 80) // brighter green on dark
        };
        let del_strong_bg = if bg_is_light {
            Color::Rgb(215, 75, 80) // richer red on light
        } else {
            Color::Rgb(155, 55, 65) // brighter red on dark
        };
        let ctx_fg = self.ctx.color_theme.fg;

        let add_style = Style::default().bg(add_bg);
        let del_style = Style::default().bg(del_bg);
        let ctx_style = Style::default().fg(ctx_fg);

        let bar_add = Span::styled(
            "▍",
            Style::default()
                .fg(self.ctx.color_theme.detail_file_change_add_fg)
                .bg(add_bg),
        );
        let bar_del = Span::styled(
            "▍",
            Style::default()
                .fg(self.ctx.color_theme.detail_file_change_delete_fg)
                .bg(del_bg),
        );

        self.expand_buttons.clear();
        let mut gap_idx = 0usize;

        // Helper: last context line's new_line_no in a hunk
        fn hunk_last_new_line(hunk: &crate::git::diff::Hunk) -> Option<u32> {
            hunk.lines.iter().rev().find_map(|l| {
                if l.line_type == DiffLineType::Context {
                    l.new_line_no
                } else {
                    None
                }
            })
        }

        // Helper: first context line's new_line_no in a hunk
        fn hunk_first_new_line(hunk: &crate::git::diff::Hunk) -> Option<u32> {
            hunk.lines.iter().find_map(|l| {
                if l.line_type == DiffLineType::Context {
                    l.new_line_no
                } else {
                    None
                }
            })
        }

        // Pre-compute the per-hunk staged/unstaged banners (opening + closing)
        // before borrowing `self` mutably through `render_gap` below.
        // Indent matches the line-number prefix width ({:>4} │ = 7 cells) so
        // banners visually butt against the hunk's coloured bg rather than
        // the screen edge.
        const ENHANCED_LINE_NUM_W: u16 = 7;
        let banner_avail = width.saturating_sub(ENHANCED_LINE_NUM_W).max(1);
        // Map hunk index → active flag. Mirrors the render-time priority:
        // hover (mouse) takes precedence over focus (keyboard), so only one
        // banner is highlighted at a time and it follows the mouse.
        let hovered = self.hovered_hunk;
        let focused = self.focused_hunk;
        let active_span_idx = hovered.or(focused);
        let hunk_active: Vec<bool> = (0..entry.hunks.len())
            .map(|i| {
                let span_idx = self
                    .hunk_spans
                    .iter()
                    .position(|s| s.hunk_idx == i && s.entry_idx == 0);
                span_idx.is_some_and(|si| Some(si) == active_span_idx)
            })
            .collect();
        let hunk_banners_open: Vec<Option<Line<'static>>> = entry
            .hunks
            .iter()
            .enumerate()
            .map(|(i, h)| {
                self.hunk_banner_line(
                    h.origin,
                    ENHANCED_LINE_NUM_W,
                    banner_avail,
                    false,
                    hunk_active[i],
                )
            })
            .collect();
        let hunk_banners_close: Vec<Option<Line<'static>>> = entry
            .hunks
            .iter()
            .enumerate()
            .map(|(i, h)| {
                self.hunk_banner_line(
                    h.origin,
                    ENHANCED_LINE_NUM_W,
                    banner_avail,
                    true,
                    hunk_active[i],
                )
            })
            .collect();

        // Render a gap between hunks (bidirectional)
        // Extract references to avoid closure capture conflicts
        let gap_states = &self.gap_states;
        let new_file_lines = &self.new_file_lines;

        // Helper: render a gap block. is_edge: None=middle, Some(true)=top, Some(false)=bottom
        let mut render_gap = |lines: &mut Vec<Line<'static>>,
                              gap_start: u32,
                              gap_end: u32,
                              gidx: usize,
                              hl: &mut Option<SyntaxHighlighter>,
                              edge: Option<bool>|
         -> usize {
            if gap_end <= gap_start {
                return gidx;
            }
            let total = (gap_end - gap_start) as usize;
            let gap_state = gap_states.get(gidx).cloned().unwrap_or(GapState {
                visible_up: if edge.is_some() { 0 } else { 3.min(total) },
                visible_down: if edge.is_some() { 0 } else { 3.min(total) },
                total,
            });

            let mut visible_up = gap_state.visible_up.min(total);
            let mut visible_down = gap_state.visible_down.min(total);
            if visible_up + visible_down > total {
                visible_up = total.saturating_sub(visible_down);
                visible_down = total.saturating_sub(visible_up);
            }

            match edge {
                Some(true) => {
                    // Top edge: ⩕ button, then text, then lines from top toward hunk
                    let hidden = total.saturating_sub(visible_up);
                    if hidden > 0 {
                        let unchanged_text = format!("─── {} lines unchanged ───", hidden);
                        let unchanged_width = unchanged_text.chars().count();
                        let label = "  ⩕  show more  ";
                        let label_len = label.chars().count();
                        let pad = unchanged_width.saturating_sub(label_len);
                        let left = pad / 2;
                        let idx = lines.len();
                        let col_start = left as u16;
                        let col_end = col_start + label_len as u16;
                        lines.push(Line::from(vec![
                            Span::styled(" ".repeat(left), Style::default()),
                            Span::styled(
                                label.to_string(),
                                Style::default()
                                    .fg(self.ctx.color_theme.status_info_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]));
                        self.expand_buttons.push(ButtonInfo {
                            line_idx: idx,
                            direction: ExpandDirection::Up,
                            col_start,
                            col_end,
                            gap_idx: gidx,
                            is_edge: true,
                        });
                        lines.push(Line::from(vec![Span::styled(
                            unchanged_text,
                            Style::default()
                                .fg(self.ctx.color_theme.fg)
                                .add_modifier(Modifier::ITALIC),
                        )]));
                    }
                    for i in 0..visible_up {
                        let line_no = (gap_end - visible_up as u32 + i as u32) as usize;
                        if line_no > 0 && line_no <= new_file_lines.len() {
                            let content = &new_file_lines[line_no - 1];
                            let line_num = format!("{:>4} │ ", line_no);
                            if let Some(ref mut h) = hl {
                                lines.extend(wrap_diff_line_with_syntax(
                                    content,
                                    &line_num,
                                    ctx_style,
                                    width,
                                    h,
                                    self.ctx.color_theme.divider_fg,
                                ));
                            } else {
                                lines.extend(wrap_diff_line(
                                    content,
                                    &line_num,
                                    ctx_style,
                                    width,
                                    self.ctx.color_theme.divider_fg,
                                ));
                            }
                        }
                    }
                }
                Some(false) => {
                    // Bottom edge: lines from hunk toward bottom, then text, then ⩖ button
                    for i in 0..visible_down {
                        let line_no = (gap_start + i as u32) as usize;
                        if line_no > 0 && line_no <= new_file_lines.len() {
                            let content = &new_file_lines[line_no - 1];
                            let line_num = format!("{:>4} │ ", line_no);
                            if let Some(ref mut h) = hl {
                                lines.extend(wrap_diff_line_with_syntax(
                                    content,
                                    &line_num,
                                    ctx_style,
                                    width,
                                    h,
                                    self.ctx.color_theme.divider_fg,
                                ));
                            } else {
                                lines.extend(wrap_diff_line(
                                    content,
                                    &line_num,
                                    ctx_style,
                                    width,
                                    self.ctx.color_theme.divider_fg,
                                ));
                            }
                        }
                    }
                    let hidden = total.saturating_sub(visible_down);
                    if hidden > 0 {
                        let unchanged_text = format!("─── {} lines unchanged ───", hidden);
                        let unchanged_width = unchanged_text.chars().count();
                        lines.push(Line::from(vec![Span::styled(
                            unchanged_text,
                            Style::default()
                                .fg(self.ctx.color_theme.fg)
                                .add_modifier(Modifier::ITALIC),
                        )]));
                        let label = "  ⩖  show more  ";
                        let label_len = label.chars().count();
                        let pad = unchanged_width.saturating_sub(label_len);
                        let left = pad / 2;
                        let idx = lines.len();
                        let col_start = left as u16;
                        let col_end = col_start + label_len as u16;
                        lines.push(Line::from(vec![
                            Span::styled(" ".repeat(left), Style::default()),
                            Span::styled(
                                label.to_string(),
                                Style::default()
                                    .fg(self.ctx.color_theme.status_info_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]));
                        self.expand_buttons.push(ButtonInfo {
                            line_idx: idx,
                            direction: ExpandDirection::Down,
                            col_start,
                            col_end,
                            gap_idx: gidx,
                            is_edge: true,
                        });
                    }
                }
                None => {
                    // Middle gap: bidirectional with both buttons
                    for i in 0..visible_up {
                        let line_no = (gap_start + i as u32) as usize;
                        if line_no > 0 && line_no <= new_file_lines.len() {
                            let content = &new_file_lines[line_no - 1];
                            let line_num = format!("{:>4} │ ", line_no);
                            if let Some(ref mut h) = hl {
                                lines.extend(wrap_diff_line_with_syntax(
                                    content,
                                    &line_num,
                                    ctx_style,
                                    width,
                                    h,
                                    self.ctx.color_theme.divider_fg,
                                ));
                            } else {
                                lines.extend(wrap_diff_line(
                                    content,
                                    &line_num,
                                    ctx_style,
                                    width,
                                    self.ctx.color_theme.divider_fg,
                                ));
                            }
                        }
                    }

                    let hidden = total.saturating_sub(visible_up + visible_down);
                    if hidden > 0 {
                        let unchanged_text = format!("─── {} lines unchanged ───", hidden);
                        let unchanged_width = unchanged_text.chars().count();

                        let up_label = "  ⩖  show more  ";
                        let up_label_len = up_label.chars().count();
                        let up_pad = unchanged_width.saturating_sub(up_label_len);
                        let up_left = up_pad / 2;
                        let up_idx = lines.len();
                        let col_start = up_left as u16;
                        let col_end = col_start + up_label_len as u16;
                        lines.push(Line::from(vec![
                            Span::styled(" ".repeat(up_left), Style::default()),
                            Span::styled(
                                up_label.to_string(),
                                Style::default()
                                    .fg(self.ctx.color_theme.status_info_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]));
                        self.expand_buttons.push(ButtonInfo {
                            line_idx: up_idx,
                            direction: ExpandDirection::Up,
                            col_start,
                            col_end,
                            gap_idx: gidx,
                            is_edge: false,
                        });
                        lines.push(Line::from(vec![Span::styled(
                            unchanged_text,
                            Style::default()
                                .fg(self.ctx.color_theme.fg)
                                .add_modifier(Modifier::ITALIC),
                        )]));
                        let down_label = "  ⩕  show more  ";
                        let down_label_len = down_label.chars().count();
                        let down_pad = unchanged_width.saturating_sub(down_label_len);
                        let down_left = down_pad / 2;
                        let down_idx = lines.len();
                        let col_start = down_left as u16;
                        let col_end = col_start + down_label_len as u16;
                        lines.push(Line::from(vec![
                            Span::styled(" ".repeat(down_left), Style::default()),
                            Span::styled(
                                down_label.to_string(),
                                Style::default()
                                    .fg(self.ctx.color_theme.status_info_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]));
                        self.expand_buttons.push(ButtonInfo {
                            line_idx: down_idx,
                            direction: ExpandDirection::Down,
                            col_start,
                            col_end,
                            gap_idx: gidx,
                            is_edge: false,
                        });
                    }

                    for i in (0..visible_down).rev() {
                        let line_no = (gap_end - i as u32 - 1) as usize;
                        if line_no > 0 && line_no <= new_file_lines.len() {
                            let content = &new_file_lines[line_no - 1];
                            let line_num = format!("{:>4} │ ", line_no);
                            if let Some(ref mut h) = hl {
                                lines.extend(wrap_diff_line_with_syntax(
                                    content,
                                    &line_num,
                                    ctx_style,
                                    width,
                                    h,
                                    self.ctx.color_theme.divider_fg,
                                ));
                            } else {
                                lines.extend(wrap_diff_line(
                                    content,
                                    &line_num,
                                    ctx_style,
                                    width,
                                    self.ctx.color_theme.divider_fg,
                                ));
                            }
                        }
                    }
                }
            }
            gidx + 1
        };

        // Gap before first hunk, single button at top
        if let Some(first_hunk) = entry.hunks.first() {
            if let Some(first_new) = hunk_first_new_line(first_hunk) {
                if first_new > 1 {
                    gap_idx = render_gap(
                        &mut lines,
                        1,
                        first_new,
                        gap_idx,
                        &mut highlighter,
                        Some(true),
                    );
                }
            }
        }

        let mut enhanced_hunk_spans: Vec<HunkRowSpan> = Vec::new();
        for (hunk_idx, hunk) in entry.hunks.iter().enumerate() {
            // Gap between hunks
            if hunk_idx > 0 {
                if let Some(prev_hunk) = entry.hunks.get(hunk_idx - 1) {
                    let prev_end = hunk_last_new_line(prev_hunk);
                    let curr_start = hunk_first_new_line(hunk);
                    if let (Some(pe), Some(cs)) = (prev_end, curr_start) {
                        if cs > pe + 1 {
                            gap_idx =
                                render_gap(&mut lines, pe + 1, cs, gap_idx, &mut highlighter, None);
                        }
                    }
                }
            }

            let hunk_row_start = lines.len();
            // Per-hunk opening banner, only shown when the hunk has a staged/unstaged origin.
            if let Some(banner) = hunk_banners_open.get(hunk_idx).and_then(|b| b.clone()) {
                lines.push(banner);
            }

            // Hunk content
            for diff_line in hunk.lines.iter().skip(1) {
                match diff_line.line_type {
                    DiffLineType::Addition => {
                        let line_num = format!("{:>4} │ ", diff_line.new_line_no.unwrap_or(0));
                        if let Some(ref mut h) = highlighter {
                            lines.extend(wrap_diff_line_with_syntax_and_bar(
                                &diff_line.content,
                                &line_num,
                                add_style,
                                width,
                                h,
                                Some(bar_add.clone()),
                                self.ctx.color_theme.divider_fg,
                                &diff_line.highlight_ranges,
                                add_strong_bg,
                            ));
                        } else {
                            lines.extend(wrap_diff_line_with_bar(
                                &diff_line.content,
                                &line_num,
                                add_style,
                                width,
                                Some(bar_add.clone()),
                                self.ctx.color_theme.divider_fg,
                                &diff_line.highlight_ranges,
                                add_strong_bg,
                            ));
                        }
                    }
                    DiffLineType::Deletion => {
                        let line_num = "     │ ".to_string();
                        if let Some(ref mut h) = highlighter {
                            lines.extend(wrap_diff_line_with_syntax_and_bar(
                                &diff_line.content,
                                &line_num,
                                del_style,
                                width,
                                h,
                                Some(bar_del.clone()),
                                self.ctx.color_theme.divider_fg,
                                &diff_line.highlight_ranges,
                                del_strong_bg,
                            ));
                        } else {
                            lines.extend(wrap_diff_line_with_bar(
                                &diff_line.content,
                                &line_num,
                                del_style,
                                width,
                                Some(bar_del.clone()),
                                self.ctx.color_theme.divider_fg,
                                &diff_line.highlight_ranges,
                                del_strong_bg,
                            ));
                        }
                    }
                    DiffLineType::Context => {
                        let line_num = format!("{:>4} │ ", diff_line.new_line_no.unwrap_or(0));
                        if let Some(ref mut h) = highlighter {
                            lines.extend(wrap_diff_line_with_syntax(
                                &diff_line.content,
                                &line_num,
                                ctx_style,
                                width,
                                h,
                                self.ctx.color_theme.divider_fg,
                            ));
                        } else {
                            lines.extend(wrap_diff_line(
                                &diff_line.content,
                                &line_num,
                                ctx_style,
                                width,
                                self.ctx.color_theme.divider_fg,
                            ));
                        }
                    }
                    DiffLineType::BinaryNote => {
                        for chunk in wrap_text(&diff_line.content, width as usize) {
                            lines.push(Line::from(vec![Span::styled(
                                chunk.to_string(),
                                Style::default()
                                    .fg(self.ctx.color_theme.fg)
                                    .add_modifier(Modifier::DIM),
                            )]));
                        }
                    }
                    _ => {}
                }
            }
            // Per-hunk closing banner.
            if let Some(banner) = hunk_banners_close.get(hunk_idx).and_then(|b| b.clone()) {
                lines.push(banner);
            }
            // Track row span for click/hover detection on staged/unstaged hunks.
            if matches!(hunk.origin, HunkOrigin::Staged | HunkOrigin::Unstaged)
                && lines.len() > hunk_row_start
            {
                enhanced_hunk_spans.push(HunkRowSpan {
                    hunk_idx,
                    entry_idx: 0,
                    origin: hunk.origin,
                    start: hunk_row_start,
                    end: lines.len(),
                });
            }
            if hunk_idx + 1 < entry.hunks.len() {
                let next_gap_has_hidden_lines = entry.hunks.get(hunk_idx + 1).is_some_and(|next| {
                    hunk_last_new_line(hunk)
                        .zip(hunk_first_new_line(next))
                        .is_some_and(|(pe, cs)| {
                            cs > pe + 1
                                && self.gap_states.get(gap_idx).is_some_and(|gap| {
                                    gap_has_hidden_lines(
                                        (cs - pe - 1) as usize,
                                        gap.visible_up,
                                        gap.visible_down,
                                    )
                                })
                        })
                });
                if next_gap_has_hidden_lines {
                    lines.push(Line::from(""));
                }
            }
        }

        // Gap after last hunk, single button at bottom
        if let Some(last_hunk) = entry.hunks.last() {
            if let Some(last_new) = hunk_last_new_line(last_hunk) {
                let file_total = self.new_file_lines.len() as u32;
                if last_new < file_total {
                    render_gap(
                        &mut lines,
                        last_new + 1,
                        file_total + 1,
                        gap_idx,
                        &mut highlighter,
                        Some(false),
                    );
                }
            }
        }

        self.hunk_spans = enhanced_hunk_spans;
        lines
    }

    pub fn select_older_commit(
        &mut self,
        repo_path: &std::path::Path,
        hash: &str,
    ) -> Result<(), String> {
        self.update_selected_commit(repo_path, hash, |state| state.select_next())
    }

    pub fn select_newer_commit(
        &mut self,
        repo_path: &std::path::Path,
        hash: &str,
    ) -> Result<(), String> {
        self.update_selected_commit(repo_path, hash, |state| state.select_prev())
    }

    pub fn select_parent_commit(
        &mut self,
        repo_path: &std::path::Path,
        hash: &str,
    ) -> Result<(), String> {
        self.update_selected_commit(repo_path, hash, |state| state.select_parent())
    }

    fn update_selected_commit<F>(
        &mut self,
        repo_path: &std::path::Path,
        _current_hash: &str,
        update_fn: F,
    ) -> Result<(), String>
    where
        F: FnOnce(&mut CommitListState<'a>),
    {
        let state = match self.commit_list_state.as_mut() {
            Some(s) => s,
            None => return Ok(()),
        };
        update_fn(state);
        let hash = state.selected_commit_hash();
        let new_entries = DiffEntry::load_for_commit(repo_path, hash.as_str())?;
        self.diff_entries = new_entries;
        self.scroll_offset = 0;
        Ok(())
    }

    fn scroll_down(&mut self) {
        let max = self.content_height.saturating_sub(1);
        if self.scroll_offset < max {
            self.scroll_offset += 1;
        }
    }

    fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    fn scroll_page_down(&mut self) {
        let page = 20;
        let max = self.content_height.saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + page).min(max);
    }

    fn scroll_page_up(&mut self) {
        let page = 20;
        self.scroll_offset = self.scroll_offset.saturating_sub(page);
    }

    fn scroll_half_page_down(&mut self) {
        let half = 10;
        let max = self.content_height.saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + half).min(max);
    }

    fn scroll_half_page_up(&mut self) {
        let half = 10;
        self.scroll_offset = self.scroll_offset.saturating_sub(half);
    }

    fn scroll_to_bottom(&mut self) {
        let max = self.content_height.saturating_sub(1);
        self.scroll_offset = max;
    }

    fn copy_file_path(&self) {
        if let Some(entry) = self.diff_entries.first() {
            let path = entry
                .new_path
                .as_deref()
                .or(entry.old_path.as_deref())
                .unwrap_or("");
            self.copy_to_clipboard("File path".into(), path.into());
        }
    }

    fn copy_commit_hash(&self) {
        if let Some(state) = self.commit_list_state.as_ref() {
            let hash = state.selected_commit_hash();
            self.copy_to_clipboard("Commit SHA".into(), hash.as_str().into());
        }
    }

    fn copy_to_clipboard(&self, name: String, value: String) {
        self.tx.send(AppEvent::CopyToClipboard { name, value });
    }

    fn cycle_file(&self, delta: isize) {
        if self.all_file_paths.is_empty() {
            return;
        }
        let is_staged = self.title.contains("(staged)");
        let current = self
            .title
            .strip_prefix("Diff (staged): ")
            .or_else(|| self.title.strip_prefix("Diff (unstaged): "))
            .or_else(|| self.title.strip_prefix("Diff: "))
            .unwrap_or("");

        let position = if self.commit_hash.is_empty() {
            self.all_file_paths
                .iter()
                .position(|(p, s)| p == current && *s == is_staged)
        } else {
            self.all_file_paths.iter().position(|(p, _)| p == current)
        };

        if let Some(idx) = position {
            let new_idx = (idx as isize + delta) as usize;
            if new_idx >= self.all_file_paths.len() {
                return;
            }
            let (new_path, new_staged) = self.all_file_paths[new_idx].clone();
            if self.commit_hash.is_empty() {
                self.tx.send(AppEvent::OpenUncommittedDiff {
                    file_path: new_path,
                    is_staged: new_staged,
                });
            } else {
                self.tx.send(AppEvent::OpenFileDiff {
                    hash: self.commit_hash.clone(),
                    file_path: new_path,
                });
            }
        }
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        std::rc::Rc::make_mut(&mut self.ctx).color_theme = theme;
    }

    pub fn refresh(&self) {
        let file_path = self
            .title
            .strip_prefix("Diff (staged): ")
            .or_else(|| self.title.strip_prefix("Diff (unstaged): "))
            .or_else(|| self.title.strip_prefix("Diff: "))
            .unwrap_or("");
        if self.commit_hash.is_empty() {
            let is_staged = self.title.contains("(staged)");
            self.tx.send(AppEvent::OpenUncommittedDiff {
                file_path: file_path.into(),
                is_staged,
            });
        } else {
            self.tx.send(AppEvent::OpenFileDiff {
                hash: self.commit_hash.clone(),
                file_path: file_path.into(),
            });
        }
    }

    pub fn footer_hint(&self) -> String {
        let mut parts = Vec::new();
        if !self.expand_buttons.is_empty() {
            parts.push("⇆:buttons");
            parts.push("Enter:expand");
        }
        if !self.all_file_paths.is_empty() {
            let is_staged = self.title.contains("(staged)");
            let current = self
                .title
                .strip_prefix("Diff (staged): ")
                .or_else(|| self.title.strip_prefix("Diff (unstaged): "))
                .or_else(|| self.title.strip_prefix("Diff: "))
                .unwrap_or("");

            let position = if self.commit_hash.is_empty() {
                self.all_file_paths
                    .iter()
                    .position(|(p, s)| p == current && *s == is_staged)
            } else {
                self.all_file_paths.iter().position(|(p, _)| p == current)
            };

            if let Some(idx) = position {
                if idx > 0 {
                    parts.push("-:prev-file");
                }
                if idx + 1 < self.all_file_paths.len() {
                    parts.push("+:next-file");
                }
            }
        }
        parts.push("f:search");
        parts.push("b:blame");
        parts.push("H:history");
        parts.push("c:copy-path");
        parts.push("r:fetch");
        format!("⌘ {}", parts.join("▕▏"))
    }

    fn build_button_line(&self, btn: &ButtonInfo, is_hovered: bool, _width: u16) -> Line<'static> {
        let style = if is_hovered {
            Style::default()
                .fg(self.ctx.color_theme.status_info_fg)
                .bg(self.ctx.color_theme.list_selected_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(self.ctx.color_theme.status_info_fg)
                .add_modifier(Modifier::BOLD)
        };
        let label = if btn.is_edge {
            // Edge buttons: icon is the opposite of direction
            match btn.direction {
                ExpandDirection::Up => "  ⩕  show more  ",
                ExpandDirection::Down => "  ⩖  show more  ",
            }
        } else {
            // Middle gap buttons: icon matches direction
            match btn.direction {
                ExpandDirection::Up => "  ⩖  show more  ",
                ExpandDirection::Down => "  ⩕  show more  ",
            }
        };
        let _label_len = label.chars().count();
        let pad = btn.col_start as usize;
        Line::from(vec![
            Span::styled(" ".repeat(pad), Style::default()),
            Span::styled(label.to_string(), style),
        ])
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        let prev_button = self.focused_button;
        let prev_hunk = self.hovered_hunk;

        if let Some(area) = self.diff_content_area {
            let in_area = col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height;
            if in_area {
                let local_row = self.scroll_offset + (row - area.y) as usize;
                let mut found = None;
                for (idx, btn) in self.expand_buttons.iter().enumerate() {
                    if btn.line_idx == local_row {
                        let local_col = col.saturating_sub(area.x);
                        if local_col >= btn.col_start && local_col < btn.col_end {
                            found = Some(idx);
                            break;
                        }
                    }
                }
                // Only update focused_button if we're over a button; leaving the button
                // area clears the selection so keyboard and mouse stay in sync.
                if found.is_some() || prev_button.is_some() {
                    self.focused_button = found;
                }

                // Hunk hover detection, find which span (if any) covers this row.
                let hovered = self
                    .hunk_spans
                    .iter()
                    .position(|s| local_row >= s.start && local_row < s.end);
                if hovered != prev_hunk {
                    self.hovered_hunk = hovered;
                    // Rebuild lines so the banner styling reflects hover.
                    self.needs_rebuild = true;
                }
                return;
            }
        }

        // Mouse left the content area entirely, clear selection
        if prev_button.is_some() {
            self.focused_button = None;
        }
        if prev_hunk.is_some() {
            self.hovered_hunk = None;
            self.needs_rebuild = true;
        }
    }

    fn activate_button(&mut self, idx: usize) {
        if let Some(btn) = self.expand_buttons.get(idx).cloned() {
            if let Some(gap_state) = self.gap_states.get_mut(btn.gap_idx) {
                match btn.direction {
                    ExpandDirection::Up => {
                        let max_expand = gap_state.total.saturating_sub(gap_state.visible_down);
                        let increment = max_expand.min(15.max(gap_state.total / 3).min(100));
                        gap_state.visible_up = (gap_state.visible_up + increment).min(max_expand);
                    }
                    ExpandDirection::Down => {
                        let max_expand = gap_state.total.saturating_sub(gap_state.visible_up);
                        let increment = max_expand.min(15.max(gap_state.total / 3).min(100));
                        gap_state.visible_down =
                            (gap_state.visible_down + increment).min(max_expand);
                    }
                }
                self.needs_rebuild = true;
            }
        }
    }

    fn scroll_to_button(&mut self, idx: usize) {
        if let Some(btn) = self.expand_buttons.get(idx) {
            let line = btn.line_idx;
            let viewport = self
                .diff_content_area
                .map(|a| a.height as usize)
                .unwrap_or(0);
            if line < self.scroll_offset {
                self.scroll_offset = line;
            } else if viewport > 0 && line >= self.scroll_offset + viewport {
                self.scroll_offset = line.saturating_sub(viewport - 1);
            }
        }
    }

    fn scroll_to_hunk(&mut self, idx: usize) {
        // When the user cycles through hunks with arrow keys we always pin
        // the hunk's opening banner to the very top of the viewport, never
        // leave the banner at the bottom or in the middle, since the user
        // wants the staged/unstaged label as the visual anchor.
        if let Some(span) = self.hunk_spans.get(idx) {
            let viewport = self
                .diff_content_area
                .map(|a| a.height as usize)
                .unwrap_or(0);
            // Clamp so we don't scroll past the last screenful of content.
            let max_scroll = self.content_height.saturating_sub(viewport);
            self.scroll_offset = span.start.min(max_scroll);
        }
    }

    /// Returns all focusable elements (expand buttons + stage-able hunks)
    /// sorted by their position in the rendered output. Used by arrow-key
    /// navigation to walk through both kinds in a single coherent sequence.
    fn focusable_order(&self) -> Vec<FocusKind> {
        let mut entries: Vec<(usize, FocusKind)> = Vec::new();
        for (i, btn) in self.expand_buttons.iter().enumerate() {
            entries.push((btn.line_idx, FocusKind::Button(i)));
        }
        for (i, span) in self.hunk_spans.iter().enumerate() {
            entries.push((span.start, FocusKind::Hunk(i)));
        }
        entries.sort_by_key(|(row, _)| *row);
        entries.into_iter().map(|(_, k)| k).collect()
    }

    fn current_focus_kind(&self) -> Option<FocusKind> {
        if let Some(i) = self.focused_hunk {
            Some(FocusKind::Hunk(i))
        } else {
            self.focused_button.map(FocusKind::Button)
        }
    }

    fn set_focus(&mut self, k: Option<FocusKind>) {
        let prev_hunk = self.focused_hunk;
        match k {
            Some(FocusKind::Button(i)) => {
                self.focused_button = Some(i);
                self.focused_hunk = None;
            }
            Some(FocusKind::Hunk(i)) => {
                self.focused_button = None;
                self.focused_hunk = Some(i);
            }
            None => {
                self.focused_button = None;
                self.focused_hunk = None;
            }
        }
        // Rebuild only when the hunk focus changed, so the banner emphasis
        // refreshes. Button focus is applied at render time without a rebuild.
        if prev_hunk != self.focused_hunk {
            self.needs_rebuild = true;
        }
    }

    fn move_focus(&mut self, forward: bool) {
        let order = self.focusable_order();
        if order.is_empty() {
            return;
        }
        let cur_pos = self
            .current_focus_kind()
            .and_then(|c| order.iter().position(|k| *k == c));
        let next_pos = match (cur_pos, forward) {
            (Some(i), true) => (i + 1).min(order.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
            (None, true) => 0,
            (None, false) => order.len() - 1,
        };
        let next = order[next_pos];
        self.set_focus(Some(next));
        match next {
            FocusKind::Button(i) => self.scroll_to_button(i),
            FocusKind::Hunk(i) => self.scroll_to_hunk(i),
        }
    }

    fn activate_focused_hunk(&mut self, idx: usize) {
        let (hunk_idx, currently_staged) = match self.hunk_spans.get(idx) {
            Some(span) => (span.hunk_idx, matches!(span.origin, HunkOrigin::Staged)),
            None => return,
        };
        let file_path = match &self.uncommitted_file_path {
            Some(p) => p.clone(),
            None => return,
        };
        self.tx.send(AppEvent::ToggleHunkStage {
            file_path,
            hunk_idx,
            currently_staged,
        });
    }

    fn update_search_matches(&mut self) {
        let query = self.search_query.to_lowercase();
        self.search_matches.clear();
        if query.is_empty() || self.base_lines.is_empty() {
            return;
        }
        let qlen = query.len();
        for (i, line) in self.base_lines.iter().enumerate() {
            let text: String = line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            let lower = text.to_lowercase();
            let mut pos = 0;
            while pos <= lower.len() {
                match lower[pos..].find(&query) {
                    Some(rel) => {
                        let start = pos + rel;
                        let end = start + qlen;
                        self.search_matches.push(SearchMatch {
                            line_idx: i,
                            start,
                            end,
                        });
                        if end == start {
                            // Empty query guard (shouldn't happen due to qlen check)
                            break;
                        }
                        pos = end;
                    }
                    None => break,
                }
            }
        }
    }

    fn scroll_to_match(&mut self, match_idx: usize) {
        if let Some(m) = self.search_matches.get(match_idx) {
            let half_height = (self.content_height.min(20)) / 2;
            self.scroll_offset = m.line_idx.saturating_sub(half_height);
        }
    }

    /// Exposed to `View::is_input_active()` so the app routes Backspace/Delete
    /// (which have no UserEvent mapping) into our handle_event.
    pub fn is_search_input_active(&self) -> bool {
        self.search_active
    }

    /// Push the current search state into the global StatusLine::Input bar at
    /// the bottom of the app, mirroring the commit list's "Search: <query>"
    /// pattern. The match counter is shown as a right-aligned transient.
    fn update_search_status_bar(&self) {
        const PREFIX: &str = "Search: ";
        let prefix_chars = PREFIX.chars().count();
        let cursor_chars = self.search_query[..self.search_cursor].chars().count();
        let msg = format!("{}{}", PREFIX, self.search_query);
        let transient = if self.search_query.is_empty() {
            None
        } else if self.search_matches.is_empty() {
            Some("[no matches]".to_string())
        } else {
            Some(format!(
                "[{}/{}]",
                self.search_current + 1,
                self.search_matches.len()
            ))
        };
        self.tx.send(AppEvent::UpdateStatusInput(
            msg,
            Some((prefix_chars + cursor_chars) as u16),
            transient,
        ));
    }

    fn clear_search_status_bar(&self) {
        self.tx.send(AppEvent::ClearStatusLine);
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(area) = self.diff_content_area {
            let in_area = col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height;
            if in_area {
                let local_row = self.scroll_offset + (row - area.y) as usize;
                let local_col = col.saturating_sub(area.x);
                for btn in self.expand_buttons.iter() {
                    if btn.line_idx == local_row
                        && local_col >= btn.col_start
                        && local_col < btn.col_end
                    {
                        if let Some(gap_state) = self.gap_states.get_mut(btn.gap_idx) {
                            match btn.direction {
                                ExpandDirection::Up => {
                                    let max_expand =
                                        gap_state.total.saturating_sub(gap_state.visible_down);
                                    let increment =
                                        max_expand.min(15.max(gap_state.total / 3).min(100));
                                    gap_state.visible_up =
                                        (gap_state.visible_up + increment).min(max_expand);
                                }
                                ExpandDirection::Down => {
                                    let max_expand =
                                        gap_state.total.saturating_sub(gap_state.visible_up);
                                    let increment =
                                        max_expand.min(15.max(gap_state.total / 3).min(100));
                                    gap_state.visible_down =
                                        (gap_state.visible_down + increment).min(max_expand);
                                }
                            }
                            self.needs_rebuild = true;
                        }
                        return;
                    }
                }

                // Hunk-level staging: clicking anywhere on a hunk in the
                // Uncommitted view toggles its stage state. Only fires when
                // the title-derived file path is present, ie. this is the
                // combined uncommitted diff and not a committed diff.
                if let Some(file_path) = self.uncommitted_file_path.clone() {
                    for span in &self.hunk_spans {
                        if local_row >= span.start && local_row < span.end {
                            self.tx.send(AppEvent::ToggleHunkStage {
                                file_path,
                                hunk_idx: span.hunk_idx,
                                currently_staged: matches!(span.origin, HunkOrigin::Staged),
                            });
                            return;
                        }
                    }
                }
            }
        }
    }
}

fn wrap_text(text: &str, max_width: usize) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.chars().count() <= max_width {
            lines.push(remaining);
            break;
        }
        let (head, tail) = split_at_width(remaining, max_width);
        lines.push(head);
        remaining = tail;
    }

    lines
}

/// Truncate `text` to `max_width` cells, replacing the trailing portion with
/// `…` if it overflows. The result is exactly `max_width` cells wide,
/// right-padded with spaces if `text` is shorter, keeping the column
/// boundaries fixed in the side-by-side diff renderer.
fn truncate_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= max_width {
        // Pad with spaces to reach exactly max_width cells.
        let pad = max_width - count;
        let mut out = String::with_capacity(text.len() + pad);
        out.push_str(text);
        for _ in 0..pad {
            out.push(' ');
        }
        out
    } else {
        // Take max_width-1 chars + ellipsis. (max_width >= 1 here since we
        // returned early on 0 above.)
        let mut out: String = text.chars().take(max_width - 1).collect();
        out.push('…');
        out
    }
}

/// Pad an empty (or short) string to exactly `width` cells of spaces, used
/// for the empty side of a paired side-by-side row.
fn pad_to_width(text: &str, width: usize) -> String {
    truncate_to_width(text, width)
}

fn gap_has_hidden_lines(total: usize, visible_up: usize, visible_down: usize) -> bool {
    visible_up.saturating_add(visible_down) < total
}

/// Split `s` at the nearest whitespace boundary before `max_chars` characters,
/// returning `(head, tail)`.  Falls back to a hard break at `max_chars` if no
/// whitespace is found.  Avoids allocating a Vec of char indices.
fn split_at_width(s: &str, max_chars: usize) -> (&str, &str) {
    // Find the byte offset of the char AFTER `max_chars` chars
    let hard_byte = s
        .char_indices()
        .nth(max_chars)
        .map(|(b, _)| b)
        .unwrap_or(s.len());

    let candidate = &s[..hard_byte];

    // Search backwards for whitespace in the candidate region
    if let Some(ws_byte) = candidate.rfind(|c: char| c.is_ascii_whitespace()) {
        let head = candidate[..ws_byte].trim_end();
        let tail = s[ws_byte + 1..].trim_start();
        (head, tail)
    } else {
        // No whitespace, hard break
        (&s[..hard_byte], &s[hard_byte..])
    }
}

fn wrap_diff_line_with_syntax(
    content: &str,
    line_num_str: &str,
    base_style: Style,
    available_width: u16,
    highlighter: &mut SyntaxHighlighter,
    divider_fg: Color,
) -> Vec<Line<'static>> {
    let line_num_width = 7; // "1234 │ " = 7 chars
    let content_width = available_width.saturating_sub(line_num_width) as usize;

    let mut lines = Vec::new();
    if content_width == 0 {
        let spans = highlighter.highlight_line(content, base_style, None);
        let mut all_spans = vec![Span::styled(
            line_num_str.to_string(),
            Style::default().fg(divider_fg),
        )];
        all_spans.extend(spans);
        lines.push(Line::from(all_spans));
        return lines;
    }

    let mut remaining = content;
    let mut first = true;

    if remaining.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            line_num_str.to_string(),
            Style::default().fg(divider_fg),
        )]));
        return lines;
    }

    while !remaining.is_empty() {
        let (chunk, rest) = if remaining.chars().count() > content_width {
            split_at_width(remaining, content_width)
        } else {
            (remaining, "")
        };

        let num_span = if first {
            Span::styled(line_num_str.to_string(), Style::default().fg(divider_fg))
        } else {
            Span::styled("     │ ".to_string(), Style::default().fg(divider_fg))
        };

        let mut spans = vec![num_span];
        spans.extend(highlighter.highlight_line(chunk, base_style, None));
        lines.push(Line::from(spans));

        remaining = rest;
        first = false;
    }

    lines
}

fn wrap_diff_line(
    content: &str,
    line_num_str: &str,
    content_style: Style,
    available_width: u16,
    divider_fg: Color,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let line_num_width = 7; // "1234 │ " = 7 chars
    let content_width = available_width.saturating_sub(line_num_width) as usize;

    if content_width == 0 {
        lines.push(Line::from(vec![
            Span::styled(line_num_str.to_string(), Style::default().fg(divider_fg)),
            Span::styled(content.to_string(), content_style),
        ]));
        return lines;
    }

    let mut remaining = content;
    let mut first = true;

    // Blank lines (empty content) must still be rendered so line numbers stay correct
    if remaining.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            line_num_str.to_string(),
            Style::default().fg(divider_fg),
        )]));
        return lines;
    }

    while !remaining.is_empty() {
        let (chunk, rest) = if remaining.chars().count() > content_width {
            split_at_width(remaining, content_width)
        } else {
            (remaining, "")
        };

        let num_span = if first {
            Span::styled(line_num_str.to_string(), Style::default().fg(divider_fg))
        } else {
            Span::styled("     │ ".to_string(), Style::default().fg(divider_fg))
        };

        lines.push(Line::from(vec![
            num_span,
            Span::styled(chunk.to_string(), content_style),
        ]));

        remaining = rest;
        first = false;
    }

    lines
}

fn wrap_diff_line_with_bar(
    content: &str,
    line_num_str: &str,
    content_style: Style,
    available_width: u16,
    bar_span: Option<Span<'static>>,
    divider_fg: Color,
    highlight_ranges: &[std::ops::Range<usize>],
    strong_bg: Color,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let line_num_width = 7; // "1234 │ " = 7 chars
    let bar_width = if bar_span.is_some() { 1 } else { 0 };
    let content_width = available_width.saturating_sub(line_num_width + bar_width) as usize;

    if content_width == 0 {
        let mut spans = vec![Span::styled(
            line_num_str.to_string(),
            Style::default().fg(divider_fg),
        )];
        if let Some(bar) = bar_span {
            spans.push(bar);
        }
        spans.extend(apply_word_diff_to_plain(
            content,
            0,
            highlight_ranges,
            content_style,
            strong_bg,
        ));
        spans.push(Span::styled(" ".repeat(200), content_style));
        lines.push(Line::from(spans));
        return lines;
    }

    let mut remaining = content;
    let mut first = true;

    if remaining.is_empty() {
        let mut spans = vec![Span::styled(
            line_num_str.to_string(),
            Style::default().fg(divider_fg),
        )];
        if let Some(bar) = bar_span {
            spans.push(bar);
        }
        spans.push(Span::styled(" ".repeat(200), content_style));
        lines.push(Line::from(spans));
        return lines;
    }

    while !remaining.is_empty() {
        let (chunk, rest) = if remaining.chars().count() > content_width {
            split_at_width(remaining, content_width)
        } else {
            (remaining, "")
        };

        let chunk_start = chunk.as_ptr() as usize - content.as_ptr() as usize;

        let num_span = if first {
            Span::styled(line_num_str.to_string(), Style::default().fg(divider_fg))
        } else {
            Span::styled("     │ ".to_string(), Style::default().fg(divider_fg))
        };

        let mut spans = vec![num_span];
        if let Some(ref bar) = bar_span {
            spans.push(bar.clone());
        }
        spans.extend(apply_word_diff_to_plain(
            chunk,
            chunk_start,
            highlight_ranges,
            content_style,
            strong_bg,
        ));
        spans.push(Span::styled(" ".repeat(200), content_style));
        lines.push(Line::from(spans));

        remaining = rest;
        first = false;
    }

    lines
}

/// Post-process syntax-highlighted spans to overlay strong_bg on word-diff ranges.
/// `chunk_start` is the byte offset of the chunk's first character within the original content.
/// Each syntect span covers a contiguous substring; we track position by accumulating lengths.
fn apply_word_diff_to_spans(
    spans: Vec<Span<'static>>,
    chunk_start: usize,
    highlight_ranges: &[std::ops::Range<usize>],
    strong_bg: Color,
) -> Vec<Span<'static>> {
    if highlight_ranges.is_empty() {
        return spans;
    }

    let mut result = Vec::new();
    let mut abs_pos = chunk_start; // byte position within original content

    for span in spans {
        let text = span.content.as_ref();
        let span_abs_start = abs_pos;
        let span_abs_end = abs_pos + text.len();
        abs_pos = span_abs_end;

        let mut inner = 0usize; // relative to span text
        let mut had_overlap = false;

        for range in highlight_ranges {
            let overlap_start = range.start.max(span_abs_start);
            let overlap_end = range.end.min(span_abs_end);
            if overlap_start >= overlap_end {
                continue;
            }
            had_overlap = true;
            let rel_start = overlap_start - span_abs_start;
            let rel_end = overlap_end - span_abs_start;

            if inner < rel_start {
                result.push(Span::styled(text[inner..rel_start].to_string(), span.style));
            }
            result.push(Span::styled(
                text[rel_start..rel_end].to_string(),
                span.style.bg(strong_bg),
            ));
            inner = rel_end;
        }

        if !had_overlap {
            result.push(span);
        } else if inner < text.len() {
            result.push(Span::styled(text[inner..].to_string(), span.style));
        }
    }

    result
}

/// Build word-diff spans for plain (non-syntax) content lines.
/// Segments within `highlight_ranges` get `strong_bg`; the rest keeps `base_style`.
fn apply_word_diff_to_plain(
    chunk: &str,
    chunk_start: usize,
    highlight_ranges: &[std::ops::Range<usize>],
    base_style: Style,
    strong_bg: Color,
) -> Vec<Span<'static>> {
    if highlight_ranges.is_empty() {
        return vec![Span::styled(chunk.to_string(), base_style)];
    }

    let mut spans = Vec::new();
    let mut pos = 0usize; // relative to chunk

    for range in highlight_ranges {
        let abs_start = range.start;
        let abs_end = range.end;

        let rel_start = abs_start.saturating_sub(chunk_start);
        let rel_end = abs_end.saturating_sub(chunk_start).min(chunk.len());

        if rel_start >= chunk.len() || rel_start >= rel_end {
            continue;
        }

        if pos < rel_start {
            spans.push(Span::styled(chunk[pos..rel_start].to_string(), base_style));
        }
        let strong_style = Style::default().bg(strong_bg);
        spans.push(Span::styled(
            chunk[rel_start..rel_end].to_string(),
            strong_style,
        ));
        pos = rel_end;
    }

    if pos < chunk.len() {
        spans.push(Span::styled(chunk[pos..].to_string(), base_style));
    }

    if spans.is_empty() {
        spans.push(Span::styled(chunk.to_string(), base_style));
    }

    spans
}

/// Apply per-occurrence search highlighting. `matches_on_line` lists every
/// occurrence in the line (byte offsets in the flattened text, same flattening
/// as `update_search_matches`). `current_start_in_line` identifies the active
/// occurrence by its start offset; if it matches, that occurrence gets an extra
/// UNDERLINED modifier so the user can tell which is the active jump target.
fn highlight_search_matches(
    line: &Line<'static>,
    matches_on_line: &[SearchMatch],
    current_start_in_line: Option<usize>,
    match_bg: Color,
    match_fg: Color,
) -> Line<'static> {
    let mut new_spans: Vec<Span<'static>> = Vec::new();
    let mut abs_pos: usize = 0;

    for span in &line.spans {
        let text = span.content.as_ref();
        let span_start = abs_pos;
        let span_end = abs_pos + text.len();
        abs_pos = span_end;

        let mut inner = 0usize;
        let mut had_overlap = false;

        for m in matches_on_line {
            // Compute overlap of this match with this span (in absolute coords).
            let ov_start = m.start.max(span_start);
            let ov_end = m.end.min(span_end);
            if ov_start >= ov_end {
                continue;
            }
            had_overlap = true;
            // Convert to span-relative byte offsets.
            let rel_start = ov_start - span_start;
            let rel_end = ov_end - span_start;

            if inner < rel_start && rel_start <= text.len() {
                new_spans.push(Span::styled(text[inner..rel_start].to_string(), span.style));
            }

            let mut modifier = Modifier::BOLD;
            if current_start_in_line == Some(m.start) {
                modifier |= Modifier::UNDERLINED;
            }

            if rel_end <= text.len() {
                new_spans.push(Span::styled(
                    text[rel_start..rel_end].to_string(),
                    span.style.fg(match_fg).bg(match_bg).add_modifier(modifier),
                ));
            }
            inner = rel_end;
        }

        if !had_overlap {
            new_spans.push(span.clone());
        } else if inner < text.len() {
            new_spans.push(Span::styled(text[inner..].to_string(), span.style));
        }
    }

    Line::from(new_spans)
}

fn wrap_diff_line_with_syntax_and_bar(
    content: &str,
    line_num_str: &str,
    base_style: Style,
    available_width: u16,
    highlighter: &mut SyntaxHighlighter,
    bar_span: Option<Span<'static>>,
    divider_fg: Color,
    highlight_ranges: &[std::ops::Range<usize>],
    strong_bg: Color,
) -> Vec<Line<'static>> {
    let line_num_width = 7; // "1234 │ " = 7 chars
    let bar_width = if bar_span.is_some() { 1 } else { 0 };
    let content_width = available_width.saturating_sub(line_num_width + bar_width) as usize;

    let mut lines = Vec::new();
    if content_width == 0 {
        let mut spans = vec![Span::styled(
            line_num_str.to_string(),
            Style::default().fg(divider_fg),
        )];
        if let Some(bar) = bar_span {
            spans.push(bar);
        }
        let syntax_spans = highlighter.highlight_line(content, base_style, None);
        spans.extend(apply_word_diff_to_spans(
            syntax_spans,
            0,
            highlight_ranges,
            strong_bg,
        ));
        spans.push(Span::styled(" ".repeat(200), base_style));
        lines.push(Line::from(spans));
        return lines;
    }

    let mut remaining = content;
    let mut first = true;

    if remaining.is_empty() {
        let mut spans = vec![Span::styled(
            line_num_str.to_string(),
            Style::default().fg(divider_fg),
        )];
        if let Some(bar) = bar_span {
            spans.push(bar);
        }
        spans.push(Span::styled(" ".repeat(200), base_style));
        lines.push(Line::from(spans));
        return lines;
    }

    while !remaining.is_empty() {
        let (chunk, rest) = if remaining.chars().count() > content_width {
            split_at_width(remaining, content_width)
        } else {
            (remaining, "")
        };

        let chunk_start = chunk.as_ptr() as usize - content.as_ptr() as usize;

        let num_span = if first {
            Span::styled(line_num_str.to_string(), Style::default().fg(divider_fg))
        } else {
            Span::styled("     │ ".to_string(), Style::default().fg(divider_fg))
        };

        let mut spans = vec![num_span];
        if let Some(ref bar) = bar_span {
            spans.push(bar.clone());
        }
        let syntax_spans = highlighter.highlight_line(chunk, base_style, None);
        spans.extend(apply_word_diff_to_spans(
            syntax_spans,
            chunk_start,
            highlight_ranges,
            strong_bg,
        ));
        spans.push(Span::styled(" ".repeat(200), base_style));
        lines.push(Line::from(spans));

        remaining = rest;
        first = false;
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fully_expanded_gaps_are_not_hidden() {
        assert!(!gap_has_hidden_lines(10, 5, 5));
        assert!(!gap_has_hidden_lines(10, 12, 0));
        assert!(gap_has_hidden_lines(10, 4, 5));
    }
}
