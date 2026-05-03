use std::rc::Rc;
use ratatui::{crossterm::event::KeyEvent, layout::Rect, Frame};
use crate::{app::AppContext, event::{AppEvent, Sender, UserEventWithCount}};

#[derive(Debug)]
pub struct TagDetailView<'a> {
    _phantom: std::marker::PhantomData<&'a ()>,
    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> TagDetailView<'a> {
    pub fn new(tag_name: String, ctx: Rc<AppContext>, tx: Sender) -> Self {
        Self { _phantom: std::marker::PhantomData, ctx, tx }
    }
    pub fn handle_event(&mut self, _event_with_count: UserEventWithCount, _key_event: KeyEvent) {}
    pub fn render(&mut self, _f: &mut Frame, _area: Rect) {}
    pub fn update_layout(&mut self, _area: Rect) {}
}
