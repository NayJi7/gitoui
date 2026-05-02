use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::{
    app::AppContext,
    config::DiffMode,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    git::diff::{DiffEntry, DiffLineType},
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

    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> DiffView<'a> {
    pub fn new(
        commit_list_state: CommitListState<'a>,
        diff_entries: Vec<DiffEntry>,
        ctx: Rc<AppContext>,
        tx: Sender,
        title: String,
    ) -> DiffView<'a> {
        DiffView {
            commit_list_state: Some(commit_list_state),
            diff_entries,
            scroll_offset: 0,
            content_height: 0,
            title,
            ctx,
            tx,
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
            UserEvent::SelectDown => {
                self.tx.send(AppEvent::SelectOlderCommit);
            }
            UserEvent::SelectUp => {
                self.tx.send(AppEvent::SelectNewerCommit);
            }
            UserEvent::GoToParent => {
                self.tx.send(AppEvent::SelectParentCommit);
            }
            UserEvent::ShortCopy => {
                self.copy_commit_short_hash();
            }
            UserEvent::FullCopy => {
                self.copy_commit_hash();
            }
            UserEvent::UserCommand(n) => {
                self.tx.send(AppEvent::OpenUserCommand(n));
            }
            UserEvent::HelpToggle => {
                self.tx.send(AppEvent::OpenHelp);
            }
            UserEvent::Confirm | UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::CloseDiff);
            }
            UserEvent::Refresh => {
                self.refresh();
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let [list_area, diff_area] = self.split_areas(area);

        let commit_list = CommitList::new(self.ctx.clone());
        f.render_stateful_widget(commit_list, list_area, self.as_mut_list_state());

        let lines = self.build_diff_lines(&diff_area);
        self.content_height = lines.len();

        if !self.title.is_empty() {
            let [title_area, separator_area, content_area] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(diff_area);

            let title_text = format!("─── {} ───", self.title);
            let title = Line::from(Span::styled(
                title_text,
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::BOLD),
            ));
            f.render_widget(Paragraph::new(title), title_area);

            let separator = Line::from(
                "─".repeat(diff_area.width as usize)
                    .fg(self.ctx.color_theme.divider_fg),
            );
            f.render_widget(Paragraph::new(separator), separator_area);

            let visible_lines: Vec<Line> = lines
                .into_iter()
                .skip(self.scroll_offset)
                .take(content_area.height as usize)
                .collect();

            let paragraph = Paragraph::new(visible_lines).wrap(Wrap { trim: false });
            f.render_widget(paragraph, content_area);
        } else {
            let visible_lines: Vec<Line> = lines
                .into_iter()
                .skip(self.scroll_offset)
                .take(diff_area.height as usize)
                .collect();

            let paragraph = Paragraph::new(visible_lines).wrap(Wrap { trim: false });
            f.render_widget(paragraph, diff_area);
        }
    }

    pub fn update_layout(&mut self, area: Rect) {
        let [list_area, _] = self.split_areas(area);
        self.as_mut_list_state()
            .update_height(list_area.height as usize);
    }

    pub fn prepare_graph_uploads(&mut self) {
        self.as_mut_list_state().ensure_visible_graph_uploaded();
    }
}

