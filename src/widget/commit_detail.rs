use std::rc::Rc;

use chrono::{DateTime, FixedOffset};
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

    pub fn select_last(&mut self) {
        self.offset = usize::MAX;
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
}

impl<'a> CommitDetail<'a> {
    pub fn new(
        commit: &'a Commit,
        changes: &'a Vec<FileChange>,
        refs: &'a Vec<Ref>,
        ctx: Rc<AppContext>,
    ) -> Self {
        Self {
            commit,
            changes,
            refs,
            ctx,
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
            Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);

        // Title block over the entire content area
        let title_block = Block::default()
            .title("Commit Details")
            .title_style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
            .borders(Borders::TOP)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg))
            .padding(Padding::new(0, 0, 1, 0));
        let inner = title_block.inner(content_area);
        title_block.render(content_area, buf);

        let [labels_area, value_area] =
            Layout::horizontal([Constraint::Length(12), Constraint::Min(0)]).areas(inner);

        let (mut label_lines, mut value_lines, changes_start) = self.contents(inner.width);

        let content_area_height = inner.height as usize;
        self.update_state(state, value_lines.len(), content_area_height);
        state.ensure_selected_visible(changes_start);

        // Apply selection highlight to the selected file line
        if !self.changes.is_empty() {
            let selected_line = changes_start + state.selected_file;
            for (i, line) in value_lines.iter_mut().enumerate() {
                if i == selected_line {
                    line.style = Style::default().add_modifier(Modifier::REVERSED);
                }
            }
        }

        label_lines = label_lines.into_iter().skip(state.offset).collect();
        value_lines = value_lines.into_iter().skip(state.offset).collect();

        self.render_labels_paragraph(label_lines, labels_area, buf);
        self.render_value_paragraph(value_lines, value_area, buf);
        self.render_action_bar(action_bar_area, buf, state);
    }
}

