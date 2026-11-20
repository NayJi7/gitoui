use std::rc::Rc;

use chrono::DateTime;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, StatefulWidget, Widget},
};

use crate::app::AppContext;
use crate::text_fit::LabelledRows;

#[derive(Debug, Default)]
pub struct BranchDetailState {
    pub hovered_action: Option<usize>,
    height: usize,
    offset: usize,
}

impl BranchDetailState {
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

pub const LOCAL_BRANCH_ACTIONS: &[(&str, crate::event::UserEvent)] = &[
    ("Checkout", crate::event::UserEvent::Checkout),
    ("Rename", crate::event::UserEvent::RenameBranch),
    ("Delete", crate::event::UserEvent::DeleteBranch),
    ("Merge into current", crate::event::UserEvent::Merge),
    ("Rebase current on", crate::event::UserEvent::Rebase),
    ("Push", crate::event::UserEvent::PushBranch),
    ("Set Upstream", crate::event::UserEvent::SetUpstream),
    ("Create Archive", crate::event::UserEvent::CreateArchive),
    ("Unselect", crate::event::UserEvent::UnselectBranch),
    ("Copy Name", crate::event::UserEvent::CopyBranchName),
];

pub const REMOTE_BRANCH_ACTIONS: &[(&str, crate::event::UserEvent)] = &[
    ("Checkout", crate::event::UserEvent::Checkout),
    ("Delete Remote", crate::event::UserEvent::DeleteBranch),
    ("Merge into current", crate::event::UserEvent::Merge),
    ("Pull into current", crate::event::UserEvent::Pull),
    ("Create Archive", crate::event::UserEvent::CreateArchive),
    ("Unselect", crate::event::UserEvent::UnselectBranch),
    ("Copy Name", crate::event::UserEvent::CopyBranchName),
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
    pub comparison_label: String,
}

pub struct BranchDetail<'a> {
    metadata: &'a BranchMetadata,
    ctx: Rc<AppContext>,
    /// The color the commit list and the refs panel give this branch (its
    /// graph lane); without it the name uses the detail name color.
    branch_color: Option<ratatui::style::Color>,
}

impl<'a> BranchDetail<'a> {
    pub fn new(metadata: &'a BranchMetadata, ctx: Rc<AppContext>) -> Self {
        Self {
            metadata,
            ctx,
            branch_color: None,
        }
    }

    pub fn with_branch_color(mut self, color: Option<ratatui::style::Color>) -> Self {
        self.branch_color = color;
        self
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

        let (mut label_lines, mut value_lines, author) = self.contents(value_area.width);

        let content_height = meta_scroll_area.height as usize;
        state.update(value_lines.len(), content_height);

        label_lines = label_lines.into_iter().skip(state.offset).collect();
        value_lines = value_lines.into_iter().skip(state.offset).collect();

        self.render_labels_paragraph(label_lines, labels_area, buf);
        self.render_value_paragraph(value_lines, value_area, buf);
        if let Some((row, email)) = author {
            self.render_author_avatar(row, &email, state.offset, value_area, buf);
        }
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
        for (i, (label, event)) in actions.iter().enumerate() {
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
            // One row per action: a label too long for the panel ends with `…`.
            lines.push(Line::from(crate::text_fit::ellipsize_spans(
                spans,
                action_inner.width as usize,
            )));
        }

        let paragraph = Paragraph::new(lines).style(Style::default().fg(self.ctx.color_theme.fg));
        paragraph.render(action_inner, buf);
    }

