use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
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
    pub tip_commit_message: String,
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
        let title_text = "Branch Details";
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
            .block(Block::default().padding(Padding::left(2)));
        paragraph.render(area, buf);
    }

    fn render_value_paragraph(&self, lines: Vec<Line>, area: Rect, buf: &mut Buffer) {
        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(self.ctx.color_theme.fg))
            .block(Block::default().padding(Padding::new(1, 2, 0, 0)));
        paragraph.render(area, buf);
    }

    fn render_action_bar(&self, area: Rect, buf: &mut Buffer, state: &BranchDetailState) {
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

        let actions = if self.metadata.is_remote {
            REMOTE_BRANCH_ACTIONS
        } else {
            LOCAL_BRANCH_ACTIONS
        };

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
                Span::styled(label.to_string(), style),
                Span::styled(format!(" ({})", key), key_style),
            ]));
        }

        let paragraph = Paragraph::new(lines).style(Style::default().fg(self.ctx.color_theme.fg));
        paragraph.render(action_inner, buf);
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
            self.metadata.tip_hash, self.metadata.tip_commit_message
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
