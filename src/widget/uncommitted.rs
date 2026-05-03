use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, StatefulWidget, Widget},
};

use crate::{app::AppContext, git::status::StatusType};

#[derive(Debug, Clone)]
pub struct UncommittedFile {
    pub status: StatusType,
    pub path: String,
    pub old_path: Option<String>,
    pub additions: usize,
    pub deletions: usize,
}

impl UncommittedFile {
    pub fn status_char(&self) -> &'static str {
        match self.status {
            StatusType::Added => "A",
            StatusType::Modified => "M",
            StatusType::Deleted => "D",
            StatusType::Renamed => "R",
            StatusType::Copied => "C",
            StatusType::Unmerged => "U",
            StatusType::Untracked => "??",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum UncommittedSection {
    #[default]
    Unstaged,
    Staged,
    Untracked,
}

#[derive(Debug, Default)]
pub struct UncommittedState {
    pub section: UncommittedSection,
    pub selected: usize,
    pub offset: usize,
    pub height: usize,
    pub hovered_action: Option<usize>,
}

impl UncommittedState {
    pub fn select_next(&mut self, total: usize) {
        if self.selected < total.saturating_sub(1) {
            self.selected += 1;
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn switch_section(&mut self) {
        self.section = match self.section {
            UncommittedSection::Unstaged => UncommittedSection::Staged,
            UncommittedSection::Staged => UncommittedSection::Untracked,
            UncommittedSection::Untracked => UncommittedSection::Unstaged,
        };
        self.selected = 0;
        self.offset = 0;
    }

    pub fn selected_file<'a>(
        &self,
        unstaged: &'a [UncommittedFile],
        staged: &'a [UncommittedFile],
        untracked: &'a [UncommittedFile],
    ) -> Option<&'a UncommittedFile> {
        match self.section {
            UncommittedSection::Unstaged => unstaged.get(self.selected),
            UncommittedSection::Staged => staged.get(self.selected),
            UncommittedSection::Untracked => untracked.get(self.selected),
        }
    }

    pub fn total_in_section(&self, unstaged_len: usize, staged_len: usize, untracked_len: usize) -> usize {
        match self.section {
            UncommittedSection::Unstaged => unstaged_len,
            UncommittedSection::Staged => staged_len,
            UncommittedSection::Untracked => untracked_len,
        }
    }
}

pub struct UncommittedWidget<'a> {
    unstaged: &'a [UncommittedFile],
    staged: &'a [UncommittedFile],
    untracked: &'a [UncommittedFile],
    ctx: Rc<AppContext>,
}

impl<'a> UncommittedWidget<'a> {
    pub fn new(
        unstaged: &'a [UncommittedFile],
        staged: &'a [UncommittedFile],
        untracked: &'a [UncommittedFile],
        ctx: Rc<AppContext>,
    ) -> Self {
        Self {
            unstaged,
            staged,
            untracked,
            ctx,
        }
    }
}

impl<'a> StatefulWidget for UncommittedWidget<'a> {
    type State = UncommittedState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let [files_area, action_bar_area] = Layout::horizontal([
            Constraint::Percentage(60),
            Constraint::Percentage(40),
        ])
        .areas(area);

        self.render_files(files_area, buf, state);
        self.render_action_bar(action_bar_area, buf, state);
    }
}

