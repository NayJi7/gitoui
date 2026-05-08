use std::rc::Rc;

use ratatui::{crossterm::event::KeyEvent, layout::Rect, Frame};

use crate::{
    app::AppContext,
    config::save,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    git::CommitHash,
    view::{ListRefreshViewContext, RefreshViewContext},
    widget::commit_list::{CommitList, CommitListState, SearchState},
};

#[derive(Debug)]
pub struct ListView<'a> {
    commit_list_state: Option<CommitListState<'a>>,

    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> ListView<'a> {
    pub fn new(
        commit_list_state: CommitListState<'a>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> ListView<'a> {
        ListView {
            commit_list_state: Some(commit_list_state),
            ctx,
            tx,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;
        if let SearchState::Searching { .. } = self.as_list_state().search_state() {
            match event {
                UserEvent::Confirm => {
                    self.as_mut_list_state().apply_search();
                    self.update_matched_message();
                }
                UserEvent::Cancel => {
                    self.as_mut_list_state().cancel_search();
                    self.clear_search_query();
                }
                _ => {
                    self.as_mut_list_state().handle_search_input(key);
                    self.update_search_query();
                }
            }
            return;
        } else {
            match event {
                UserEvent::Quit => {
                    self.tx.send(AppEvent::Quit);
                }
                UserEvent::NavigateDown | UserEvent::SelectDown => {
                    for _ in 0..count {
                        self.as_mut_list_state().select_next();
                    }
                }
                UserEvent::NavigateUp | UserEvent::SelectUp => {
                    for _ in 0..count {
                        self.as_mut_list_state().select_prev();
                    }
                }
                UserEvent::GoToParent => {
                    for _ in 0..count {
                        self.as_mut_list_state().select_parent();
                    }
                }
                UserEvent::GoToTop => {
                    self.as_mut_list_state().select_first();
                }
                UserEvent::GoToBottom => {
                    self.as_mut_list_state().select_last();
                }
                UserEvent::ScrollDown => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_down();
                    }
                }
                UserEvent::ScrollUp => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_up();
                    }
                }
                UserEvent::PageDown => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_down_page();
                    }
                }
                UserEvent::PageUp => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_up_page();
                    }
                }
                UserEvent::HalfPageDown => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_down_half();
                    }
                }
                UserEvent::HalfPageUp => {
                    for _ in 0..count {
                        self.as_mut_list_state().scroll_up_half();
                    }
                }
                UserEvent::SelectTop => {
                    self.as_mut_list_state().select_high();
                }
                UserEvent::SelectMiddle => {
                    self.as_mut_list_state().select_middle();
                }
                UserEvent::SelectBottom => {
                    self.as_mut_list_state().select_low();
                }
                UserEvent::ShortCopy => {
                    self.copy_commit_hash();
                }
                UserEvent::FullCopy => {
                    self.copy_commit_message();
                }
                UserEvent::Search => {
                    self.as_mut_list_state().start_search();
                    self.update_search_query();
                }
                UserEvent::UserCommand(n) => {
                    self.tx.send(AppEvent::OpenUserCommand(n));
                }
                UserEvent::HelpToggle => {
                    self.tx.send(AppEvent::OpenHelp);
                }
                UserEvent::Cancel => {
                    self.as_mut_list_state().cancel_search();
                    self.clear_search_query();
                }
                UserEvent::Confirm => {
                    if let SearchState::Applied { .. } = self.as_list_state().search_state() {
                        self.as_mut_list_state().cancel_search();
                        self.clear_search_query();
                    }
                    if self.as_list_state().is_uncommitted_selected() {
                        self.tx.send(AppEvent::OpenUncommitted);
                    } else {
                        self.tx.send(AppEvent::OpenDetail);
                    }
                }
                UserEvent::RefList => {
                    self.tx.send(AppEvent::OpenRefs);
                }
                UserEvent::Refresh => {
                    self.refresh();
                }
                UserEvent::Push => {
                    self.tx.send(AppEvent::PushCurrentBranch);
                }
                UserEvent::Pull => {
                    self.tx.send(AppEvent::PullCurrentBranch);
                }
                UserEvent::AbortOperation => {
                    self.tx.send(AppEvent::CheckAbortOperation);
                }
                UserEvent::Config => {
                    self.tx.send(AppEvent::OpenConfig);
                }
                _ => {}
            }
        }

        if let SearchState::Applied { .. } = self.as_list_state().search_state() {
            match event {
                UserEvent::GoToNext => {
                    self.as_mut_list_state().select_next_match();
                    self.update_matched_message();
                }
                UserEvent::GoToPrevious => {
                    self.as_mut_list_state().select_prev_match();
                    self.update_matched_message();
                }
                UserEvent::IgnoreCaseToggle => {
                    if let Some((ignore_case, fuzzy)) =
                        self.as_mut_list_state().toggle_ignore_case()
                    {
                        let ctx = Rc::make_mut(&mut self.ctx);
                        ctx.core_config.search.ignore_case = ignore_case;
                        ctx.core_config.search.fuzzy = fuzzy;
                        let _ = save(&ctx.core_config, &ctx.ui_config);
                    }
                    self.update_matched_message();
                }
                UserEvent::FuzzyToggle => {
                    if let Some((ignore_case, fuzzy)) = self.as_mut_list_state().toggle_fuzzy() {
                        let ctx = Rc::make_mut(&mut self.ctx);
                        ctx.core_config.search.ignore_case = ignore_case;
                        ctx.core_config.search.fuzzy = fuzzy;
                        let _ = save(&ctx.core_config, &ctx.ui_config);
                    }
                    self.update_matched_message();
                }
                UserEvent::Cancel => {
                    self.as_mut_list_state().cancel_search();
                    self.clear_search_query();
                }
                _ => {}
            }
            // Do not return here
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let commit_list = CommitList::new(self.ctx.clone());
        f.render_stateful_widget(commit_list, area, self.as_mut_list_state());
    }

    pub fn update_layout(&mut self, area: Rect) {
        let height = if area.height >= 2 {
            area.height - 2
        } else {
            area.height
        };
        self.as_mut_list_state().update_height(height as usize);
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

impl<'a> ListView<'a> {
    pub fn take_list_state(&mut self) -> CommitListState<'a> {
        self.commit_list_state.take().unwrap()
    }

    #[allow(dead_code)]
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

    fn update_search_query(&self) {
        if let SearchState::Searching { .. } = self.as_list_state().search_state() {
            let list_state = self.as_list_state();
            if let Some(query) = list_state.search_query_string() {
                let cursor_pos = list_state.search_query_cursor_position();
                let transient_msg = list_state.transient_message_string();
                self.tx.send(AppEvent::UpdateStatusInput(
                    query,
                    Some(cursor_pos),
                    transient_msg,
                ));
            }
        }
    }

    fn clear_search_query(&self) {
        self.tx.send(AppEvent::ClearStatusLine);
    }

    fn update_matched_message(&self) {
        if let Some((msg, matched)) = self.as_list_state().matched_query_string() {
            if matched {
                self.tx.send(AppEvent::NotifyInfo(msg));
            } else {
                self.tx.send(AppEvent::NotifyWarn(msg));
            }
        } else {
            self.tx.send(AppEvent::ClearStatusLine);
        }
    }

    fn copy_commit_hash(&self) {
        let selected = self.as_list_state().selected_commit_hash();
        self.copy_to_clipboard("Commit SHA".into(), selected.as_str().into());
    }

    fn copy_commit_message(&self) {
        if let Some(commit_message) = self.as_list_state().selected_commit_message() {
            self.copy_to_clipboard("Commit message".into(), commit_message.into());
        }
    }

    fn copy_to_clipboard(&self, name: String, value: String) {
        self.tx.send(AppEvent::CopyToClipboard { name, value });
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        let bg = theme.bg;
        std::rc::Rc::make_mut(&mut self.ctx).color_theme = theme;
        self.as_mut_list_state().invalidate_image_caches(bg);
        self.ctx.avatar_manager.lock().unwrap().clear_prepared_images();
    }

    pub fn refresh(&self) {
        let list_state = self.as_list_state();
        let list_context = ListRefreshViewContext::from(list_state);
        let context = RefreshViewContext::List {
            list_context,
            pending_notification: None,
        };
        self.tx.send(AppEvent::Refresh(context));
    }

    pub fn reset_commit_list_with(&mut self, list_context: &ListRefreshViewContext) {
        let ListRefreshViewContext {
            commit_hash,
            selected,
            height,
            scroll_to_top,
        } = list_context;
        let list_state = self.as_mut_list_state();
        list_state.reset_height(*height);
        if *scroll_to_top {
            list_state.select_first();
        } else {
            list_state.select_commit_hash(&CommitHash::from(commit_hash.as_str()));
            if list_state.total() > *height {
                list_state.restore_visual_selection(*selected);
            }
        }
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(list_state) = self.commit_list_state.as_mut() {
            let header_height = 5u16; // app header (3) + column header (2)
            if row < header_height {
                return;
            }

            if let Some(branch_name) = list_state.branch_at_position(col, row) {
                self.tx.send(AppEvent::OpenBranchDetail { branch_name });
                return;
            }
            if let Some(tag_name) = list_state.tag_at_position(col, row) {
                self.tx.send(AppEvent::OpenTagDetail { tag_name });
                return;
            }

            let (_, offset, height) = list_state.current_list_status();
            let row = (row - header_height) as usize;
            if row < height {
                let clicked_index = offset + row;
                if clicked_index < list_state.total() {
                    list_state.select(clicked_index);
                    let is_uncommitted = list_state.is_uncommitted_selected();
                    if list_state.search_state().is_active() {
                        list_state.cancel_search();
                        self.clear_search_query();
                    }
                    if is_uncommitted {
                        let _ = self.tx.send(AppEvent::OpenUncommitted);
                    } else {
                        let _ = self.tx.send(AppEvent::OpenDetail);
                    }
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) -> bool {
        let Some(list_state) = self.commit_list_state.as_mut() else {
            return false;
        };
        let (prev_selected, prev_offset, _) = list_state.current_list_status();
        let prev_branch = list_state.hovered_branch.clone();
        let prev_tag = list_state.hovered_tag.clone();

        let header_height = 2u16;
        if row < header_height {
            list_state.set_hovered_branch(None);
            list_state.set_hovered_tag(None);
            list_state.set_hovered_row(None);
        } else {
            let (selected, offset, height) = list_state.current_list_status();
            let visible_row = (row - header_height) as usize;

            if let Some(branch_name) = list_state.branch_at_position(col, row) {
                list_state.set_hovered_branch(Some(branch_name));
                list_state.set_hovered_tag(None);
                list_state.set_hovered_row(Some(visible_row));
            } else if let Some(tag_name) = list_state.tag_at_position(col, row) {
                list_state.set_hovered_tag(Some(tag_name));
                list_state.set_hovered_branch(None);
                list_state.set_hovered_row(Some(visible_row));
            } else {
                list_state.set_hovered_branch(None);
                list_state.set_hovered_tag(None);
                list_state.set_hovered_row(None);
                if visible_row < height {
                    let hover_idx = offset + visible_row;
                    let current_selected = offset + selected;
                    if hover_idx != current_selected {
                        list_state.select(hover_idx);
                    }
                }
            }
        }

        let (new_selected, new_offset, _) = list_state.current_list_status();
        prev_selected != new_selected
            || prev_offset != new_offset
            || list_state.hovered_branch != prev_branch
            || list_state.hovered_tag != prev_tag
    }
}