impl<'a> DiffView<'a> {
    pub fn take_list_state(&mut self) -> CommitListState<'a> {
        self.commit_list_state.take().unwrap()
    }

    fn as_mut_list_state(&mut self) -> &mut CommitListState<'a> {
        self.commit_list_state.as_mut().unwrap()
    }

    pub fn as_list_state(&self) -> &CommitListState<'a> {
        self.commit_list_state.as_ref().unwrap()
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        self.as_mut_list_state().drain_pending_graph_uploads()
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        self.as_list_state().graph_image_ids_sorted()
    }

    fn split_areas(&self, area: Rect) -> [Rect; 2] {
        let available_height = area.height;
        let content_lines = self.count_diff_lines();

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

    fn count_diff_lines(&self) -> usize {
        let dummy_area = Rect::new(0, 0, 1000, 1);
        self.build_diff_lines(&dummy_area).len()
    }

    fn build_diff_lines(&self, diff_area: &Rect) -> Vec<Line<'static>> {
        match self.ctx.ui_config.common.diff_mode {
            DiffMode::Raw => self.build_raw_diff_lines(diff_area),
            DiffMode::Enhanced => self.build_enhanced_diff_lines(),
        }
    }

    fn build_raw_diff_lines(&self, diff_area: &Rect) -> Vec<Line<'static>> {
        let width = diff_area.width as usize;
        let mut lines = Vec::new();

        for entry in &self.diff_entries {
            if let Some(path) = &entry.new_path {
                lines.push(Line::from(Span::styled(
                    format!("--- {}", path),
                    Style::default()
                        .fg(self.ctx.color_theme.detail_hash_fg)
                        .add_modifier(Modifier::DIM),
                )));
            } else if let Some(path) = &entry.old_path {
                lines.push(Line::from(Span::styled(
                    format!("--- {}", path),
                    Style::default()
                        .fg(self.ctx.color_theme.detail_hash_fg)
                        .add_modifier(Modifier::DIM),
                )));
            }

            if let Some(path) = &entry.new_path {
                lines.push(Line::from(Span::styled(
                    format!("+++ {}", path),
                    Style::default()
                        .fg(self.ctx.color_theme.detail_hash_fg)
                        .add_modifier(Modifier::DIM),
                )));
            }

            for hunk in &entry.hunks {
                for diff_line in &hunk.lines {
                    let line = match diff_line.line_type {
                        DiffLineType::Context => Line::from(Span::styled(
                            format!(" {}", diff_line.content),
                            Style::default().fg(self.ctx.color_theme.fg),
                        )),
                        DiffLineType::Addition => Line::from(Span::styled(
                            format!("+{}", diff_line.content),
                            Style::default().fg(self.ctx.color_theme.detail_file_change_add_fg),
                        )),
                        DiffLineType::Deletion => Line::from(Span::styled(
                            format!("-{}", diff_line.content),
                            Style::default().fg(self.ctx.color_theme.detail_file_change_delete_fg),
                        )),
                        DiffLineType::HunkHeader => Line::from(Span::styled(
                            truncate_line(&diff_line.content, width),
                            Style::default()
                                .fg(self.ctx.color_theme.detail_hash_fg)
                                .add_modifier(Modifier::DIM),
                        )),
                        DiffLineType::FileHeader => Line::from(Span::styled(
                            diff_line.content.clone(),
                            Style::default()
                                .fg(self.ctx.color_theme.detail_hash_fg)
                                .add_modifier(Modifier::DIM),
                        )),
                        DiffLineType::BinaryNote => Line::from(Span::styled(
                            format!(" {}", diff_line.content),
                            Style::default()
                                .fg(self.ctx.color_theme.fg)
                                .add_modifier(Modifier::DIM),
                        )),
                    };
                    lines.push(line);
                }
            }

            lines.push(Line::from(""));
        }

        lines
    }

    fn build_enhanced_diff_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        for entry in &self.diff_entries {
            // File header
            let filename = entry
                .new_path
                .as_deref()
                .or(entry.old_path.as_deref())
                .unwrap_or("unknown");
            lines.push(Line::from(vec![
                Span::styled(
                    "─── ",
                    Style::default().fg(Color::Rgb(59, 66, 97)),
                ),
                Span::styled(
                    filename.to_string(),
                    Style::default()
                        .fg(Color::Rgb(192, 202, 245))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " ───",
                    Style::default().fg(Color::Rgb(59, 66, 97)),
                ),
            ]));
            lines.push(Line::from(""));

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
                            lines.push(Line::from(vec![Span::styled(
                                format!("─── {} lines unchanged ───", gap),
                                Style::default()
                                    .fg(Color::Rgb(59, 66, 97))
                                    .add_modifier(Modifier::ITALIC),
                            )]));
                            lines.push(Line::from(""));
                        }
                    }
                }

                // Hunk header
                if let Some(header_line) = hunk.lines.first() {
                    lines.push(Line::from(vec![Span::styled(
                        header_line.content.clone(),
                        Style::default()
                            .fg(Color::Rgb(86, 95, 137))
                            .add_modifier(Modifier::ITALIC),
                    )]));
                }

                // Context/addition/deletion lines
                for diff_line in hunk.lines.iter().skip(1) {
                    match diff_line.line_type {
                        DiffLineType::Addition => {
                            let line_num =
                                format!("{:>4}", diff_line.new_line_no.unwrap_or(0));
                            lines.push(Line::from(vec![
                                Span::styled(
                                    line_num,
                                    Style::default().fg(Color::Rgb(59, 66, 97)),
                                ),
                                Span::styled(
                                    " │ ",
                                    Style::default().fg(Color::Rgb(59, 66, 97)),
                                ),
                                Span::styled(
                                    diff_line.content.clone(),
                                    Style::default()
                                        .fg(Color::Rgb(158, 206, 106))
                                        .bg(Color::Rgb(29, 43, 59)),
                                ),
                            ]));
                        }
                        DiffLineType::Deletion => {
                            let line_num =
                                format!("{:>4}", diff_line.old_line_no.unwrap_or(0));
                            lines.push(Line::from(vec![
                                Span::styled(
                                    line_num,
                                    Style::default().fg(Color::Rgb(59, 66, 97)),
                                ),
                                Span::styled(
                                    " │ ",
                                    Style::default().fg(Color::Rgb(59, 66, 97)),
                                ),
                                Span::styled(
                                    diff_line.content.clone(),
                                    Style::default()
                                        .fg(Color::Rgb(247, 118, 142))
                                        .bg(Color::Rgb(59, 29, 43)),
                                ),
                            ]));
                        }
                        DiffLineType::Context => {
                            let line_num =
                                format!("{:>4}", diff_line.old_line_no.unwrap_or(0));
                            lines.push(Line::from(vec![
                                Span::styled(
                                    line_num,
                                    Style::default().fg(Color::Rgb(59, 66, 97)),
                                ),
                                Span::styled(
                                    " │ ",
                                    Style::default().fg(Color::Rgb(59, 66, 97)),
                                ),
                                Span::styled(
                                    diff_line.content.clone(),
                                    Style::default().fg(Color::Rgb(192, 202, 245)),
                                ),
                            ]));
                        }
                        DiffLineType::BinaryNote => {
                            lines.push(Line::from(vec![Span::styled(
                                diff_line.content.clone(),
                                Style::default()
                                    .fg(Color::Rgb(192, 202, 245))
                                    .add_modifier(Modifier::DIM),
                            )]));
                        }
                        _ => {}
                    }
                }
                lines.push(Line::from(""));
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
        let state = self.as_mut_list_state();
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

    fn copy_commit_short_hash(&self) {
        let hash = self.as_list_state().selected_commit_hash();
        self.copy_to_clipboard("Commit SHA (short)".into(), hash.as_short_hash().into());
    }

    fn copy_commit_hash(&self) {
        let hash = self.as_list_state().selected_commit_hash();
        self.copy_to_clipboard("Commit SHA".into(), hash.as_str().into());
    }

    fn copy_to_clipboard(&self, name: String, value: String) {
        self.tx.send(AppEvent::CopyToClipboard { name, value });
    }

    pub fn refresh(&self) {
        let list_state = self.as_list_state();
        let list_context = ListRefreshViewContext::from(list_state);
        let context = RefreshViewContext::Detail { list_context };
        self.tx.send(AppEvent::Refresh(context));
    }

    pub fn handle_click(&mut self, _col: u16, row: u16) {
        let list_state = self.as_mut_list_state();
        let (_, offset, height) = list_state.current_list_status();
        let clicked_index = offset + (row as usize).min(height.saturating_sub(1));
        list_state.select(clicked_index);
    }
}

fn truncate_line(s: &str, max_width: usize) -> String {
    if max_width > 0 && s.len() > max_width {
        s[..max_width].to_string()
    } else {
        s.to_string()
    }
}
