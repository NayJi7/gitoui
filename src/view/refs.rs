use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Constraint, Layout, Rect},
    Frame,
};

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    git::Ref,
    view::{ListRefreshViewContext, RefreshViewContext, RefsRefreshViewContext},
    widget::{
        commit_list::{CommitList, CommitListState},
        ref_list::{RefList, RefListState},
    },
};

#[derive(Debug)]
pub struct RefsView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    ref_list_state: RefListState,

    refs: Vec<Ref>,

    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> RefsView<'a> {
    pub fn new(
        commit_list_state: CommitListState<'a>,
        refs: Vec<Ref>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> RefsView<'a> {
        RefsView {
            commit_list_state: Some(commit_list_state),
            ref_list_state: RefListState::new(),
            refs,
            ctx,
            tx,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;

        match event {
            UserEvent::Quit => {
                self.tx.send(AppEvent::Quit);
            }
            UserEvent::Cancel | UserEvent::Close | UserEvent::RefList => {
                self.tx.send(AppEvent::CloseRefs);
            }
            UserEvent::NavigateDown | UserEvent::SelectDown => {
                for _ in 0..count {
                    self.ref_list_state.select_next();
                }
                self.update_commit_list_selected();
            }
            UserEvent::NavigateUp | UserEvent::SelectUp => {
                for _ in 0..count {
                    self.ref_list_state.select_prev();
                }
                self.update_commit_list_selected();
            }
            UserEvent::ScrollDown => {
                for _ in 0..count {
                    self.ref_list_state.scroll_down(3);
                }
            }
            UserEvent::ScrollUp => {
                for _ in 0..count {
                    self.ref_list_state.scroll_up(3);
                }
            }
            UserEvent::GoToTop => {
                self.ref_list_state.select_first();
                self.update_commit_list_selected();
            }
            UserEvent::GoToBottom => {
                self.ref_list_state.select_last();
                self.update_commit_list_selected();
            }
            UserEvent::NavigateRight => {
                self.ref_list_state.open_node();
                self.update_commit_list_selected();
            }
            UserEvent::NavigateLeft => {
                self.ref_list_state.close_node();
                self.update_commit_list_selected();
            }
            UserEvent::Confirm => {
                if self.ref_list_state.selected_is_node() {
                    self.ref_list_state.toggle_selected();
                    self.update_commit_list_selected();
                } else if let Some(branch_name) = self.ref_list_state.selected_branch() {
                    self.tx.send(AppEvent::OpenBranchDetail { branch_name });
                } else if let Some(tag_name) = self.ref_list_state.selected_tag() {
                    self.tx.send(AppEvent::OpenTagDetail { tag_name });
                } else {
                    self.ref_list_state.toggle_selected();
                    self.update_commit_list_selected();
                }
            }
            UserEvent::ShortCopy | UserEvent::FullCopy => {
                self.copy_ref_name();
            }
            UserEvent::HelpToggle => {
                self.tx.send(AppEvent::OpenHelp);
            }
            UserEvent::Refresh => {
                self.refresh();
            }
            _ => {}
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let [list_area, refs_area] = self.split_areas(area);

        let commit_list = CommitList::new(self.ctx.clone());
        f.render_stateful_widget(commit_list, list_area, self.as_mut_list_state());

        // Split refs area into header (2 lines) and ref list
        let [header_area, refs_list_area] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(2),
            ratatui::layout::Constraint::Min(0),
        ])
        .areas(refs_area);

        self.render_refs_header(f, header_area);

        let ref_list = RefList::new(&self.refs, self.ctx.clone());
        f.render_stateful_widget(ref_list, refs_list_area, &mut self.ref_list_state);
    }

    fn render_refs_header(&self, f: &mut ratatui::Frame, area: ratatui::layout::Rect) {
        use ratatui::{
            style::{Color, Modifier, Style},
            text::{Line, Span},
            widgets::Paragraph,
        };

        let header_text = "Refs";
        let style = Style::default()
            .fg(Color::Rgb(86, 95, 137))
            .add_modifier(Modifier::BOLD);
        let line = Line::from(Span::styled(header_text.to_string(), style));
        let para = Paragraph::new(line);
        f.render_widget(
            para,
            ratatui::layout::Rect::new(area.x, area.y, area.width, 1),
        );

        // Draw separator line below header
        let sep_style = Style::default().fg(Color::Rgb(59, 66, 97));
        let sep_span = Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(Color::Rgb(59, 66, 97)),
        );
        let sep_line = Line::from(sep_span);
        let sep_para = Paragraph::new(sep_line);
        f.render_widget(
            sep_para,
            ratatui::layout::Rect::new(area.x, area.y + 1, area.width, 1),
        );
    }

    pub fn update_layout(&mut self, area: Rect) {
        let [list_area, _] = self.split_areas(area);
        let height = if list_area.height >= 2 {
            list_area.height - 2
        } else {
            list_area.height
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

impl<'a> RefsView<'a> {
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
        let graph_width = self.as_list_state().graph_area_cell_width() + 1; // graph area + marker
        let refs_width =
            (area.width.saturating_sub(graph_width)).min(self.ctx.ui_config.refs.width);
        Layout::horizontal([Constraint::Min(0), Constraint::Length(refs_width)]).areas(area)
    }

    fn update_commit_list_selected(&mut self) {
        if let Some(selected) = self.ref_list_state.selected_ref_name() {
            self.as_mut_list_state().select_ref(&selected)
        }
    }

    fn copy_ref_name(&self) {
        if let Some(selected) = self.ref_list_state.selected_branch() {
            self.copy_to_clipboard("Branch Name".into(), selected);
        } else if let Some(selected) = self.ref_list_state.selected_tag() {
            self.copy_to_clipboard("Tag Name".into(), selected);
        }
    }

    fn copy_to_clipboard(&self, name: String, value: String) {
        self.tx.send(AppEvent::CopyToClipboard { name, value });
    }

    pub fn refresh(&self) {
        let list_state = self.as_list_state();
        let list_context = ListRefreshViewContext::from(list_state);
        let (tree_selected, tree_opened) = self.ref_list_state.current_tree_status();
        let refs_context = RefsRefreshViewContext {
            selected: tree_selected,
            opened: tree_opened,
        };
        let context = RefreshViewContext::Refs {
            list_context,
            refs_context,
        };
        self.tx.send(AppEvent::Refresh(context));
    }

    pub fn reset_refs_with(&mut self, refs_context: RefsRefreshViewContext) {
        self.ref_list_state
            .reset_tree_status(refs_context.selected, refs_context.opened);
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        self.ref_list_state.handle_click(col, row);
        if self.ref_list_state.selected_is_node() {
            self.update_commit_list_selected();
        } else if let Some(branch_name) = self.ref_list_state.selected_branch() {
            self.tx.send(AppEvent::OpenBranchDetail { branch_name });
        } else if let Some(tag_name) = self.ref_list_state.selected_tag() {
            self.tx.send(AppEvent::OpenTagDetail { tag_name });
        } else {
            self.update_commit_list_selected();
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        if self.ref_list_state.handle_mouse_move(col, row) {
            self.update_commit_list_selected();
        }
    }
}
