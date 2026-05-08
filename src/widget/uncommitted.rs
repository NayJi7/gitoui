use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, StatefulWidget, Widget},
};

use crate::{app::AppContext, git::status::StatusType};

#[derive(Debug, Clone)]
pub struct UncommittedFile {
    pub status: StatusType,
    pub path: String,
    #[allow(dead_code)]
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

#[allow(dead_code)]
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

    pub fn select_next_global(
        &mut self,
        staged_len: usize,
        unstaged_len: usize,
        untracked_len: usize,
    ) {
        // Order: Staged -> Unstaged -> Untracked, skipping empty sections
        match self.section {
            UncommittedSection::Staged => {
                if self.selected + 1 < staged_len {
                    self.selected += 1;
                } else if unstaged_len > 0 {
                    self.section = UncommittedSection::Unstaged;
                    self.selected = 0;
                } else if untracked_len > 0 {
                    self.section = UncommittedSection::Untracked;
                    self.selected = 0;
                }
            }
            UncommittedSection::Unstaged => {
                if self.selected + 1 < unstaged_len {
                    self.selected += 1;
                } else if untracked_len > 0 {
                    self.section = UncommittedSection::Untracked;
                    self.selected = 0;
                }
            }
            UncommittedSection::Untracked => {
                if self.selected + 1 < untracked_len {
                    self.selected += 1;
                }
            }
        }
    }

    pub fn select_prev_global(
        &mut self,
        staged_len: usize,
        unstaged_len: usize,
        _untracked_len: usize,
    ) {
        // Order: Staged -> Unstaged -> Untracked, skipping empty sections
        match self.section {
            UncommittedSection::Untracked => {
                if self.selected > 0 {
                    self.selected -= 1;
                } else if unstaged_len > 0 {
                    self.section = UncommittedSection::Unstaged;
                    self.selected = unstaged_len - 1;
                } else if staged_len > 0 {
                    self.section = UncommittedSection::Staged;
                    self.selected = staged_len - 1;
                }
            }
            UncommittedSection::Unstaged => {
                if self.selected > 0 {
                    self.selected -= 1;
                } else if staged_len > 0 {
                    self.section = UncommittedSection::Staged;
                    self.selected = staged_len - 1;
                }
            }
            UncommittedSection::Staged => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
        }
    }

    pub fn switch_section_forward(
        &mut self,
        staged_len: usize,
        unstaged_len: usize,
        untracked_len: usize,
    ) {
        // Cycle: Staged -> Unstaged -> Untracked -> Staged, skipping empty
        let (next_section, _) = match self.section {
            UncommittedSection::Staged => {
                if unstaged_len > 0 {
                    (UncommittedSection::Unstaged, unstaged_len)
                } else if untracked_len > 0 {
                    (UncommittedSection::Untracked, untracked_len)
                } else {
                    (UncommittedSection::Staged, staged_len)
                }
            }
            UncommittedSection::Unstaged => {
                if untracked_len > 0 {
                    (UncommittedSection::Untracked, untracked_len)
                } else if staged_len > 0 {
                    (UncommittedSection::Staged, staged_len)
                } else {
                    (UncommittedSection::Unstaged, unstaged_len)
                }
            }
            UncommittedSection::Untracked => {
                if staged_len > 0 {
                    (UncommittedSection::Staged, staged_len)
                } else if unstaged_len > 0 {
                    (UncommittedSection::Unstaged, unstaged_len)
                } else {
                    (UncommittedSection::Untracked, untracked_len)
                }
            }
        };
        self.section = next_section;
        self.selected = 0;
        self.offset = 0;
    }

