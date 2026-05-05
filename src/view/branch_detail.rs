use std::rc::Rc;

use ratatui::{crossterm::event::KeyEvent, layout::Rect, Frame};

use crate::{
    app::AppContext,
    event::{AppEvent, DialogKind, GitAction, Sender, UserEvent, UserEventWithCount},
    widget::branch_detail::{BranchDetail, BranchDetailState, BranchMetadata},
    widget::commit_list::{CommitList, CommitListState},
};

#[derive(Debug)]
pub struct BranchDetailView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    branch_detail_state: BranchDetailState,
    metadata: BranchMetadata,
    ctx: Rc<AppContext>,
    tx: Sender,
    detail_area: Option<Rect>,
}

impl<'a> BranchDetailView<'a> {
    pub fn new(
        _branch_name: String,
        metadata: BranchMetadata,
        commit_list_state: Option<CommitListState<'a>>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        Self {
            commit_list_state,
            branch_detail_state: BranchDetailState::default(),
            metadata,
            ctx,
            tx,
            detail_area: None,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _: KeyEvent) {
        let event = event_with_count.event;
        match event {
            UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::CloseDetail);
            }
            UserEvent::Checkout => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Checkout {
                    target: self.metadata.branch_name.clone(),
                    is_branch: true,
                }));
            }
            UserEvent::RenameBranch => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::RenameBranch {
                    branch: self.metadata.branch_name.clone(),
                }));
            }
            UserEvent::DeleteBranch => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::DeleteBranch {
                    branch: self.metadata.branch_name.clone(),
                    is_remote: self.metadata.is_remote,
                }));
            }
            UserEvent::Merge => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Merge {
                    target: self.metadata.branch_name.clone(),
                    is_branch: true,
                }));
            }
            UserEvent::Rebase => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::Rebase {
                    target: self.metadata.branch_name.clone(),
                }));
            }
            UserEvent::PushBranch => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::PushBranch {
                    branch: self.metadata.branch_name.clone(),
                }));
            }
            UserEvent::PullBranch => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::PullBranch {
                    branch: self.metadata.branch_name.clone(),
                }));
            }
            UserEvent::CreateArchive => {
                self.tx.send(AppEvent::ExecuteGitAction {
                    target: self.metadata.branch_name.clone(),
                    action: GitAction::CreateArchive,
                });
            }
            UserEvent::UnselectBranch => {
                self.tx.send(AppEvent::NotifyInfo(
                    "Unselect branch not yet implemented".into(),
                ));
            }
            UserEvent::CopyBranchName => {
                self.tx.send(AppEvent::CopyToClipboard {
                    name: "Branch Name".into(),
                    value: self.metadata.branch_name.clone(),
                });
            }
            UserEvent::Refresh => {
                self.refresh();
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let detail_height = (area.height - 1).min(12);
        let [list_area, detail_area] =
            ratatui::layout::Layout::vertical([
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(detail_height),
            ]).areas(area);

        if let Some(ref mut list_state) = self.commit_list_state {
            let commit_list = CommitList::new(self.ctx.clone());
            f.render_stateful_widget(commit_list, list_area, list_state);
        }

        let branch_detail = BranchDetail::new(&self.metadata, self.ctx.clone());
        f.render_stateful_widget(branch_detail, detail_area, &mut self.branch_detail_state);
        self.detail_area = Some(detail_area);
    }

    pub fn update_layout(&mut self, area: Rect) {
        let detail_height = (area.height - 1).min(12);
        let [list_area, _] =
            ratatui::layout::Layout::vertical([
                ratatui::layout::Constraint::Min(0),
                ratatui::layout::Constraint::Length(detail_height),
            ]).areas(area);
        if let Some(ref mut list_state) = self.commit_list_state {
            list_state.update_height(list_area.height as usize);
        }
    }
}

