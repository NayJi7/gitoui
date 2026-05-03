use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::Rect,
    Frame,
};

use crate::{
    app::AppContext,
    event::{AppEvent, DialogKind, Sender, UserEvent, UserEventWithCount},
    widget::uncommitted::{UncommittedFile, UncommittedSection, UncommittedState, UncommittedWidget},
};

#[derive(Debug)]
pub struct UncommittedView<'a> {
    _phantom: std::marker::PhantomData<&'a ()>,
    ctx: Rc<AppContext>,
    tx: Sender,
    unstaged: Vec<UncommittedFile>,
    staged: Vec<UncommittedFile>,
    state: UncommittedState,
    files_area: Option<Rect>,
    action_bar_area: Option<Rect>,
}

impl<'a> UncommittedView<'a> {
    pub fn new(
        unstaged: Vec<UncommittedFile>,
        staged: Vec<UncommittedFile>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
            ctx,
            tx,
            unstaged,
            staged,
            state: UncommittedState::default(),
            files_area: None,
            action_bar_area: None,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _key_event: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;

        match event {
            UserEvent::NavigateDown => {
                for _ in 0..count {
                    let total = self.state.total_in_section(self.unstaged.len(), self.staged.len());
                    self.state.select_next(total);
                }
            }
            UserEvent::NavigateUp => {
                for _ in 0..count {
                    self.state.select_prev();
                }
            }
            UserEvent::NavigateRight | UserEvent::NavigateLeft => {
                self.state.switch_section();
            }
            UserEvent::Stage => {
                if let Some(file) = self.state.selected_file(&self.unstaged, &self.staged) {
                    self.tx.send(AppEvent::StageFile {
                        file: file.path.clone(),
                    });
                }
            }
            UserEvent::StageAll => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmStageAll));
            }
            UserEvent::Unstage => {
                if let Some(file) = self.state.selected_file(&self.unstaged, &self.staged) {
                    self.tx.send(AppEvent::UnstageFile {
                        file: file.path.clone(),
                    });
                }
            }
            UserEvent::UnstageAll => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmUnstageAll));
            }
            UserEvent::Discard => {
                if let Some(file) = self.state.selected_file(&self.unstaged, &self.staged) {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmDiscardFile {
                        file: file.path.clone(),
                    }));
                }
            }
            UserEvent::DiscardAll => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmDiscardAll));
            }
            UserEvent::Stash => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::StashWithMessage));
            }
            UserEvent::Commit => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::CommitWithMessage));
            }
            UserEvent::CleanUntracked => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::CleanUntracked));
            }
            UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::CloseDetail);
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let widget = UncommittedWidget::new(&self.unstaged, &self.staged, self.ctx.clone());
        f.render_stateful_widget(widget, area, &mut self.state);

        // Store areas for click handling
        let [files_area, action_bar_area] = ratatui::layout::Layout::horizontal([
            ratatui::layout::Constraint::Percentage(60),
            ratatui::layout::Constraint::Percentage(40),
        ])
        .areas(area);
        self.files_area = Some(files_area);
        self.action_bar_area = Some(action_bar_area);
    }

    pub fn update_layout(&mut self, area: Rect) {
        let [files_area, action_bar_area] = ratatui::layout::Layout::horizontal([
            ratatui::layout::Constraint::Percentage(60),
            ratatui::layout::Constraint::Percentage(40),
        ])
        .areas(area);
        self.files_area = Some(files_area);
        self.action_bar_area = Some(action_bar_area);
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(action_bar_area) = self.action_bar_area {
            if col >= action_bar_area.x && col < action_bar_area.x + action_bar_area.width
                && row >= action_bar_area.y && row < action_bar_area.y + action_bar_area.height
            {
                let action_bar_row = row.saturating_sub(action_bar_area.y + 1) as usize;
                if action_bar_row == 0 {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::StashWithMessage));
                } else if action_bar_row == 1 {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::CommitWithMessage));
                } else if action_bar_row == 2 {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::CleanUntracked));
                }
                return;
            }
        }

        // Click in files area - could implement file selection here
        // For now, just handle switching sections
        if let Some(files_area) = self.files_area {
            if col >= files_area.x
                && col < files_area.x + files_area.width
                && row >= files_area.y
                && row < files_area.y + files_area.height
            {
                // Simple click handling: switch section based on approximate position
                let local_row = row.saturating_sub(files_area.y + 3) as usize;
                let unstaged_count = self.unstaged.len().saturating_add(1);
                if local_row < unstaged_count {
                    self.state.section = UncommittedSection::Unstaged;
                    if local_row > 0 && local_row <= self.unstaged.len() {
                        self.state.selected = local_row - 1;
                    }
                } else {
                    self.state.section = UncommittedSection::Staged;
                    let staged_row = local_row.saturating_sub(unstaged_count + 2);
                    if staged_row < self.staged.len() {
                        self.state.selected = staged_row;
                    }
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        if let Some(action_bar_area) = self.action_bar_area {
            if col >= action_bar_area.x && col < action_bar_area.x + action_bar_area.width
                && row >= action_bar_area.y && row < action_bar_area.y + action_bar_area.height
            {
                let action_bar_row = row.saturating_sub(action_bar_area.y + 1) as usize;
                self.state.hovered_action = if action_bar_row < 3 {
                    Some(action_bar_row)
                } else {
                    None
                };
                return;
            }
        }
        self.state.hovered_action = None;
    }
}
