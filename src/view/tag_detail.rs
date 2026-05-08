use std::rc::Rc;

use ratatui::{crossterm::event::KeyEvent, layout::Rect, Frame};

use crate::{
    app::AppContext,
    event::{AppEvent, DialogKind, Sender, UserEvent, UserEventWithCount},
    widget::commit_list::{CommitList, CommitListState},
    widget::tag_detail::{TagDetail, TagDetailState, TagMetadata},
};

#[derive(Debug)]
pub struct TagDetailView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    tag_detail_state: TagDetailState,
    metadata: TagMetadata,
    ctx: Rc<AppContext>,
    tx: Sender,
    detail_area: Option<Rect>,
}

impl<'a> TagDetailView<'a> {
    pub fn new(
        _tag_name: String,
        metadata: TagMetadata,
        commit_list_state: Option<CommitListState<'a>>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        Self {
            commit_list_state,
            tag_detail_state: TagDetailState::default(),
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
            UserEvent::PushTag | UserEvent::Push => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::PushTag {
                    tag: self.metadata.tag_name.clone(),
                }));
            }
            UserEvent::DeleteTag | UserEvent::DeleteBranch => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::DeleteTag {
                    tag: self.metadata.tag_name.clone(),
                }));
            }
            UserEvent::CopyTagName => {
                self.tx.send(AppEvent::CopyToClipboard {
                    name: "Tag Name".into(),
                    value: self.metadata.tag_name.clone(),
                });
            }
            UserEvent::Refresh => {
                self.refresh();
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let detail_height = if self.commit_list_state.is_some() {
            crate::view::adaptive_detail_height(area.height, 10, 5)
        } else {
            area.height.min(10)
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

        let tag_detail = TagDetail::new(&self.metadata, self.ctx.clone());
        f.render_stateful_widget(tag_detail, detail_area, &mut self.tag_detail_state);
        self.detail_area = Some(detail_area);
    }

    pub fn update_layout(&mut self, area: Rect) {
        if let Some(ref mut list_state) = self.commit_list_state {
            let detail_height = crate::view::adaptive_detail_height(area.height, 10, 5);
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
                self.tag_detail_state.hovered_action = self.action_index_at_row(action_bar_row);
            } else {
                self.tag_detail_state.hovered_action = None;
            }
        }
    }

    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
    }

    #[allow(dead_code)]
    pub fn set_list_state(&mut self, state: CommitListState<'a>) {
        self.commit_list_state = Some(state);
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        std::rc::Rc::make_mut(&mut self.ctx).color_theme = theme;
    }

    pub fn refresh(&self) {
        self.tx.send(AppEvent::OpenTagDetail {
            tag_name: self.metadata.tag_name.clone(),
        });
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

    fn action_index_at_row(&self, action_bar_row: usize) -> Option<usize> {
        if action_bar_row < crate::widget::tag_detail::TAG_ACTIONS.len() {
            Some(action_bar_row)
        } else {
            None
        }
    }

    fn execute_action(&self, action_idx: usize) {
        let name = self.metadata.tag_name.clone();
        match action_idx {
            0 => self
                .tx
                .send(AppEvent::OpenDialog(DialogKind::PushTag { tag: name })),
            1 => self
                .tx
                .send(AppEvent::OpenDialog(DialogKind::DeleteTag { tag: name })),
            2 => self.tx.send(AppEvent::CopyToClipboard {
                name: "Tag Name".into(),
                value: name,
            }),
            _ => {}
        }
    }
}