impl CommitDetail<'_> {
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

    fn render_action_bar(&self, area: Rect, buf: &mut Buffer, state: &CommitDetailState) {
        let block = Block::default()
            .borders(Borders::TOP | Borders::LEFT)
            .style(Style::default().fg(self.ctx.color_theme.divider_fg))
            .padding(Padding::new(1, 1, 0, 0));
        let inner = block.inner(area);
        block.render(area, buf);

        let actions = if self.is_stash() {
            STASH_ACTIONS
        } else {
            COMMIT_ACTIONS
        };

        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            "Git Actions",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from("─".repeat(inner.width as usize).fg(self.ctx.color_theme.divider_fg)));
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

        let paragraph = Paragraph::new(lines)
            .style(Style::default().fg(self.ctx.color_theme.fg));
        paragraph.render(inner, buf);
    }

    fn is_stash(&self) -> bool {
        use crate::git::CommitType;
        matches!(self.commit.commit_type, CommitType::Stash)
    }

    fn contents(&self, width: u16) -> (Vec<Line<'_>>, Vec<Line<'_>>, usize) {
        let mut label_lines: Vec<Line> = Vec::new();
        let mut value_lines: Vec<Line> = Vec::new();

        label_lines.push(Line::from("   Author: ").fg(self.ctx.color_theme.detail_label_fg));
        label_lines.push(self.empty_line());
        value_lines.extend(self.author_lines());

        if is_author_committer_different(self.commit) {
            label_lines.push(Line::from("Committer: ").fg(self.ctx.color_theme.detail_label_fg));
            label_lines.push(self.empty_line());
            value_lines.extend(self.committer_lines());
        }

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
            label_lines.push(Line::from("       Refs: ").fg(self.ctx.color_theme.detail_label_fg));
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

    fn author_lines(&self) -> Vec<Line<'_>> {
        self.author_committer_lines(
            &self.commit.author_name,
            &self.commit.author_email,
            &self.commit.author_date,
        )
    }

    fn committer_lines(&self) -> Vec<Line<'_>> {
        self.author_committer_lines(
            &self.commit.committer_name,
            &self.commit.committer_email,
            &self.commit.committer_date,
        )
    }

    fn author_committer_lines<'a>(
        &'a self,
        name: &'a str,
        email: &'a str,
        date: &'a DateTime<FixedOffset>,
    ) -> Vec<Line<'a>> {
        let date_str = self.ctx.core_config.date_time_format().format(
            date,
            self.ctx.core_config.date_time_local(),
        );
        vec![
            Line::from(vec![
                name.fg(self.ctx.color_theme.detail_name_fg),
                " <".into(),
                email.fg(self.ctx.color_theme.detail_email_fg),
                "> ".into(),
            ]),
            Line::from(date_str.fg(self.ctx.color_theme.detail_date_fg)),
        ]
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
        // Build compacted branch display: local + remote branches sharing the same short name
        // are shown as "main|origin" instead of "main origin/main"
        // Use BTreeMap to preserve insertion order and avoid random ordering on re-render.
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

        for (short_name, full_names) in compacted_branches.iter() {
            if !first {
                spans.push(Span::raw(" "));
            }
            first = false;

            let is_local = full_names.iter().any(|n| !n.contains('/'));
            let remotes: Vec<&str> = full_names
                .iter()
                .filter(|n| n.contains('/'))
                .map(|n| n.split_once('/').map(|(r, _)| r).unwrap_or(n))
                .collect();

            // Use branch_color_map for local branches to match commit list colors
            let fg = if is_local {
                self.ctx
                    .branch_color_map
                    .get(short_name)
                    .copied()
                    .unwrap_or(self.ctx.color_theme.list_ref_branch_fg)
            } else {
                self.ctx.color_theme.list_ref_remote_branch_fg
            };

            let display = if remotes.is_empty() {
                short_name.clone()
            } else {
                format!("{}|{}", short_name, remotes.join("|"))
            };

            spans.push(Span::styled("⎇ ", Style::default().fg(fg).add_modifier(Modifier::BOLD)));
            spans.push(Span::styled(display, Style::default().fg(fg).add_modifier(Modifier::BOLD)));
        }

        for tag_name in tags.iter() {
            if !first {
                spans.push(Span::raw(" "));
            }
            first = false;
            let fg = self.ctx.color_theme.list_ref_tag_fg;
            spans.push(Span::styled("🏷 ", Style::default().fg(fg).add_modifier(Modifier::BOLD)));
            spans.push(Span::styled(tag_name.clone(), Style::default().fg(fg).add_modifier(Modifier::BOLD)));
        }

        Line::from(spans)
    }

    fn commit_message_lines(&self) -> Vec<Line<'_>> {
        let subject_line = Line::from(self.commit.subject.as_str().bold());

        let mut lines = vec![subject_line];

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
                    FileChange::Modify { path, additions, deletions } => ("M", path.as_str(), *additions, *deletions),
                    FileChange::Delete { path, deletions } => ("D", path.as_str(), 0usize, *deletions),
                    FileChange::Move { from, to, additions, deletions } => ("R", to.as_str(), *additions, *deletions),
                };
                let add_str = if add > 0 { format!(" +{add}") } else { "".to_string() };
                let del_str = if del > 0 { format!(" -{del}") } else { "".to_string() };
                let status_color = match status {
                    "A" => self.ctx.color_theme.detail_file_change_add_fg,
                    "M" => self.ctx.color_theme.detail_file_change_modify_fg,
                    "D" => self.ctx.color_theme.detail_file_change_delete_fg,
                    _ => self.ctx.color_theme.detail_file_change_move_fg,
                };
                let is_deleted = matches!(c, FileChange::Delete { .. });
                let path_style = if is_deleted {
                    Style::default().fg(self.ctx.color_theme.fg).add_modifier(Modifier::CROSSED_OUT)
                } else {
                    Style::default().fg(self.ctx.color_theme.fg)
                };
                Line::from(vec![
                    status.fg(status_color),
                    "  ".into(),
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

    fn update_state(
        &self,
        state: &mut CommitDetailState,
        line_count: usize,
        area_height: usize,
    ) {
        state.height = area_height;
        state.offset = state.offset.min(line_count.saturating_sub(area_height));
    }
}

fn is_author_committer_different(commit: &Commit) -> bool {
    commit.author_name != commit.committer_name
        || commit.author_email != commit.committer_email
        || commit.author_date != commit.committer_date
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
