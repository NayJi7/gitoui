use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    Frame,
};

use crate::{
    app::AppContext,
    event::{AppEvent, DialogKind, Sender, UserEvent, UserEventWithCount},
    git::{Commit, CommitType, FileChange, Ref, Repository},
    view::{ListRefreshViewContext, RefreshViewContext},
    widget::{
        commit_detail::{CommitDetail, CommitDetailState},
        commit_list::{CommitList, CommitListState},
    },
};

#[derive(Debug)]
pub struct DetailView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    commit_detail_state: CommitDetailState,

    commit: Commit,
    changes: Vec<FileChange>,
    refs: Vec<Ref>,
    head_branch_name: Option<String>,

    ctx: Rc<AppContext>,
    tx: Sender,
    list_height: usize,
    detail_area: Option<Rect>,
}

impl<'a> DetailView<'a> {
    pub fn new(
        commit_list_state: CommitListState<'a>,
        commit: Commit,
        changes: Vec<FileChange>,
        refs: Vec<Ref>,
        head_branch_name: Option<String>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> DetailView<'a> {
        DetailView {
            commit_list_state: Some(commit_list_state),
            commit_detail_state: CommitDetailState::default(),
            commit,
            changes,
            refs,
            head_branch_name,
            ctx,
            tx,
            list_height: 0,
            detail_area: None,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;

        match event {
            UserEvent::NavigateDown => {
                for _ in 0..count {
                    self.commit_detail_state.select_next_file(self.changes.len());
                }
            }
            UserEvent::NavigateUp => {
                for _ in 0..count {
                    self.commit_detail_state.select_prev_file();
                }
            }
            UserEvent::NavigateRight => {
                self.commit_detail_state.select_last_file(self.changes.len());
            }
            UserEvent::NavigateLeft => {
                self.commit_detail_state.select_first_file();
            }
            UserEvent::PageDown => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_page_down();
                }
            }
            UserEvent::PageUp => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_page_up();
                }
            }
            UserEvent::HalfPageDown => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_half_page_down();
                }
            }
            UserEvent::HalfPageUp => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_half_page_up();
                }
            }
            UserEvent::ScrollDown => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_down();
                }
            }
            UserEvent::ScrollUp => {
                for _ in 0..count {
                    self.commit_detail_state.scroll_up();
                }
            }
            UserEvent::GoToTop => {
                self.commit_detail_state.select_first();
            }
            UserEvent::GoToBottom => {
                self.commit_detail_state.select_last();
            }
            UserEvent::SelectDown => {
                self.tx.send(AppEvent::SelectOlderCommit);
            }
            UserEvent::SelectUp => {
                self.tx.send(AppEvent::SelectNewerCommit);
            }
            UserEvent::GoToParent => {
                self.tx.send(AppEvent::SelectParentCommit);
            }
            UserEvent::ShortCopy => {
                self.copy_commit_hash();
            }
            UserEvent::FullCopy => {
                self.copy_commit_message();
            }
            UserEvent::UserCommand(n) => {
                self.tx.send(AppEvent::OpenUserCommand(n));
            }
            UserEvent::HelpToggle => {
                self.tx.send(AppEvent::OpenHelp);
            }
            UserEvent::Confirm => {
                self.open_selected_file_diff();
            }
            UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::CloseDetail);
            }
            UserEvent::Refresh => {
                self.refresh();
            }
            UserEvent::AddTag => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::AddTag { target: self.commit.commit_hash.as_str().into() }));
            }
            UserEvent::CreateBranch => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::CreateBranch { target: self.commit.commit_hash.as_str().into() }));
            }
            UserEvent::Checkout => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Checkout { target: self.commit.commit_hash.as_str().into(), is_branch: false }));
            }
            UserEvent::CherryPick => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::CherryPick { target: self.commit.commit_hash.as_str().into() }));
            }
            UserEvent::Revert => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Revert { target: self.commit.commit_hash.as_str().into() }));
            }
            UserEvent::Drop => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Drop { target: self.commit.commit_hash.as_str().into() }));
            }
            UserEvent::Merge => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Merge { target: self.commit.commit_hash.as_str().into(), is_branch: false }));
            }
            UserEvent::Rebase => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Rebase { target: self.commit.commit_hash.as_str().into() }));
            }
            UserEvent::Reset => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Reset { target: self.commit.commit_hash.as_str().into() }));
            }
            UserEvent::ApplyStash => {
                self.tx.send(AppEvent::ExecuteGitAction {
                    target: self.commit.commit_hash.as_str().into(),
                    action: crate::event::GitAction::ApplyStash,
                });
            }
            UserEvent::PopStash => {
                if let Some(stash_ref) = self.stash_ref() {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmPopStash { stash_ref }));
                }
            }
            UserEvent::DropStash => {
                if let Some(stash_ref) = self.stash_ref() {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmDropStash { stash_ref }));
                }
            }
            UserEvent::CreateBranchFromStash => {
                if let Some(stash_ref) = self.stash_ref() {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::CreateBranchFromStash {
                        target: self.commit.commit_hash.as_str().into(),
                        stash_ref,
                    }));
                }
            }
            UserEvent::CopyStashName => {
                if let Some(stash_ref) = self.stash_ref() {
                    self.copy_to_clipboard("Stash name".into(), stash_ref);
                }
            }
            UserEvent::CopyStashHash => {
                self.copy_commit_hash();
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let [list_area, detail_area] = self.split_areas(area);
        self.detail_area = Some(detail_area);

        let commit_list = CommitList::new(self.ctx.clone());
        f.render_stateful_widget(commit_list, list_area, self.as_mut_list_state());

        let commit_detail = CommitDetail::new(
            &self.commit,
            &self.changes,
            &self.refs,
            self.ctx.clone(),
            self.head_branch_name.clone(),
        );
        f.render_stateful_widget(commit_detail, detail_area, &mut self.commit_detail_state);
    }

    pub fn update_layout(&mut self, area: Rect) {
        let [list_area, _] = self.split_areas(area);
        self.list_height = list_area.height as usize;
        self.as_mut_list_state()
            .update_height(list_area.height as usize);
    }

    pub fn prepare_graph_uploads(&mut self) {
        self.as_mut_list_state().ensure_visible_graph_uploaded();
    }

    pub fn clear_graph_images(&mut self) {
        self.as_mut_list_state().clear_graph_images();
    }
}

