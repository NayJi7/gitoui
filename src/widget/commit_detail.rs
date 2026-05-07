use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, StatefulWidget, Widget},
};

use crate::{
    app::AppContext,
    git::{Commit, FileChange, Ref},
};

#[derive(Debug, Default)]
pub struct CommitDetailState {
    height: usize,
    offset: usize,
    pub selected_file: usize,
    pub hovered_action: Option<usize>,
    last_selected_file: usize,
}

impl CommitDetailState {
    pub fn offset(&self) -> usize {
        self.offset
    }

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
        self.selected_file = 0;
    }

    pub fn select_first_file(&mut self) {
        self.selected_file = 0;
    }

    pub fn select_last(&mut self) {
        self.offset = usize::MAX;
    }

    pub fn select_last_file(&mut self, total: usize) {
        if total > 0 {
            self.selected_file = total - 1;
        }
    }

    pub fn select_next_file(&mut self, total: usize) {
        if total == 0 {
            return;
        }
        if self.selected_file < total.saturating_sub(1) {
            self.selected_file += 1;
        }
    }

    pub fn select_prev_file(&mut self) {
        if self.selected_file > 0 {
            self.selected_file -= 1;
        }
    }

    fn ensure_selected_visible(&mut self, changes_start: usize) {
        if self.selected_file != self.last_selected_file {
            self.last_selected_file = self.selected_file;
            let selected_line = changes_start + self.selected_file;
            if selected_line < self.offset {
                self.offset = selected_line;
            } else if selected_line >= self.offset + self.height {
                self.offset = selected_line.saturating_sub(self.height - 1);
            }
        }
    }
}

pub struct CommitDetail<'a> {
    commit: &'a Commit,
    changes: &'a Vec<FileChange>,
    refs: &'a Vec<Ref>,
    ctx: Rc<AppContext>,
    head_branch_name: Option<String>,
}

impl<'a> CommitDetail<'a> {
    pub fn new(
        commit: &'a Commit,
        changes: &'a Vec<FileChange>,
        refs: &'a Vec<Ref>,
        ctx: Rc<AppContext>,
        head_branch_name: Option<String>,
    ) -> Self {
        Self {
            commit,
            changes,
            refs,
            ctx,
            head_branch_name,
        }
    }
}

pub const COMMIT_ACTIONS: &[(&str, &str)] = &[
    ("Add Tag", "t"),
    ("Create Branch", "b"),
    ("Checkout", "o"),
    ("Cherry Pick", "P"),
    ("Revert", "R"),
    ("Drop", "d"),
    ("Merge into current", "m"),
    ("Rebase current on", "e"),
    ("Reset current to", "S"),
];

pub const STASH_ACTIONS: &[(&str, &str)] = &[
    ("Apply Stash", "y"),
    ("Pop Stash", "Ctrl+P"),
    ("Drop Stash", "Ctrl+X"),
    ("Create Branch from Stash", "Ctrl+N"),
    ("Copy Stash Name", "Ctrl+I"),
    ("Copy Stash Hash", "Ctrl+O"),
];

impl StatefulWidget for CommitDetail<'_> {
    type State = CommitDetailState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let [content_area, action_bar_area] =
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)])
                .areas(area);

        // Left panel: top border forms the horizontal separator
        let content_block = Block::default()
            .borders(Borders::TOP)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg));
        let content_inner = content_block.inner(content_area);
        content_block.render(content_area, buf);

        // Content inner: title + underline + fixed author header + spacer + scrollable content
        let [content_title_area, content_underline_area, author_header_area, _content_spacer_area, content_scroll_area] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(content_inner);

        // Render centered title
        let title_text = "Commit Details";
        let title_len = title_text.chars().count() as u16;
        let title_pad = content_title_area.width.saturating_sub(title_len);
        let title_left = title_pad / 2;
        let title_line = Line::from(vec![
            Span::styled(" ".repeat(title_left as usize), Style::default()),
            Span::styled(
                title_text.to_string(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
        Paragraph::new(title_line).render(content_title_area, buf);

        // Render small underline (slightly shorter than title)
        let underline_len = (title_len as usize).saturating_sub(4).max(3);
        let underline_pad = content_underline_area
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
        Paragraph::new(underline_line).render(content_underline_area, buf);

        self.render_author_header(author_header_area, buf);

        let [labels_area, value_area] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)])
                .areas(content_scroll_area);

        let (mut label_lines, mut value_lines, changes_start) = self.contents(value_area.width);

        let content_area_height = content_scroll_area.height as usize;
        self.update_state(state, value_lines.len(), content_area_height);
        state.ensure_selected_visible(changes_start);

        label_lines = label_lines.into_iter().skip(state.offset).collect();
        value_lines = value_lines.into_iter().skip(state.offset).collect();

        // Apply selection highlight to the selected file line (after skipping offset)
        if !self.changes.is_empty() {
            let selected_line = changes_start + state.selected_file;
            let visible_selected = selected_line.saturating_sub(state.offset);
            if visible_selected < value_lines.len() {
                value_lines[visible_selected].style =
                    Style::default().add_modifier(Modifier::REVERSED);
            }
        }

        self.render_labels_paragraph(label_lines, labels_area, buf);
        self.render_value_paragraph(value_lines, value_area, buf);
        self.render_action_bar(action_bar_area, buf, state);
    }
}

