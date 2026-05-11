use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::Rect,
    style::{Modifier, Stylize},
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

#[derive(Debug)]
pub struct FileHistoryView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    file_path: String,
    entries: Vec<FileHistoryEntry>,
    selected: usize,
    scroll_offset: usize,
    view_height: usize,
    ctx: Rc<AppContext>,
    tx: Sender,
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
            scroll_offset: 0,
            view_height: 0,
            ctx,
            tx,
        }
    }

    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
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

    pub fn handle_click(&mut self, _col: u16, row: u16) {
        if self.view_height == 0 {
            return;
        }
        let clicked_idx = self.scroll_offset + row as usize;
        if clicked_idx < self.entries.len() {
            if self.selected == clicked_idx {
                self.open_selected();
            } else {
                self.selected = clicked_idx;
            }
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
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Reserve 2 lines for separator + title
        self.view_height = area.height.saturating_sub(2) as usize;

        let [sep_area, title_area, content_area] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(0),
        ])
        .areas(area);

        // Separator line
        let separator = Line::from(
            "─".repeat(area.width as usize)
                .fg(self.ctx.color_theme.divider_fg),
        );
        f.render_widget(Paragraph::new(separator), sep_area);

        // Title line
        let title = Line::from(vec![
            Span::styled(
                format!("─── File History: {} ", self.file_path),
                ratatui::style::Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({} commits)", self.entries.len()),
                ratatui::style::Style::default().fg(self.ctx.color_theme.list_hash_fg),
            ),
        ]);
        f.render_widget(Paragraph::new(title), title_area);

        self.scroll_to_selected();
        let visible_height = content_area.height as usize;

        if self.entries.is_empty() {
            let no_history = Line::from(Span::styled(
                " No history found for this file.",
                ratatui::style::Style::default().fg(self.ctx.color_theme.status_warn_fg),
            ));
            f.render_widget(Paragraph::new(no_history), content_area);
            return;
        }

        let max_subject = area.width.saturating_sub(30) as usize;

        let lines: Vec<Line> = self
            .entries
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(visible_height)
            .map(|(i, entry)| {
                let is_selected = i == self.selected;
                let bg = if is_selected {
                    self.ctx.color_theme.list_selected_bg
                } else {
                    self.ctx.color_theme.bg
                };
                let hash_style = ratatui::style::Style::default()
                    .fg(self.ctx.color_theme.list_hash_fg)
                    .bg(bg);
                let subject_style = ratatui::style::Style::default()
                    .fg(self.ctx.color_theme.list_commit_message_fg)
                    .bg(bg);
                let meta_style = ratatui::style::Style::default()
                    .fg(self.ctx.color_theme.detail_label_fg)
                    .bg(bg);

                let prefix = if is_selected { "▶ " } else { "  " };
                let subject = if entry.subject.chars().count() > max_subject {
                    format!(
                        "{}…",
                        &entry
                            .subject
                            .chars()
                            .take(max_subject.saturating_sub(1))
                            .collect::<String>()
                    )
                } else {
                    entry.subject.clone()
                };

                Line::from(vec![
                    Span::styled(prefix.to_string(), subject_style),
                    Span::styled(format!("{} ", entry.short_hash), hash_style),
                    Span::styled(
                        format!("{:<width$} ", subject, width = max_subject),
                        subject_style,
                    ),
                    Span::styled(
                        format!("{} ({})", entry.author, entry.date),
                        meta_style,
                    ),
                ])
            })
            .collect();

        f.render_widget(Paragraph::new(lines), content_area);
    }

    pub fn update_layout(&mut self, _area: Rect) {}

    fn move_down(&mut self) {
        if self.selected + 1 < self.entries.len() {
            self.selected += 1;
            self.scroll_to_selected();
        }
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
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