impl<'a> BranchDetailView<'a> {
    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
    }

    pub fn set_list_state(&mut self, state: CommitListState<'a>) {
        self.commit_list_state = Some(state);
    }

    pub fn refresh(&self) {
        self.tx.send(AppEvent::OpenBranchDetail {
            branch_name: self.metadata.branch_name.clone(),
        });
    }

    pub fn prepare_graph_uploads(&mut self) {
        if let Some(ref mut list_state) = self.commit_list_state {
            list_state.ensure_visible_graph_uploaded();
            list_state.ensure_visible_avatars_uploaded(&mut self.ctx.avatar_manager.lock().unwrap(), self.ctx.color_theme.bg, self.ctx.color_theme.list_selected_bg);
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

    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(detail_area) = self.detail_area {
            let action_bar_x = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            if col >= action_bar_x && row >= detail_area.y {
                let action_bar_row = (row - detail_area.y).saturating_sub(4) as usize;
                if let Some(action_idx) = self.action_index_at_row(action_bar_row) {
                    self.execute_action(action_idx);
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        if let Some(detail_area) = self.detail_area {
            let action_bar_x = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            if col >= action_bar_x && row >= detail_area.y {
                let action_bar_row = (row - detail_area.y).saturating_sub(4) as usize;
                self.branch_detail_state.hovered_action = self.action_index_at_row(action_bar_row);
            } else {
                self.branch_detail_state.hovered_action = None;
            }
        }
    }

    fn action_index_at_row(&self, action_bar_row: usize) -> Option<usize> {
        let actions = if self.metadata.is_remote {
            crate::widget::branch_detail::REMOTE_BRANCH_ACTIONS
        } else {
            crate::widget::branch_detail::LOCAL_BRANCH_ACTIONS
        };
        if action_bar_row < actions.len() {
            Some(action_bar_row)
        } else {
            None
        }
    }

    fn execute_action(&self, action_idx: usize) {
        let actions = if self.metadata.is_remote {
            crate::widget::branch_detail::REMOTE_BRANCH_ACTIONS
        } else {
            crate::widget::branch_detail::LOCAL_BRANCH_ACTIONS
        };
        if action_idx >= actions.len() {
            return;
        }
        let name = self.metadata.branch_name.clone();
        if self.metadata.is_remote {
            match action_idx {
                0 => self.tx.send(AppEvent::OpenDialog(DialogKind::Checkout {
                    target: name,
                    is_branch: true,
                })),
                1 => self.tx.send(AppEvent::OpenDialog(DialogKind::DeleteBranch {
                    branch: name,
                    is_remote: true,
                })),
                2 => self.tx.send(AppEvent::OpenDialog(DialogKind::Merge {
                    target: name,
                    is_branch: true,
                })),
                3 => self.tx.send(AppEvent::OpenDialog(DialogKind::PullBranch {
                    branch: name,
                })),
                4 => self.tx.send(AppEvent::ExecuteGitAction {
                    target: name,
                    action: GitAction::CreateArchive,
                }),
                5 => self.tx.send(AppEvent::NotifyInfo(
                    "Unselect branch not yet implemented".into(),
                )),
                6 => self.tx.send(AppEvent::CopyToClipboard {
                    name: "Branch Name".into(),
                    value: name,
                }),
                _ => {}
            }
        } else {
            match action_idx {
                0 => self.tx.send(AppEvent::OpenDialog(DialogKind::Checkout {
                    target: name,
                    is_branch: true,
                })),
                1 => self.tx.send(AppEvent::OpenDialog(DialogKind::RenameBranch {
                    branch: name,
                })),
                2 => self.tx.send(AppEvent::OpenDialog(DialogKind::DeleteBranch {
                    branch: name,
                    is_remote: false,
                })),
                3 => self.tx.send(AppEvent::OpenDialog(DialogKind::Merge {
                    target: name,
                    is_branch: true,
                })),
                4 => self.tx.send(AppEvent::OpenDialog(DialogKind::Rebase {
                    target: name,
                })),
                5 => self.tx.send(AppEvent::OpenDialog(DialogKind::PushBranch {
                    branch: name,
                })),
                6 => self.tx.send(AppEvent::ExecuteGitAction {
                    target: name,
                    action: GitAction::CreateArchive,
                }),
                7 => self.tx.send(AppEvent::NotifyInfo(
                    "Unselect branch not yet implemented".into(),
                )),
                8 => self.tx.send(AppEvent::CopyToClipboard {
                    name: "Branch Name".into(),
                    value: name,
                }),
                _ => {}
            }
        }
    }
}
