use std::rc::Rc;

use chrono::DateTime;
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
    height: usize,
    offset: usize,
}

impl TagDetailState {
    pub fn scroll_down(&mut self) {
        self.offset = self.offset.saturating_add(1);
    }

    pub fn scroll_up(&mut self) {
        self.offset = self.offset.saturating_sub(1);
    }

    pub fn scroll_page_down(&mut self) {
        self.offset = self.offset.saturating_add(self.height);
    }

    pub fn scroll_page_up(&mut self) {
        self.offset = self.offset.saturating_sub(self.height);
    }

    pub fn scroll_half_page_down(&mut self) {
        self.offset = self.offset.saturating_add(self.height / 2);
    }

    pub fn scroll_half_page_up(&mut self) {
        self.offset = self.offset.saturating_sub(self.height / 2);
    }

    pub fn select_first(&mut self) {
        self.offset = 0;
    }

    pub fn select_last(&mut self) {
        self.offset = usize::MAX;
    }

    fn update(&mut self, line_count: usize, area_height: usize) {
        self.height = area_height;
        self.offset = self.offset.min(line_count.saturating_sub(area_height));
    }
}

pub const TAG_ACTIONS: &[(&str, crate::event::UserEvent)] = &[
    ("Push Tag", crate::event::UserEvent::PushTag),
    ("Delete Tag", crate::event::UserEvent::DeleteTag),
    ("Copy Name", crate::event::UserEvent::CopyTagName),
];

#[derive(Debug, Clone)]
pub struct TagMetadata {
    pub tag_name: String,
    pub tag_type: String,
    pub target_hash: String,
    pub target_commit_message: String,
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
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(area);

        // Metadata area: top border forms the horizontal separator
        let meta_block = Block::default()
            .borders(Borders::TOP)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg));
        let meta_inner = meta_block.inner(metadata_area);
        meta_block.render(metadata_area, buf);

        // Inner: title + underline + spacer + scrollable content
        let [meta_title_area, meta_underline_area, _meta_spacer_area, meta_scroll_area] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(meta_inner);

        // Render centered title
        let title_text = "Tag Details";
        let title_len = title_text.chars().count() as u16;
        let title_pad = meta_title_area.width.saturating_sub(title_len);
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
        Paragraph::new(title_line).render(meta_title_area, buf);

        // Render small underline
        let underline_len = (title_len as usize).saturating_sub(4).max(3);
        let underline_pad = meta_underline_area
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
        Paragraph::new(underline_line).render(meta_underline_area, buf);

        let [labels_area, value_area] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)])
                .areas(meta_scroll_area);

        let (mut label_lines, mut value_lines) = self.contents(value_area.width);

        let content_height = meta_scroll_area.height as usize;
        state.update(value_lines.len(), content_height);

        label_lines = label_lines.into_iter().skip(state.offset).collect();
        value_lines = value_lines.into_iter().skip(state.offset).collect();

        self.render_labels_paragraph(label_lines, labels_area, buf);
        self.render_value_paragraph(value_lines, value_area, buf);
        self.render_action_bar(action_bar_area, buf, state);
    }
}

impl TagDetail<'_> {
    fn render_labels_paragraph(&self, lines: Vec<Line>, area: Rect, buf: &mut Buffer) {
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(self.ctx.color_theme.fg))
            .block(Block::default().padding(Padding::left(2)));
        paragraph.render(area, buf);
    }

    fn render_value_paragraph(&self, lines: Vec<Line>, area: Rect, buf: &mut Buffer) {
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(self.ctx.color_theme.fg))
            .block(Block::default().padding(Padding::new(1, 2, 0, 0)));
        paragraph.render(area, buf);
    }

    fn render_action_bar(&self, area: Rect, buf: &mut Buffer, state: &TagDetailState) {
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

        let mut lines = Vec::new();
        for (i, (label, event)) in TAG_ACTIONS.iter().enumerate() {
            let is_hovered = state.hovered_action == Some(i);
            let style = if is_hovered {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            let key_style = style.add_modifier(Modifier::BOLD);
            let key = self.ctx.keybind.primary_global_key(*event);
            let mut spans = vec![Span::styled(label.to_string(), style)];
            if !key.is_empty() {
                spans.push(Span::styled(format!(" ({})", key), key_style));
            }
            lines.push(Line::from(spans));
        }

        let paragraph = Paragraph::new(lines).style(Style::default().fg(self.ctx.color_theme.fg));
        paragraph.render(action_inner, buf);
    }

    fn contents(&self, width: u16) -> (Vec<Line<'_>>, Vec<Line<'_>>) {
        let mut label_lines: Vec<Line> = Vec::new();
        let mut value_lines: Vec<Line> = Vec::new();

        // Tag name
        label_lines.push(Line::from("      Tag: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.push(Line::from(Span::styled(
            self.metadata.tag_name.as_str(),
            Style::default()
                .fg(self.ctx.color_theme.detail_name_fg)
                .add_modifier(Modifier::BOLD),
        )));

        // Type (annotated vs lightweight)
        label_lines.push(Line::from("     Type: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.push(Line::from(self.metadata.tag_type.as_str()));

        // Target commit: short hash + subject
        label_lines.push(Line::from("   Target: ").fg(self.ctx.color_theme.detail_label_fg));
        if self.metadata.target_hash.is_empty() {
            value_lines.push(Line::from("(unknown)"));
        } else {
            value_lines.push(Line::from(vec![
                Span::styled(
                    self.metadata.target_hash.as_str(),
                    Style::default().fg(self.ctx.color_theme.detail_hash_fg),
                ),
                Span::raw(" "),
                Span::raw(self.metadata.target_commit_message.as_str()),
            ]));
        }

        // Tagger identity (annotated only)
        if let Some(tagger) = &self.metadata.tagger {
            label_lines.push(Line::from("   Tagger: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(Line::from(Span::styled(
                tagger.as_str(),
                Style::default().fg(self.ctx.color_theme.detail_name_fg),
            )));
        }

        // Date (annotated only), formatted with the user's configured format
        if let Some(date) = &self.metadata.date {
            label_lines.push(Line::from("     Date: ").fg(self.ctx.color_theme.detail_label_fg));
            let formatted = DateTime::parse_from_str(date, "%Y-%m-%d %H:%M:%S %z")
                .map(|dt| {
                    self.ctx
                        .core_config
                        .date_time_format()
                        .format(&dt, self.ctx.core_config.date_time_local())
                })
                .unwrap_or_else(|_| date.clone());
            value_lines.push(Line::from(Span::styled(
                formatted,
                Style::default().fg(self.ctx.color_theme.detail_date_fg),
            )));
        }

        // Divider before message
        if self.metadata.message.is_some() {
            label_lines.push(Line::from(""));
            value_lines.push(self.divider_line(width as usize));
        }

        // Tag message body (annotated only, multi-line supported)
        if let Some(message) = &self.metadata.message {
            let msg_lines: Vec<&str> = message.lines().collect();
            label_lines.push(Line::from("  Message: ").fg(self.ctx.color_theme.detail_label_fg));
            if let Some((first, rest)) = msg_lines.split_first() {
                value_lines.push(Line::from(*first));
                for line in rest {
                    label_lines.push(Line::from(""));
                    value_lines.push(Line::from(*line));
                }
            } else {
                value_lines.push(Line::from(message.as_str()));
            }
        }

        (label_lines, value_lines)
    }

    fn divider_line(&self, width: usize) -> Line<'_> {
        Line::from("\u{2500}".repeat(width).fg(self.ctx.color_theme.divider_fg))
    }
}
