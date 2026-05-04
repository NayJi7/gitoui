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

    pub fn select_next_global(&mut self, unstaged_len: usize, staged_len: usize, untracked_len: usize) {
        let total = unstaged_len + staged_len + untracked_len;
        if total == 0 {
            return;
        }
        let current_global = match self.section {
            UncommittedSection::Unstaged => self.selected,
            UncommittedSection::Staged => unstaged_len + self.selected,
            UncommittedSection::Untracked => unstaged_len + staged_len + self.selected,
        };
        let next_global = (current_global + 1).min(total.saturating_sub(1));
        if next_global < unstaged_len {
            self.section = UncommittedSection::Unstaged;
            self.selected = next_global;
        } else if next_global < unstaged_len + staged_len {
            self.section = UncommittedSection::Staged;
            self.selected = next_global - unstaged_len;
        } else {
            self.section = UncommittedSection::Untracked;
            self.selected = next_global - unstaged_len - staged_len;
        }
    }

    pub fn select_prev_global(&mut self, unstaged_len: usize, staged_len: usize, untracked_len: usize) {
        let total = unstaged_len + staged_len + untracked_len;
        if total == 0 {
            return;
        }
        let current_global = match self.section {
            UncommittedSection::Unstaged => self.selected,
            UncommittedSection::Staged => unstaged_len + self.selected,
            UncommittedSection::Untracked => unstaged_len + staged_len + self.selected,
        };
        let prev_global = current_global.saturating_sub(1);
        if prev_global < unstaged_len {
            self.section = UncommittedSection::Unstaged;
            self.selected = prev_global;
        } else if prev_global < unstaged_len + staged_len {
            self.section = UncommittedSection::Staged;
            self.selected = prev_global - unstaged_len;
        } else {
            self.section = UncommittedSection::Untracked;
            self.selected = prev_global - unstaged_len - staged_len;
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
        // Title block over the entire files area
        let title_block = Block::default()
            .title("Uncommitted Details")
            .title_style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
            .borders(Borders::TOP)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg))
            .padding(Padding::new(0, 0, 1, 0));
        let inner = title_block.inner(area);
        title_block.render(area, buf);

        let [labels_area, value_area] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)]).areas(inner);

        let (label_lines, value_lines) = self.build_content_lines(state, value_area.width as usize);

        let labels_paragraph = Paragraph::new(label_lines)
            .style(Style::default().fg(self.ctx.color_theme.fg))
            .block(Block::default().padding(Padding::left(2)));
        labels_paragraph.render(labels_area, buf);

        let values_paragraph = Paragraph::new(value_lines)
            .style(Style::default().fg(self.ctx.color_theme.fg))
            .block(Block::default().padding(Padding::new(1, 2, 0, 0)));
        values_paragraph.render(value_area, buf);
    }

    fn build_content_lines(
        &self,
        state: &UncommittedState,
        value_width: usize,
    ) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
        let mut labels: Vec<Line<'static>> = Vec::new();
        let mut values: Vec<Line<'static>> = Vec::new();

        // Order: Staged, Unstaged, Untracked
        let sections = [
            ("Staged", "(no staged changes)", &self.staged, UncommittedSection::Staged),
            ("Unstaged", "(no unstaged changes)", &self.unstaged, UncommittedSection::Unstaged),
            ("Untracked", "(no untracked files)", &self.untracked, UncommittedSection::Untracked),
        ];

        for (idx, (title, empty_msg, files, section)) in sections.iter().enumerate() {
            self.build_section(
                state,
                title,
                empty_msg,
                files,
                *section,
                &mut labels,
                &mut values,
            );
            // Add full-width separator between sections (but not after the last one)
            if idx < sections.len() - 1 {
                labels.push(Line::from(""));
                values.push(self.full_divider(value_width));
            }
        }

        (labels, values)
    }

    fn build_section(
        &self,
        state: &UncommittedState,
        title: &str,
        empty_msg: &str,
        files: &[UncommittedFile],
        section: UncommittedSection,
        labels: &mut Vec<Line<'static>>,
        values: &mut Vec<Line<'static>>,
    ) {
        labels.push(Line::from(Span::styled(
            title.to_string(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )));

        if files.is_empty() {
            values.push(Line::from(Span::styled(
                empty_msg.to_string(),
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for (i, file) in files.iter().enumerate() {
                let is_selected = state.section == section && i == state.selected;
                let style = if is_selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                values.push(self.file_line(file, style));
                if i < files.len() - 1 {
                    labels.push(Line::from(""));
                }
            }
        }
    }

    fn file_line(&self, file: &UncommittedFile, style: Style) -> Line<'static> {
        let status_color = match file.status {
            StatusType::Added => self.ctx.color_theme.detail_file_change_add_fg,
            StatusType::Modified => self.ctx.color_theme.detail_file_change_modify_fg,
            StatusType::Deleted | StatusType::Unmerged => self.ctx.color_theme.detail_file_change_delete_fg,
            _ => self.ctx.color_theme.detail_file_change_move_fg,
        };
        let path_style = if matches!(file.status, StatusType::Deleted | StatusType::Unmerged) {
            Style::default().add_modifier(Modifier::CROSSED_OUT)
        } else {
            Style::default()
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
                format!("{:2}", file.status_char()),
                Style::default().fg(status_color),
            ),
            Span::raw(" "),
            Span::styled(file.path.clone(), path_style),
            Span::styled(
                add_str,
                Style::default().fg(self.ctx.color_theme.detail_file_change_add_fg),
            ),
            Span::styled(
                del_str,
                Style::default().fg(self.ctx.color_theme.detail_file_change_delete_fg),
            ),
        ])
        .style(style)
    }

    fn full_divider(&self, width: usize) -> Line<'static> {
        Line::from("─".repeat(width).fg(self.ctx.color_theme.divider_fg))
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
        lines.push(Line::from(Span::styled(
            "Git Actions",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )));
        lines.push(self.full_divider(inner.width as usize));
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