impl CommitDetail<'_> {
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

    fn render_author_header(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let avatar_width = self.render_author_avatar(area, buf);
        let text_x = area.left() + avatar_width + 1;
        if text_x >= area.right() {
            return;
        }

        let text_width = area.right().saturating_sub(text_x);
        let name_line = Line::from(vec![
            self.commit
                .author_name
                .as_str()
                .fg(self.ctx.color_theme.detail_name_fg),
            " <".into(),
            self.commit
                .author_email
                .as_str()
                .fg(self.ctx.color_theme.detail_email_fg),
            ">".into(),
        ]);
        Paragraph::new(name_line).render(Rect::new(text_x, area.top(), text_width, 1), buf);

        if area.height > 1 {
            let date_str = self.ctx.core_config.date_time_format().format(
                &self.commit.author_date,
                self.ctx.core_config.date_time_local(),
            );
            let date_line = Line::from(date_str.fg(self.ctx.color_theme.detail_date_fg));
            Paragraph::new(date_line).render(Rect::new(text_x, area.top() + 1, text_width, 1), buf);
        }
    }

    fn render_author_avatar(&self, area: Rect, buf: &mut Buffer) -> u16 {
        if area.width <= 5 || area.height == 0 {
            return 0;
        }
        let mut avatar_manager = self.ctx.avatar_manager.lock().unwrap();
        if !avatar_manager.is_enabled() {
            return 0;
        }
        avatar_manager.prefetch(
            vec![self.commit.commit_hash.as_str().to_string()],
            &self.commit.author_email,
        );
        avatar_manager.ensure_uploaded(
            &self.commit.author_email,
            1,
            false,
            self.ctx.color_theme.bg,
        );
        if let Some(prepared) = avatar_manager.prepared_image(&self.commit.author_email, 1, false) {
            for (x, image_cell) in prepared.cells().iter().enumerate() {
                let cell = &mut buf[(area.left() + x as u16 + 1, area.top())];
                cell.set_symbol(image_cell.symbol());
                cell.set_style(image_cell.style());
                cell.set_skip(image_cell.skip());
            }
            return prepared.cell_width() as u16 + 1;
        }
        0
    }

    fn render_action_bar(&self, area: Rect, buf: &mut Buffer, state: &CommitDetailState) {
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
                    .fg(Color::White)
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
        let action_pad = Block::default().padding(Padding::new(2, 1, 0, 0));
        let action_inner = action_pad.inner(action_actions_area);
        action_pad.render(action_actions_area, buf);

        let actions = if self.is_stash() {
            STASH_ACTIONS
        } else {
            COMMIT_ACTIONS
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

    fn is_stash(&self) -> bool {
        use crate::git::CommitType;
        matches!(self.commit.commit_type, CommitType::Stash)
    }

    fn contents(&self, width: u16) -> (Vec<Line<'_>>, Vec<Line<'_>>, usize) {
        let mut label_lines: Vec<Line> = Vec::new();
        let mut value_lines: Vec<Line> = Vec::new();

        label_lines.push(Line::from("      SHA: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.push(self.sha_line());

        if has_parent(self.commit) {
            let parent_label = if self.commit.parent_commit_hashes.len() == 1 {
                "   Parent: "
            } else {
                "  Parents: "
            };
            label_lines.push(Line::from(parent_label).fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(self.parents_line());
        }

        if has_refs(self.refs) {
            label_lines.push(Line::from("     Refs: ").fg(self.ctx.color_theme.detail_label_fg));
            value_lines.push(self.refs_line());
        }

        // Divider before message
        label_lines.push(Line::from(""));
        value_lines.push(self.divider_line(width as usize));

        label_lines.push(Line::from("  Message: ").fg(self.ctx.color_theme.detail_label_fg));
        value_lines.extend(self.commit_message_lines());

        // Divider before changes
        label_lines.push(Line::from(""));
        value_lines.push(self.divider_line(width as usize));
        value_lines.extend(self.changes_lines());

        let changes_start = value_lines.len() - self.changes.len();

        (label_lines, value_lines, changes_start)
    }

    fn sha_line(&self) -> Line<'_> {
        Line::from(
            self.commit
                .commit_hash
                .as_str()
                .fg(self.ctx.color_theme.detail_hash_fg),
        )
    }

    fn parents_line(&self) -> Line<'_> {
        let mut spans = Vec::new();
        let parents = &self.commit.parent_commit_hashes;
        for (i, hash) in parents
            .iter()
            .map(|hash| hash.as_short_hash().fg(self.ctx.color_theme.detail_hash_fg))
            .enumerate()
        {
            spans.push(hash);
            if i < parents.len() - 1 {
                spans.push(Span::raw(" "));
            }
        }
        Line::from(spans)
    }

    fn refs_line(&self) -> Line<'_> {
        let mut compacted_branches: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        let mut tags: Vec<String> = Vec::new();

        for r in self.refs.iter() {
            match r {
                Ref::Branch { name, .. } => {
                    compacted_branches
                        .entry(name.clone())
                        .or_default()
                        .push(name.clone());
                }
                Ref::RemoteBranch { name, .. } => {
                    if name.ends_with("/HEAD") {
                        continue;
                    }
                    let short_name = name
                        .split_once('/')
                        .map(|(_, rest)| rest.to_string())
                        .unwrap_or_else(|| name.clone());
                    compacted_branches
                        .entry(short_name)
                        .or_default()
                        .push(name.clone());
                }
                Ref::Tag { name, .. } => {
                    tags.push(name.clone());
                }
                _ => {}
            }
        }

        let mut spans = Vec::new();
        let mut first = true;

        // HEAD indicator first, like in commit list
        if let Some(ref head_name) = self.head_branch_name {
            if compacted_branches.contains_key(head_name) {
                let fg = self.ctx.color_theme.list_head_fg;
                spans.push(Span::styled(
                    "಄ ",
                    Style::default().fg(fg).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    "HEAD -> ",
                    Style::default().fg(fg).add_modifier(Modifier::BOLD),
                ));
                first = false;
            }
        }

        for (short_name, full_names) in compacted_branches.iter() {
            let is_head = self.head_branch_name.as_deref() == Some(short_name.as_str());
            if !first {
                spans.push(Span::raw(", "));
            }
            first = false;

            let is_local = full_names.iter().any(|n| !n.contains('/'));
            let remotes: Vec<&str> = full_names
                .iter()
                .filter(|n| n.contains('/'))
                .map(|n| n.split_once('/').map(|(r, _)| r).unwrap_or(n))
                .collect();

            let fg = if is_head {
                self.ctx.color_theme.list_head_fg
            } else if is_local {
                self.ctx
                    .branch_color_map
                    .get(short_name)
                    .copied()
                    .unwrap_or(self.ctx.color_theme.list_ref_branch_fg)
            } else {
                self.ctx
                    .branch_color_map
                    .get(short_name)
                    .copied()
                    .unwrap_or(self.ctx.color_theme.list_ref_remote_branch_fg)
            };

            let display = if remotes.is_empty() {
                short_name.clone()
            } else {
                format!("{}|{}", short_name, remotes.join("|"))
            };

            spans.push(Span::styled(
                "⎇ ",
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                display,
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ));
        }

        for tag_name in tags.iter() {
            if !first {
                spans.push(Span::raw(", "));
            }
            first = false;
            let fg = self.ctx.color_theme.list_ref_tag_fg;
            spans.push(Span::styled(
                "🏷  ",
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                tag_name.clone(),
                Style::default().fg(fg).add_modifier(Modifier::BOLD),
            ));
        }

        Line::from(spans)
    }

    fn commit_message_lines(&self) -> Vec<Line<'_>> {
        let commit_message_line = Line::from(self.commit.commit_message.as_str().bold());

        let mut lines = vec![commit_message_line];

        if self.commit.body.is_empty() {
            return lines;
        }

        let body_lines = self.commit.body.lines().map(Line::raw);

        lines.push(self.empty_line());
        lines.extend(body_lines);

        lines
    }

    fn changes_lines(&self) -> Vec<Line<'_>> {
        self.changes
            .iter()
            .map(|c| {
                let (status, path, add, del) = match c {
                    FileChange::Add { path, additions } => ("A", path.as_str(), *additions, 0usize),
                    FileChange::Modify {
                        path,
                        additions,
                        deletions,
                    } => ("M", path.as_str(), *additions, *deletions),
                    FileChange::Delete { path, deletions } => {
                        ("D", path.as_str(), 0usize, *deletions)
                    }
                    FileChange::Move {
                        from,
                        to,
                        additions,
                        deletions,
                    } => ("R", to.as_str(), *additions, *deletions),
                };
                let add_str = if add > 0 {
                    format!(" +{add}")
                } else {
                    "".to_string()
                };
                let del_str = if del > 0 {
                    format!(" -{del}")
                } else {
                    "".to_string()
                };
                let status_color = match status {
                    "A" => self.ctx.color_theme.detail_file_change_add_fg,
                    "M" => self.ctx.color_theme.detail_file_change_modify_fg,
                    "D" => self.ctx.color_theme.detail_file_change_delete_fg,
                    _ => self.ctx.color_theme.detail_file_change_move_fg,
                };
                let is_deleted = matches!(c, FileChange::Delete { .. });
                let path_style = if is_deleted {
                    Style::default()
                        .fg(self.ctx.color_theme.fg)
                        .add_modifier(Modifier::CROSSED_OUT)
                } else {
                    Style::default().fg(self.ctx.color_theme.fg)
                };
                Line::from(vec![
                    Span::styled(format!("{} ", status), Style::default().fg(status_color)),
                    Span::styled(path, path_style),
                    add_str.fg(self.ctx.color_theme.detail_file_change_add_fg),
                    del_str.fg(self.ctx.color_theme.detail_file_change_delete_fg),
                ])
            })
            .collect()
    }

    fn empty_line(&self) -> Line<'_> {
        Line::raw("")
    }

    fn divider_line(&self, width: usize) -> Line<'_> {
        Line::from("─".repeat(width).fg(self.ctx.color_theme.divider_fg))
    }

    fn update_state(&self, state: &mut CommitDetailState, line_count: usize, area_height: usize) {
        state.height = area_height;
        state.offset = state.offset.min(line_count.saturating_sub(area_height));
    }
}

