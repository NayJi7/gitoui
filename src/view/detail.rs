use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    Frame,
};

use crate::{
    app::AppContext,
    event::{AppEvent, DialogKind, Sender, UserEvent, UserEventWithCount},
    git::{Commit, CommitHash, CommitType, FileChange, Ref, Repository},
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
    head_commit_hash: Option<CommitHash>,
    /// PR number the user is currently exploring, surfaces in the
    /// "Commit Details" panel title so they always know the origin.
    pr_origin: Option<u64>,

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
        head_commit_hash: Option<CommitHash>,
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
            head_commit_hash,
            pr_origin: None,
            ctx,
            tx,
            list_height: 0,
            detail_area: None,
        }
    }

    pub fn set_pr_origin(&mut self, pr_number: Option<u64>) {
        self.pr_origin = pr_number;
    }

    /// Pre-select the file whose path matches in the commit's change list.
    /// Used when arriving from Blame / FileHistory so the user lands directly
    /// on the file they were inspecting, `ensure_selected_visible` (run on
    /// the next render) scrolls the file row into view automatically. Matches
    /// against every variant of `FileChange` including the `from`/`to` sides
    /// of a Move so renames in either direction still resolve.
    pub fn select_file_by_path(&mut self, target: &str) {
        let idx = self.changes.iter().position(|c| match c {
            FileChange::Add { path, .. }
            | FileChange::Modify { path, .. }
            | FileChange::Delete { path, .. } => path == target,
            FileChange::Move { from, to, .. } => to == target || from == target,
        });
        if let Some(i) = idx {
            self.commit_detail_state.selected_file = i;
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;

        // Per-view shortcut: `revert` is scoped to [scope.detail] because
        // `v` is taken globally by `clean_untracked` (uncommitted view).
        // Going through the scope resolver, instead of a raw-key
        // intercept, means a user rebinding `[keybind.detail] revert`
        // works without any extra wiring, and the action panel can
        // surface the actual bound key.
        if let Some("revert") = self.ctx.keybind.resolve_scoped(&["detail"], key) {
            self.tx.send(AppEvent::OpenDialog(DialogKind::Revert {
                target: self.commit.commit_hash.as_str().into(),
            }));
            return;
        }

        match event {
            UserEvent::NavigateDown => {
                for _ in 0..count {
                    self.commit_detail_state
                        .select_next_file(self.changes.len());
                }
            }
            UserEvent::NavigateUp => {
                for _ in 0..count {
                    self.commit_detail_state.select_prev_file();
                }
            }
            UserEvent::NavigateRight => {
                self.tx.send(AppEvent::SelectOlderCommit);
            }
            UserEvent::NavigateLeft => {
                self.tx.send(AppEvent::SelectNewerCommit);
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
            UserEvent::Blame => {
                // Blame the currently-highlighted file in the commit's file list.
                if let Some(change) = self.changes.get(self.commit_detail_state.selected_file) {
                    let path = match change {
                        FileChange::Add { path, .. }
                        | FileChange::Modify { path, .. }
                        | FileChange::Delete { path, .. } => path.clone(),
                        FileChange::Move { to, .. } => to.clone(),
                    };
                    self.tx.send(AppEvent::OpenBlame { file_path: path });
                }
            }
            UserEvent::FileHistory => {
                if let Some(change) = self.changes.get(self.commit_detail_state.selected_file) {
                    let path = match change {
                        FileChange::Add { path, .. }
                        | FileChange::Modify { path, .. }
                        | FileChange::Delete { path, .. } => path.clone(),
                        FileChange::Move { to, .. } => to.clone(),
                    };
                    self.tx.send(AppEvent::OpenFileHistory { file_path: path });
                }
            }
            UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::CloseDetail);
            }
            UserEvent::Refresh => {
                self.refresh();
            }
            UserEvent::AddTag => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::AddTag {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::CreateBranch => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::CreateBranch {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Checkout => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Checkout {
                    target: self.commit.commit_hash.as_str().into(),
                    is_branch: false,
                }));
            }
            UserEvent::CherryPick => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::CherryPick {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Revert => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Revert {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Drop => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Drop {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Merge => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Merge {
                    target: self.commit.commit_hash.as_str().into(),
                    is_branch: false,
                }));
            }
            UserEvent::Rebase => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Rebase {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Reset => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Reset {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::Squash => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Squash {
                    target: self.commit.commit_hash.as_str().into(),
                }));
            }
            UserEvent::ApplyStash => {
                self.tx.send(AppEvent::ExecuteGitAction {
                    target: self.commit.commit_hash.as_str().into(),
                    action: crate::event::GitAction::ApplyStash,
                });
            }
            UserEvent::PopStash => {
                if let Some(stash_ref) = self.stash_ref() {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::ConfirmPopStash {
                            stash_ref,
                        }));
                }
            }
            UserEvent::DropStash => {
                if let Some(stash_ref) = self.stash_ref() {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::ConfirmDropStash {
                            stash_ref,
                        }));
                }
            }
            UserEvent::CreateBranchFromStash => {
                if let Some(stash_ref) = self.stash_ref() {
                    self.tx
                        .send(AppEvent::OpenDialog(DialogKind::CreateBranchFromStash {
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
            UserEvent::AbortOperation => {
                self.tx.send(AppEvent::CheckAbortOperation);
            }
            UserEvent::AmendCommit => {
                if self.is_head_commit() {
                    self.tx.send(AppEvent::OpenDialog(DialogKind::AmendMessage {
                        current_message: self.commit.commit_message.clone(),
                    }));
                } else {
                    self.tx.send(AppEvent::NotifyWarn(
                        "Amend is only available for the HEAD commit".into(),
                    ));
                }
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
            self.is_head_commit(),
        )
        .with_pr_origin(self.pr_origin);
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
        let ctx = self.ctx.clone();
        self.as_mut_list_state().ensure_visible_avatars_uploaded(
            &mut ctx.avatar_manager.lock().unwrap(),
            ctx.color_theme.bg,
            ctx.color_theme.list_selected_bg,
        );
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

    pub fn drain_pending_avatar_deletes(&mut self) -> Vec<u16> {
        self.commit_detail_state
            .drain_pending_avatar_delete()
            .into_iter()
            .collect()
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        self.as_list_state().graph_image_ids_sorted()
    }

    fn split_areas(&self, area: Rect) -> [Rect; 2] {
        let detail_height =
            crate::view::adaptive_detail_height(area.height, self.ctx.ui_config.detail.height, 5);
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

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        std::rc::Rc::make_mut(&mut self.ctx).color_theme = theme;
    }

    pub fn refresh(&self) {
        let list_state = self.as_list_state();
        let list_context = ListRefreshViewContext::from(list_state);
        let context = RefreshViewContext::Detail { list_context };
        self.tx.send(AppEvent::Refresh(context));
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        let row = row as usize;
        let Some(detail_area) = self.detail_area else {
            return;
        };
        let detail_y = detail_area.y as usize;

        if row < detail_y {
            // Click in commit list pane, ignore
            return;
        }

        // Check if click is in action bar area (right 40%)
        let action_bar_x = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
        if col >= action_bar_x {
            let action_bar_row = (row - detail_y).saturating_sub(4);
            if let Some(action_idx) = self.action_index_at_row(action_bar_row) {
                self.execute_action(action_idx);
            }
            return;
        }

        // Detail widget layout: separator(0) + title(1) + underline(2) + spacer(3) + content(4+)
        let detail_local_row = row - detail_y;
        if detail_local_row < 4 {
            return;
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
        let Some(detail_area) = self.detail_area else {
            self.commit_detail_state.hover_file = None;
            return;
        };
        let detail_y = detail_area.y as usize;

        if row < detail_y {
            self.commit_detail_state.hover_file = None;
            self.commit_detail_state.hovered_action = None;
            return;
        }

        // Check if hover is in action bar area (right 40%)
        let action_bar_x = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
        if col >= action_bar_x {
            let action_bar_row = (row - detail_y).saturating_sub(4);
            self.commit_detail_state.hovered_action = self.action_index_at_row(action_bar_row);
            self.commit_detail_state.hover_file = None;
            return;
        } else {
            self.commit_detail_state.hovered_action = None;
        }

        let detail_local_row = row - detail_y;
        // Detail widget layout: separator(0) + title(1) + underline(2) + spacer(3) + content(4+)
        if detail_local_row < 4 {
            self.commit_detail_state.hover_file = None;
            return;
        }

        let content_row = detail_local_row - 4;
        let hover_line = self.commit_detail_state.offset() + content_row;
        let changes_start = self.compute_changes_start_line();

        if hover_line >= changes_start && hover_line < changes_start + self.changes.len() {
            self.commit_detail_state.hover_file = Some(hover_line - changes_start);
        } else {
            self.commit_detail_state.hover_file = None;
        }
    }

    fn action_index_at_row(&self, action_bar_row: usize) -> Option<usize> {
        use crate::widget::commit_detail::{commit_actions, STASH_ACTIONS};
        let actions = if self.is_stash() {
            STASH_ACTIONS
        } else {
            commit_actions(self.is_head_commit())
        };
        if action_bar_row < actions.len() {
            Some(action_bar_row)
        } else {
            None
        }
    }

    fn execute_action(&self, action_idx: usize) {
        use crate::widget::commit_detail::{commit_actions, STASH_ACTIONS};
        let actions = if self.is_stash() {
            STASH_ACTIONS
        } else {
            commit_actions(self.is_head_commit())
        };
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
                        self.tx
                            .send(AppEvent::OpenDialog(DialogKind::ConfirmPopStash {
                                stash_ref,
                            }));
                    }
                }
                2 => {
                    if let Some(stash_ref) = self.stash_ref() {
                        self.tx
                            .send(AppEvent::OpenDialog(DialogKind::ConfirmDropStash {
                                stash_ref,
                            }));
                    }
                }
                3 => {
                    if let Some(stash_ref) = self.stash_ref() {
                        self.tx
                            .send(AppEvent::OpenDialog(DialogKind::CreateBranchFromStash {
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
            // Indices must stay in sync with `COMMIT_ACTIONS` in
            // `src/widget/commit_detail.rs`.
            match action_idx {
                0 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::AddTag { target: hash })),
                1 => {
                    // Blame the currently-highlighted file in the commit's
                    // change list. Same logic as the `b` keybind handler.
                    if let Some(change) = self.changes.get(self.commit_detail_state.selected_file) {
                        let path = match change {
                            FileChange::Add { path, .. }
                            | FileChange::Modify { path, .. }
                            | FileChange::Delete { path, .. } => path.clone(),
                            FileChange::Move { to, .. } => to.clone(),
                        };
                        self.tx.send(AppEvent::OpenBlame { file_path: path });
                    }
                }
                2 => self.tx.send(AppEvent::OpenDialog(DialogKind::CreateBranch {
                    target: hash,
                })),
                3 => self.tx.send(AppEvent::OpenDialog(DialogKind::Checkout {
                    target: hash,
                    is_branch: false,
                })),
                4 => self.tx.send(AppEvent::OpenDialog(DialogKind::CherryPick {
                    target: hash,
                })),
                5 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::Revert { target: hash })),
                6 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::Drop { target: hash })),
                7 => self.tx.send(AppEvent::OpenDialog(DialogKind::Merge {
                    target: hash,
                    is_branch: false,
                })),
                8 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::Rebase { target: hash })),
                9 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::Reset { target: hash })),
                10 => self
                    .tx
                    .send(AppEvent::OpenDialog(DialogKind::Squash { target: hash })),
                11 => {
                    if self.is_head_commit() {
                        self.tx.send(AppEvent::OpenDialog(DialogKind::AmendMessage {
                            current_message: self.commit.commit_message.clone(),
                        }));
                    } else {
                        self.tx.send(AppEvent::NotifyWarn(
                            "Amend is only available for the HEAD commit".into(),
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    fn is_head_commit(&self) -> bool {
        if let Some(ref head_hash) = self.head_commit_hash {
            return *head_hash == self.commit.commit_hash;
        }
        // Fallback: check if any branch ref on this commit matches head_branch_name
        if let Some(ref head_name) = self.head_branch_name {
            return self.refs.iter().any(|r| {
                if let Ref::Branch { name, .. } = r {
                    name == head_name
                } else {
                    false
                }
            });
        }
        false
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
                    self.tx.send(AppEvent::OpenFileDiff {
                        hash: self.commit.commit_hash.as_str().to_string(),
                        file_path: path.clone(),
                    });
                }
                FileChange::Move { to, .. } => {
                    self.tx.send(AppEvent::OpenFileDiff {
                        hash: self.commit.commit_hash.as_str().to_string(),
                        file_path: to.clone(),
                    });
                }
                FileChange::Delete { path, .. } => {
                    self.tx.send(AppEvent::OpenFileDiff {
                        hash: self.commit.commit_hash.as_str().to_string(),
                        file_path: path.clone(),
                    });
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
        count += 1; // commit message (subject)
                    // Body lines come straight after the subject, no separator row
                    // since commit_message_lines() stopped pushing one. Counting an
                    // empty line here used to push hover detection 1 row off.
        if !self.commit.body.is_empty() {
            count += self.commit.body.lines().count();
        }
        count += 1; // divider before changes
        count
    }
}
