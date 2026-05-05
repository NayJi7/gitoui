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
    git::diff::{DiffEntry, DiffLineType},
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
    hovered_button: Option<usize>,
    focused_button: Option<usize>,

    // Full file contents for directional gap expansion
    old_file_lines: Vec<String>,
    new_file_lines: Vec<String>,
    // Per-gap state: (visible_up, visible_down, total_gap_size)
    gap_states: Vec<GapState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpandDirection {
    Up,
    Down,
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

impl<'a> DiffView<'a> {
    pub fn all_file_paths(&self) -> &Vec<(String, bool)> {
        &self.all_file_paths
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
        let (old_lines, new_lines) = Self::load_file_versions(&repo_path, &commit_hash, file_path, title.contains("(staged)"));

        let file_line_count = new_lines.len() as u32;
        let gap_states = Self::compute_initial_gap_states(&diff_entries, file_line_count);

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
            hovered_button: None,
            focused_button: None,
            old_file_lines: old_lines,
            new_file_lines: new_lines,
            gap_states,
        }
    }

    fn load_file_versions(repo_path: &std::path::Path, commit_hash: &str, file_path: &str, is_staged: bool) -> (Vec<String>, Vec<String>) {
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
            let hash = if commit_hash.is_empty() { "HEAD".to_string() } else { commit_hash.to_string() };
            match Command::new("git")
                .args(["show", &format!("{}:{}", hash, file_path)])
                .current_dir(repo_path)
                .output()
            {
                Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).lines().map(|s| s.to_string()).collect(),
                _ => Vec::new(),
            }
        };

        let old_lines = match old_cmd {
            Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout).lines().map(|s| s.to_string()).collect(),
            _ => Vec::new(),
        };

        (old_lines, new_lines)
    }

    fn compute_initial_gap_states(diff_entries: &[DiffEntry], file_line_count: u32) -> Vec<GapState> {
        let mut states = Vec::new();
        if let Some(entry) = diff_entries.first() {
            // Gap before first hunk — no lines shown by default, single button
            if let Some(first_new) = entry.hunks.first().and_then(|h| {
                h.lines.iter().find_map(|l| if l.line_type == DiffLineType::Context { l.new_line_no } else { None })
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

            // Gaps between hunks — show some context by default
            for hunk_idx in 1..entry.hunks.len() {
                if let (Some(prev), Some(curr)) = (entry.hunks.get(hunk_idx - 1), entry.hunks.get(hunk_idx)) {
                    let prev_end = prev.lines.iter().rev().find_map(|l| {
                        if l.line_type == DiffLineType::Context { l.new_line_no } else { None }
                    });
                    let curr_start = curr.lines.iter().find_map(|l| {
                        if l.line_type == DiffLineType::Context { l.new_line_no } else { None }
                    });
                    if let (Some(pe), Some(cs)) = (prev_end, curr_start) {
                        if cs > pe + 1 {
                            let gap = (cs - pe - 1) as usize;
                            let default_visible = 3.min(gap / 2).max(0);
                            states.push(GapState {
                                visible_up: default_visible,
                                visible_down: default_visible,
                                total: gap,
                            });
                        }
                    }
                }
            }

            // Gap after last hunk — no lines shown by default, single button
            if let Some(last_new) = entry.hunks.last().and_then(|h| {
                h.lines.iter().rev().find_map(|l| if l.line_type == DiffLineType::Context { l.new_line_no } else { None })
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

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _: KeyEvent) {
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
                if let Some(idx) = self.focused_button {
                    let next = (idx + 1).min(self.expand_buttons.len().saturating_sub(1));
                    self.focused_button = Some(next);
                    self.scroll_to_button(next);
                }
            }
            UserEvent::NavigateLeft => {
                if let Some(idx) = self.focused_button {
                    let prev = idx.saturating_sub(1);
                    self.focused_button = Some(prev);
                    self.scroll_to_button(prev);
                }
            }
            UserEvent::Confirm => {
                if let Some(idx) = self.focused_button {
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
                "─".repeat(diff_area.width as usize)
                    .fg(self.ctx.color_theme.divider_fg),
            );
            f.render_widget(Paragraph::new(separator), separator_area);

            // Build title with diff stats
            let (add_count, del_count) = if let Some(entry) = self.diff_entries.first() {
                let a = entry.hunks.iter().flat_map(|h| h.lines.iter()).filter(|l| l.line_type == DiffLineType::Addition).count();
                let d = entry.hunks.iter().flat_map(|h| h.lines.iter()).filter(|l| l.line_type == DiffLineType::Deletion).count();
                (a, d)
            } else {
                (0, 0)
            };
            let mut title_spans = vec![
                Span::styled(
                    format!("─── {} ", self.title),
                    Style::default()
                        .fg(self.ctx.color_theme.fg)
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            if add_count > 0 {
                title_spans.push(Span::styled(
                    format!("+{add_count} "),
                    Style::default().fg(Color::Rgb(0, 255, 135)).add_modifier(Modifier::BOLD),
                ));
            }
            if del_count > 0 {
                title_spans.push(Span::styled(
                    format!("-{del_count} "),
                    Style::default().fg(Color::Rgb(255, 80, 80)).add_modifier(Modifier::BOLD),
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

        self.diff_content_area = Some(content_area);

        // Rebuild base lines if needed (content changed, not just hover)
        if self.needs_rebuild || self.base_lines.is_empty() {
            self.base_lines = self.build_diff_lines(&content_area);
            self.needs_rebuild = false;
            self.focused_button = if self.expand_buttons.is_empty() { None } else { Some(0) };
        }
        self.content_height = self.base_lines.len();

        // Build visible lines from cache, applying hover only to button lines
        let mut visible_lines: Vec<Line> = self.base_lines
            .iter()
            .skip(self.scroll_offset)
            .take(content_area.height as usize)
            .cloned()
            .collect();

        // Apply hover styling to the hovered button line
        if let Some(btn_idx) = self.hovered_button {
            if let Some(btn) = self.expand_buttons.get(btn_idx) {
                let visible_idx = btn.line_idx.saturating_sub(self.scroll_offset);
                if visible_idx < visible_lines.len() {
                    visible_lines[visible_idx] = self.build_button_line(btn, true, content_area.width);
                }
            }
        }

        // Apply keyboard focus highlight to the focused button line
        if let Some(btn_idx) = self.focused_button {
            if Some(btn_idx) != self.hovered_button {
                if let Some(btn) = self.expand_buttons.get(btn_idx) {
                    if btn.line_idx >= self.scroll_offset {
                        let visible_idx = btn.line_idx - self.scroll_offset;
                        if visible_idx < visible_lines.len() {
                            visible_lines[visible_idx] = self.build_button_line(btn, true, content_area.width);
                        }
                    }
                }
            }
        }

        let paragraph = Paragraph::new(visible_lines);
        f.render_widget(paragraph, content_area);
    }

    pub fn update_layout(&mut self, area: Rect) {
        let [list_area, _] = self.split_areas(area);
        if let Some(ref mut state) = self.commit_list_state {
            state.update_height(list_area.height as usize);
        }
    }

    pub fn prepare_graph_uploads(&mut self) {
        if let Some(ref mut state) = self.commit_list_state {
            state.ensure_visible_graph_uploaded();
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
        if !self.base_lines.is_empty() {
            return self.base_lines.len();
        }
        let dummy_area = Rect::new(0, 0, width, 1);
        self.build_diff_lines(&dummy_area).len()
    }

    fn build_diff_lines(&mut self, diff_area: &Rect) -> Vec<Line<'static>> {
        match self.ctx.ui_config.common.diff_mode {
            DiffMode::Raw => self.build_raw_diff_lines(diff_area),
            DiffMode::Enhanced => self.build_base_lines(diff_area.width),
        }
    }

    fn build_raw_diff_lines(&self, diff_area: &Rect) -> Vec<Line<'static>> {
        let width = diff_area.width as usize;
        let mut lines = Vec::new();

        for entry in &self.diff_entries {
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

            for hunk in &entry.hunks {
                for diff_line in &hunk.lines {
                    match diff_line.line_type {
                        DiffLineType::Context => {
                            for chunk in wrap_text(&format!(" {}", diff_line.content), width) {
                                lines.push(Line::from(Span::styled(
                                    chunk.to_string(),
                                    Style::default().fg(self.ctx.color_theme.fg),
                                )));
                            }
                        }
                        DiffLineType::Addition => {
                            for chunk in wrap_text(&format!("+{}", diff_line.content), width) {
                                lines.push(Line::from(Span::styled(
                                    chunk.to_string(),
                                    Style::default()
                                        .fg(self.ctx.color_theme.detail_file_change_add_fg),
                                )));
                            }
                        }
                        DiffLineType::Deletion => {
                            for chunk in wrap_text(&format!("-{}", diff_line.content), width) {
                                lines.push(Line::from(Span::styled(
                                    chunk.to_string(),
                                    Style::default()
                                        .fg(self.ctx.color_theme.detail_file_change_delete_fg),
                                )));
                            }
                        }
                        DiffLineType::HunkHeader => {
                            for chunk in wrap_text(&diff_line.content, width) {
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
                        DiffLineType::BinaryNote => {
                            for chunk in wrap_text(&format!(" {}", diff_line.content), width) {
                                lines.push(Line::from(Span::styled(
                                    chunk.to_string(),
                                    Style::default()
                                        .fg(self.ctx.color_theme.fg)
                                        .add_modifier(Modifier::DIM),
                                )));
                            }
                        }
                    }
                }
            }

            lines.push(Line::from(""));
        }

        lines
    }

    fn build_base_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let entry = match self.diff_entries.first() {
            Some(e) => e,
            None => return lines,
        };

        let file_path = entry.new_path.as_deref().or(entry.old_path.as_deref()).unwrap_or("");
        let mut highlighter = SyntaxHighlighter::new_with_theme(
            file_path,
            &self.ctx.core_config.option.syntax_theme,
        );

        // VS Code-style diff colors
        let add_bg = Color::Rgb(32, 68, 45);
        let del_bg = Color::Rgb(68, 35, 40);
        let ctx_fg = Color::Rgb(192, 202, 245);

        let add_style = Style::default().bg(add_bg);
        let del_style = Style::default().bg(del_bg);
        let ctx_style = Style::default().fg(ctx_fg);

        let bar_add = Span::styled("▍", Style::default().fg(Color::Rgb(63, 185, 80)).bg(add_bg));
        let bar_del = Span::styled("▍", Style::default().fg(Color::Rgb(248, 81, 73)).bg(del_bg));

        self.expand_buttons.clear();
        let mut gap_idx = 0usize;

        // Helper: last context line's new_line_no in a hunk
        fn hunk_last_new_line(hunk: &crate::git::diff::Hunk) -> Option<u32> {
            hunk.lines.iter().rev().find_map(|l| {
                if l.line_type == DiffLineType::Context { l.new_line_no } else { None }
            })
        }

        // Helper: first context line's new_line_no in a hunk
        fn hunk_first_new_line(hunk: &crate::git::diff::Hunk) -> Option<u32> {
            hunk.lines.iter().find_map(|l| {
                if l.line_type == DiffLineType::Context { l.new_line_no } else { None }
            })
        }

        // Render a gap between hunks (bidirectional)
        // Extract references to avoid closure capture conflicts
        let gap_states = &self.gap_states;
        let new_file_lines = &self.new_file_lines;

        // Helper: render a gap block. is_edge: None=middle, Some(true)=top, Some(false)=bottom
        let mut render_gap = |lines: &mut Vec<Line<'static>>, gap_start: u32, gap_end: u32, gidx: usize, hl: &mut Option<SyntaxHighlighter>, edge: Option<bool>| -> usize {
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
                            Span::styled(label.to_string(), Style::default().fg(Color::Rgb(122, 162, 247)).add_modifier(Modifier::BOLD)),
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
                            Style::default().fg(self.ctx.color_theme.fg).add_modifier(Modifier::ITALIC),
                        )]));
                    }
                    for i in 0..visible_up {
                        let line_no = (gap_end - visible_up as u32 + i as u32) as usize;
                        if line_no > 0 && line_no <= new_file_lines.len() {
                            let content = &new_file_lines[line_no - 1];
                            let line_num = format!("{:>4} │ ", line_no);
                            if let Some(ref mut h) = hl {
                                lines.extend(wrap_diff_line_with_syntax(content, &line_num, ctx_style, width, h));
                            } else {
                                lines.extend(wrap_diff_line(content, &line_num, ctx_style, width));
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
                                lines.extend(wrap_diff_line_with_syntax(content, &line_num, ctx_style, width, h));
                            } else {
                                lines.extend(wrap_diff_line(content, &line_num, ctx_style, width));
                            }
                        }
                    }
                    let hidden = total.saturating_sub(visible_down);
                    if hidden > 0 {
                        let unchanged_text = format!("─── {} lines unchanged ───", hidden);
                        let unchanged_width = unchanged_text.chars().count();
                        lines.push(Line::from(vec![Span::styled(
                            unchanged_text,
                            Style::default().fg(self.ctx.color_theme.fg).add_modifier(Modifier::ITALIC),
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
                            Span::styled(label.to_string(), Style::default().fg(Color::Rgb(122, 162, 247)).add_modifier(Modifier::BOLD)),
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
                                lines.extend(wrap_diff_line_with_syntax(content, &line_num, ctx_style, width, h));
                            } else {
                                lines.extend(wrap_diff_line(content, &line_num, ctx_style, width));
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
                            Span::styled(up_label.to_string(), Style::default().fg(Color::Rgb(122, 162, 247)).add_modifier(Modifier::BOLD)),
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
                            Style::default().fg(self.ctx.color_theme.fg).add_modifier(Modifier::ITALIC),
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
                            Span::styled(down_label.to_string(), Style::default().fg(Color::Rgb(122, 162, 247)).add_modifier(Modifier::BOLD)),
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
                                lines.extend(wrap_diff_line_with_syntax(content, &line_num, ctx_style, width, h));
                            } else {
                                lines.extend(wrap_diff_line(content, &line_num, ctx_style, width));
                            }
                        }
                    }
                }
            }
            gidx + 1
        };

        // Gap before first hunk — single button at top
        if let Some(first_hunk) = entry.hunks.first() {
            if let Some(first_new) = hunk_first_new_line(first_hunk) {
                if first_new > 1 {
                    gap_idx = render_gap(&mut lines, 1, first_new, gap_idx, &mut highlighter, Some(true));
                }
            }
        }

        for (hunk_idx, hunk) in entry.hunks.iter().enumerate() {
            // Gap between hunks
            if hunk_idx > 0 {
                if let Some(prev_hunk) = entry.hunks.get(hunk_idx - 1) {
                    let prev_end = hunk_last_new_line(prev_hunk);
                    let curr_start = hunk_first_new_line(hunk);
                    if let (Some(pe), Some(cs)) = (prev_end, curr_start) {
                        if cs > pe + 1 {
                            gap_idx = render_gap(&mut lines, pe + 1, cs, gap_idx, &mut highlighter, None);
                        }
                    }
                }
            }

            // Hunk content
            for diff_line in hunk.lines.iter().skip(1) {
                match diff_line.line_type {
                    DiffLineType::Addition => {
                        let line_num = format!("{:>4} │ ", diff_line.new_line_no.unwrap_or(0));
                        if let Some(ref mut h) = highlighter {
                            lines.extend(wrap_diff_line_with_syntax_and_bar(
                                &diff_line.content, &line_num, add_style, width, h, Some(bar_add.clone()),
                            ));
                        } else {
                            lines.extend(wrap_diff_line_with_bar(
                                &diff_line.content, &line_num, add_style, width, Some(bar_add.clone()),
                            ));
                        }
                    }
                    DiffLineType::Deletion => {
                        let line_num = "     │ ".to_string();
                        if let Some(ref mut h) = highlighter {
                            lines.extend(wrap_diff_line_with_syntax_and_bar(
                                &diff_line.content, &line_num, del_style, width, h, Some(bar_del.clone()),
                            ));
                        } else {
                            lines.extend(wrap_diff_line_with_bar(
                                &diff_line.content, &line_num, del_style, width, Some(bar_del.clone()),
                            ));
                        }
                    }
                    DiffLineType::Context => {
                        let line_num = format!("{:>4} │ ", diff_line.new_line_no.unwrap_or(0));
                        if let Some(ref mut h) = highlighter {
                            lines.extend(wrap_diff_line_with_syntax(
                                &diff_line.content, &line_num, ctx_style, width, h,
                            ));
                        } else {
                            lines.extend(wrap_diff_line(
                                &diff_line.content, &line_num, ctx_style, width,
                            ));
                        }
                    }
                    DiffLineType::BinaryNote => {
                        for chunk in wrap_text(&diff_line.content, width as usize) {
                            lines.push(Line::from(vec![Span::styled(
                                chunk.to_string(),
                                Style::default().fg(Color::Rgb(192, 202, 245)).add_modifier(Modifier::DIM),
                            )]));
                        }
                    }
                    _ => {}
                }
            }
            lines.push(Line::from(""));
        }

        // Gap after last hunk — single button at bottom
        if let Some(last_hunk) = entry.hunks.last() {
            if let Some(last_new) = hunk_last_new_line(last_hunk) {
                let file_total = self.new_file_lines.len() as u32;
                if last_new < file_total {
                    render_gap(&mut lines, last_new + 1, file_total + 1, gap_idx, &mut highlighter, Some(false));
                }
            }
        }

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
            let path = entry.new_path.as_deref().or(entry.old_path.as_deref()).unwrap_or("");
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
        let current = self.title.strip_prefix("Diff (staged): ")
            .or_else(|| self.title.strip_prefix("Diff (unstaged): "))
            .or_else(|| self.title.strip_prefix("Diff: "))
            .unwrap_or("");

        let position = if self.commit_hash.is_empty() {
            self.all_file_paths.iter().position(|(p, s)| p == current && *s == is_staged)
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
                self.tx.send(AppEvent::OpenUncommittedDiff { file_path: new_path, is_staged: new_staged });
            } else {
                self.tx.send(AppEvent::OpenFileDiff {
                    hash: self.commit_hash.clone(),
                    file_path: new_path,
                });
            }
        }
    }

    pub fn refresh(&self) {
        let file_path = self.title
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
            parts.push("←→:buttons");
            parts.push("Enter:expand");
        }
        if !self.all_file_paths.is_empty() {
            let is_staged = self.title.contains("(staged)");
            let current = self.title.strip_prefix("Diff (staged): ")
                .or_else(|| self.title.strip_prefix("Diff (unstaged): "))
                .or_else(|| self.title.strip_prefix("Diff: "))
                .unwrap_or("");

            let position = if self.commit_hash.is_empty() {
                self.all_file_paths.iter().position(|(p, s)| p == current && *s == is_staged)
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
        parts.push("c:copy-path");
        parts.push("r:refresh");
        parts.push("Esc:close");
        parts.join(" ")
    }

    fn build_button_line(&self, btn: &ButtonInfo, is_hovered: bool, _width: u16) -> Line<'static> {
        let style = if is_hovered {
            Style::default()
                .fg(Color::Rgb(192, 202, 245))
                .bg(Color::Rgb(41, 46, 66))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Color::Rgb(122, 162, 247))
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
        let prev_hover = self.hovered_button;
        self.hovered_button = None;

        if let Some(area) = self.diff_content_area {
            let in_area = col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height;
            if in_area {
                let local_row = (self.scroll_offset + (row - area.y) as usize) as usize;
                // Check if mouse is over any button (considering column range)
                for (idx, btn) in self.expand_buttons.iter().enumerate() {
                    if btn.line_idx == local_row {
                        let local_col = col.saturating_sub(area.x);
                        if local_col >= btn.col_start && local_col < btn.col_end {
                            self.hovered_button = Some(idx);
                            break;
                        }
                    }
                }
            }
        }

        // Only mark needs redraw if hover state changed
        if prev_hover != self.hovered_button {
            // The render loop will pick up the new hover state
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
                        gap_state.visible_down = (gap_state.visible_down + increment).min(max_expand);
                    }
                }
                self.needs_rebuild = true;
            }
        }
    }

    fn scroll_to_button(&mut self, idx: usize) {
        if let Some(btn) = self.expand_buttons.get(idx) {
            let line = btn.line_idx;
            let viewport = self.diff_content_area.map(|a| a.height as usize).unwrap_or(0);
            if line < self.scroll_offset {
                self.scroll_offset = line;
            } else if viewport > 0 && line >= self.scroll_offset + viewport {
                self.scroll_offset = line.saturating_sub(viewport - 1);
            }
        }
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(area) = self.diff_content_area {
            let in_area = col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height;
            if in_area {
                let local_row = (self.scroll_offset + (row - area.y) as usize) as usize;
                let local_col = col.saturating_sub(area.x);
                for btn in self.expand_buttons.iter() {
                    if btn.line_idx == local_row && local_col >= btn.col_start && local_col < btn.col_end {
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
                                    gap_state.visible_down = (gap_state.visible_down + increment).min(max_expand);
                                }
                            }
                            self.needs_rebuild = true;
                        }
                        break;
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

        let char_indices: Vec<(usize, char)> =
            remaining.char_indices().take(max_width + 1).collect();
        let mut break_at = max_width;

        while break_at > 0 {
            if char_indices[break_at - 1].1.is_ascii_whitespace() {
                break;
            }
            break_at -= 1;
        }

        if break_at == 0 {
            break_at = max_width;
        }

        let byte_break = if break_at < char_indices.len() {
            char_indices[break_at].0
        } else {
            remaining.len()
        };

        let (before, after) = remaining.split_at(byte_break);
        if break_at > 0 && char_indices[break_at - 1].1.is_ascii_whitespace() {
            lines.push(before.trim_end());
            remaining = after.trim_start();
        } else {
            lines.push(before);
            remaining = after;
        }
    }

    lines
}