fn has_parent(commit: &Commit) -> bool {
    !commit.parent_commit_hashes.is_empty()
}

fn has_refs(refs: &[Ref]) -> bool {
    refs.iter().any(|r| {
        matches!(
            r,
            Ref::Branch { .. } | Ref::RemoteBranch { .. } | Ref::Tag { .. }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    fn test_commit() -> Commit {
        let date = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 5, 7, 20, 39, 0)
            .unwrap();
        Commit {
            commit_hash: "281d15fa8de043a2ee72d0971f5739ebf4335ba6".into(),
            author_name: "Adam".into(),
            author_email: "adamterraka@gmail.com".into(),
            author_date: date,
            committer_name: "Adam".into(),
            committer_email: "adamterraka@gmail.com".into(),
            committer_date: date,
            commit_message: "feat: fixed author header".into(),
            body: String::new(),
            parent_commit_hashes: vec![],
            commit_type: crate::git::CommitType::Commit,
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<&str>>()
            .join("")
    }

    #[test]
    fn scrollable_contents_do_not_include_author_label_or_identity() {
        let commit = test_commit();
        let changes = Vec::new();
        let refs = Vec::new();
        let ctx = Rc::new(AppContext::default());
        let detail = CommitDetail::new(&commit, &changes, &refs, ctx, None);

        let (label_lines, value_lines, _) = detail.contents(80);
        let labels = label_lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        let values = value_lines
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!labels.contains("Author"));
        assert!(!values.contains("Adam"));
        assert!(!values.contains("adamterraka@gmail.com"));
    }
}