    pub fn switch_section_backward(
        &mut self,
        staged_len: usize,
        unstaged_len: usize,
        untracked_len: usize,
    ) {
        // Cycle: Staged -> Untracked -> Unstaged -> Staged, skipping empty
        let (next_section, _) = match self.section {
            UncommittedSection::Staged => {
                if untracked_len > 0 {
                    (UncommittedSection::Untracked, untracked_len)
                } else if unstaged_len > 0 {
                    (UncommittedSection::Unstaged, unstaged_len)
                } else {
                    (UncommittedSection::Staged, staged_len)
                }
            }
            UncommittedSection::Unstaged => {
                if staged_len > 0 {
                    (UncommittedSection::Staged, staged_len)
                } else if untracked_len > 0 {
                    (UncommittedSection::Untracked, untracked_len)
                } else {
                    (UncommittedSection::Unstaged, unstaged_len)
                }
            }
            UncommittedSection::Untracked => {
                if unstaged_len > 0 {
                    (UncommittedSection::Unstaged, unstaged_len)
                } else if staged_len > 0 {
                    (UncommittedSection::Staged, staged_len)
                } else {
                    (UncommittedSection::Untracked, untracked_len)
                }
            }
        };
        self.section = next_section;
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

    #[allow(dead_code)]
    pub fn total_in_section(
        &self,
        unstaged_len: usize,
        staged_len: usize,
        untracked_len: usize,
    ) -> usize {
        match self.section {
            UncommittedSection::Unstaged => unstaged_len,
            UncommittedSection::Staged => staged_len,
            UncommittedSection::Untracked => untracked_len,
        }
    }

    pub fn selected_global_line(
        &self,
        staged_len: usize,
        unstaged_len: usize,
        _untracked_len: usize,
    ) -> usize {
        let staged_rows = if staged_len == 0 { 1 } else { staged_len };
        let unstaged_rows = if unstaged_len == 0 { 1 } else { unstaged_len };
        match self.section {
            UncommittedSection::Staged => self.selected,
            UncommittedSection::Unstaged => staged_rows + 1 + self.selected,
            UncommittedSection::Untracked => staged_rows + 1 + unstaged_rows + 1 + self.selected,
        }
    }

    pub fn ensure_selected_visible(
        &mut self,
        staged_len: usize,
        unstaged_len: usize,
        untracked_len: usize,
    ) {
        let selected_line = self.selected_global_line(staged_len, unstaged_len, untracked_len);
        if selected_line < self.offset {
            self.offset = selected_line;
        } else if selected_line >= self.offset + self.height {
            self.offset = selected_line.saturating_sub(self.height.saturating_sub(1));
        }
    }

    pub fn update_state(&mut self, total_lines: usize, area_height: usize) {
        self.height = area_height;
        self.offset = self.offset.min(total_lines.saturating_sub(area_height));
    }

    pub fn scroll_down(&mut self) {
        self.offset = self.offset.saturating_add(1);
    }

    pub fn scroll_up(&mut self) {
        self.offset = self.offset.saturating_sub(1);
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
        let [files_area, action_bar_area] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(area);

        self.render_files(files_area, buf, state);
        self.render_action_bar(action_bar_area, buf, state);
    }
}

impl<'a> UncommittedWidget<'a> {
    fn render_files(&self, area: Rect, buf: &mut Buffer, state: &mut UncommittedState) {
        // Files area: top border forms the horizontal separator
        let files_block = Block::default()
            .borders(Borders::TOP)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg));
        let files_inner = files_block.inner(area);
        files_block.render(area, buf);

        // Inner: title + underline + spacer + scrollable content
        let [files_title_area, files_underline_area, _files_spacer_area, scroll_area] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(files_inner);