fn wrap_diff_line_with_syntax(
    content: &str,
    line_num_str: &str,
    base_style: Style,
    available_width: u16,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let line_num_width = 7; // "1234 │ " = 7 chars
    let content_width = available_width.saturating_sub(line_num_width) as usize;

    let mut lines = Vec::new();
    if content_width == 0 {
        let spans = highlighter.highlight_line(content, base_style, None);
        let mut all_spans = vec![Span::styled(line_num_str.to_string(), Style::default().fg(Color::Rgb(59, 66, 97)))];
        all_spans.extend(spans);
        lines.push(Line::from(all_spans));
        return lines;
    }

    let mut remaining = content;
    let mut first = true;

    while !remaining.is_empty() {
        let (chunk, rest) = if remaining.chars().count() > content_width {
            let char_indices: Vec<(usize, char)> =
                remaining.char_indices().take(content_width + 1).collect();
            let mut break_at = content_width;
            while break_at > 0 {
                if char_indices[break_at - 1].1.is_ascii_whitespace() {
                    break;
                }
                break_at -= 1;
            }
            if break_at == 0 {
                break_at = content_width;
            }
            let byte_break = if break_at < char_indices.len() {
                char_indices[break_at].0
            } else {
                remaining.len()
            };
            let (before, after) = remaining.split_at(byte_break);
            if break_at > 0 && char_indices[break_at - 1].1.is_ascii_whitespace() {
                (before.trim_end(), after.trim_start())
            } else {
                (before, after)
            }
        } else {
            (remaining, "")
        };

        let num_span = if first {
            Span::styled(line_num_str.to_string(), Style::default().fg(Color::Rgb(59, 66, 97)))
        } else {
            Span::styled("     │ ".to_string(), Style::default().fg(Color::Rgb(59, 66, 97)))
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
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let line_num_width = 7; // "1234 │ " = 7 chars
    let content_width = available_width.saturating_sub(line_num_width) as usize;

    if content_width == 0 {
        lines.push(Line::from(vec![
            Span::styled(
                line_num_str.to_string(),
                Style::default().fg(Color::Rgb(59, 66, 97)),
            ),
            Span::styled(content.to_string(), content_style),
        ]));
        return lines;
    }

    let mut remaining = content;
    let mut first = true;

    while !remaining.is_empty() {
        let (chunk, rest) = if remaining.chars().count() > content_width {
            let char_indices: Vec<(usize, char)> =
                remaining.char_indices().take(content_width + 1).collect();
            let mut break_at = content_width;

            while break_at > 0 {
                if char_indices[break_at - 1].1.is_ascii_whitespace() {
                    break;
                }
                break_at -= 1;
            }

            if break_at == 0 {
                break_at = content_width;
            }

            let byte_break = if break_at < char_indices.len() {
                char_indices[break_at].0
            } else {
                remaining.len()
            };

            let (before, after) = remaining.split_at(byte_break);
            if break_at > 0 && char_indices[break_at - 1].1.is_ascii_whitespace() {
                (before.trim_end(), after.trim_start())
            } else {
                (before, after)
            }
        } else {
            (remaining, "")
        };

        let num_span = if first {
            Span::styled(
                line_num_str.to_string(),
                Style::default().fg(Color::Rgb(59, 66, 97)),
            )
        } else {
            Span::styled(
                "     │ ".to_string(),
                Style::default().fg(Color::Rgb(59, 66, 97)),
            )
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
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let line_num_width = 7; // "1234 │ " = 7 chars
    let bar_width = if bar_span.is_some() { 1 } else { 0 };
    let content_width = available_width.saturating_sub(line_num_width + bar_width) as usize;

    if content_width == 0 {
        let mut spans = vec![Span::styled(
            line_num_str.to_string(),
            Style::default().fg(Color::Rgb(59, 66, 97)),
        )];
        if let Some(bar) = bar_span {
            spans.push(bar);
        }
        spans.push(Span::styled(content.to_string(), content_style));
        spans.push(Span::styled(" ".repeat(200), content_style));
        lines.push(Line::from(spans));
        return lines;
    }

    let mut remaining = content;
    let mut first = true;

    while !remaining.is_empty() {
        let (chunk, rest) = if remaining.chars().count() > content_width {
            let char_indices: Vec<(usize, char)> =
                remaining.char_indices().take(content_width + 1).collect();
            let mut break_at = content_width;
            while break_at > 0 {
                if char_indices[break_at - 1].1.is_ascii_whitespace() {
                    break;
                }
                break_at -= 1;
            }
            if break_at == 0 {
                break_at = content_width;
            }
            let byte_break = if break_at < char_indices.len() {
                char_indices[break_at].0
            } else {
                remaining.len()
            };
            let (before, after) = remaining.split_at(byte_break);
            if break_at > 0 && char_indices[break_at - 1].1.is_ascii_whitespace() {
                (before.trim_end(), after.trim_start())
            } else {
                (before, after)
            }
        } else {
            (remaining, "")
        };

        let num_span = if first {
            Span::styled(
                line_num_str.to_string(),
                Style::default().fg(Color::Rgb(59, 66, 97)),
            )
        } else {
            Span::styled(
                "     │ ".to_string(),
                Style::default().fg(Color::Rgb(59, 66, 97)),
            )
        };

        let mut spans = vec![num_span];
        if let Some(ref bar) = bar_span {
            spans.push(bar.clone());
        }
        spans.push(Span::styled(chunk.to_string(), content_style));
        spans.push(Span::styled(" ".repeat(200), content_style));
        lines.push(Line::from(spans));

        remaining = rest;
        first = false;
    }

    lines
}

fn wrap_diff_line_with_syntax_and_bar(
    content: &str,
    line_num_str: &str,
    base_style: Style,
    available_width: u16,
    highlighter: &mut SyntaxHighlighter,
    bar_span: Option<Span<'static>>,
) -> Vec<Line<'static>> {
    let line_num_width = 7; // "1234 │ " = 7 chars
    let bar_width = if bar_span.is_some() { 1 } else { 0 };
    let content_width = available_width.saturating_sub(line_num_width + bar_width) as usize;

    let mut lines = Vec::new();
    if content_width == 0 {
        let mut spans = vec![Span::styled(
            line_num_str.to_string(),
            Style::default().fg(Color::Rgb(59, 66, 97)),
        )];
        if let Some(bar) = bar_span {
            spans.push(bar);
        }
        spans.extend(highlighter.highlight_line(content, base_style, None));
        spans.push(Span::styled(" ".repeat(200), base_style));
        lines.push(Line::from(spans));
        return lines;
    }

    let mut remaining = content;
    let mut first = true;

    while !remaining.is_empty() {
        let (chunk, rest) = if remaining.chars().count() > content_width {
            let char_indices: Vec<(usize, char)> =
                remaining.char_indices().take(content_width + 1).collect();
            let mut break_at = content_width;
            while break_at > 0 {
                if char_indices[break_at - 1].1.is_ascii_whitespace() {
                    break;
                }
                break_at -= 1;
            }
            if break_at == 0 {
                break_at = content_width;
            }
            let byte_break = if break_at < char_indices.len() {
                char_indices[break_at].0
            } else {
                remaining.len()
            };
            let (before, after) = remaining.split_at(byte_break);
            if break_at > 0 && char_indices[break_at - 1].1.is_ascii_whitespace() {
                (before.trim_end(), after.trim_start())
            } else {
                (before, after)
            }
        } else {
            (remaining, "")
        };

        let num_span = if first {
            Span::styled(
                line_num_str.to_string(),
                Style::default().fg(Color::Rgb(59, 66, 97)),
            )
        } else {
            Span::styled(
                "     │ ".to_string(),
                Style::default().fg(Color::Rgb(59, 66, 97)),
            )
        };

        let mut spans = vec![num_span];
        if let Some(ref bar) = bar_span {
            spans.push(bar.clone());
        }
        spans.extend(highlighter.highlight_line(chunk, base_style, None));
        spans.push(Span::styled(" ".repeat(200), base_style));
        lines.push(Line::from(spans));

        remaining = rest;
        first = false;
    }

    lines
}
