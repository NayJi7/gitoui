use std::rc::Rc;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
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
    pub hover_file: Option<usize>,
    last_selected_file: usize,
    avatar_row: Option<u16>,
    pending_avatar_delete_row: Option<u16>,
}

impl CommitDetailState {
    pub fn drain_pending_avatar_delete(&mut self) -> Option<u16> {
        self.pending_avatar_delete_row.take()
    }
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
    head_branch_name: Option<String>,
    is_head_commit: bool,
    /// The graph lane color of this commit, which the commit list paints its
    /// branch labels with. Without it (a commit opened from a PR, not in the
    /// list) the labels fall back to the branch color table.
    graph_color: Option<ratatui::style::Color>,
    /// `Some(n)` when this commit was opened from a PR drilldown
    /// surfaces as `· PR #n` next to the panel title so the user
    /// always sees the originating context.
    pr_origin: Option<u64>,
}

impl<'a> CommitDetail<'a> {
    pub fn new(
        commit: &'a Commit,
        changes: &'a Vec<FileChange>,
        refs: &'a Vec<Ref>,
        ctx: Rc<AppContext>,
        head_branch_name: Option<String>,
        is_head_commit: bool,
    ) -> Self {
        Self {
            commit,
            changes,
            refs,
            ctx,
            head_branch_name,
            is_head_commit,
            graph_color: None,
            pr_origin: None,
        }
    }

    /// The lane color the commit list gives this commit's branch labels.
    pub fn with_graph_color(mut self, color: Option<ratatui::style::Color>) -> Self {
        self.graph_color = color;
        self
    }

    /// Attach a PR origin to the widget so the title row reads
    /// `Commit Details · PR #N` instead of just `Commit Details`.
    pub fn with_pr_origin(mut self, pr_number: Option<u64>) -> Self {
        self.pr_origin = pr_number;
        self
    }
}

/// Display label + the global `UserEvent` it dispatches. The on-screen
/// key glyph is looked up from `KeyBinds` at render time so a rebind
/// in the user's config shows up in the panel automatically, no need
/// to keep these strings in sync with `assets/default-keybind.toml`.
pub const COMMIT_ACTIONS: &[(&str, crate::event::UserEvent)] = &[
    ("Add Tag", crate::event::UserEvent::AddTag),
    ("Blame", crate::event::UserEvent::Blame),
    ("Create Branch", crate::event::UserEvent::CreateBranch),
    ("Checkout", crate::event::UserEvent::Checkout),
    ("Cherry Pick", crate::event::UserEvent::CherryPick),
    ("Revert", crate::event::UserEvent::Revert),
    ("Drop", crate::event::UserEvent::Drop),
    ("Merge into current", crate::event::UserEvent::Merge),
    ("Rebase current on", crate::event::UserEvent::Rebase),
    ("Reset current to", crate::event::UserEvent::Reset),
    ("Squash with parent", crate::event::UserEvent::Squash),
    ("Amend", crate::event::UserEvent::AmendCommit),
];

/// Actions slice to use: HEAD commit gets Amend, others don't.
/// Squash is available for every non-initial commit (parent existence
/// checked at apply-time), so it's part of the shared prefix.
pub fn commit_actions(is_head_commit: bool) -> &'static [(&'static str, crate::event::UserEvent)] {
    if is_head_commit {
        COMMIT_ACTIONS
    } else {
        &COMMIT_ACTIONS[..11]
    }
}

/// Map a `UserEvent` to its `[scope.detail]` action name when the
/// event has no global binding (Revert today, `v` is already taken
/// globally by `clean_untracked`).
fn scoped_action_name(event: crate::event::UserEvent) -> Option<&'static str> {
    match event {
        crate::event::UserEvent::Revert => Some("revert"),
        _ => None,
    }
}

