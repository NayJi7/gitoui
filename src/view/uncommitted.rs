use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::Rect,
    Frame,
};

use crate::{
    app::AppContext,
    event::{AppEvent, DialogKind, Sender, UserEvent, UserEventWithCount},
    widget::commit_list::{CommitList, CommitListState},
    widget::uncommitted::{UncommittedFile, UncommittedSection, UncommittedState, UncommittedWidget},
};

#[derive(Debug)]
pub struct UncommittedView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    ctx: Rc<AppContext>,
    tx: Sender,
    unstaged: Vec<UncommittedFile>,
    staged: Vec<UncommittedFile>,
    untracked: Vec<UncommittedFile>,
    state: UncommittedState,
    detail_area: Option<Rect>,
}

impl<'a> UncommittedView<'a> {
    pub fn new(
        unstaged: Vec<UncommittedFile>,
        staged: Vec<UncommittedFile>,
        untracked: Vec<UncommittedFile>,
        commit_list_state: Option<CommitListState<'a>>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        Self {
            commit_list_state,
            ctx,
            tx,
            unstaged,
            staged,
            untracked,
            state: UncommittedState::default(),
            detail_area: None,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _key_event: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;

        match event {
            UserEvent::NavigateDown => {
                for _ in 0..count {
                    self.state.select_next_global(self.unstaged.len(), self.staged.len(), self.untracked.len());
                }
            }
            UserEvent::NavigateUp => {
                for _ in 0..count {
                    self.state.select_prev_global(self.unstaged.len(), self.staged.len(), self.untracked.len());
                }
            }
            UserEvent::NavigateRight | UserEvent::NavigateLeft => {
                self.state.switch_section();
            }
            UserEvent::Stage => {
                if matches!(self.state.section, UncommittedSection::Unstaged | UncommittedSection::Untracked) {
                    if let Some(file) = self.state.selected_file(&self.unstaged, &self.staged, &self.untracked) {
                        self.tx.send(AppEvent::StageFile {
                            file: file.path.clone(),
                        });
                    }
                }
            }
            UserEvent::StageAll => {
                if !self.unstaged.is_empty() || !self.untracked.is_empty() {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmStageAll));
                }
            }
            UserEvent::Unstage => {
                if self.state.section == UncommittedSection::Staged {
                    if let Some(file) = self.state.selected_file(&self.unstaged, &self.staged, &self.untracked) {
                        self.tx.send(AppEvent::UnstageFile {
                            file: file.path.clone(),
                        });
                    }
                }
            }
            UserEvent::UnstageAll => {
                if !self.staged.is_empty() {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmUnstageAll));
                }
            }
            UserEvent::Discard => {
                if let Some(file) = self.state.selected_file(&self.unstaged, &self.staged, &self.untracked) {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmDiscardFile {
                        file: file.path.clone(),
                    }));
                }
            }
            UserEvent::DiscardAll => {
                if !self.unstaged.is_empty() || !self.staged.is_empty() {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmDiscardAll));
                }
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
        let detail_height = if self.commit_list_state.is_some() {
            (area.height - 1).min(self.ctx.ui_config.detail.height)
        } else {
            area.height
        };
        let [list_area, detail_area] =
            ratatui::layout::Layout::vertical([
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(detail_height),
            ])
            .areas(area);

        if let Some(ref mut list_state) = self.commit_list_state {
            let commit_list = CommitList::new(self.ctx.clone());
            f.render_stateful_widget(commit_list, list_area, list_state);
        }

        let widget = UncommittedWidget::new(&self.unstaged, &self.staged, &self.untracked, self.ctx.clone());
        f.render_stateful_widget(widget, detail_area, &mut self.state);
        self.detail_area = Some(detail_area);
    }

    pub fn update_layout(&mut self, area: Rect) {
        if let Some(ref mut list_state) = self.commit_list_state {
            let detail_height = (area.height - 1).min(self.ctx.ui_config.detail.height);
            let [list_area, _] =
                ratatui::layout::Layout::vertical([
                    ratatui::layout::Constraint::Min(0),
                    ratatui::layout::Constraint::Length(detail_height),
                ])
                .areas(area);
            list_state.update_height(list_area.height as usize);
        }
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(detail_area) = self.detail_area {
            if col >= detail_area.x + (detail_area.width as f32 * 0.6) as u16
                && row >= detail_area.y
            {
                let action_bar_row = (row - detail_area.y).saturating_sub(3) as usize;
                if action_bar_row == 0 {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::StashWithMessage));
                } else if action_bar_row == 1 {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::CommitWithMessage));
                } else if action_bar_row == 2 {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::CleanUntracked));
                }
                return;
            }

            let files_area_x_end = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            if col >= detail_area.x && col < files_area_x_end && row >= detail_area.y {
                let local_row = row.saturating_sub(detail_area.y + 1) as usize;
                // Approximate layout: title(0), empty(1), unstaged header(2), files(3..), empty, staged header, files, empty, untracked header, files
                let unstaged_count = self.unstaged.len().max(1) + 2; // header + files + empty
                let staged_count = self.staged.len().max(1) + 2; // header + files + empty
                if local_row < unstaged_count {
                    self.state.section = UncommittedSection::Unstaged;
                    let file_row = local_row.saturating_sub(1);
                    if file_row < self.unstaged.len() {
                        self.state.selected = file_row;
                    }
                } else if local_row < unstaged_count + staged_count {
                    self.state.section = UncommittedSection::Staged;
                    let file_row = local_row.saturating_sub(unstaged_count + 1);
                    if file_row < self.staged.len() {
                        self.state.selected = file_row;
                    }
                } else {
                    self.state.section = UncommittedSection::Untracked;
                    let file_row = local_row.saturating_sub(unstaged_count + staged_count + 1);
                    if file_row < self.untracked.len() {
                        self.state.selected = file_row;
                    }
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        if let Some(detail_area) = self.detail_area {
            if col >= detail_area.x + (detail_area.width as f32 * 0.6) as u16
                && row >= detail_area.y
            {
                let action_bar_row = (row - detail_area.y).saturating_sub(3) as usize;
                self.state.hovered_action = if action_bar_row < 3 {
                    Some(action_bar_row)
                } else {
                    None
                };
                return;
            }

            let files_area_x_end = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            if col >= detail_area.x && col < files_area_x_end && row >= detail_area.y {
                let local_row = row.saturating_sub(detail_area.y + 1) as usize;
                let unstaged_count = self.unstaged.len().max(1) + 2;
                let staged_count = self.staged.len().max(1) + 2;
                if local_row < unstaged_count {
                    self.state.section = UncommittedSection::Unstaged;
                    let file_row = local_row.saturating_sub(1);
                    if file_row < self.unstaged.len() {
                        self.state.selected = file_row;
                    }
                } else if local_row < unstaged_count + staged_count {
                    self.state.section = UncommittedSection::Staged;
                    let file_row = local_row.saturating_sub(unstaged_count + 1);
                    if file_row < self.staged.len() {
                        self.state.selected = file_row;
                    }
                } else {
                    self.state.section = UncommittedSection::Untracked;
                    let file_row = local_row.saturating_sub(unstaged_count + staged_count + 1);
                    if file_row < self.untracked.len() {
                        self.state.selected = file_row;
                    }
                }
                self.state.hovered_action = None;
                return;
            }
        }
        self.state.hovered_action = None;
    }

    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
    }

    pub fn set_list_state(&mut self, state: CommitListState<'a>) {
        self.commit_list_state = Some(state);
    }

    pub fn prepare_graph_uploads(&mut self) {
        if let Some(ref mut list_state) = self.commit_list_state {
            list_state.ensure_visible_graph_uploaded();
        }
    }

    pub fn clear_graph_images(&mut self) {
        if let Some(ref mut list_state) = self.commit_list_state {
            list_state.clear_graph_images();
        }
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        if let Some(ref mut list_state) = self.commit_list_state {
            list_state.drain_pending_graph_uploads()
        } else {
            Vec::new()
        }
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        if let Some(ref list_state) = self.commit_list_state {
            list_state.graph_image_ids_sorted()
        } else {
            Vec::new()
        }
    }

    pub fn footer_hint(&self) -> String {
        let has_unstaged = !self.unstaged.is_empty() || !self.untracked.is_empty();
        let has_staged = !self.staged.is_empty();
        let can_stage = has_unstaged;
        let can_unstage = has_staged;
        let can_discard = has_unstaged || has_staged;
        let mut parts = Vec::new();
        if can_stage {
            parts.push("a:stage".to_string());
            if has_unstaged || !self.untracked.is_empty() {
                parts.push("A:stage-all".to_string());
            }
        }
        if can_unstage {
            parts.push("u:unstage".to_string());
            if has_staged {
                parts.push("U:unstage-all".to_string());
            }
        }
        if can_discard {
            parts.push("x:discard".to_string());
            if has_unstaged || has_staged {
                parts.push("X:discard-all".to_string());
            }
        }
        parts.push("Esc:close".to_string());
        parts.join(" ")
    }
}
