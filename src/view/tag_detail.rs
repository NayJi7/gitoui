use std::rc::Rc;

use ratatui::{crossterm::event::KeyEvent, layout::Rect, Frame};

use crate::{
    app::AppContext,
    event::{AppEvent, DialogKind, Sender, UserEvent, UserEventWithCount},
    widget::tag_detail::{TagDetail, TagDetailState, TagMetadata},
};

#[derive(Debug)]
pub struct TagDetailView<'a> {
    _phantom: std::marker::PhantomData<&'a ()>,
    tag_detail_state: TagDetailState,
    metadata: TagMetadata,
    ctx: Rc<AppContext>,
    tx: Sender,
    detail_area: Option<Rect>,
}

impl<'a> TagDetailView<'a> {
    pub fn new(_tag_name: String, metadata: TagMetadata, ctx: Rc<AppContext>, tx: Sender) -> Self {
        Self {
            _phantom: std::marker::PhantomData,
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
            UserEvent::PushTag => {
                self.tx.send(AppEvent::OpenDialog(DialogKind::PushTag {
                    tag: self.metadata.tag_name.clone(),
                }));
            }
            UserEvent::DeleteTag => {
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
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let tag_detail = TagDetail::new(&self.metadata, self.ctx.clone());
        f.render_stateful_widget(tag_detail, area, &mut self.tag_detail_state);
        self.detail_area = Some(area);
    }

    pub fn update_layout(&mut self, _area: Rect) {}

    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(detail_area) = self.detail_area {
            let action_bar_x = detail_area.x + (detail_area.width as f32 * 0.6) as u16;
            if col >= action_bar_x && row >= detail_area.y {
                let action_bar_row = (row - detail_area.y).saturating_sub(1) as usize;
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
                let action_bar_row = (row - detail_area.y).saturating_sub(1) as usize;
                self.tag_detail_state.hovered_action = self.action_index_at_row(action_bar_row);
            } else {
                self.tag_detail_state.hovered_action = None;
            }
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
        if let Some((_, key)) = crate::widget::tag_detail::TAG_ACTIONS.get(action_idx) {
            match *key {
                'p' => self.tx.send(AppEvent::OpenDialog(DialogKind::PushTag {
                    tag: self.metadata.tag_name.clone(),
                })),
                'D' => self.tx.send(AppEvent::OpenDialog(DialogKind::DeleteTag {
                    tag: self.metadata.tag_name.clone(),
                })),
                'c' => self.tx.send(AppEvent::CopyToClipboard {
                    name: "Tag Name".into(),
                    value: self.metadata.tag_name.clone(),
                }),
                _ => {}
            }
        }
    }
}