pub const STASH_ACTIONS: &[(&str, crate::event::UserEvent)] = &[
    ("Apply Stash", crate::event::UserEvent::ApplyStash),
    ("Pop Stash", crate::event::UserEvent::PopStash),
    ("Drop Stash", crate::event::UserEvent::DropStash),
    (
        "Create Branch from Stash",
        crate::event::UserEvent::CreateBranchFromStash,
    ),
    ("Copy Stash Name", crate::event::UserEvent::CopyStashName),
    ("Copy Stash Hash", crate::event::UserEvent::CopyStashHash),
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

        // Content inner: title + underline + spacer + scrollable content
        let [content_title_area, content_underline_area, _content_spacer_area, content_scroll_area] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .areas(content_inner);

        // Render centered title, appends ` · PR #N` in accent
        // colour when the commit was opened from a PR drilldown.
        let pr_suffix = self
            .pr_origin
            .map(|n| format!(" · PR #{}", n))
            .unwrap_or_default();
        let title_text = "Commit Details";
        let visible_text_len = (title_text.chars().count() + pr_suffix.chars().count()) as u16;
        let title_pad = content_title_area.width.saturating_sub(visible_text_len);
        let title_left = title_pad / 2;
        let mut title_spans: Vec<Span<'_>> = vec![
            Span::styled(" ".repeat(title_left as usize), Style::default()),
            Span::styled(
                title_text.to_string(),
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if !pr_suffix.is_empty() {
            title_spans.push(Span::styled(
                pr_suffix,
                Style::default()
                    .fg(self.ctx.color_theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        Paragraph::new(Line::from(title_spans)).render(content_title_area, buf);

        // Render small underline (slightly shorter than title)
        let title_len = visible_text_len;
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

        let [labels_area, value_area] =
            Layout::horizontal([Constraint::Length(LABEL_WIDTH), Constraint::Min(0)])
                .areas(content_scroll_area);

        let (mut label_lines, mut value_lines, changes_start) = self.contents(value_area.width);

        let content_area_height = content_scroll_area.height as usize;
        self.update_state(state, value_lines.len(), content_area_height);
        state.ensure_selected_visible(changes_start);

        label_lines = label_lines.into_iter().skip(state.offset).collect();
        value_lines = value_lines.into_iter().skip(state.offset).collect();

        // Highlight hovered file (mouse) or keyboard-selected file
        let highlight_file = state.hover_file.unwrap_or(state.selected_file);
        if !self.changes.is_empty() {
            let selected_line = changes_start + highlight_file;
            let visible_selected = selected_line.saturating_sub(state.offset);
            if visible_selected < value_lines.len() {
                value_lines[visible_selected].style =
                    Style::default().add_modifier(Modifier::REVERSED);
            }
        }

        self.render_labels_paragraph(label_lines, labels_area, buf);
        self.render_value_paragraph(value_lines, value_area, buf);

        if state.offset == 0 && content_scroll_area.height > 0 {
            // Overlay avatar in the value column, right before the author name
            if self.render_author_avatar(value_area, buf) {
                state.avatar_row = Some(content_scroll_area.top());
            }
        } else if state.avatar_row.is_some() {
            // Avatar scrolled out of view, schedule row-scoped deletion
            state.pending_avatar_delete_row = state.avatar_row.take();
        }
        self.render_action_bar(action_bar_area, buf, state);
    }
}

/// Width of the column of labels (`Author:`, `Message:`…).
const LABEL_WIDTH: u16 = 12;
use crate::text_fit::{wrap_line, VALUE_PADDING};

/// Width of the value column when the panel is drawn in `area`. The detail
/// view needs it to find the rows of the file list for the mouse.
pub fn value_column_width(area: Rect) -> u16 {
    let [content_area, _] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(area);
    content_area.width.saturating_sub(LABEL_WIDTH)
}

impl CommitDetail<'_> {
    /// The row of the value column where the list of changed files starts,
    /// for a panel drawn in `area` (the mouse maps a click to a file with it).
    pub fn changes_start_in(&self, area: Rect) -> usize {
        self.contents(value_column_width(area)).2
    }

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

    fn render_author_avatar(&self, area: Rect, buf: &mut Buffer) -> bool {
        if area.width <= 5 || area.height == 0 {
            return false;
        }
        let mut avatar_manager = self.ctx.avatar_manager.lock().unwrap();
        if !avatar_manager.is_enabled() {
            return false;
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
            return true;
        }
        false
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
        let action_pad = Block::default().padding(Padding::new(2, 1, 0, 0));
        let action_inner = action_pad.inner(action_actions_area);
        action_pad.render(action_actions_area, buf);

        let actions = if self.is_stash() {
            STASH_ACTIONS
        } else {
            commit_actions(self.is_head_commit)
        };
        let rebasing = !self.ctx.repo_path.as_os_str().is_empty()
            && crate::git::rebase::rebase_in_progress(&self.ctx.repo_path);

        let mut lines = Vec::new();
        for (i, (label, event)) in actions.iter().enumerate() {
            let is_hovered = state.hovered_action == Some(i);
            // While a rebase is in progress, the "Rebase current on" slot
            // re-labels to "Resume rebase" (yellow) and dispatches into the
            // resume view. The bound key still drives both, the action
            // just means something different in this state.
            let (effective_label, label_color) = if rebasing && *label == "Rebase current on" {
                ("Resume rebase", Some(self.ctx.color_theme.status_warn_fg))
            } else {
                (*label, None)
            };
            let mut style = if is_hovered {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            if let Some(c) = label_color {
                style = style.fg(c).add_modifier(Modifier::BOLD);
            }
            let key_style = style.add_modifier(Modifier::BOLD);
            // Most actions resolve through the global keymap. Revert is
            // an exception, `v` is bound globally to `clean_untracked`
            // (uncommitted view) and re-scoped to `revert` in
            // [scope.detail]; we fall through to the scoped lookup when
            // the global one comes up empty so the panel still surfaces
            // the actual bound key.
            let mut key = self.ctx.keybind.primary_global_key(*event);
            if key.is_empty() {
                if let Some(scoped_name) = scoped_action_name(*event) {
                    key = self
                        .ctx
                        .keybind
                        .primary_scoped_key(&["detail"], scoped_name);
                }
            }
            let mut spans = vec![Span::styled(effective_label.to_string(), style)];
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

    fn is_stash(&self) -> bool {
        use crate::git::CommitType;
        matches!(self.commit.commit_type, CommitType::Stash)
    }

    fn contents(&self, width: u16) -> (Vec<Line<'_>>, Vec<Line<'_>>, usize) {
        let mut label_lines: Vec<Line> = Vec::new();
        let mut value_lines: Vec<Line> = Vec::new();
        // What the value paragraph can really draw: every value that is
        // longer goes on the next rows instead of running off the panel.
        let vw = (width as usize).saturating_sub(VALUE_PADDING).max(1);
        let label_fg = self.ctx.color_theme.detail_label_fg;

        // One labelled value: the label on its first row, blank under it for
        // the rows the value wraps onto, so both columns stay aligned.
        let mut add = |label: &'static str, value: Line<'_>| {
            let rows = wrap_line(value, vw);
            label_lines.push(Line::from(label).fg(label_fg));
            for _ in 1..rows.len() {
                label_lines.push(Line::from(""));
            }
            value_lines.extend(rows);
        };

        // Reserve 3 chars before the author name when avatars are on (2 cells avatar + 1 gap)
        let avatar_enabled = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        let author_prefix: &'static str = if avatar_enabled { "   " } else { "" };

        add("   Author: ", self.author_line(author_prefix));
        add("     Date: ", self.author_date_line());

        if self.commit.author_name != self.commit.committer_name
            || self.commit.author_email != self.commit.committer_email
            || self.commit.author_date != self.commit.committer_date
        {
            add("Committer: ", self.committer_line());
            add("     Date: ", self.committer_date_line());
        }

        add("      SHA: ", self.sha_line());

        if has_parent(self.commit) {
            let parent_label = if self.commit.parent_commit_hashes.len() == 1 {
                "   Parent: "
            } else {
                "  Parents: "
            };
            add(parent_label, self.parents_line());
        }

        if has_refs(self.refs) {
            add("     Refs: ", self.refs_line());
        }

        // Divider before message
        label_lines.push(Line::from(""));
        value_lines.push(self.divider_line(width as usize));

        // The subject, then the body (kept as written: its indentation and
        // blank lines survive, long lines wrap).
        let subject = wrap_line(Line::from(self.commit.commit_message.as_str().bold()), vw);
        label_lines.push(Line::from("  Message: ").fg(label_fg));
        for _ in 1..subject.len() {
            label_lines.push(Line::from(""));
        }
        value_lines.extend(subject);
        if !self.commit.body.is_empty() {
            let body = crate::text_fit::wrap_body(&self.commit.body, vw);
            label_lines.push(Line::from("     Body: ").fg(label_fg));
            for _ in 1..body.len() {
                label_lines.push(Line::from(""));
            }
            value_lines.extend(body.into_iter().map(Line::from));
        }

        // Divider before changes
        label_lines.push(Line::from(""));
        value_lines.push(self.divider_line(width as usize));
        value_lines.extend(self.changes_lines(vw));

        let changes_start = value_lines.len() - self.changes.len();

        (label_lines, value_lines, changes_start)
    }

    fn author_line(&self, prefix: &'static str) -> Line<'_> {
        let mut spans = Vec::new();
        if !prefix.is_empty() {
            spans.push(Span::raw(prefix));
        }
        spans.push(
            self.commit
                .author_name
                .as_str()
                .fg(self.ctx.color_theme.detail_name_fg),
        );
        spans.push(" <".into());
        spans.push(
            self.commit
                .author_email
                .as_str()
                .fg(self.ctx.color_theme.detail_email_fg),
        );
        spans.push(">".into());
        Line::from(spans)
    }

    fn author_date_line(&self) -> Line<'_> {
        let date_str = self.ctx.core_config.date_time_format().format(
            &self.commit.author_date,
            self.ctx.core_config.date_time_local(),
        );
        Line::from(date_str.fg(self.ctx.color_theme.detail_date_fg))
    }

    fn committer_line(&self) -> Line<'_> {
        Line::from(vec![
            self.commit
                .committer_name
                .as_str()
                .fg(self.ctx.color_theme.detail_name_fg),
            " <".into(),
            self.commit
                .committer_email
                .as_str()
                .fg(self.ctx.color_theme.detail_email_fg),
            ">".into(),
        ])
    }

    fn committer_date_line(&self) -> Line<'_> {
        let date_str = self.ctx.core_config.date_time_format().format(
            &self.commit.committer_date,
            self.ctx.core_config.date_time_local(),
        );
        Line::from(date_str.fg(self.ctx.color_theme.detail_date_fg))
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
            }
        }

        // The branch HEAD points to comes first, right after `HEAD -> `.
        let mut ordered: Vec<(&String, &Vec<String>)> = compacted_branches.iter().collect();
        ordered.sort_by_key(|(name, _)| self.head_branch_name.as_deref() != Some(name.as_str()));
        for (short_name, full_names) in ordered {
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

            // Like the commit list: the label has the color of the commit's
            // graph lane, and only the `HEAD ->` marker is the HEAD color.
            let fg = if let Some(lane_color) = self.graph_color {
                lane_color
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

    fn changes_lines(&self, width: usize) -> Vec<Line<'_>> {
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
                        from: _,
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
                // One row per file: the stats stay, the path loses its middle.
                let room = width
                    .saturating_sub(2)
                    .saturating_sub(crate::text_fit::width(&add_str))
                    .saturating_sub(crate::text_fit::width(&del_str))
                    .max(1);
                let path = crate::text_fit::ellipsize_path(path, room);
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
            author_name: "Alice".into(),
            author_email: "alice@example.com".into(),
            author_date: date,
            committer_name: "Alice".into(),
            committer_email: "alice@example.com".into(),
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
    fn scrollable_contents_include_author_and_identity() {
        let commit = test_commit();
        let changes = Vec::new();
        let refs = Vec::new();
        let ctx = Rc::new(AppContext::default());
        let detail = CommitDetail::new(&commit, &changes, &refs, ctx, None, false);

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

        assert!(labels.contains("Author"));
        assert!(values.contains("Alice"));
        assert!(values.contains("alice@example.com"));
    }

    #[test]
    fn scrollable_contents_start_with_author_line() {
        let commit = test_commit();
        let changes = Vec::new();
        let refs = Vec::new();
        let ctx = Rc::new(AppContext::default());
        let detail = CommitDetail::new(&commit, &changes, &refs, ctx, None, false);

        let (label_lines, _, _) = detail.contents(80);
        assert_eq!(line_text(&label_lines[0]).trim(), "Author:");
    }

    #[test]
    fn long_values_wrap_and_the_two_columns_stay_aligned() {
        let mut commit = test_commit();
        commit.commit_message =
            "fix(diff): a subject that is quite a bit longer than the panel".into();
        commit.body = "The file diff and the whole-commit diff both ran `git diff <hash>^ <hash>`.\n\n    indented line that also needs to wrap around the edge".into();
        commit.parent_commit_hashes = vec![
            "bf609851a2b3c4d5e6f708192a3b4c5d6e7f8091".into(),
            "1f8bd9c0a1b2c3d4e5f60718293a4b5c6d7e8f90".into(),
        ];
        let changes = vec![crate::git::FileChange::Modify {
            path: "src/view/actions/a_very_long_directory_name/and_a_long_file_name.rs".into(),
            additions: 42,
            deletions: 7,
        }];
        let refs = Vec::new();
        let ctx = Rc::new(AppContext::default());
        let detail = CommitDetail::new(&commit, &changes, &refs, ctx, None, false);

        let (labels, values, changes_start) = detail.contents(40);
        // Every row fits the 37 columns the value paragraph can draw, and the
        // column of labels has exactly one row for each row of values above the files.
        // (the file rows below have no label)
        assert_eq!(labels.len(), changes_start);
        for row in values.iter().filter(|r| !line_text(r).starts_with('─')) {
            assert!(
                crate::text_fit::width(&line_text(row)) <= 37,
                "{:?}",
                line_text(row)
            );
        }
        // The file row keeps its stats and its file name: one row, middle cut.
        let file_row = line_text(&values[changes_start]);
        assert!(
            file_row.contains("+42") && file_row.contains("-7"),
            "{file_row}"
        );
        assert!(
            file_row.contains("and_a_long_file_name.rs") || file_row.contains('…'),
            "{file_row}"
        );
        assert_eq!(values.len() - changes_start, 1);
        // The body lost nothing in the wrap.
        let all = values.iter().map(line_text).collect::<Vec<_>>().join(" ");
        assert!(all.contains("whole-commit"));
        assert!(all.contains("edge"));
    }

    #[test]
    fn the_row_the_mouse_maths_expects_is_the_row_that_is_drawn() {
        use ratatui::{buffer::Buffer, layout::Rect, widgets::StatefulWidget};
        let mut commit = test_commit();
        commit.commit_message =
            "fix(diff): a subject that is quite a bit longer than the panel is wide".into();
        commit.body = "A body line that goes on and on and on and on until it has to wrap around.\n\nSecond paragraph.".into();
        let changes = vec![crate::git::FileChange::Add {
            path: "marker.rs".into(),
            additions: 3,
        }];
        let refs = Vec::new();
        let ctx = Rc::new(AppContext::default());
        for width in [60u16, 90, 140] {
            let area = Rect::new(0, 0, width, 30);
            let detail = CommitDetail::new(&commit, &changes, &refs, ctx.clone(), None, false);
            let expected = detail.changes_start_in(area);
            let mut buf = Buffer::empty(area);
            let mut state = CommitDetailState::default();
            detail.render(area, &mut buf, &mut state);
            // The scrollable part starts 4 rows under the top of the panel.
            let drawn = (0..area.height).find(|&y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .contains("marker.rs")
            });
            assert_eq!(drawn, Some(4 + expected as u16), "width {width}");
        }
    }
}