        // Render centered title
        let title_text = "Uncommitted Details";
        let title_len = title_text.chars().count() as u16;
        let title_pad = files_title_area.width.saturating_sub(title_len);
        let title_left = title_pad / 2;
        let title_line = Line::from(vec![
            Span::styled(" ".repeat(title_left as usize), Style::default()),
            Span::styled(
                title_text.to_string(),
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        Paragraph::new(title_line).render(files_title_area, buf);

        // Render small underline
        let underline_len = (title_len as usize).saturating_sub(4).max(3);
        let underline_pad = files_underline_area
            .width
            .saturating_sub(underline_len as u16);
        let underline_left = underline_pad / 2;
        let underline_line = Line::from(vec![
            Span::styled(" ".repeat(underline_left as usize), Style::default()),
            Span::styled(
                "─".repeat(underline_len),
                Style::default().fg(self.ctx.color_theme.divider_fg),
            ),
        ]);
        Paragraph::new(underline_line).render(files_underline_area, buf);

        let [labels_area, value_area] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)]).areas(scroll_area);

        let (label_lines, value_lines) = self.build_content_lines(state, value_area.width as usize);

        let total_lines = label_lines.len().max(value_lines.len());
        let content_height = scroll_area.height as usize;
        state.update_state(total_lines, content_height);
        state.ensure_selected_visible(self.staged.len(), self.unstaged.len(), self.untracked.len());

        let label_lines: Vec<_> = label_lines.into_iter().skip(state.offset).collect();
        let value_lines: Vec<_> = value_lines.into_iter().skip(state.offset).collect();

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
            (
                "Staged",
                "(no staged changes)",
                &self.staged,
                UncommittedSection::Staged,
            ),
            (
                "Unstaged",
                "(no unstaged changes)",
                &self.unstaged,
                UncommittedSection::Unstaged,
            ),
            (
                "Untracked",
                "(no untracked files)",
                &self.untracked,
                UncommittedSection::Untracked,
            ),
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
            Style::default()
                .fg(self.ctx.color_theme.fg)
                .add_modifier(Modifier::BOLD),
        )));

        if files.is_empty() {
            values.push(Line::from(Span::styled(
                empty_msg.to_string(),
                Style::default().fg(self.ctx.color_theme.detail_label_fg),
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
            StatusType::Deleted | StatusType::Unmerged => {
                self.ctx.color_theme.detail_file_change_delete_fg
            }
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
        // Action bar with top+left borders forming a corner with the content border
        let action_block = Block::default()
            .borders(Borders::TOP | Borders::LEFT)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg));
        let inner = action_block.inner(area);
        action_block.render(area, buf);

        // Inner: title + underline + spacer + actions
        let [action_title_area, action_underline_area, _action_spacer_area, action_actions_area] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(inner);

        // Render centered title
        let title_text = "Git Actions";
        let title_len = title_text.chars().count() as u16;
        let title_pad = action_title_area.width.saturating_sub(title_len);
        let title_left = title_pad / 2;
        let title_line = Line::from(vec![
            Span::styled(" ".repeat(title_left as usize), Style::default()),
            Span::styled(
                title_text.to_string(),
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        Paragraph::new(title_line).render(action_title_area, buf);

        // Render small underline
        let underline_len = (title_len as usize).saturating_sub(4).max(3);
        let underline_pad = action_underline_area
            .width
            .saturating_sub(underline_len as u16);
        let underline_left = underline_pad / 2;
        let underline_line = Line::from(vec![
            Span::styled(" ".repeat(underline_left as usize), Style::default()),
            Span::styled(
                "─".repeat(underline_len),
                Style::default().fg(self.ctx.color_theme.divider_fg),
            ),
        ]);
        Paragraph::new(underline_line).render(action_underline_area, buf);

        // Action content padding matching left column
        let action_block = Block::default().padding(Padding::new(2, 1, 0, 0));
        let action_inner = action_block.inner(action_actions_area);
        action_block.render(action_actions_area, buf);

        let actions = &[("Stash", 'i'), ("Commit", 'w'), ("Clean Untracked", 'v')];

        let mut lines = Vec::new();
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

        let paragraph = Paragraph::new(lines).style(Style::default().fg(self.ctx.color_theme.fg));
        paragraph.render(action_inner, buf);
    }
}
