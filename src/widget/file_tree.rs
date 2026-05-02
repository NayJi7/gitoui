use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, StatefulWidget, Widget, Wrap},
};

use crate::git::FileChange;

pub struct FileTree<'a> {
    changes: &'a [FileChange],
}

pub struct FileTreeState {
    pub selected: usize,
    offset: usize,
    height: usize,
}

impl Default for FileTreeState {
    fn default() -> Self {
        Self {
            selected: 0,
            offset: 0,
            height: 0,
        }
    }
}

impl FileTreeState {
    pub fn select_next(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.adjust_offset();
        }
    }

    pub fn select_prev(&mut self, total: usize) {
        if self.selected < total.saturating_sub(1) {
            self.selected += 1;
            self.adjust_offset();
        }
    }

    fn adjust_offset(&mut self) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.height > 0 && self.selected >= self.offset + self.height {
            self.offset = self.selected.saturating_sub(self.height) + 1;
        }
    }
}

impl<'a> FileTree<'a> {
    pub fn new(changes: &'a [FileChange]) -> Self {
        Self { changes }
    }
}

impl StatefulWidget for FileTree<'_> {
    type State = FileTreeState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        state.height = area.height as usize;

        let items: Vec<Line> = self
            .changes
            .iter()
            .skip(state.offset)
            .enumerate()
            .map(|(i, change)| {
                let is_selected = state.offset + i == state.selected;
                let (status_char, status_color, path) = match change {
                    FileChange::Add { path } => ("A", Color::Rgb(158, 206, 106), path.as_str()),
                    FileChange::Modify { path } => ("M", Color::Rgb(224, 175, 104), path.as_str()),
                    FileChange::Delete { path } => ("D", Color::Rgb(247, 118, 142), path.as_str()),
                    FileChange::Move { to, .. } => ("R", Color::Rgb(125, 207, 255), to.as_str()),
                };

                let bg = if is_selected {
                    Color::Rgb(59, 66, 97)
                } else {
                    Color::Reset
                };

                Line::from(vec![
                    Span::styled(" ", Style::default().bg(bg)),
                    Span::styled(
                        status_char.to_string(),
                        Style::default()
                            .fg(status_color)
                            .add_modifier(Modifier::BOLD)
                            .bg(bg),
                    ),
                    Span::styled(" ", Style::default().bg(bg)),
                    Span::styled(
                        path.to_string(),
                        Style::default().fg(Color::Rgb(192, 202, 245)).bg(bg),
                    ),
                ])
            })
            .collect();

        let paragraph = Paragraph::new(items).wrap(Wrap { trim: false });
        paragraph.render(area, buf);
    }
}
