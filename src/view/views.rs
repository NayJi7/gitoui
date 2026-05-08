use std::rc::Rc;

use ratatui::{crossterm::event::KeyEvent, layout::Rect, Frame};

use crate::{
    app::AppContext,
    event::{DialogKind, Sender, UserEventWithCount},
    git::{Commit, CommitHash, FileChange, Ref},
    view::{
        branch_detail::BranchDetailView, config::ConfigView, detail::DetailView,
        dialog::DialogView, diff::DiffView, help::HelpView, list::ListView, refs::RefsView,
        tag_detail::TagDetailView, uncommitted::UncommittedView, user_command::UserCommandView,
    },
    widget::commit_list::CommitListState,
};

#[derive(Debug, Default)]
pub enum View<'a> {
    #[default]
    Default, // dummy variant to make #[default] work
    List(Box<ListView<'a>>),
    Detail(Box<DetailView<'a>>),
    Diff(Box<DiffView<'a>>),
    UserCommand(Box<UserCommandView<'a>>),
    Refs(Box<RefsView<'a>>),
    Help(Box<HelpView<'a>>),
    Config(Box<ConfigView<'a>>),
    Dialog(Box<DialogView<'a>>),
    BranchDetail(Box<BranchDetailView<'a>>),
    TagDetail(Box<TagDetailView<'a>>),
    Uncommitted(Box<UncommittedView<'a>>),
}

impl<'a> View<'a> {
    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key_event: KeyEvent) {
        match self {
            View::Default => {}
            View::List(view) => view.handle_event(event_with_count, key_event),
            View::Detail(view) => view.handle_event(event_with_count, key_event),
            View::Diff(view) => view.handle_event(event_with_count, key_event),
            View::UserCommand(view) => view.handle_event(event_with_count, key_event),
            View::Refs(view) => view.handle_event(event_with_count, key_event),
            View::Help(view) => view.handle_event(event_with_count, key_event),
            View::Config(view) => view.handle_event(event_with_count, key_event),
            View::Dialog(view) => view.handle_event(event_with_count, key_event),
            View::BranchDetail(view) => view.handle_event(event_with_count, key_event),
            View::TagDetail(view) => view.handle_event(event_with_count, key_event),
            View::Uncommitted(view) => view.handle_event(event_with_count, key_event),
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        match self {
            View::Default => {}
            View::List(view) => view.render(f, area),
            View::Detail(view) => view.render(f, area),
            View::Diff(view) => view.render(f, area),
            View::UserCommand(view) => view.render(f, area),
            View::Refs(view) => view.render(f, area),
            View::Help(view) => view.render(f, area),
            View::Config(view) => view.render(f, area),
            View::Dialog(view) => view.render(f, area),
            View::BranchDetail(view) => view.render(f, area),
            View::TagDetail(view) => view.render(f, area),
            View::Uncommitted(view) => view.render(f, area),
        }
    }

    pub fn update_layout(&mut self, area: Rect) {
        match self {
            View::Default => {}
            View::List(view) => view.update_layout(area),
            View::Detail(view) => view.update_layout(area),
            View::Diff(view) => view.update_layout(area),
            View::UserCommand(view) => view.update_layout(area),
            View::Refs(view) => view.update_layout(area),
            View::Help(_) => {}
            View::Config(_) => {}
            View::Dialog(_) => {}
            View::BranchDetail(view) => view.update_layout(area),
            View::TagDetail(view) => view.update_layout(area),
            View::Uncommitted(view) => view.update_layout(area),
        }
    }

    pub fn prepare_graph_uploads(&mut self) {
        match self {
            View::Default => {}
            View::List(view) => view.prepare_graph_uploads(),
            View::Detail(view) => view.prepare_graph_uploads(),
            View::Diff(view) => view.prepare_graph_uploads(),
            View::UserCommand(view) => view.prepare_graph_uploads(),
            View::Refs(view) => view.prepare_graph_uploads(),
            View::Help(_) => {}
            View::Config(_) => {}
            View::Dialog(_) => {}
            View::BranchDetail(view) => view.prepare_graph_uploads(),
            View::TagDetail(view) => view.prepare_graph_uploads(),
            View::Uncommitted(view) => view.prepare_graph_uploads(),
        }
    }

    pub fn clear_graph_images(&mut self) {
        match self {
            View::List(view) => view.clear_graph_images(),
            View::Detail(view) => view.clear_graph_images(),
            View::Diff(view) => view.clear_graph_images(),
            View::UserCommand(view) => view.clear_graph_images(),
            View::Refs(view) => view.clear_graph_images(),
            View::BranchDetail(view) => view.clear_graph_images(),
            View::TagDetail(view) => view.clear_graph_images(),
            View::Uncommitted(view) => view.clear_graph_images(),
            _ => {}
        }
    }

    pub fn dialog_area(&self) -> Option<ratatui::layout::Rect> {
        if let View::Dialog(view) = self {
            Some(view.dialog_area())
        } else {
            None
        }
    }

    pub fn drain_pending_avatar_deletes(&mut self) -> Vec<u16> {
        match self {
            View::Detail(view) => view.drain_pending_avatar_deletes(),
            _ => Vec::new(),
        }
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        match self {
            View::Default => Vec::new(),
            View::List(view) => view.drain_pending_graph_uploads(),
            View::Detail(view) => view.drain_pending_graph_uploads(),
            View::Diff(view) => view.drain_pending_graph_uploads(),
            View::UserCommand(view) => view.drain_pending_graph_uploads(),
            View::Refs(view) => view.drain_pending_graph_uploads(),
            View::Help(_) => Vec::new(),
            View::Config(_) => Vec::new(),
            View::Dialog(_) => Vec::new(),
            View::BranchDetail(view) => view.drain_pending_graph_uploads(),
            View::TagDetail(view) => view.drain_pending_graph_uploads(),
            View::Uncommitted(view) => view.drain_pending_graph_uploads(),
        }
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        match self {
            View::Default => Vec::new(),
            View::List(view) => view.graph_image_ids_sorted(),
            View::Detail(view) => view.graph_image_ids_sorted(),
            View::Diff(view) => view.graph_image_ids_sorted(),
            View::UserCommand(view) => view.graph_image_ids_sorted(),
            View::Refs(view) => view.graph_image_ids_sorted(),
            View::Help(view) => view.graph_image_ids_sorted(),
            View::Config(view) => view.graph_image_ids_sorted(),
            View::Dialog(_) => Vec::new(),
            View::BranchDetail(view) => view.graph_image_ids_sorted(),
            View::TagDetail(view) => view.graph_image_ids_sorted(),
            View::Uncommitted(view) => view.graph_image_ids_sorted(),
        }
    }

    pub fn is_search_active(&self) -> bool {
        match self {
            View::Default => false,
            View::List(view) => view.as_list_state().search_state().is_active(),
            View::Detail(view) => view.as_list_state().search_state().is_active(),
            View::Diff(view) => view
                .as_list_state()
                .map_or(false, |s| s.search_state().is_active()),
            View::UserCommand(view) => view.as_list_state().search_state().is_active(),
            View::Refs(view) => view.as_list_state().search_state().is_active(),
            View::Help(view) => view.is_search_active(),
            View::Config(view) => view.is_search_active(),
            View::Dialog(_) => false,
            View::BranchDetail(_) => false,
            View::TagDetail(_) => false,
            View::Uncommitted(_) => false,
        }
    }

    pub fn is_search_querying(&self) -> bool {
        match self {
            View::Default => false,
            View::List(view) => view.as_list_state().search_state().is_querying(),
            View::Detail(view) => view.as_list_state().search_state().is_querying(),
            View::Diff(view) => view
                .as_list_state()
                .map_or(false, |s| s.search_state().is_querying()),
            View::UserCommand(view) => view.as_list_state().search_state().is_querying(),
            View::Refs(view) => view.as_list_state().search_state().is_querying(),
            View::Help(view) => view.is_search_querying(),
            View::Config(view) => view.is_search_querying(),
            View::Dialog(_) => false,
            View::BranchDetail(_) => false,
            View::TagDetail(_) => false,
            View::Uncommitted(_) => false,
        }
    }

    pub fn search_case_fuzzy(&self) -> Option<(bool, bool)> {
        match self {
            View::Default => None,
            View::List(view) => view.as_list_state().search_case_fuzzy(),
            View::Detail(view) => view.as_list_state().search_case_fuzzy(),
            View::Diff(view) => view.as_list_state().and_then(|s| s.search_case_fuzzy()),
            View::UserCommand(view) => view.as_list_state().search_case_fuzzy(),
            View::Refs(view) => view.as_list_state().search_case_fuzzy(),
            View::Help(view) => view.search_case_fuzzy(),
            View::Config(view) => view.search_case_fuzzy(),
            View::Dialog(_) => None,
            View::BranchDetail(_) => None,
            View::TagDetail(_) => None,
            View::Uncommitted(_) => None,
        }
    }

    pub fn is_config_active(&self) -> bool {
        matches!(self, View::Config(_))
    }

    pub fn diff_footer_hint(&self) -> Option<String> {
        match self {
            View::Diff(view) => Some(view.footer_hint()),
            _ => None,
        }
    }

    pub fn uncommitted_footer_hint(&self) -> Option<String> {
        match self {
            View::Uncommitted(view) => Some(view.footer_hint()),
            _ => None,
        }
    }

    pub fn config_footer_hint(&self) -> Option<String> {
        match self {
            View::Config(view) => Some(view.footer_hint()),
            _ => None,
        }
    }

    pub fn of_list(
        commit_list_state: CommitListState<'a>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        View::List(Box::new(ListView::new(commit_list_state, ctx, tx)))
    }

    pub fn of_detail(
        commit_list_state: CommitListState<'a>,
        commit: Commit,
        changes: Vec<FileChange>,
        refs: Vec<Ref>,
        head_branch_name: Option<String>,
        head_commit_hash: Option<CommitHash>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        View::Detail(Box::new(DetailView::new(
            commit_list_state,
            commit,
            changes,
            refs,
            head_branch_name,
            head_commit_hash,
            ctx,
            tx,
        )))
    }

    pub fn of_diff_with_entries(
        commit_list_state: CommitListState<'a>,
        diff_entries: Vec<crate::git::diff::DiffEntry>,
        ctx: Rc<AppContext>,
        tx: Sender,
        title: String,
        commit_hash: String,
        all_file_paths: Vec<(String, bool)>,
        repo_path: std::path::PathBuf,
    ) -> Self {
        View::Diff(Box::new(DiffView::new(
            Some(commit_list_state),
            diff_entries,
            ctx,
            tx,
            title,
            commit_hash,
            all_file_paths,
            repo_path,
        )))
    }

    pub fn of_uncommitted_diff(
        commit_list_state: Option<CommitListState<'a>>,
        diff_entries: Vec<crate::git::diff::DiffEntry>,
        ctx: Rc<AppContext>,
        tx: Sender,
        title: String,
        all_file_paths: Vec<(String, bool)>,
        repo_path: std::path::PathBuf,
    ) -> Self {
        View::Diff(Box::new(DiffView::new(
            commit_list_state,
            diff_entries,
            ctx,
            tx,
            title,
            String::new(),
            all_file_paths,
            repo_path,
        )))
    }

    pub fn of_user_command(
        commit_list_state: CommitListState<'a>,
        command_output: String,
        user_command_number: usize,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        View::UserCommand(Box::new(UserCommandView::new(
            commit_list_state,
            command_output,
            user_command_number,
            ctx,
            tx,
        )))
    }

    pub fn of_refs(
        commit_list_state: CommitListState<'a>,
        refs: Vec<Ref>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        View::Refs(Box::new(RefsView::new(commit_list_state, refs, ctx, tx)))
    }

    pub fn of_help(before: View<'a>, ctx: Rc<AppContext>, tx: Sender) -> Self {
        View::Help(Box::new(HelpView::new(before, ctx, tx)))
    }

    pub fn of_config(before: View<'a>, ctx: Rc<AppContext>, tx: Sender) -> Self {
        View::Config(Box::new(ConfigView::new(before, ctx, tx)))
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        match self {
            View::List(view) => view.handle_click(col, row),
            View::Detail(view) => view.handle_click(col, row),
            View::Diff(view) => view.handle_click(col, row),
            View::UserCommand(view) => view.handle_click(col, row),
            View::Refs(view) => view.handle_click(col, row),
            View::Config(view) => view.handle_click(col, row),
            View::Dialog(view) => view.handle_click(col, row),
            View::BranchDetail(view) => view.handle_click(col, row),
            View::TagDetail(view) => view.handle_click(col, row),
            View::Uncommitted(view) => view.handle_click(col, row),
            _ => {}
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) -> bool {
        match self {
            View::List(view) => view.handle_mouse_move(col, row),
            View::Detail(view) => {
                view.handle_mouse_move(col, row);
                true
            }
            View::Diff(view) => {
                view.handle_mouse_move(col, row);
                true
            }
            View::Refs(view) => {
                view.handle_mouse_move(col, row);
                true
            }
            View::Config(view) => {
                view.handle_mouse_move(col, row);
                true
            }
            View::Dialog(view) => {
                view.handle_mouse_move(col, row);
                true
            }
            View::BranchDetail(view) => {
                view.handle_mouse_move(col, row);
                true
            }
            View::TagDetail(view) => {
                view.handle_mouse_move(col, row);
                true
            }
            View::Uncommitted(view) => {
                view.handle_mouse_move(col, row);
                true
            }
            _ => false,
        }
    }

    pub fn refresh(&mut self) {
        match self {
            View::Default => {}
            View::List(view) => view.refresh(),
            View::Detail(view) => view.refresh(),
            View::Diff(view) => view.refresh(),
            View::UserCommand(view) => view.refresh(),
            View::Refs(view) => view.refresh(),
            View::Help(_) => {}
            View::Config(_) => {}
            View::Dialog(_) => {}
            View::BranchDetail(view) => view.refresh(),
            View::TagDetail(view) => view.refresh(),
            View::Uncommitted(view) => view.refresh(),
        }
    }

    pub fn is_input_active(&self) -> bool {
        match self {
            View::Config(view) => view.is_editing_text(),
            View::Dialog(view) => view.is_input_focused(),
            _ => false,
        }
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        match self {
            View::Default => {}
            View::List(v) => v.update_color_theme(theme),
            View::Detail(v) => v.update_color_theme(theme),
            View::Diff(v) => v.update_color_theme(theme),
            View::UserCommand(v) => v.update_color_theme(theme),
            View::Refs(v) => v.update_color_theme(theme),
            View::Help(v) => v.update_color_theme(theme),
            View::Config(v) => v.update_color_theme(theme),
            View::Dialog(v) => v.update_color_theme(theme),
            View::BranchDetail(v) => v.update_color_theme(theme),
            View::TagDetail(v) => v.update_color_theme(theme),
            View::Uncommitted(v) => v.update_color_theme(theme),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RefreshViewContext {
    List {
        list_context: ListRefreshViewContext,
        pending_notification: Option<String>,
    },
    Detail {
        list_context: ListRefreshViewContext,
    },
    Diff {
        list_context: ListRefreshViewContext,
    },
    UserCommand {
        list_context: ListRefreshViewContext,
        user_command_context: UserCommandRefreshViewContext,
    },
    Refs {
        list_context: ListRefreshViewContext,
        refs_context: RefsRefreshViewContext,
    },
}

impl RefreshViewContext {
    pub fn list_context(&self) -> &ListRefreshViewContext {
        match self {
            RefreshViewContext::List { list_context, .. }
            | RefreshViewContext::Detail { list_context }
            | RefreshViewContext::Diff { list_context }
            | RefreshViewContext::UserCommand { list_context, .. }
            | RefreshViewContext::Refs { list_context, .. } => list_context,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ListRefreshViewContext {
    pub commit_hash: String,
    pub selected: usize,
    pub height: usize,
    pub scroll_to_top: bool,
}

impl From<&CommitListState<'_>> for ListRefreshViewContext {
    fn from(list_state: &CommitListState) -> Self {
        let commit_hash = list_state.selected_commit_hash().as_str().into();
        let (selected, offset, height) = list_state.current_list_status();
        // If the selected commit is the top one and there is no offset, it means the list is already scrolled to the top.
        // In this case, we set scroll_to_top to true to indicate that the view should be scrolled to the top after refresh.
        let scroll_to_top = selected == 0 && offset == 0;
        ListRefreshViewContext {
            commit_hash,
            selected,
            height,
            scroll_to_top,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserCommandRefreshViewContext {
    pub n: usize,
}

#[derive(Debug, Clone)]
pub struct RefsRefreshViewContext {
    pub selected: Vec<String>,
    pub opened: Vec<Vec<String>>,
}
