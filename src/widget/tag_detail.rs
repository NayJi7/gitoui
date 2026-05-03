use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, StatefulWidget, Widget},
};

use crate::app::AppContext;

#[derive(Debug, Default)]
pub struct TagDetailState {
    pub hovered_action: Option<usize>,
}

pub const TAG_ACTIONS: &[(&str, char)] = &[
    ("Push Tag", 'p'),
    ("Delete Tag", 'D'),
    ("Copy Name", 'c'),
];

#[derive(Debug, Clone)]
pub struct TagMetadata {
    pub tag_name: String,
    pub tag_type: String,
    pub target_hash: String,
    pub target_subject: String,
    pub tagger: Option<String>,
    pub date: Option<String>,
    pub message: Option<String>,
}

pub struct TagDetail<'a> {
    metadata: &'a TagMetadata,
    ctx: Rc<AppContext>,
}

impl<'a> TagDetail<'a> {
    pub fn new(metadata: &'a TagMetadata, ctx: Rc<AppContext>) -> Self {
        Self { metadata, ctx }
    }
}

impl StatefulWidget for TagDetail<'_> {
    type State = TagDetailState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let [metadata_area, action_bar_area] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);

        let [labels_area, value_area] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)]).areas(metadata_area);

        let (label_lines, value_lines) = self.contents();

        self.render_labels_paragraph(label_lines, labels_area, buf);
        self.render_value_paragraph(value_lines, value_area, buf);
        self.render_action_bar(action_bar_area, buf, state);
    }
}

impl TagDetail<'_> {
    fn render_labels_paragraph(&self, lines: Vec<Line>, area: Rect, buf: &mut Buffer) {
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(self.ctx.color_theme.fg))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .style(Style::default().fg(self.ctx.color_theme.divider_fg))
                    .padding(Padding::left(2)),
            );
        paragraph.render(area, buf);
    }

    fn render_value_paragraph(&self, lines: Vec<Line>, area: Rect, buf: &mut Buffer) {
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(self.ctx.color_theme.fg))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .style(Style::default().fg(self.ctx.color_theme.divider_fg))
                    .padding(Padding::new(1, 2, 0, 0)),
            );
        paragraph.render(area, buf);
    }

    fn render_action_bar(&self, area: Rect, buf: &mut Buffer, state: &TagDetailState) {
        let block = Block::default()
            .borders(Borders::TOP | Borders::LEFT)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg))
            .padding(Padding::new(1, 1, 0, 0));
        let inner = block.inner(area);
        block.render(area, buf);

        let mut lines = Vec::new();
        lines.push(Line::from("Git Actions").add_modifier(Modifier::BOLD));
        lines.push(Line::from("───".fg(self.ctx.color_theme.divider_fg)));
        for (i, (label, key)) in TAG_ACTIONS.iter().enumerate() {
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
        paragraph.render(inner, buf);
    }

    fn contents(&self) -> (Vec<Line<'_>>, Vec<Line<'_>>) {
        let mut label_lines: Vec<Line> = Vec::new();
        let mut value_lines: Vec<Line> = Vec::new();

        label_lines.push(Line::from("      Tag: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.push(Line::from(self.metadata.tag_name.as_str()));

        label_lines.push(Line::from("     Type: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.push(Line::from(self.metadata.tag_type.as_str()));

        label_lines.push(Line::from("   Target: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.push(Line::from(format!(
            "{} {}",
            self.metadata.target_hash, self.metadata.target_subject
        )));

        if let Some(tagger) = &self.metadata.tagger {
            label_lines.push(Line::from("   Tagger: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(Line::from(tagger.as_str()));
        }

        if let Some(date) = &self.metadata.date {
            label_lines.push(Line::from("     Date: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(Line::from(date.as_str()));
        }

        if let Some(message) = &self.metadata.message {
            label_lines.push(Line::from("  Message: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(Line::from(message.as_str()));
        }

        (label_lines, value_lines)
    }
}