impl<'a> UncommittedWidget<'a> {
    fn render_files(&self, area: Rect, buf: &mut Buffer, state: &UncommittedState) {
        let block = Block::default()
            .borders(Borders::TOP)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg))
            .padding(Padding::new(1, 1, 0, 0));
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines = Vec::new();

        // Title
        lines.push(Line::from(vec![Span::styled(
            "Uncommitted Changes",
            Style::default().add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from(""));

        // Unstaged section
        lines.push(Line::from(vec![Span::styled(
            "-- Unstaged --",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(self.ctx.color_theme.divider_fg),
        )]));
        if self.unstaged.is_empty() {
            lines.push(Line::from("  (no unstaged changes)").fg(Color::DarkGray));
        } else {
            for (i, file) in self.unstaged.iter().enumerate() {
                let is_selected =
                    state.section == UncommittedSection::Unstaged && i == state.selected;
                let style = if is_selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                lines.push(self.file_line(file, style));
            }
        }
        lines.push(Line::from(""));

        // Staged section
        lines.push(Line::from(vec![Span::styled(
            "-- Staged --",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(self.ctx.color_theme.divider_fg),
        )]));
        if self.staged.is_empty() {
            lines.push(Line::from("  (no staged changes)").fg(Color::DarkGray));
        } else {
            for (i, file) in self.staged.iter().enumerate() {
                let is_selected =
                    state.section == UncommittedSection::Staged && i == state.selected;
                let style = if is_selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                lines.push(self.file_line(file, style));
            }
        }
        lines.push(Line::from(""));

        // Untracked section
        lines.push(Line::from(vec![Span::styled(
            "-- Untracked --",
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(self.ctx.color_theme.divider_fg),
        )]));
        if self.untracked.is_empty() {
            lines.push(Line::from("  (no untracked files)").fg(Color::DarkGray));
        } else {
            for (i, file) in self.untracked.iter().enumerate() {
                let is_selected =
                    state.section == UncommittedSection::Untracked && i == state.selected;
                let style = if is_selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                lines.push(self.file_line(file, style));
            }
        }

        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(self.ctx.color_theme.fg));
        paragraph.render(inner, buf);
    }

    fn file_line(&self, file: &UncommittedFile, style: Style) -> Line {
        let status_color = match file.status {
            StatusType::Added => self.ctx.color_theme.detail_file_change_add_fg,
            StatusType::Modified => self.ctx.color_theme.detail_file_change_modify_fg,
            StatusType::Deleted => self.ctx.color_theme.detail_file_change_delete_fg,
            _ => self.ctx.color_theme.detail_file_change_move_fg,
        };
        let add_str = if file.additions > 0 {
            format!(" +{}", file.additions)
        } else {
            "".to_string()
        };
        let del_str = if file.deletions > 0 {
            format!(" -{}", file.deletions)
        } else {
            "".to_string()
        };
        Line::from(vec![
            Span::styled(
                format!("  {:2}", file.status_char()),
                Style::default().fg(status_color),
            ),
            Span::styled(" ", style),
            Span::styled(file.path.clone(), style),
            Span::styled(
                add_str,
                Style::default().fg(self.ctx.color_theme.detail_file_change_add_fg),
            ),
            Span::styled(
                del_str,
                Style::default().fg(self.ctx.color_theme.detail_file_change_delete_fg),
            ),
        ])
    }

    fn render_action_bar(&self, area: Rect, buf: &mut Buffer, state: &UncommittedState) {
        let block = Block::default()
            .borders(Borders::TOP | Borders::LEFT)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg))
            .padding(Padding::new(1, 1, 0, 0));
        let inner = block.inner(area);
        block.render(area, buf);

        let actions = &[("Stash", 'i'), ("Commit", 'w'), ("Clean Untracked", 'v')];

        let mut lines = Vec::new();
        lines.push(Line::from("Git Actions").add_modifier(Modifier::BOLD));
        lines.push(Line::from("───".fg(self.ctx.color_theme.divider_fg)));
        for (i, (label, key)) in actions.iter().enumerate() {
            let is_hovered = state.hovered_action == Some(i);
            let style = if is_hovered {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let key_style = style.add_modifier(Modifier::BOLD);
            lines.push(Line::from(vec![
                Span::styled(format!("{}", label), style),
                Span::styled(format!(" ({})", key), key_style),
            ]));
        }

        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(self.ctx.color_theme.fg));
        paragraph.render(inner, buf);
    }
}