    /// The label and value rows, plus the content row and the email of the
    /// author line when an avatar goes in front of it.
    #[allow(clippy::type_complexity)]
    fn contents(&self, width: u16) -> (Vec<Line<'_>>, Vec<Line<'_>>, Option<(usize, String)>) {
        let theme = &self.ctx.color_theme;
        let mut rows = LabelledRows::new(width, theme.detail_label_fg);

        // Branch name
        rows.row(
            "   Branch: ",
            Line::from(Span::styled(
                self.metadata.branch_name.clone(),
                Style::default()
                    .fg(self.branch_color.unwrap_or(theme.detail_name_fg))
                    .add_modifier(Modifier::BOLD),
            )),
        );

        // Type
        let type_str = if self.metadata.is_remote {
            "Remote Tracking Branch"
        } else {
            "Local Branch"
        };
        rows.row("     Type: ", Line::from(type_str));

        // Tip commit: hash + subject
        let tip = if self.metadata.tip_hash.is_empty() {
            Line::from("(unknown)")
        } else {
            Line::from(vec![
                Span::styled(
                    self.metadata.tip_hash.clone(),
                    Style::default().fg(theme.detail_hash_fg),
                ),
                Span::raw(" "),
                Span::raw(self.metadata.tip_commit_message.clone()),
            ])
        };
        rows.row("      Tip: ", tip);

        // Author of tip commit, drawn like the author of a commit: avatar,
        // name and `<email>` each in their own color.
        let mut author = None;
        if !self.metadata.tip_author.is_empty() {
            let (name, email) = split_author(&self.metadata.tip_author);
            let avatar_enabled = self.ctx.avatar_manager.lock().unwrap().is_enabled();
            let mut spans = Vec::new();
            if avatar_enabled && !email.is_empty() {
                // 2 cells of avatar and 1 of gap before the name.
                spans.push(Span::raw("   "));
                author = Some((rows.values.len(), email.to_string()));
            }
            spans.push(Span::styled(
                name.to_string(),
                Style::default().fg(theme.detail_name_fg),
            ));
            if !email.is_empty() {
                spans.push(Span::raw(" <"));
                spans.push(Span::styled(
                    email.to_string(),
                    Style::default().fg(theme.detail_email_fg),
                ));
                spans.push(Span::raw(">"));
            }
            rows.row("   Author: ", Line::from(spans));
        }

        // Date of tip commit, formatted with the user's configured format
        if !self.metadata.tip_date.is_empty() {
            let formatted =
                DateTime::parse_from_str(self.metadata.tip_date.trim(), "%Y-%m-%d %H:%M:%S %z")
                    .map(|dt| {
                        self.ctx
                            .core_config
                            .date_time_format()
                            .format(&dt, self.ctx.core_config.date_time_local())
                    })
                    .unwrap_or_else(|_| self.metadata.tip_date.clone());
            rows.row(
                "     Date: ",
                Line::from(Span::styled(
                    formatted,
                    Style::default().fg(theme.detail_date_fg),
                )),
            );
        }

        if let Some(upstream) = &self.metadata.upstream {
            rows.row("   Remote: ", Line::from(upstream.clone()));
        }

        let ahead_zero = self.metadata.ahead.is_empty() || self.metadata.ahead == "0";
        let behind_zero = self.metadata.behind.is_empty() || self.metadata.behind == "0";

        if !self.metadata.comparison_label.is_empty() || self.metadata.upstream.is_some() {
            let status = if ahead_zero && behind_zero {
                Line::from(Span::styled(
                    "\u{2713} up to date",
                    Style::default().fg(theme.status_success_fg),
                ))
            } else {
                let ahead_num = if ahead_zero {
                    "0"
                } else {
                    self.metadata.ahead.as_str()
                };
                let behind_num = if behind_zero {
                    "0"
                } else {
                    self.metadata.behind.as_str()
                };
                Line::from(vec![
                    Span::styled(
                        format!("\u{2191} {}", ahead_num),
                        Style::default().fg(theme.detail_file_change_add_fg),
                    ),
                    Span::raw(" \u{00b7} "),
                    Span::styled(
                        format!("\u{2193} {}", behind_num),
                        Style::default().fg(theme.detail_file_change_delete_fg),
                    ),
                ])
            };
            rows.row("   Status: ", status);

            if !self.metadata.comparison_label.is_empty() {
                rows.row(
                    "       vs: ",
                    Line::from(self.metadata.comparison_label.clone()),
                );
            }
        }

        (rows.labels, rows.values, author)
    }

    /// Draws the avatar of the tip's author in front of the author name, on
    /// the row `row` of the content (`offset` rows are scrolled out of view).
    fn render_author_avatar(
        &self,
        row: usize,
        email: &str,
        offset: usize,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if row < offset || area.width <= 5 {
            return;
        }
        let y = area.top() as usize + (row - offset);
        if y >= area.bottom() as usize {
            return;
        }
        let mut avatar_manager = self.ctx.avatar_manager.lock().unwrap();
        if !avatar_manager.is_enabled() {
            return;
        }
        avatar_manager.prefetch(vec![self.metadata.tip_hash.clone()], email);
        avatar_manager.ensure_uploaded(email, 1, false, self.ctx.color_theme.bg);
        if let Some(prepared) = avatar_manager.prepared_image(email, 1, false) {
            for (x, image_cell) in prepared.cells().iter().enumerate() {
                let cell = &mut buf[(area.left() + x as u16 + 1, y as u16)];
                cell.set_symbol(image_cell.symbol());
                cell.set_style(image_cell.style());
                cell.set_skip(image_cell.skip());
            }
        }
    }
}

/// Splits `Name <email>` (git's `%an <%ae>`) into its two parts. Without an
/// email the whole text is the name.
pub(crate) fn split_author(author: &str) -> (&str, &str) {
    match author.rsplit_once(" <") {
        Some((name, rest)) => (name, rest.strip_suffix('>').unwrap_or(rest)),
        None => (author, ""),
    }
}

#[cfg(test)]
mod author_tests {
    use super::split_author;

    #[test]
    fn the_author_is_split_into_name_and_email() {
        assert_eq!(split_author("NayJi7 <a@b.com>"), ("NayJi7", "a@b.com"));
        assert_eq!(
            split_author("Jane <Doe> <j@d.org>"),
            ("Jane <Doe>", "j@d.org")
        );
        assert_eq!(split_author("Just a name"), ("Just a name", ""));
    }
}
