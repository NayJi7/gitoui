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
pub struct BranchDetailState {
    pub hovered_action: Option<usize>,
}

pub const LOCAL_BRANCH_ACTIONS: &[(&str, &str)] = &[
    ("Checkout", "o"),
    ("Rename", "Ctrl+R"),
    ("Delete", "D"),
    ("Merge into current", "m"),
    ("Rebase current on", "e"),
    ("Push", "Q"),
    ("Create Archive", "E"),
    ("Unselect", "T"),
    ("Copy Name", "V"),
];

pub const REMOTE_BRANCH_ACTIONS: &[(&str, &str)] = &[
    ("Checkout", "o"),
    ("Delete Remote", "D"),
    ("Merge into current", "m"),
    ("Pull into current", "Z"),
    ("Create Archive", "E"),
    ("Unselect", "T"),
    ("Copy Name", "V"),
];

#[derive(Debug, Clone)]
pub struct BranchMetadata {
    pub branch_name: String,
    pub is_remote: bool,
    pub tip_hash: String,
    pub tip_subject: String,
    pub tip_author: String,
    pub tip_date: String,
    pub upstream: Option<String>,
    pub ahead: String,
    pub behind: String,
}

pub struct BranchDetail<'a> {
    metadata: &'a BranchMetadata,
    ctx: Rc<AppContext>,
}

impl<'a> BranchDetail<'a> {
    pub fn new(metadata: &'a BranchMetadata, ctx: Rc<AppContext>) -> Self {
        Self { metadata, ctx }
    }
}

impl StatefulWidget for BranchDetail<'_> {
    type State = BranchDetailState;

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

impl BranchDetail<'_> {
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

    fn render_action_bar(&self, area: Rect, buf: &mut Buffer, state: &BranchDetailState) {
        let block = Block::default()
            .borders(Borders::TOP | Borders::LEFT)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg))
            .padding(Padding::new(1, 1, 0, 0));
        let inner = block.inner(area);
        block.render(area, buf);

        let actions = if self.metadata.is_remote {
            REMOTE_BRANCH_ACTIONS
        } else {
            LOCAL_BRANCH_ACTIONS
        };

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
                Span::styled(label.to_string(), style),
                Span::styled(format!(" ({})", key), key_style),
            ]));
        }

        let paragraph = Paragraph::new(lines).style(Style::default().fg(self.ctx.color_theme.fg));
        paragraph.render(inner, buf);
    }

    fn contents(&self) -> (Vec<Line<'_>>, Vec<Line<'_>>) {
        let mut label_lines: Vec<Line> = Vec::new();
        let mut value_lines: Vec<Line> = Vec::new();

        label_lines.push(Line::from("   Branch: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.push(Line::from(self.metadata.branch_name.as_str()));

        label_lines.push(Line::from("     Type: ").fg(self.ctx.color_theme.detail_label_fg));
        let type_str = if self.metadata.is_remote {
            "Remote Tracking Branch"
        } else {
            "Local Branch"
        };
        value_lines.push(Line::from(type_str));

        label_lines.push(Line::from("      Tip: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.push(Line::from(format!(
            "{} {}",
            self.metadata.tip_hash, self.metadata.tip_subject
        )));

        if !self.metadata.tip_author.is_empty() {
            label_lines.push(Line::from("   Author: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(Line::from(self.metadata.tip_author.as_str()));
        }

        if !self.metadata.tip_date.is_empty() {
            label_lines.push(Line::from("     Date: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(Line::from(self.metadata.tip_date.as_str()));
        }

        if let Some(upstream) = &self.metadata.upstream {
            label_lines.push(Line::from("   Remote: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(Line::from(upstream.as_str()));
        }

        if !self.metadata.ahead.is_empty() && self.metadata.ahead != "0" {
            label_lines.push(Line::from("    Ahead: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(Line::from(format!("{} commits", self.metadata.ahead)));
        }

        if !self.metadata.behind.is_empty() && self.metadata.behind != "0" {
            label_lines.push(Line::from("   Behind: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(Line::from(format!("{} commits", self.metadata.behind)));
        }

        (label_lines, value_lines)
    }
}