impl<'a> DetailView<'a> {
    pub fn take_list_state(&mut self) -> CommitListState<'a> {
        self.commit_list_state.take().unwrap()
    }

    fn as_mut_list_state(&mut self) -> &mut CommitListState<'a> {
        self.commit_list_state.as_mut().unwrap()
    }

    pub fn as_list_state(&self) -> &CommitListState<'a> {
        self.commit_list_state.as_ref().unwrap()
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        self.as_mut_list_state().drain_pending_graph_uploads()
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        self.as_list_state().graph_image_ids_sorted()
    }

    fn split_areas(&self, area: Rect) -> [Rect; 2] {
        let detail_height = (area.height - 1).min(self.ctx.ui_config.detail.height);
        Layout::vertical([Constraint::Min(0), Constraint::Length(detail_height)]).areas(area)
    }

    pub fn select_older_commit(&mut self, repository: &Repository) {
        self.update_selected_commit(repository, |state| state.select_next());
    }

    pub fn select_newer_commit(&mut self, repository: &Repository) {
        self.update_selected_commit(repository, |state| state.select_prev());
    }

    pub fn select_parent_commit(&mut self, repository: &Repository) {
        self.update_selected_commit(repository, |state| state.select_parent());
    }

    fn update_selected_commit<F>(&mut self, repository: &Repository, update_commit_list_state: F)
    where
        F: FnOnce(&mut CommitListState<'a>),
    {
        let commit_list_state = self.as_mut_list_state();
        update_commit_list_state(commit_list_state);
        let selected = commit_list_state.selected_commit_hash().clone();
        let (commit, changes) = repository.commit_detail(&selected);
        let refs = repository.refs(&selected).into_iter().cloned().collect();
        self.commit = commit;
        self.changes = changes;
        self.refs = refs;

        self.commit_detail_state.select_first();
    }

    fn copy_commit_hash(&self) {
        let selected = &self.commit.commit_hash;
        self.copy_to_clipboard("Commit SHA".into(), selected.as_str().into());
    }

    fn copy_commit_message(&self) {
        self.copy_to_clipboard("Commit message".into(), self.commit.commit_message.clone());
    }

    fn copy_to_clipboard(&self, name: String, value: String) {
        self.tx.send(AppEvent::CopyToClipboard { name, value });
    }

    pub fn refresh(&self) {
        let list_state = self.as_list_state();
        let list_context = ListRefreshViewContext::from(list_state);
        let context = RefreshViewContext::Detail { list_context };
        self.tx.send(AppEvent::Refresh(context));
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        let row = row as usize;
        if row < self.list_height {
            // Ignore clicks in the commit list pane when in detail view
            return;
        }

        // Check if click is in action bar area (right 40%)
        if let Some(detail_area) = self.detail_area {
            let action_bar_x = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            if col >= action_bar_x && row >= detail_area.y as usize {
                let action_bar_row = (row - detail_area.y as usize).saturating_sub(4);
                if let Some(action_idx) = self.action_index_at_row(action_bar_row) {
                    self.execute_action(action_idx);
                }
                return;
            }
        }

        let detail_local_row = row - self.list_height;
        // Detail widget layout: separator(0) + title(1) + underline(2) + spacer(3) + content(4+)
        if detail_local_row < 4 {
            return; // clicked on header area
        }

        let content_row = detail_local_row - 4;
        let clicked_line = self.commit_detail_state.offset() + content_row;
        let changes_start = self.compute_changes_start_line();

        if clicked_line >= changes_start && clicked_line < changes_start + self.changes.len() {
            let file_idx = clicked_line - changes_start;
            self.commit_detail_state.selected_file = file_idx;
            self.open_selected_file_diff();
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        let row = row as usize;
        if row < self.list_height {
            // Ignore mouse movement in the commit list pane
            return;
        }

        // Check if hover is in action bar area (right 40%)
        if let Some(detail_area) = self.detail_area {
            let action_bar_x = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            if col >= action_bar_x && row >= detail_area.y as usize {
                let action_bar_row = (row - detail_area.y as usize).saturating_sub(4);
                self.commit_detail_state.hovered_action = self.action_index_at_row(action_bar_row);
                return;
            } else {
                self.commit_detail_state.hovered_action = None;
            }
        }

        let detail_local_row = row - self.list_height;
        // Detail widget layout: separator(0) + title(1) + underline(2) + spacer(3) + content(4+)
        if detail_local_row < 4 {
            return; // hovering header area
        }

        let content_row = detail_local_row - 4;
        let hover_line = self.commit_detail_state.offset() + content_row;
        let changes_start = self.compute_changes_start_line();

        if hover_line >= changes_start && hover_line < changes_start + self.changes.len() {
            let file_idx = hover_line - changes_start;
            if file_idx != self.commit_detail_state.selected_file {
                self.commit_detail_state.selected_file = file_idx;
            }
        }
    }

    fn action_index_at_row(&self, action_bar_row: usize) -> Option<usize> {
        use crate::widget::commit_detail::{COMMIT_ACTIONS, STASH_ACTIONS};
        let actions = if self.is_stash() { STASH_ACTIONS } else { COMMIT_ACTIONS };
        if action_bar_row < actions.len() {
            Some(action_bar_row)
        } else {
            None
        }
    }

    fn execute_action(&self, action_idx: usize) {
        use crate::widget::commit_detail::{COMMIT_ACTIONS, STASH_ACTIONS};
        let actions = if self.is_stash() { STASH_ACTIONS } else { COMMIT_ACTIONS };
        if action_idx >= actions.len() {
            return;
        }
        let hash = self.commit.commit_hash.as_str().into();
        if self.is_stash() {
            match action_idx {
                0 => self.tx.send(AppEvent::ExecuteGitAction {
                    target: hash,
                    action: crate::event::GitAction::ApplyStash,
                }),
                1 => {
                    if let Some(stash_ref) = self.stash_ref() {
                        self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmPopStash { stash_ref }));
                    }
                }
                2 => {
                    if let Some(stash_ref) = self.stash_ref() {
                        self.tx.send(AppEvent::OpenDialog(DialogKind::ConfirmDropStash { stash_ref }));
                    }
                }
                3 => {
                    if let Some(stash_ref) = self.stash_ref() {
                        self.tx.send(AppEvent::OpenDialog(DialogKind::CreateBranchFromStash {
                            target: hash,
                            stash_ref,
                        }));
                    }
                }
                4 => {
                    if let Some(stash_ref) = self.stash_ref() {
                        self.copy_to_clipboard("Stash name".into(), stash_ref);
                    }
                }
                5 => self.copy_commit_hash(),
                _ => {}
            }
        } else {
            match action_idx {
                0 => self.tx.send(AppEvent::OpenDialog(DialogKind::AddTag { target: hash })),
                1 => self.tx.send(AppEvent::OpenDialog(DialogKind::CreateBranch { target: hash })),
                2 => self.tx.send(AppEvent::OpenDialog(DialogKind::Checkout { target: hash, is_branch: false })),
                3 => self.tx.send(AppEvent::OpenDialog(DialogKind::CherryPick { target: hash })),
                4 => self.tx.send(AppEvent::OpenDialog(DialogKind::Revert { target: hash })),
                5 => self.tx.send(AppEvent::OpenDialog(DialogKind::Drop { target: hash })),
                6 => self.tx.send(AppEvent::OpenDialog(DialogKind::Merge { target: hash, is_branch: false })),
                7 => self.tx.send(AppEvent::OpenDialog(DialogKind::Rebase { target: hash })),
                8 => self.tx.send(AppEvent::OpenDialog(DialogKind::Reset { target: hash })),
                _ => {}
            }
        }
    }

    fn is_stash(&self) -> bool {
        matches!(self.commit.commit_type, CommitType::Stash)
    }

    fn stash_ref(&self) -> Option<String> {
        self.refs.iter().find_map(|r| {
            if let Ref::Stash { name, .. } = r {
                Some(name.clone())
            } else {
                None
            }
        })
    }

    fn open_selected_file_diff(&self) {
        if let Some(change) = self.changes.get(self.commit_detail_state.selected_file) {
            match change {
                FileChange::Add { path, .. } | FileChange::Modify { path, .. } => {
                    let _ = self.tx.send(AppEvent::OpenFileDiff {
                        hash: self.commit.commit_hash.as_str().to_string(),
                        file_path: path.clone(),
                    });
                }
                FileChange::Move { to, .. } => {
                    let _ = self.tx.send(AppEvent::OpenFileDiff {
                        hash: self.commit.commit_hash.as_str().to_string(),
                        file_path: to.clone(),
                    });
                }
                FileChange::Delete { .. } => {
                    let _ = self.tx.send(AppEvent::NotifyWarn("Cannot view diff for a deleted file.".to_string()));
                }
            }
        }
    }

    fn compute_changes_start_line(&self) -> usize {
        let mut count = 0;
        count += 2; // author name + date
        if self.commit.author_name != self.commit.committer_name
            || self.commit.author_email != self.commit.committer_email
            || self.commit.author_date != self.commit.committer_date
        {
            count += 2; // committer name + date
        }
        count += 1; // SHA
        if !self.commit.parent_commit_hashes.is_empty() {
            count += 1; // Parents
        }
        if self.refs.iter().any(|r| {
            matches!(
                r,
                Ref::Branch { .. } | Ref::RemoteBranch { .. } | Ref::Tag { .. }
            )
        }) {
            count += 1; // Refs
        }
        count += 1; // divider
        count += 1; // commit message
        if !self.commit.body.is_empty() {
            count += 1; // empty line
            count += self.commit.body.lines().count();
        }
        count += 1; // divider
        count
    }
}
