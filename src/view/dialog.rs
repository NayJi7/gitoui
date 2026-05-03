use crate::{
    app::AppContext,
    config::CursorType,
    event::{AppEvent, DialogKind, GitAction, Sender, UserEvent, UserEventWithCount},
    view::View,
};
use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogElement {
    Input,
    Checkbox(usize),
    Radio(usize),
    Validate,
    Cancel,
}

#[derive(Debug)]
pub struct DialogView<'a> {
    before: View<'a>,
    kind: DialogKind,
    input_value: String,
    dropdown_selected: usize,
    checkboxes: Vec<bool>,
    focused: DialogElement,
    hovered: Option<DialogElement>,
    dialog_area: Rect,
    inner_area: Rect,
    input_row: Option<usize>,
    checkbox_rows: Vec<usize>,
    radio_rows: Vec<usize>,
    button_row: usize,
    validate_col: (u16, u16),
    cancel_col: (u16, u16),
    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> DialogView<'a> {
    pub fn new(before: View<'a>, kind: DialogKind, ctx: Rc<AppContext>, tx: Sender) -> Self {
        let (checkboxes, dropdown_selected) = match &kind {
            DialogKind::AddTag { .. } => (vec![false], 0),
            DialogKind::CreateBranch { .. } => (vec![false], 0),
            DialogKind::CherryPick { .. } => (vec![false, false], 0),
            DialogKind::Merge { .. } => (vec![true, false, false], 0),
            DialogKind::Rebase { .. } => (vec![false, true], 0),
            DialogKind::Reset { .. } => (vec![], 1),
            DialogKind::PushBranch { .. } => (vec![false], 0),
            DialogKind::Checkout { .. } => (vec![false], 0),
            DialogKind::RenameBranch { .. } => (vec![], 0),
            DialogKind::DeleteBranch { .. } => (vec![false], 0),
            DialogKind::PullBranch { .. } => (vec![false], 0),
            DialogKind::CreateBranchFromStash { .. } => (vec![false], 0),
            DialogKind::StashWithMessage => (vec![false], 0),
            DialogKind::CommitWithMessage => (vec![false], 0),
            DialogKind::CleanUntracked => (vec![], 0),
            DialogKind::ConfirmPopStash { .. } => (vec![], 0),
            DialogKind::ConfirmDropStash { .. } => (vec![], 0),
            _ => (vec![], 0),
        };

        let focused = if Self::has_input_for(&kind) {
            DialogElement::Input
        } else if matches!(kind, DialogKind::Reset { .. }) {
            DialogElement::Radio(dropdown_selected)
        } else if !checkboxes.is_empty() {
            DialogElement::Checkbox(0)
        } else {
            DialogElement::Validate
        };

        Self {
            before,
            kind,
            input_value: String::new(),
            dropdown_selected,
            checkboxes,
            focused,
            hovered: None,
            dialog_area: Rect::default(),
            inner_area: Rect::default(),
            input_row: None,
            checkbox_rows: Vec::new(),
            radio_rows: Vec::new(),
            button_row: 0,
            validate_col: (0, 0),
            cancel_col: (0, 0),
            ctx,
            tx,
        }
    }

    fn has_input_for(kind: &DialogKind) -> bool {
        matches!(
            kind,
            DialogKind::AddTag { .. }
                | DialogKind::CreateBranch { .. }
                | DialogKind::RenameBranch { .. }
                | DialogKind::CreateBranchFromStash { .. }
                | DialogKind::StashWithMessage
                | DialogKind::CommitWithMessage
        )
    }

    fn has_input(&self) -> bool {
        Self::has_input_for(&self.kind)
    }

    fn elements(&self) -> Vec<DialogElement> {
        let mut els = vec![];
        if self.has_input() {
            els.push(DialogElement::Input);
        }
        if matches!(self.kind, DialogKind::Reset { .. }) {
            for i in 0..3 {
                els.push(DialogElement::Radio(i));
            }
        } else {
            for i in 0..self.checkboxes.len() {
                els.push(DialogElement::Checkbox(i));
            }
        }
        els.push(DialogElement::Validate);
        els.push(DialogElement::Cancel);
        els
    }

    fn focus_next(&mut self) {
        let els = self.elements();
        if let Some(idx) = els.iter().position(|e| *e == self.focused) {
            self.focused = els[(idx + 1) % els.len()];
        }
    }

    fn focus_prev(&mut self) {
        let els = self.elements();
        if let Some(idx) = els.iter().position(|e| *e == self.focused) {
            self.focused = els[if idx == 0 { els.len() - 1 } else { idx - 1 }];
        }
    }

    fn is_highlighted(&self, element: DialogElement) -> bool {
        self.focused == element || self.hovered == Some(element)
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        if matches!(self.focused, DialogElement::Input) {
            match key.code {
                ratatui::crossterm::event::KeyCode::Char(c) => {
                    self.input_value.push(c);
                    return;
                }
                ratatui::crossterm::event::KeyCode::Backspace => {
                    self.input_value.pop();
                    return;
                }
                _ => {}
            }
        }

        let event = event_with_count.event;
        match event {
            UserEvent::Confirm => match self.focused {
                DialogElement::Validate => self.confirm(),
                DialogElement::Cancel => self.tx.send(AppEvent::DialogCancel),
                DialogElement::Checkbox(i) => self.toggle_checkbox_at(i),
                DialogElement::Radio(i) => {
                    self.dropdown_selected = i;
                }
                DialogElement::Input => {
                    self.focused = DialogElement::Validate;
                }
            },
            UserEvent::Cancel => self.tx.send(AppEvent::DialogCancel),
            UserEvent::Close => self.tx.send(AppEvent::DialogCancel),
            UserEvent::NavigateDown => match self.focused {
                DialogElement::Radio(i) if i < 2 => {
                    self.dropdown_selected = i + 1;
                    self.focused = DialogElement::Radio(i + 1);
                }
                _ => self.focus_next(),
            },
            UserEvent::NavigateUp => match self.focused {
                DialogElement::Radio(i) if i > 0 => {
                    self.dropdown_selected = i - 1;
                    self.focused = DialogElement::Radio(i - 1);
                }
                _ => self.focus_prev(),
            },
            UserEvent::NavigateLeft => match self.focused {
                DialogElement::Validate => self.focused = DialogElement::Cancel,
                DialogElement::Cancel => self.focused = DialogElement::Validate,
                DialogElement::Checkbox(i) => self.toggle_checkbox_at(i),
                _ => self.focus_prev(),
            },
            UserEvent::NavigateRight => match self.focused {
                DialogElement::Validate => self.focused = DialogElement::Cancel,
                DialogElement::Cancel => self.focused = DialogElement::Validate,
                DialogElement::Checkbox(i) => self.toggle_checkbox_at(i),
                _ => self.focus_next(),
            },
            UserEvent::RefList => {
                self.focus_next();
            }
            _ => {}
        }
    }

    fn toggle_checkbox_at(&mut self, index: usize) {
        if let Some(cb) = self.checkboxes.get_mut(index) {
            *cb = !*cb;
        }
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(element) = self.find_element_at(col, row) {
            self.focused = element;
            match element {
                DialogElement::Validate => self.confirm(),
                DialogElement::Cancel => self.tx.send(AppEvent::DialogCancel),
                DialogElement::Checkbox(i) => self.toggle_checkbox_at(i),
                DialogElement::Radio(i) => {
                    self.dropdown_selected = i;
                }
                DialogElement::Input => {}
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        if col >= self.inner_area.x
            && col < self.inner_area.x + self.inner_area.width
            && row >= self.inner_area.y
            && row < self.inner_area.y + self.inner_area.height
        {
            let inner_col = col - self.inner_area.x;
            let inner_row = row - self.inner_area.y;
            self.hovered = self.find_element_at_inner(inner_col, inner_row);
        } else {
            self.hovered = None;
        }
    }

    fn find_element_at(&self, col: u16, row: u16) -> Option<DialogElement> {
        if col < self.inner_area.x || row < self.inner_area.y {
            return None;
        }
        let inner_col = col - self.inner_area.x;
        let inner_row = row - self.inner_area.y;
        if inner_col >= self.inner_area.width || inner_row >= self.inner_area.height {
            return None;
        }
        self.find_element_at_inner(inner_col, inner_row)
    }

    fn find_element_at_inner(&self, col: u16, row: u16) -> Option<DialogElement> {
        let row = row as usize;

        if row == self.button_row {
            let (vs, ve) = self.validate_col;
            let (cs, ce) = self.cancel_col;
            if col >= vs && col < ve {
                return Some(DialogElement::Validate);
            }
            if col >= cs && col < ce {
                return Some(DialogElement::Cancel);
            }
            return None;
        }

        if let Some(input_row) = self.input_row {
            if row == input_row {
                return Some(DialogElement::Input);
            }
        }

        for (i, &cb_row) in self.checkbox_rows.iter().enumerate() {
            if row == cb_row {
                return Some(DialogElement::Checkbox(i));
            }
        }

        for (i, &radio_row) in self.radio_rows.iter().enumerate() {
            if row == radio_row {
                return Some(DialogElement::Radio(i));
            }
        }

        None
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.before.render(f, area);

        let width = (area.width as u32 * 50 / 100).clamp(42, 65) as u16;
        let inner_width = width.saturating_sub(2);

        let lines = self.build_lines(inner_width);

        let height = (lines.len() as u16 + 2).min(area.height);
        let dialog_area = centered_rect_exact(width, height, area);
        self.dialog_area = dialog_area;

        f.render_widget(Clear, dialog_area);

        let theme = &self.ctx.color_theme;
        let border_color = theme.divider_fg;
        let title_color = theme.fg;

        let title = self.title();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color))
            .title(
                Line::from(Span::styled(
                    title,
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(dialog_area);
        self.inner_area = inner;

        f.render_widget(block, dialog_area);
        f.render_widget(Paragraph::new(lines), inner);

        if self.is_highlighted(DialogElement::Input) {
            if let Some(input_row) = self.input_row {
                let cursor_x = self.inner_area.x + 2 + self.input_value.len() as u16;
                let cursor_y = self.inner_area.y + input_row as u16;
                match &self.ctx.ui_config.common.cursor_type {
                    CursorType::Native => {
                        f.set_cursor_position((cursor_x, cursor_y));
                    }
                    CursorType::Virtual(cursor) => {
                        let style =
                            Style::default().fg(self.ctx.color_theme.virtual_cursor_fg);
                        f.buffer_mut()
                            .set_string(cursor_x, cursor_y, cursor, style);
                    }
                }
            }
        }
    }

    fn build_lines(&mut self, inner_width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        self.input_row = None;
        self.checkbox_rows.clear();
        self.radio_rows.clear();

        let theme = &self.ctx.color_theme;
        let dim_fg = theme.list_subject_fg;
        let fg = theme.fg;
        let yellow = theme.list_hash_fg;
        let warn_fg = theme.status_warn_fg;
        let divider_fg = theme.divider_fg;

        lines.push(separator_line(inner_width, divider_fg));

        match &self.kind {
            DialogKind::AddTag { target } => {
                lines.push(info_line("Target:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(label_line("Tag name:", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Annotated tag"));
            }
            DialogKind::CreateBranch { target } => {
                lines.push(info_line("At:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(label_line("Branch name:", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Checkout after creation"));
            }
            DialogKind::Checkout { target, is_branch } => {
                let kind_str = if *is_branch { "branch" } else { "commit" };
                lines.push(info_line(
                    &format!("Checkout {}:", kind_str),
                    target,
                    dim_fg,
                    yellow,
                ));
            }
            DialogKind::CherryPick { target } => {
                lines.push(info_line("Commit:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Record origin (-x)"));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(1, "No commit (-n)"));
            }
            DialogKind::Revert { target } => {
                lines.push(info_line("Commit:", target, dim_fg, yellow));
            }
            DialogKind::Drop { target } => {
                lines.push(info_line("Commit:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(warning_line(
                    "This will permanently remove the commit.",
                    warn_fg,
                ));
            }
            DialogKind::Merge { target, is_branch } => {
                let kind_str = if *is_branch { "branch" } else { "commit" };
                lines.push(info_line(
                    &format!("Merge {}:", kind_str),
                    target,
                    dim_fg,
                    yellow,
                ));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "No fast-forward (--no-ff)"));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(1, "Squash (--squash)"));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(2, "No commit (--no-commit)"));
            }
            DialogKind::Rebase { target } => {
                lines.push(info_line("Rebase onto:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Interactive (-i)"));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(1, "Ignore date (--ignore-date)"));
            }
            DialogKind::Reset { target } => {
                lines.push(info_line("Reset to:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(label_line("Mode:", dim_fg));
                self.radio_rows.push(lines.len());
                lines.push(self.radio_line(0, "Soft  — Keep all changes, reset head"));
                self.radio_rows.push(lines.len());
                lines.push(self.radio_line(1, "Mixed — Keep working tree, reset index"));
                self.radio_rows.push(lines.len());
                lines.push(self.radio_line(2, "Hard  — Discard all changes"));
            }
            DialogKind::RenameBranch { branch } => {
                lines.push(info_line("Current:", branch, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(label_line("New name:", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg));
            }
            DialogKind::DeleteBranch { branch, is_remote } => {
                let kind_str = if *is_remote {
                    "Remote branch"
                } else {
                    "Branch"
                };
                lines.push(info_line(kind_str, branch, dim_fg, yellow));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Force delete"));
            }
            DialogKind::DeleteTag { tag } => {
                lines.push(info_line("Tag:", tag, dim_fg, yellow));
            }
            DialogKind::PushTag { tag } => {
                lines.push(info_line("Tag:", tag, dim_fg, yellow));
            }
            DialogKind::PushBranch { branch } => {
                lines.push(info_line("Branch:", branch, dim_fg, yellow));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Force with lease (--force-with-lease)"));
            }
            DialogKind::PullBranch { branch } => {
                lines.push(info_line("Branch:", branch, dim_fg, yellow));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Rebase (--rebase)"));
            }
            DialogKind::CreateBranchFromStash {
                target: _,
                stash_ref,
            } => {
                lines.push(info_line("Stash:", stash_ref, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(label_line("Branch name:", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Checkout after creation"));
            }
            DialogKind::StashWithMessage => {
                lines.push(label_line("Message (optional):", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Include untracked files"));
            }
            DialogKind::CommitWithMessage => {
                lines.push(label_line("Message:", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Amend previous commit"));
            }
            DialogKind::ConfirmDiscardFile { file } => {
                lines.push(Line::from(Span::styled(
                    format!("Discard changes to '{}?", file),
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(warning_line("This action cannot be undone.", warn_fg));
            }
            DialogKind::ConfirmDiscardAll => {
                lines.push(Line::from(Span::styled(
                    "Discard all unstaged changes?",
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(warning_line("This action cannot be undone.", warn_fg));
            }
            DialogKind::ConfirmStageAll => {
                lines.push(Line::from(Span::styled(
                    "Stage all changes?",
                    Style::default().fg(fg),
                )));
            }
            DialogKind::ConfirmUnstageAll => {
                lines.push(Line::from(Span::styled(
                    "Unstage all changes?",
                    Style::default().fg(fg),
                )));
            }
            DialogKind::CleanUntracked => {
                lines.push(Line::from(Span::styled(
                    "Remove all untracked files and directories?",
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(warning_line("This action cannot be undone.", warn_fg));
            }
            DialogKind::ConfirmPopStash { stash_ref } => {
                lines.push(Line::from(Span::styled(
                    format!("Apply and remove stash '{}'?", stash_ref),
                    Style::default().fg(fg),
                )));
            }
            DialogKind::ConfirmDropStash { stash_ref } => {
                lines.push(Line::from(Span::styled(
                    format!("Permanently remove stash '{}'?", stash_ref),
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(warning_line("This action cannot be undone.", warn_fg));
            }
        }

        lines.push(Line::from(""));
        lines.push(separator_line(inner_width, self.ctx.color_theme.divider_fg));

        self.button_row = lines.len();
        let (btn_line, v_col, c_col) = self.button_line(inner_width);
        self.validate_col = v_col;
        self.cancel_col = c_col;
        lines.push(btn_line);

        lines
    }

    fn title(&self) -> String {
        match &self.kind {
            DialogKind::AddTag { .. } => " Add Tag ",
            DialogKind::CreateBranch { .. } => " Create Branch ",
            DialogKind::Checkout { .. } => " Checkout ",
            DialogKind::CherryPick { .. } => " Cherry Pick ",
            DialogKind::Revert { .. } => " Revert ",
            DialogKind::Drop { .. } => " Drop Commit ",
            DialogKind::Merge { .. } => " Merge ",
            DialogKind::Rebase { .. } => " Rebase ",
            DialogKind::Reset { .. } => " Reset ",
            DialogKind::RenameBranch { .. } => " Rename Branch ",
            DialogKind::DeleteBranch { .. } => " Delete Branch ",
            DialogKind::DeleteTag { .. } => " Delete Tag ",
            DialogKind::PushTag { .. } => " Push Tag ",
            DialogKind::PushBranch { .. } => " Push Branch ",
            DialogKind::PullBranch { .. } => " Pull Branch ",
            DialogKind::CreateBranchFromStash { .. } => " Branch from Stash ",
            DialogKind::StashWithMessage => " Stash Changes ",
            DialogKind::CommitWithMessage => " Commit ",
            DialogKind::CleanUntracked => " Clean Untracked ",
            DialogKind::ConfirmDiscardFile { .. } => " Confirm Discard ",
            DialogKind::ConfirmDiscardAll => " Discard All ",
            DialogKind::ConfirmStageAll => " Stage All ",
            DialogKind::ConfirmUnstageAll => " Unstage All ",
            DialogKind::ConfirmPopStash { .. } => " Pop Stash ",
            DialogKind::ConfirmDropStash { .. } => " Drop Stash ",
        }
        .to_string()
    }

    fn input_line(&self, fg: Color) -> Line<'static> {
        let focused = self.is_highlighted(DialogElement::Input);
        let style = if focused {
            Style::default().fg(fg)
        } else {
            Style::default().fg(self.ctx.color_theme.list_subject_fg)
        };
        Line::from(vec![
            Span::raw("  "),
            Span::styled(self.input_value.clone(), style),
        ])
    }

    fn checkbox_line(&self, index: usize, label: &str) -> Line<'static> {
        let theme = &self.ctx.color_theme;
        let checked = self.checkboxes.get(index).copied().unwrap_or(false);
        let highlighted = self.is_highlighted(DialogElement::Checkbox(index));

        let indicator = if highlighted { "▸ " } else { "  " };
        let check_span = if checked {
            Span::styled("[✓] ", Style::default().fg(theme.status_success_fg))
        } else {
            Span::styled("[ ] ", Style::default().fg(theme.divider_fg))
        };
        let label_style = if highlighted {
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };

        Line::from(vec![
            Span::raw(indicator),
            check_span,
            Span::styled(label.to_string(), label_style),
        ])
    }

    fn radio_line(&self, index: usize, label: &str) -> Line<'static> {
        let theme = &self.ctx.color_theme;
        let selected = self.dropdown_selected == index;
        let highlighted = self.is_highlighted(DialogElement::Radio(index));

        let indicator = if highlighted { "▸ " } else { "  " };
        let bullet_span = if selected {
            Span::styled("● ", Style::default().fg(theme.status_info_fg))
        } else {
            Span::styled("○ ", Style::default().fg(theme.divider_fg))
        };
        let label_style = if highlighted {
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default().fg(theme.fg)
        } else {
            Style::default().fg(theme.list_subject_fg)
        };

        Line::from(vec![
            Span::raw(indicator),
            bullet_span,
            Span::styled(label.to_string(), label_style),
        ])
    }

    fn button_line(&self, inner_width: u16) -> (Line<'static>, (u16, u16), (u16, u16)) {
        let theme = &self.ctx.color_theme;
        let validate_text = "[ Validate ]";
        let cancel_text = "[ Cancel ]";
        let spacing = 4;
        let total = validate_text.len() + cancel_text.len() + spacing;
        let start = (inner_width as usize).saturating_sub(total) / 2;

        let v_start = start as u16;
        let v_end = (start + validate_text.len()) as u16;
        let c_start = (start + validate_text.len() + spacing) as u16;
        let c_end = (start + validate_text.len() + spacing + cancel_text.len()) as u16;

        let validate_style = if self.is_highlighted(DialogElement::Validate) {
            Style::default()
                .fg(Color::White)
                .bg(theme.status_success_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.status_success_fg)
        };

        let cancel_style = if self.is_highlighted(DialogElement::Cancel) {
            Style::default()
                .fg(Color::White)
                .bg(theme.status_error_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.status_error_fg)
        };

        let padding = " ".repeat(start);
        let gap = " ".repeat(spacing);

        let line = Line::from(vec![
            Span::raw(padding),
            Span::styled(validate_text, validate_style),
            Span::raw(gap),
            Span::styled(cancel_text, cancel_style),
        ]);

        (line, (v_start, v_end), (c_start, c_end))
    }

    fn confirm(&mut self) {
        let (target, action) = match &self.kind {
            DialogKind::AddTag { target } => {
                if self.input_value.trim().is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("Tag name cannot be empty".into()));
                    return;
                }
                let annotated = self.checkboxes.get(0).copied().unwrap_or(false);
                let msg = if annotated && !self.input_value.is_empty() {
                    Some(self.input_value.clone())
                } else {
                    None
                };
                (
                    target.clone(),
                    GitAction::AddTag {
                        name: self.input_value.clone(),
                        annotated,
                        message: msg,
                    },
                )
            }
            DialogKind::CreateBranch { target }
            | DialogKind::CreateBranchFromStash { target, .. } => {
                if self.input_value.trim().is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("Branch name cannot be empty".into()));
                    return;
                }
                let checkout = self.checkboxes.get(0).copied().unwrap_or(false);
                (
                    target.clone(),
                    GitAction::CreateBranch {
                        name: self.input_value.clone(),
                        checkout,
                    },
                )
            }
            DialogKind::Checkout { target, .. } => (target.clone(), GitAction::Checkout),
            DialogKind::CherryPick { target } => {
                let record_origin = self.checkboxes.get(0).copied().unwrap_or(false);
                let no_commit = self.checkboxes.get(1).copied().unwrap_or(false);
                (
                    target.clone(),
                    GitAction::CherryPick {
                        no_commit,
                        record_origin,
                    },
                )
            }
            DialogKind::Revert { target } => (target.clone(), GitAction::Revert),
            DialogKind::Drop { target } => (target.clone(), GitAction::Drop),
            DialogKind::Merge { target, .. } => {
                let no_ff = self.checkboxes.get(0).copied().unwrap_or(true);
                let squash = self.checkboxes.get(1).copied().unwrap_or(false);
                let no_commit = self.checkboxes.get(2).copied().unwrap_or(false);
                (
                    target.clone(),
                    GitAction::Merge {
                        no_ff,
                        squash,
                        no_commit,
                    },
                )
            }
            DialogKind::Rebase { target } => {
                let interactive = self.checkboxes.get(0).copied().unwrap_or(false);
                let ignore_date = self.checkboxes.get(1).copied().unwrap_or(true);
                (
                    target.clone(),
                    GitAction::Rebase {
                        ignore_date,
                        interactive,
                    },
                )
            }
            DialogKind::Reset { target } => {
                let mode = match self.dropdown_selected {
                    0 => "--soft",
                    1 => "--mixed",
                    2 => "--hard",
                    _ => "--mixed",
                };
                (
                    target.clone(),
                    GitAction::Reset {
                        mode: mode.to_string(),
                    },
                )
            }
            DialogKind::RenameBranch { branch } => {
                if self.input_value.trim().is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("Branch name cannot be empty".into()));
                    return;
                }
                (
                    branch.clone(),
                    GitAction::RenameBranch {
                        new_name: self.input_value.clone(),
                    },
                )
            }
            DialogKind::DeleteBranch { branch, .. } => {
                let force = self.checkboxes.get(0).copied().unwrap_or(false);
                (branch.clone(), GitAction::DeleteBranch { force })
            }
            DialogKind::DeleteTag { tag } => (tag.clone(), GitAction::DeleteTag),
            DialogKind::PushTag { tag } => (tag.clone(), GitAction::PushTag),
            DialogKind::PullBranch { branch } => {
                let rebase = self.checkboxes.get(0).copied().unwrap_or(false);
                (branch.clone(), GitAction::PullBranch { rebase })
            }
            DialogKind::PushBranch { branch } => {
                let force = self.checkboxes.get(0).copied().unwrap_or(false);
                (branch.clone(), GitAction::PushBranch { force })
            }
            DialogKind::StashWithMessage => {
                let msg = if self.input_value.is_empty() {
                    None
                } else {
                    Some(self.input_value.clone())
                };
                (String::new(), GitAction::Stash { message: msg })
            }
            DialogKind::CommitWithMessage => {
                if self.input_value.trim().is_empty() {
                    self.tx.send(AppEvent::NotifyError(
                        "Commit message cannot be empty".into(),
                    ));
                    return;
                }
                let amend = self.checkboxes.get(0).copied().unwrap_or(false);
                (
                    String::new(),
                    GitAction::Commit {
                        message: self.input_value.clone(),
                        amend,
                    },
                )
            }
            DialogKind::ConfirmDiscardFile { file } => {
                (file.clone(), GitAction::DiscardFile { file: file.clone() })
            }
            DialogKind::ConfirmDiscardAll => (String::new(), GitAction::DiscardAll),
            DialogKind::ConfirmStageAll => (String::new(), GitAction::StageAll),
            DialogKind::ConfirmUnstageAll => (String::new(), GitAction::UnstageAll),
            DialogKind::CleanUntracked => (String::new(), GitAction::CleanUntracked),
            DialogKind::ConfirmPopStash { stash_ref } => {
                (stash_ref.clone(), GitAction::PopStash)
            }
            DialogKind::ConfirmDropStash { stash_ref } => {
                (stash_ref.clone(), GitAction::DropStash)
            }
        };
        self.tx.send(AppEvent::ExecuteGitAction { target, action });
    }

    pub fn take_before_view(&mut self) -> View<'a> {
        std::mem::take(&mut self.before)
    }
}

fn centered_rect_exact(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

fn separator_line(width: u16, color: Color) -> Line<'static> {
    let sep = "─".repeat(width as usize);
    Line::from(Span::styled(sep, Style::default().fg(color)))
}

fn info_line(label: &str, value: &str, label_fg: Color, value_fg: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {} ", label), Style::default().fg(label_fg)),
        Span::styled(value.to_string(), Style::default().fg(value_fg)),
    ])
}

fn label_line(label: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {}", label),
        Style::default().fg(color),
    ))
}

fn warning_line(text: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {}", text),
        Style::default().fg(color),
    ))
}
