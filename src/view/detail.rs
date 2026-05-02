use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    Frame,
};

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    git::{Commit, FileChange, Ref, Repository},
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

    ctx: Rc<AppContext>,
    tx: Sender,
    list_height: usize,
}

impl<'a> DetailView<'a> {
    pub fn new(
        commit_list_state: CommitListState<'a>,
        commit: Commit,
        changes: Vec<FileChange>,
        refs: Vec<Ref>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> DetailView<'a> {
        DetailView {
            commit_list_state: Some(commit_list_state),
            commit_detail_state: CommitDetailState::default(),
            commit,
            changes,
            refs,
            ctx,
            tx,
            list_height: 0,
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
                self.copy_commit_short_hash();
            }
            UserEvent::FullCopy => {
                self.copy_commit_hash();
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
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let [list_area, detail_area] = self.split_areas(area);

        let commit_list = CommitList::new(self.ctx.clone());
        f.render_stateful_widget(commit_list, list_area, self.as_mut_list_state());

        let commit_detail =
            CommitDetail::new(&self.commit, &self.changes, &self.refs, self.ctx.clone());
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

    fn copy_commit_short_hash(&self) {
        let selected = &self.commit.commit_hash;
        self.copy_to_clipboard("Commit SHA (short)".into(), selected.as_short_hash().into());
    }

    fn copy_commit_hash(&self) {
        let selected = &self.commit.commit_hash;
        self.copy_to_clipboard("Commit SHA".into(), selected.as_str().into());
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

    pub fn handle_click(&mut self, _col: u16, row: u16) {
        let row = row as usize;
        if row < self.list_height {
            // Ignore clicks in the commit list pane when in detail view
            return;
        }

        let detail_local_row = row - self.list_height;
        if detail_local_row == 0 {
            return; // clicked on border
        }

        let content_row = detail_local_row - 1;
        let clicked_line = self.commit_detail_state.offset() + content_row;
        let changes_start = self.compute_changes_start_line();

        if clicked_line >= changes_start && clicked_line < changes_start + self.changes.len() {
            let file_idx = clicked_line - changes_start;
            self.commit_detail_state.selected_file = file_idx;
            self.open_selected_file_diff();
        }
    }

    pub fn handle_mouse_move(&mut self, _col: u16, row: u16) {
        let row = row as usize;
        if row < self.list_height {
            // Ignore mouse movement in the commit list pane
            return;
        }

        let detail_local_row = row - self.list_height;
        if detail_local_row == 0 {
            return; // border
        }

        let content_row = detail_local_row - 1;
        let hover_line = self.commit_detail_state.offset() + content_row;
        let changes_start = self.compute_changes_start_line();

        if hover_line >= changes_start && hover_line < changes_start + self.changes.len() {
            let file_idx = hover_line - changes_start;
            if file_idx != self.commit_detail_state.selected_file {
                self.commit_detail_state.selected_file = file_idx;
            }
        }
    }

    fn open_selected_file_diff(&self) {
        if let Some(change) = self.changes.get(self.commit_detail_state.selected_file) {
            let file_path = match change {
                FileChange::Add { path } | FileChange::Modify { path } | FileChange::Delete { path } => path.clone(),
                FileChange::Move { to, .. } => to.clone(),
            };
            let _ = self.tx.send(AppEvent::OpenFileDiff {
                hash: self.commit.commit_hash.as_str().to_string(),
                file_path,
            });
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
        count += 1; // subject
        if !self.commit.body.is_empty() {
            count += 1; // empty line
            count += self.commit.body.lines().count();
        }
        count += 1; // divider
        count
    }
}
