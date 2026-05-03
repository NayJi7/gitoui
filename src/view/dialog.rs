use crate::{
    app::AppContext,
    event::{AppEvent, DialogKind, GitAction, Sender, UserEvent, UserEventWithCount},
    view::View,
};
use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use std::rc::Rc;

#[derive(Debug)]
pub struct DialogView<'a> {
    before: View<'a>,
    kind: DialogKind,
    input_value: String,
    dropdown_selected: usize,
    checkboxes: Vec<bool>,
    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> DialogView<'a> {
    pub fn new(before: View<'a>, kind: DialogKind, ctx: Rc<AppContext>, tx: Sender) -> Self {
        let (checkboxes, dropdown_selected) = match &kind {
            DialogKind::AddTag { .. } => (vec![false], 0),
            DialogKind::CreateBranch { .. } => (vec![false], 0),
            DialogKind::CherryPick { .. } => (vec![false, false], 0),
            DialogKind::Merge { .. } => (vec![true, false, false], 0), // no-ff default on
            DialogKind::Rebase { .. } => (vec![false, true], 0),       // ignore-date default on
            DialogKind::Reset { .. } => (vec![], 1),                   // Mixed default
            DialogKind::PushBranch { .. } => (vec![false], 0),         // force checkbox
            DialogKind::Checkout { .. } => (vec![false], 0),           // remember choice
            DialogKind::RenameBranch { .. } => (vec![], 0),
            DialogKind::DeleteBranch { .. } => (vec![false], 0), // force checkbox
            DialogKind::PullBranch { .. } => (vec![false], 0),   // rebase checkbox
            DialogKind::CreateBranchFromStash { .. } => (vec![false], 0), // checkout
            DialogKind::StashWithMessage => (vec![false], 0),    // include untracked
            DialogKind::CommitWithMessage => (vec![false], 0),   // amend
            DialogKind::CleanUntracked => (vec![], 0),
            DialogKind::ConfirmPopStash { .. } => (vec![], 0),
            DialogKind::ConfirmDropStash { .. } => (vec![], 0),
            _ => (vec![], 0),
        };
        Self {
            before,
            kind,
            input_value: String::new(),
            dropdown_selected,
            checkboxes,
            ctx,
            tx,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        let event = event_with_count.event;
        match event {
            UserEvent::Confirm => self.confirm(),
            UserEvent::Cancel | UserEvent::Close => self.tx.send(AppEvent::DialogCancel),
            UserEvent::NavigateDown => self.navigate_down(),
            UserEvent::NavigateUp => self.navigate_up(),
            UserEvent::NavigateLeft | UserEvent::NavigateRight => self.toggle_checkbox(),
            _ => {
                // Handle text input directly from key events
                match key.code {
                    ratatui::crossterm::event::KeyCode::Char(c) => {
                        self.input_value.push(c);
                    }
                    ratatui::crossterm::event::KeyCode::Backspace => {
                        self.input_value.pop();
                    }
                    _ => {}
                }
            }
        }
    }

    fn navigate_down(&mut self) {
        match &self.kind {
            DialogKind::Reset { .. } => {
                if self.dropdown_selected < 2 {
                    self.dropdown_selected += 1;
                }
            }
            _ => {}
        }
    }

    fn navigate_up(&mut self) {
        if self.dropdown_selected > 0 {
            self.dropdown_selected -= 1;
        }
    }

    fn toggle_checkbox(&mut self) {
        match &self.kind {
            DialogKind::AddTag { .. }
            | DialogKind::CreateBranch { .. }
            | DialogKind::PushBranch { .. }
            | DialogKind::Checkout { .. }
            | DialogKind::DeleteBranch { .. }
            | DialogKind::PullBranch { .. }
            | DialogKind::CreateBranchFromStash { .. }
            | DialogKind::StashWithMessage
            | DialogKind::CommitWithMessage => {
                if let Some(cb) = self.checkboxes.get_mut(0) {
                    *cb = !*cb;
                }
            }
            DialogKind::CherryPick { .. } => {
                if self.dropdown_selected < 2 {
                    if let Some(cb) = self.checkboxes.get_mut(self.dropdown_selected) {
                        *cb = !*cb;
                    }
                }
            }
            DialogKind::Merge { .. } => {
                if self.dropdown_selected < 3 {
                    if let Some(cb) = self.checkboxes.get_mut(self.dropdown_selected) {
                        *cb = !*cb;
                    }
                }
            }
            DialogKind::Rebase { .. } => {
                if self.dropdown_selected < 2 {
                    if let Some(cb) = self.checkboxes.get_mut(self.dropdown_selected) {
                        *cb = !*cb;
                    }
                }
            }
            _ => {}
        }
    }

    fn confirm(&mut self) {
        let (target, action) = match &self.kind {
            DialogKind::AddTag { target } => {
                if self.input_value.trim().is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("Tag name cannot be empty".into()));
                    return;
                }
                let annotated = self.dropdown_selected == 0;
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
                let action = GitAction::Commit {
                    message: self.input_value.clone(),
                    amend,
                };
                (String::new(), action)
            }
            DialogKind::ConfirmDiscardFile { file } => {
                (file.clone(), GitAction::DiscardFile { file: file.clone() })
            }
            DialogKind::ConfirmDiscardAll => (String::new(), GitAction::DiscardAll),
            DialogKind::ConfirmStageAll => (String::new(), GitAction::StageAll),
            DialogKind::ConfirmUnstageAll => (String::new(), GitAction::UnstageAll),
            DialogKind::CleanUntracked => (String::new(), GitAction::CleanUntracked),
            DialogKind::ConfirmPopStash { stash_ref } => (stash_ref.clone(), GitAction::PopStash),
            DialogKind::ConfirmDropStash { stash_ref } => (stash_ref.clone(), GitAction::DropStash),
        };
        self.tx.send(AppEvent::ExecuteGitAction { target, action });
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.before.render(f, area);
        let dialog_area = centered_rect(60, 50, area);
        f.render_widget(Clear, dialog_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.ctx.color_theme.divider_fg));
        let inner = block.inner(dialog_area);
        f.render_widget(block, dialog_area);

        let lines = self.build_lines();
        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, inner);
    }

    fn build_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![];

        match &self.kind {
            DialogKind::AddTag { target } => {
                lines.push(Line::from(vec![Span::styled(
                    "Add Tag",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("Target: {}", target)));
                lines.push(Line::from(""));
                lines.push(Line::from("Tag name:"));
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        self.input_value.clone(),
                        Style::default().add_modifier(Modifier::UNDERLINED),
                    ),
                ]));
                lines.push(Line::from(""));
                lines.push(Line::from("Options:"));
                let annotated = if self.checkboxes.get(0).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                lines.push(Line::from(format!("{} Annotated tag", annotated)));
            }
            DialogKind::CreateBranch { target } => {
                lines.push(Line::from(vec![Span::styled(
                    "Create Branch",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("At: {}", target)));
                lines.push(Line::from(""));
                lines.push(Line::from("Branch name:"));
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        self.input_value.clone(),
                        Style::default().add_modifier(Modifier::UNDERLINED),
                    ),
                ]));
                lines.push(Line::from(""));
                let checkout = if self.checkboxes.get(0).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                lines.push(Line::from(format!("{} Checkout after creation", checkout)));
            }
            DialogKind::Checkout { target, is_branch } => {
                lines.push(Line::from(vec![Span::styled(
                    "Checkout",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                let kind = if *is_branch { "branch" } else { "commit" };
                lines.push(Line::from(format!("Checkout {}: {}", kind, target)));
            }
            DialogKind::CherryPick { target } => {
                lines.push(Line::from(vec![Span::styled(
                    "Cherry Pick",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("Commit: {}", target)));
                lines.push(Line::from(""));
                let record = if self.checkboxes.get(0).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                let no_commit = if self.checkboxes.get(1).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                lines.push(Line::from(format!("{} Record origin (-x)", record)));
                lines.push(Line::from(format!("{} No commit (-n)", no_commit)));
            }
            DialogKind::Revert { target } => {
                lines.push(Line::from(vec![Span::styled(
                    "Revert Commit",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("Commit: {}", target)));
            }
            DialogKind::Drop { target } => {
                lines.push(Line::from(vec![Span::styled(
                    "Drop Commit",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("Commit: {}", target)));
                lines.push(Line::from(""));
                lines.push(Line::from(
                    "Warning: This will permanently remove the commit.",
                ));
            }
            DialogKind::Merge { target, is_branch } => {
                lines.push(Line::from(vec![Span::styled(
                    "Merge",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                let kind = if *is_branch { "branch" } else { "commit" };
                lines.push(Line::from(format!("Merge {}: {}", kind, target)));
                lines.push(Line::from(""));
                let no_ff = if self.checkboxes.get(0).copied().unwrap_or(true) {
                    "[x]"
                } else {
                    "[ ]"
                };
                let squash = if self.checkboxes.get(1).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                let no_commit = if self.checkboxes.get(2).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                lines.push(Line::from(format!("{} No fast-forward (--no-ff)", no_ff)));
                lines.push(Line::from(format!("{} Squash (--squash)", squash)));
                lines.push(Line::from(format!("{} No commit (--no-commit)", no_commit)));
            }
            DialogKind::Rebase { target } => {
                lines.push(Line::from(vec![Span::styled(
                    "Rebase",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!(
                    "Rebase current branch onto: {}",
                    target
                )));
                lines.push(Line::from(""));
                let interactive = if self.checkboxes.get(0).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                let ignore_date = if self.checkboxes.get(1).copied().unwrap_or(true) {
                    "[x]"
                } else {
                    "[ ]"
                };
                lines.push(Line::from(format!("{} Interactive (-i)", interactive)));
                lines.push(Line::from(format!(
                    "{} Ignore date (--ignore-date)",
                    ignore_date
                )));
            }
            DialogKind::Reset { target } => {
                lines.push(Line::from(vec![Span::styled(
                    "Reset",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("Reset current branch to: {}", target)));
                lines.push(Line::from(""));
                let modes = vec![
                    "Soft - Keep all changes, but reset head",
                    "Mixed - Keep working tree, but reset index",
                    "Hard - Discard all changes",
                ];
                lines.push(Line::from("Mode:"));
                for (i, mode) in modes.iter().enumerate() {
                    let indicator = if i == self.dropdown_selected {
                        "> "
                    } else {
                        "  "
                    };
                    lines.push(Line::from(format!("{}{}", indicator, mode)));
                }
            }
            DialogKind::RenameBranch { branch } => {
                lines.push(Line::from(vec![Span::styled(
                    "Rename Branch",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("Current name: {}", branch)));
                lines.push(Line::from(""));
                lines.push(Line::from("New name:"));
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        self.input_value.clone(),
                        Style::default().add_modifier(Modifier::UNDERLINED),
                    ),
                ]));
            }
            DialogKind::DeleteBranch { branch, is_remote } => {
                lines.push(Line::from(vec![Span::styled(
                    "Delete Branch",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                let kind = if *is_remote {
                    "Remote branch"
                } else {
                    "Branch"
                };
                lines.push(Line::from(format!("{}: {}", kind, branch)));
                lines.push(Line::from(""));
                let force = if self.checkboxes.get(0).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                lines.push(Line::from(format!("{} Force delete", force)));
            }
            DialogKind::DeleteTag { tag } => {
                lines.push(Line::from(vec![Span::styled(
                    "Delete Tag",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("Tag: {}", tag)));
            }
            DialogKind::PushTag { tag } => {
                lines.push(Line::from(vec![Span::styled(
                    "Push Tag",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("Tag: {}", tag)));
            }
            DialogKind::PullBranch { branch } => {
                lines.push(Line::from(vec![Span::styled(
                    "Pull Branch",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("Branch: {}", branch)));
                lines.push(Line::from(""));
                let rebase = if self.checkboxes.get(0).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                lines.push(Line::from(format!("{} Rebase (--rebase)", rebase)));
            }
            DialogKind::PushBranch { branch } => {
                lines.push(Line::from(vec![Span::styled(
                    "Push Branch",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("Branch: {}", branch)));
                lines.push(Line::from(""));
                let force = if self.checkboxes.get(0).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                lines.push(Line::from(format!(
                    "{} Force with lease (--force-with-lease)",
                    force
                )));
            }
            DialogKind::CreateBranchFromStash {
                target: _,
                stash_ref,
            } => {
                lines.push(Line::from(vec![Span::styled(
                    "Create Branch from Stash",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(format!("Stash: {}", stash_ref)));
                lines.push(Line::from(""));
                lines.push(Line::from("Branch name:"));
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        self.input_value.clone(),
                        Style::default().add_modifier(Modifier::UNDERLINED),
                    ),
                ]));
                lines.push(Line::from(""));
                let checkout = if self.checkboxes.get(0).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                lines.push(Line::from(format!("{} Checkout after creation", checkout)));
            }
            DialogKind::StashWithMessage => {
                lines.push(Line::from(vec![Span::styled(
                    "Stash Changes",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(""));
                lines.push(Line::from("Message (optional):"));
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        self.input_value.clone(),
                        Style::default().add_modifier(Modifier::UNDERLINED),
                    ),
                ]));
                lines.push(Line::from(""));
                let untracked = if self.checkboxes.get(0).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                lines.push(Line::from(format!("{} Include untracked files", untracked)));
            }
            DialogKind::CommitWithMessage => {
                lines.push(Line::from(vec![Span::styled(
                    "Commit",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(""));
                lines.push(Line::from("Message:"));
                lines.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        self.input_value.clone(),
                        Style::default().add_modifier(Modifier::UNDERLINED),
                    ),
                ]));
                lines.push(Line::from(""));
                let amend = if self.checkboxes.get(0).copied().unwrap_or(false) {
                    "[x]"
                } else {
                    "[ ]"
                };
                lines.push(Line::from(format!("{} Amend previous commit", amend)));
            }
            DialogKind::ConfirmDiscardFile { file } => {
                lines.push(Line::from(vec![Span::styled(
                    "Confirm Discard File",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(""));
                lines.push(Line::from(format!("Discard changes to '{}' ?", file)));
                lines.push(Line::from("This action cannot be undone."));
            }
            DialogKind::ConfirmDiscardAll => {
                lines.push(Line::from(vec![Span::styled(
                    "Confirm Discard All",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(""));
                lines.push(Line::from("Discard all unstaged changes?"));
                lines.push(Line::from("This action cannot be undone."));
            }
            DialogKind::ConfirmStageAll => {
                lines.push(Line::from(vec![Span::styled(
                    "Confirm Stage All",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(""));
                lines.push(Line::from("Stage all changes?"));
            }
            DialogKind::ConfirmUnstageAll => {
                lines.push(Line::from(vec![Span::styled(
                    "Confirm Unstage All",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(""));
                lines.push(Line::from("Unstage all changes?"));
            }
            DialogKind::CleanUntracked => {
                lines.push(Line::from(vec![Span::styled(
                    "Clean Untracked Files",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(""));
                lines.push(Line::from("Remove all untracked files and directories?"));
                lines.push(Line::from("This action cannot be undone."));
            }
            DialogKind::ConfirmPopStash { stash_ref } => {
                lines.push(Line::from(vec![Span::styled(
                    "Pop Stash",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "Apply and remove stash '{}' ?",
                    stash_ref
                )));
            }
            DialogKind::ConfirmDropStash { stash_ref } => {
                lines.push(Line::from(vec![Span::styled(
                    "Drop Stash",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                lines.push(Line::from(""));
                lines.push(Line::from(format!(
                    "Permanently remove stash '{}' ?",
                    stash_ref
                )));
                lines.push(Line::from("This action cannot be undone."));
            }
        }

        // Footer
        lines.push(Line::from(""));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Enter:confirm Esc:cancel",
            Style::default().fg(Color::Gray),
        )]));

        lines
    }

    pub fn take_before_view(&mut self) -> View<'a> {
        std::mem::take(&mut self.before)
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);
    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
