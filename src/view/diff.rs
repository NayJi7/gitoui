use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};

use crate::{
    app::AppContext,
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

    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> DiffView<'a> {
    pub fn new(
        commit_list_state: CommitListState<'a>,
        diff_entries: Vec<DiffEntry>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> DiffView<'a> {
        DiffView {
            commit_list_state: Some(commit_list_state),
            diff_entries,
            scroll_offset: 0,
            content_height: 0,
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

        let visible_lines: Vec<Line> = lines
            .into_iter()
            .skip(self.scroll_offset)
            .take(diff_area.height as usize)
            .collect();

        let paragraph = Paragraph::new(visible_lines).wrap(Wrap { trim: false });
        f.render_widget(paragraph, diff_area);
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
        let diff_height = (area.height / 2).max(8);
        Layout::vertical([Constraint::Min(0), Constraint::Length(diff_height)]).areas(area)
    }

    fn build_diff_lines(&self, diff_area: &Rect) -> Vec<Line<'static>> {
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
