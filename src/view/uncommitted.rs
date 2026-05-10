use std::rc::Rc;

use ratatui::{crossterm::event::KeyEvent, layout::Rect, Frame};

use crate::{
    app::AppContext,
    event::{AppEvent, DialogKind, Sender, UserEvent, UserEventWithCount},
    git::status::StatusType,
    widget::commit_list::{CommitList, CommitListState},
    widget::uncommitted::{
        UncommittedFile, UncommittedSection, UncommittedState, UncommittedWidget,
    },
};

#[derive(Debug)]
pub struct UncommittedView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    ctx: Rc<AppContext>,
    tx: Sender,
    pub unstaged: Vec<UncommittedFile>,
    pub staged: Vec<UncommittedFile>,
    pub untracked: Vec<UncommittedFile>,
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
        let mut state = UncommittedState::default();
        // Select the first available file by default (Staged -> Unstaged -> Untracked)
        if !staged.is_empty() {
            state.section = UncommittedSection::Staged;
            state.selected = 0;
        } else if !unstaged.is_empty() {
            state.section = UncommittedSection::Unstaged;
            state.selected = 0;
        } else if !untracked.is_empty() {
            state.section = UncommittedSection::Untracked;
            state.selected = 0;
        }
        Self {
            commit_list_state,
            ctx,
            tx,
            unstaged,
            staged,
            untracked,
            state,
            detail_area: None,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _key_event: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;

        match event {
            UserEvent::NavigateDown => {
                for _ in 0..count {
                    self.state.select_next_global(
                        self.staged.len(),
                        self.unstaged.len(),
                        self.untracked.len(),
                    );
                }
            }
            UserEvent::NavigateUp => {
                for _ in 0..count {
                    self.state.select_prev_global(
                        self.staged.len(),
                        self.unstaged.len(),
                        self.untracked.len(),
                    );
                }
            }
            UserEvent::ScrollDown => {
                for _ in 0..count {
                    self.state.scroll_down();
                }
            }
            UserEvent::ScrollUp => {
                for _ in 0..count {
                    self.state.scroll_up();
                }
            }
            UserEvent::NavigateRight => {
                self.state.switch_section_forward(
                    self.staged.len(),
                    self.unstaged.len(),
                    self.untracked.len(),
                );
            }
            UserEvent::NavigateLeft => {
                self.state.switch_section_backward(
                    self.staged.len(),
                    self.unstaged.len(),
                    self.untracked.len(),
                );
            }
            UserEvent::Stage => {
                if matches!(
                    self.state.section,
                    UncommittedSection::Unstaged | UncommittedSection::Untracked
                ) {
                    if let Some(file) =
                        self.state
                            .selected_file(&self.unstaged, &self.staged, &self.untracked)
                    {
                        self.tx.send(AppEvent::StageFile {
                            file: file.path.clone(),
                        });
                    }
                }
            }
            UserEvent::StageAll => {
                if !self.unstaged.is_empty() || !self.untracked.is_empty() {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::ConfirmStageAll));
                }
            }
            UserEvent::Unstage => {
                if self.state.section == UncommittedSection::Staged {
                    if let Some(file) =
                        self.state
                            .selected_file(&self.unstaged, &self.staged, &self.untracked)
                    {
                        self.tx.send(AppEvent::UnstageFile {
                            file: file.path.clone(),
                        });
                    }
                }
            }
            UserEvent::UnstageAll | UserEvent::Pull => {
                if !self.staged.is_empty() {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::ConfirmUnstageAll));
                }
            }
            UserEvent::Discard => {
                if let Some(file) =
                    self.state
                        .selected_file(&self.unstaged, &self.staged, &self.untracked)
                {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::ConfirmDiscardFile {
                            file: file.path.clone(),
                        }));
                }
            }
            UserEvent::DiscardAll => {
                if !self.unstaged.is_empty() || !self.staged.is_empty() {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::ConfirmDiscardAll));
                }
            }
            UserEvent::Stash | UserEvent::Reset => {
                self.tx
                    .send(AppEvent::OpenDialog(DialogKind::StashWithMessage));
            }
            UserEvent::Commit => {
                self.tx
                    .send(AppEvent::OpenDialog(DialogKind::CommitWithMessage));
            }
            UserEvent::CleanUntracked => {
                self.tx
                    .send(AppEvent::OpenDialog(DialogKind::CleanUntracked));
            }
            UserEvent::Confirm => {
                if let Some(file) =
                    self.state
                        .selected_file(&self.unstaged, &self.staged, &self.untracked)
                {
                    if file.status == StatusType::Unmerged {
                        self.tx.send(AppEvent::NotifyWarn(
                            "Conflicted file — edit to resolve, then press [a] to mark as resolved".to_string(),
                        ));
                    } else if file.status == StatusType::Deleted {
                        self.tx.send(AppEvent::NotifyWarn(
                            "Cannot view diff for a deleted file.".to_string(),
                        ));
                    } else {
                        let is_staged = self.state.section
                            == crate::widget::uncommitted::UncommittedSection::Staged;
                        self.tx.send(AppEvent::OpenUncommittedDiff {
                            file_path: file.path.clone(),
                            is_staged,
                        });
                    }
                }
            }
            UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::CloseDetail);
            }
            UserEvent::Refresh => {
                self.refresh();
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let detail_height = if self.commit_list_state.is_some() {
            crate::view::adaptive_detail_height(
                area.height,
                self.ctx.ui_config.detail.height,
                5,
            )
        } else {
            area.height
        };
        let [list_area, detail_area] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Min(0),
            ratatui::layout::Constraint::Length(detail_height),
        ])
        .areas(area);

        if let Some(ref mut list_state) = self.commit_list_state {
            let commit_list = CommitList::new(self.ctx.clone());
            f.render_stateful_widget(commit_list, list_area, list_state);
        }

        let widget = UncommittedWidget::new(
            &self.unstaged,
            &self.staged,
            &self.untracked,
            self.ctx.clone(),
        );
        f.render_stateful_widget(widget, detail_area, &mut self.state);
        self.detail_area = Some(detail_area);
    }

    pub fn update_layout(&mut self, area: Rect) {
        if let Some(ref mut list_state) = self.commit_list_state {
            let detail_height = crate::view::adaptive_detail_height(
                area.height,
                self.ctx.ui_config.detail.height,
                5,
            );
            let [list_area, _] = ratatui::layout::Layout::vertical([
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
                let action_bar_row = (row - detail_area.y).saturating_sub(4) as usize;
                if action_bar_row == 0 {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::StashWithMessage));
                } else if action_bar_row == 1 {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::CommitWithMessage));
                } else if action_bar_row == 2 {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::CleanUntracked));
                }
                return;
            }

            let files_area_x_end = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            let mut clicked_on_file = false;
            if col >= detail_area.x && col < files_area_x_end && row >= detail_area.y {
                // Widget layout: separator(0) + title(1) + underline(2) + spacer(3) + content(4+)
                let visible_row = row.saturating_sub(detail_area.y + 4) as usize;

                // Content layout: each section takes N lines (N=1 if empty, N=len if files)
                // Separators between sections add 1 line each
                let staged_rows = if self.staged.is_empty() {
                    1
                } else {
                    self.staged.len()
                };
                let unstaged_rows = if self.unstaged.is_empty() {
                    1
                } else {
                    self.unstaged.len()
                };
                let untracked_rows = if self.untracked.is_empty() {
                    1
                } else {
                    self.untracked.len()
                };
                let total_lines = staged_rows + 1 + unstaged_rows + 1 + untracked_rows;

                // Ignore clicks outside the visible content area or beyond total data
                if visible_row < self.state.height {
                    let local_row = visible_row + self.state.offset;
                    if local_row < total_lines {
                        let unstaged_start = staged_rows + 1; // +1 separator
                        let untracked_start = unstaged_start + unstaged_rows + 1; // +1 separator

                        // Files start at section start (title row = first file row)
                        if local_row < staged_rows && !self.staged.is_empty() {
                            self.state.section = UncommittedSection::Staged;
                            self.state.selected = local_row;
                            clicked_on_file = true;
                        } else if local_row >= unstaged_start
                            && local_row < unstaged_start + unstaged_rows
                            && !self.unstaged.is_empty()
                        {
                            self.state.section = UncommittedSection::Unstaged;
                            self.state.selected = local_row - unstaged_start;
                            clicked_on_file = true;
                        } else if local_row >= untracked_start
                            && local_row < untracked_start + untracked_rows
                            && !self.untracked.is_empty()
                        {
                            self.state.section = UncommittedSection::Untracked;
                            self.state.selected = local_row - untracked_start;
                            clicked_on_file = true;
                        }
                    }
                }
            }

            // Open diff only when clicking directly on a file
            if clicked_on_file {
                if let Some(file) =
                    self.state
                        .selected_file(&self.unstaged, &self.staged, &self.untracked)
                {
                    if file.status == StatusType::Deleted {
                        self.tx.send(AppEvent::NotifyWarn(
                            "Cannot view diff for a deleted file.".to_string(),
                        ));
                    } else {
                        let is_staged = self.state.section
                            == crate::widget::uncommitted::UncommittedSection::Staged;
                        self.tx.send(AppEvent::OpenUncommittedDiff {
                            file_path: file.path.clone(),
                            is_staged,
                        });
                    }
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        // Block hover on commit list when in uncommitted detail view
        // Only process hover inside the detail area
        if let Some(detail_area) = self.detail_area {
            if row < detail_area.y {
                // Mouse is in commit list area - do nothing (block hover)
                return;
            }
        } else {
            return;
        }

        if let Some(detail_area) = self.detail_area {
            // Action bar hover (right side)
            if col >= detail_area.x + (detail_area.width as f32 * 0.6) as u16
                && row >= detail_area.y
            {
                let action_bar_row = (row - detail_area.y).saturating_sub(4) as usize;
                self.state.hovered_action = if action_bar_row < 3 {
                    Some(action_bar_row)
                } else {
                    None
                };
                return;
            }

            // Files area hover (left side)
            let files_area_x_end = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            if col >= detail_area.x && col < files_area_x_end && row >= detail_area.y {
                // Widget layout: separator(0) + title(1) + underline(2) + spacer(3) + content(4+)
                let scroll_start = detail_area.y + 4;
                let visible_row = row.saturating_sub(scroll_start) as usize;

                // Content layout: each section takes N lines (N=1 if empty, N=len if files)
                // Separators between sections add 1 line each
                let staged_rows = if self.staged.is_empty() {
                    1
                } else {
                    self.staged.len()
                };
                let unstaged_rows = if self.unstaged.is_empty() {
                    1
                } else {
                    self.unstaged.len()
                };
                let untracked_rows = if self.untracked.is_empty() {
                    1
                } else {
                    self.untracked.len()
                };
                let total_lines = staged_rows + 1 + unstaged_rows + 1 + untracked_rows;

                // Only update selection if mouse is inside visible content and within data bounds
                if visible_row < self.state.height {
                    let local_row = visible_row + self.state.offset;
                    if local_row < total_lines {
                        let unstaged_start = staged_rows + 1; // +1 separator
                        let untracked_start = unstaged_start + unstaged_rows + 1; // +1 separator

                        // Files start at section start (title row = first file row)
                        if local_row < staged_rows && !self.staged.is_empty() {
                            self.state.section = UncommittedSection::Staged;
                            self.state.selected = local_row;
                        } else if local_row >= unstaged_start
                            && local_row < unstaged_start + unstaged_rows
                            && !self.unstaged.is_empty()
                        {
                            self.state.section = UncommittedSection::Unstaged;
                            self.state.selected = local_row - unstaged_start;
                        } else if local_row >= untracked_start
                            && local_row < untracked_start + untracked_rows
                            && !self.untracked.is_empty()
                        {
                            self.state.section = UncommittedSection::Untracked;
                            self.state.selected = local_row - untracked_start;
                        }
                    }
                }
            } else {
                self.state.hovered_action = None;
            }
        }
    }

    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.state
            .selected_file(&self.unstaged, &self.staged, &self.untracked)
            .map(|f| f.path.as_str())
    }

    pub fn section(&self) -> UncommittedSection {
        self.state.section
    }

    pub fn reselect(&mut self, path: &str) {
        if let Some(idx) = self.staged.iter().position(|f| f.path == path) {
            self.state.section = UncommittedSection::Staged;
            self.state.selected = idx;
        } else if let Some(idx) = self.unstaged.iter().position(|f| f.path == path) {
            self.state.section = UncommittedSection::Unstaged;
            self.state.selected = idx;
        } else if let Some(idx) = self.untracked.iter().position(|f| f.path == path) {
            self.state.section = UncommittedSection::Untracked;
            self.state.selected = idx;
        } else {
            let files: &[UncommittedFile] = match self.state.section {
                UncommittedSection::Staged => &self.staged,
                UncommittedSection::Unstaged => &self.unstaged,
                UncommittedSection::Untracked => &self.untracked,
            };
            self.state.selected = self.state.selected.min(files.len().saturating_sub(1));
        }
    }

    #[allow(dead_code)]
    pub fn set_list_state(&mut self, state: CommitListState<'a>) {
        self.commit_list_state = Some(state);
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        std::rc::Rc::make_mut(&mut self.ctx).color_theme = theme;
    }

    pub fn refresh(&self) {
        self.tx.send(AppEvent::RefreshUncommitted);
    }

    pub fn prepare_graph_uploads(&mut self) {
        if let Some(ref mut list_state) = self.commit_list_state {
            list_state.ensure_visible_graph_uploaded();
            list_state.ensure_visible_avatars_uploaded(
                &mut self.ctx.avatar_manager.lock().unwrap(),
                self.ctx.color_theme.bg,
                self.ctx.color_theme.list_selected_bg,
            );
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
        let has_conflicts = self
            .unstaged
            .iter()
            .any(|f| f.status == StatusType::Unmerged);
        let has_unstaged = !self.unstaged.is_empty() || !self.untracked.is_empty();
        let has_staged = !self.staged.is_empty();
        let can_unstage = has_staged;
        let can_discard = has_unstaged || has_staged;
        let mut parts = Vec::new();
        if has_conflicts {
            parts.push("a:resolve".to_string());
        } else if has_unstaged {
            parts.push("a:stage".to_string());
            parts.push("A:stage-all".to_string());
        }
        if can_unstage {
            parts.push("u:unstage".to_string());
            if has_staged {
                parts.push("B:unstage-all".to_string());
            }
        }
        if can_discard {
            parts.push("x:discard".to_string());
            if has_unstaged || has_staged {
                parts.push("X:discard-all".to_string());
            }
        }
        parts.push("r:fetch".to_string());
        parts.push("Esc:close".to_string());
        format!("⌘ {}", parts.join("▕▏"))
    }
}
