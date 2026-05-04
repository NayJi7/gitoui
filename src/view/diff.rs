use std::cell::RefCell;
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
    view::{ListRefreshViewContext, RefreshViewContext},
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

    // Mouse hover state for expand buttons
    hovered_row: Option<u16>,
    diff_content_area: Option<Rect>,

    repo_path: std::path::PathBuf,
    context_lines: u32,
    expand_buttons: RefCell<Vec<(usize, ExpandDirection)>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpandDirection {
    Up,
    Down,
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
            hovered_row: None,
            diff_content_area: None,
            repo_path,
            context_lines: 3,
            expand_buttons: RefCell::new(Vec::new()),
        }
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
                self.cycle_file(1);
            }
            UserEvent::NavigateLeft => {
                self.cycle_file(-1);
            }
            UserEvent::UserCommand(n) => {
                self.tx.send(AppEvent::OpenUserCommand(n));
            }
            UserEvent::HelpToggle => {
                self.tx.send(AppEvent::OpenHelp);
            }
            UserEvent::Confirm | UserEvent::Cancel | UserEvent::Close => {
                if self.commit_hash.is_empty() {
                    self.tx.send(AppEvent::CloseDiffToUncommitted);
                } else {
                    self.tx.send(AppEvent::CloseDiffToDetail);
                }
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
        let (content_area, hovered_line_idx) = if !self.title.is_empty() {
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

            let hovered = self.hovered_row.map(|r| self.scroll_offset + r as usize);
            (content_area, hovered)
        } else {
            let hovered = self.hovered_row.map(|r| self.scroll_offset + r as usize);
            (diff_area, hovered)
        };

        self.diff_content_area = Some(content_area);
        let lines = self.build_diff_lines_with_hover(&content_area, hovered_line_idx);
        self.content_height = lines.len();

        let visible_lines: Vec<Line> = lines
            .into_iter()
            .skip(self.scroll_offset)
            .take(content_area.height as usize)
            .collect();

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

    fn split_areas(&self, area: Rect) -> [Rect; 2] {
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

    fn count_diff_lines(&self, width: u16) -> usize {
        let dummy_area = Rect::new(0, 0, width, 1);
        self.build_diff_lines(&dummy_area).len()
    }

    fn build_diff_lines(&self, diff_area: &Rect) -> Vec<Line<'static>> {
        match self.ctx.ui_config.common.diff_mode {
            DiffMode::Raw => self.build_raw_diff_lines(diff_area),
            DiffMode::Enhanced => self.build_enhanced_diff_lines(diff_area.width, None),
        }
    }

    fn build_diff_lines_with_hover(
        &self,
        diff_area: &Rect,
        hovered_line_idx: Option<usize>,
    ) -> Vec<Line<'static>> {
        match self.ctx.ui_config.common.diff_mode {
            DiffMode::Raw => self.build_raw_diff_lines(diff_area),
            DiffMode::Enhanced => self.build_enhanced_diff_lines(diff_area.width, hovered_line_idx),
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

    fn build_enhanced_diff_lines(
        &self,
        width: u16,
        hovered_line_idx: Option<usize>,
    ) -> Vec<Line<'static>> {
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

        // VS Code-style diff colors: brighter backgrounds, vivid bars
        let add_bg = Color::Rgb(32, 68, 45);
        let del_bg = Color::Rgb(68, 35, 40);
        let ctx_fg = Color::Rgb(192, 202, 245);

        // Only change background so syntax highlighting / default text color is preserved
        let add_style = Style::default().bg(add_bg);
        let del_style = Style::default().bg(del_bg);
        let ctx_style = Style::default().fg(ctx_fg);

        // Thicker bar (▍) with matching background so it visually merges with the highlight block
        let bar_add = Span::styled("▍", Style::default().fg(Color::Rgb(63, 185, 80)).bg(add_bg));
        let bar_del = Span::styled("▍", Style::default().fg(Color::Rgb(248, 81, 73)).bg(del_bg));

        self.expand_buttons.borrow_mut().clear();

        for (hunk_idx, hunk) in entry.hunks.iter().enumerate() {
            // Show collapsed region between hunks
            if hunk_idx > 0 {
                if let Some(prev_hunk) = entry.hunks.get(hunk_idx - 1) {
                    let old_gap = hunk
                        .old_start
                        .saturating_sub(prev_hunk.old_start + prev_hunk.old_count);
                    let new_gap = hunk
                        .new_start
                        .saturating_sub(prev_hunk.new_start + prev_hunk.new_count);
                    let gap = old_gap.max(new_gap);
                    if gap > 0 {
                        let unchanged_text = format!("─── {} lines unchanged ───", gap);
                        let unchanged_width = unchanged_text.chars().count();

                        let up_idx = lines.len();
                        let up_hover = hovered_line_idx == Some(up_idx);
                        let up_style = if up_hover {
                            Style::default()
                                .fg(Color::Rgb(192, 202, 245))
                                .bg(Color::Rgb(41, 46, 66))
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                                .fg(Color::Rgb(122, 162, 247))
                                .add_modifier(Modifier::BOLD)
                        };
                        // Expand button above — centered under the unchanged text
                        let up_label = "  ▼  show more  ";
                        let up_label_len = up_label.chars().count();
                        let up_pad = unchanged_width.saturating_sub(up_label_len);
                        let up_left = up_pad / 2;
                        lines.push(Line::from(vec![
                            Span::styled(" ".repeat(up_left), Style::default()),
                            Span::styled(up_label.to_string(), up_style),
                        ]));
                        self.expand_buttons.borrow_mut().push((up_idx, ExpandDirection::Up));
                        lines.push(Line::from(vec![Span::styled(
                            unchanged_text,
                            Style::default()
                                .fg(Color::Rgb(59, 66, 97))
                                .add_modifier(Modifier::ITALIC),
                        )]));
                        let down_idx = lines.len();
                        let down_hover = hovered_line_idx == Some(down_idx);
                        let down_style = if down_hover {
                            Style::default()
                                .fg(Color::Rgb(192, 202, 245))
                                .bg(Color::Rgb(41, 46, 66))
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                                .fg(Color::Rgb(122, 162, 247))
                                .add_modifier(Modifier::BOLD)
                        };
                        // Expand button below — centered under the unchanged text
                        let down_label = "  ▲  show more  ";
                        let down_label_len = down_label.chars().count();
                        let down_pad = unchanged_width.saturating_sub(down_label_len);
                        let down_left = down_pad / 2;
                        lines.push(Line::from(vec![
                            Span::styled(" ".repeat(down_left), Style::default()),
                            Span::styled(down_label.to_string(), down_style),
                        ]));
                        self.expand_buttons.borrow_mut().push((down_idx, ExpandDirection::Down));
                        lines.push(Line::from(""));
                    }
                }
            }

            // Hunk header — show line range instead of raw @@
            let hunk_label = format!(
                "@@ lines {}–{} (old)  →  lines {}–{} (new) @@",
                hunk.old_start,
                hunk.old_start + hunk.old_count.saturating_sub(1),
                hunk.new_start,
                hunk.new_start + hunk.new_count.saturating_sub(1),
            );
            for chunk in wrap_text(&hunk_label, width as usize) {
                lines.push(Line::from(vec![Span::styled(
                    chunk.to_string(),
                    Style::default()
                        .fg(Color::Rgb(86, 95, 137))
                        .add_modifier(Modifier::ITALIC),
                )]));
            }

            // Context/addition/deletion lines
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
                            ));
                        } else {
                            lines.extend(wrap_diff_line_with_bar(
                                &diff_line.content,
                                &line_num,
                                add_style,
                                width,
                                Some(bar_add.clone()),
                            ));
                        }
                    }
                    DiffLineType::Deletion => {
                        // No line number for deleted lines; keep alignment with gutter
                        let line_num = "     │ ".to_string();
                        if let Some(ref mut h) = highlighter {
                            lines.extend(wrap_diff_line_with_syntax_and_bar(
                                &diff_line.content,
                                &line_num,
                                del_style,
                                width,
                                h,
                                Some(bar_del.clone()),
                            ));
                        } else {
                            lines.extend(wrap_diff_line_with_bar(
                                &diff_line.content,
                                &line_num,
                                del_style,
                                width,
                                Some(bar_del.clone()),
                            ));
                        }
                    }
                    DiffLineType::Context => {
                        let line_num = format!("{:>4} │ ", diff_line.old_line_no.unwrap_or(0));
                        if let Some(ref mut h) = highlighter {
                            lines.extend(wrap_diff_line_with_syntax(
                                &diff_line.content,
                                &line_num,
                                ctx_style,
                                width,
                                h,
                            ));
                        } else {
                            lines.extend(wrap_diff_line(
                                &diff_line.content,
                                &line_num,
                                ctx_style,
                                width,
                            ));
                        }
                    }
                    DiffLineType::BinaryNote => {
                        for chunk in wrap_text(&diff_line.content, width as usize) {
                            lines.push(Line::from(vec![Span::styled(
                                chunk.to_string(),
                                Style::default()
                                    .fg(Color::Rgb(192, 202, 245))
                                    .add_modifier(Modifier::DIM),
                            )]));
                        }
                    }
                    _ => {}
                }
            }
            lines.push(Line::from(""));
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
        if let Some(list_state) = self.commit_list_state.as_ref() {
            let list_context = ListRefreshViewContext::from(list_state);
            let context = RefreshViewContext::Detail { list_context };
            self.tx.send(AppEvent::Refresh(context));
        }
    }

    pub fn footer_hint(&self) -> String {
        let mut parts = Vec::new();
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
                    parts.push("←:prev-file");
                }
                if idx + 1 < self.all_file_paths.len() {
                    parts.push("→:next-file");
                }
            }
        }
        parts.push("c:copy-path");
        parts.push("Esc:close");
        parts.join(" ")
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(area) = self.diff_content_area {
            let in_area = col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height;
            if in_area {
                let clicked_screen_line = row - area.y;
                let clicked_line = self.scroll_offset + clicked_screen_line as usize;
                let btn = self
                    .expand_buttons
                    .borrow()
                    .iter()
                    .find(|(idx, _)| *idx == clicked_line)
                    .map(|(_, dir)| *dir);
                if let Some(direction) = btn {
                    let old_context = self.context_lines;
                    // Double the context each time for a more dramatic reveal
                    self.context_lines = old_context.saturating_mul(2).max(old_context + 5);
                    let _ = self.reload_diff();
                    // Directional scroll: jump to show the newly revealed content
                    match direction {
                        ExpandDirection::Up => {
                            // Scroll up aggressively to reveal the newly loaded content above
                            let jump = (self.context_lines as usize).saturating_sub(old_context as usize);
                            self.scroll_offset = self.scroll_offset.saturating_sub(jump.max(10));
                        }
                        ExpandDirection::Down => {
                            // Small scroll down to reveal content below the button
                            self.scroll_offset = self.scroll_offset.saturating_add(3);
                        }
                    }
                }
            }
        }
    }

    fn reload_diff(&mut self) -> Result<(), String> {
        let file_path = self
            .title
            .strip_prefix("Diff (staged): ")
            .or_else(|| self.title.strip_prefix("Diff (unstaged): "))
            .or_else(|| self.title.strip_prefix("Diff: "))
            .unwrap_or("");

        let new_entries = if !self.commit_hash.is_empty() {
            vec![DiffEntry::load_for_file_with_context(
                &self.repo_path,
                &self.commit_hash,
                file_path,
                self.context_lines,
            )?]
        } else if self.title.contains("(staged)") {
            vec![DiffEntry::load_staged_for_file_with_context(
                &self.repo_path,
                file_path,
                self.context_lines,
            )?]
        } else {
            vec![DiffEntry::load_unstaged_for_file_with_context(
                &self.repo_path,
                file_path,
                self.context_lines,
            )?]
        };

        self.diff_entries = new_entries;
        // Preserve scroll offset so the user's view stays anchored
        Ok(())
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        if let Some(area) = self.diff_content_area {
            let in_area = col >= area.x
                && col < area.x + area.width
                && row >= area.y
                && row < area.y + area.height;
            if in_area {
                self.hovered_row = Some(row - area.y);
            } else {
                self.hovered_row = None;
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
