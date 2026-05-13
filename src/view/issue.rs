//! GitHub Issues view — slices I1 → I4. List + detail (Conversation,
//! Timeline, Linked tabs) + Compose-new-issue, with full CRUD on
//! comments, reactions, labels/assignees/milestone, and close/reopen.
//!
//! Mirrors `view::pr` patterns to keep visual + interaction parity.
//! Where rendering primitives are reusable (comment cards, label
//! chips, markdown body), we import from `view::pr` rather than
//! duplicating.

use std::rc::Rc;

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use rustc_hash::FxHashMap;

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    github::issue::{
        Issue, IssueDetail, IssueState, IssueStateReason, LinkedPr, TimelineEvent, TimelineKind,
    },
    github::RepoCoords,
    view::pr::{
        build_body_prefix, build_junction_prefix, cursor_screen_pos, fit_cell,
        label_chip_spans_local, parse_hex_color, push_comment_card, render_markdown_body,
        short_label_chip_spans, word_left_boundary, word_right_boundary, wrap_styled_spans,
        CommentAction, CommentCardInput, MERGED_PURPLE, TREE_LEVEL_WIDTH,
    },
};

#[derive(Debug)]
pub struct IssuesView<'a> {
    commit_list_state: Option<crate::widget::commit_list::CommitListState<'a>>,
    coords: RepoCoords,
    token: String,
    me_login: Option<String>,
    items: Vec<Issue>,
    list_filter: IssueListFilter,
    filter_tab_rects: Vec<(IssueListFilter, Rect)>,
    hovered_filter: Option<IssueListFilter>,
    detail_cache: FxHashMap<u64, IssueDetail>,
    timeline_cache: FxHashMap<u64, Vec<TimelineEvent>>,
    linked_cache: FxHashMap<u64, Vec<LinkedPr>>,
    loading_for: Option<u64>,
    loading_timeline_for: Option<u64>,
    loading_linked_for: Option<u64>,
    hovered: usize,
    opened_issue_number: Option<u64>,
    mode: Mode,
    active_tab: Tab,
    tab_bar_rects: Vec<(Tab, Rect)>,
    hovered_tab: Option<Tab>,
    /// Active conversation tab state.
    conversation_scroll: usize,
    conversation_selected: usize,
    conversation_scroll_to_selected: bool,
    conversation_comment_spans: Vec<(usize, usize, usize)>,
    conversation_comment_count: usize,
    /// Open editor in either New or Edit mode — routes raw keys.
    comment_editor: Option<CommentEditor>,
    comment_editor_cursor_pos: Option<(u16, u16)>,
    editor_body_area: Option<Rect>,
    /// Reaction picker overlay; takes key+mouse focus when present.
    reaction_picker: Option<ReactionPicker>,
    /// Compose-new-issue draft. `Some` only when `mode == Compose`.
    compose: Option<ComposeState>,
    compose_field_rects: Vec<(ComposeField, Rect)>,
    last_error: Option<String>,
    last_action_toast: Option<String>,
    list_area: Option<Rect>,
    tab_content_area: Option<Rect>,
    list_inner_y: u16,
    list_scroll_offset: usize,
    ctx: Rc<AppContext>,
    tx: Sender,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    List,
    Detail,
    Compose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Conversation,
    Timeline,
    Linked,
}

impl Tab {
    fn all() -> [Tab; 3] {
        [Tab::Conversation, Tab::Timeline, Tab::Linked]
    }
    fn label(self) -> &'static str {
        match self {
            Tab::Conversation => "Conversation",
            Tab::Timeline => "Timeline",
            Tab::Linked => "Linked",
        }
    }
    fn index(self) -> usize {
        match self {
            Tab::Conversation => 0,
            Tab::Timeline => 1,
            Tab::Linked => 2,
        }
    }
    fn from_index(i: usize) -> Option<Self> {
        Self::all().get(i).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssueListFilter {
    Open,
    Closed,
    All,
}

impl IssueListFilter {
    fn label(self) -> &'static str {
        match self {
            IssueListFilter::Open => "Open",
            IssueListFilter::Closed => "Closed",
            IssueListFilter::All => "All",
        }
    }

    fn matches(self, s: IssueState) -> bool {
        match self {
            IssueListFilter::Open => matches!(s, IssueState::Open),
            IssueListFilter::Closed => matches!(s, IssueState::Closed),
            IssueListFilter::All => true,
        }
    }
}

/// Lean comment editor — only the two flows issues support: new
/// top-level comment and edit-own. Reuses the buffer/cursor model
/// from the PR view's editor.
#[derive(Debug, Clone)]
struct CommentEditor {
    kind: CommentEditorKind,
    buffer: String,
    cursor: usize,
    submitting: bool,
    scroll_offset: u16,
    last_body_height: u16,
}

#[derive(Debug, Clone)]
enum CommentEditorKind {
    NewTopLevel,
    EditTopLevel { comment_id: u64 },
}

impl CommentEditorKind {
    fn header(&self) -> &'static str {
        match self {
            CommentEditorKind::NewTopLevel => "Add a comment",
            CommentEditorKind::EditTopLevel { .. } => "Edit comment",
        }
    }
}

/// 8-emoji reaction overlay anchored to the selected comment.
#[derive(Debug, Clone)]
struct ReactionPicker {
    /// Conversation index the picker is anchored to.
    target_idx: usize,
    hovered: usize,
    overlay_rect: Option<Rect>,
    row_rects: Vec<Rect>,
}

#[derive(Debug, Clone)]
struct ComposeState {
    title: String,
    title_cursor: usize,
    body: String,
    body_cursor: usize,
    body_scroll: u16,
    body_last_height: u16,
    labels: Vec<String>,
    /// Available labels with colours — fetched on compose open to
    /// keep the picker render instant.
    available_labels: Vec<crate::github::pr::Label>,
    assignees: Vec<String>,
    available_assignees: Vec<String>,
    milestone: Option<u64>,
    available_milestones: Vec<crate::github::issue::Milestone>,
    field: ComposeField,
    submitting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposeField {
    Title,
    Body,
    Labels,
    Assignees,
    Milestone,
}

impl ComposeField {
    fn all() -> &'static [ComposeField] {
        &[
            ComposeField::Title,
            ComposeField::Body,
            ComposeField::Labels,
            ComposeField::Assignees,
            ComposeField::Milestone,
        ]
    }
    fn next(self) -> ComposeField {
        let all = Self::all();
        let i = all.iter().position(|f| *f == self).unwrap_or(0);
        all[(i + 1) % all.len()]
    }
    fn prev(self) -> ComposeField {
        let all = Self::all();
        let i = all.iter().position(|f| *f == self).unwrap_or(0);
        all[(i + all.len() - 1) % all.len()]
    }
}

impl<'a> IssuesView<'a> {
    pub fn new(
        commit_list_state: Option<crate::widget::commit_list::CommitListState<'a>>,
        coords: RepoCoords,
        token: String,
        items: Vec<Issue>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        let me_login = ctx
            .github_auth_state
            .login
            .clone()
            .filter(|s| !s.is_empty());
        Self {
            commit_list_state,
            coords,
            token,
            me_login,
            items,
            list_filter: IssueListFilter::Open,
            filter_tab_rects: Vec::new(),
            hovered_filter: None,
            detail_cache: FxHashMap::default(),
            timeline_cache: FxHashMap::default(),
            linked_cache: FxHashMap::default(),
            loading_for: None,
            loading_timeline_for: None,
            loading_linked_for: None,
            hovered: 0,
            opened_issue_number: None,
            mode: Mode::List,
            active_tab: Tab::Conversation,
            tab_bar_rects: Vec::new(),
            hovered_tab: None,
            conversation_scroll: 0,
            conversation_selected: 0,
            conversation_scroll_to_selected: true,
            conversation_comment_spans: Vec::new(),
            conversation_comment_count: 1,
            comment_editor: None,
            comment_editor_cursor_pos: None,
            editor_body_area: None,
            reaction_picker: None,
            compose: None,
            compose_field_rects: Vec::new(),
            last_error: None,
            last_action_toast: None,
            list_area: None,
            tab_content_area: None,
            list_inner_y: 0,
            list_scroll_offset: 0,
            ctx,
            tx,
        }
    }

    pub fn take_list_state(
        &mut self,
    ) -> Option<crate::widget::commit_list::CommitListState<'a>> {
        self.commit_list_state.take()
    }

    pub fn on_detail_fetched(&mut self, number: u64, result: Result<IssueDetail, String>) {
        if self.loading_for == Some(number) {
            self.loading_for = None;
        }
        match result {
            Ok(d) => {
                self.detail_cache.insert(number, d);
                self.last_error = None;
            }
            Err(e) => {
                self.last_error = Some(format!("Fetch issue #{}: {}", number, e));
            }
        }
    }

    pub fn on_timeline_fetched(
        &mut self,
        number: u64,
        result: Result<Vec<TimelineEvent>, String>,
    ) {
        if self.loading_timeline_for == Some(number) {
            self.loading_timeline_for = None;
        }
        match result {
            Ok(v) => {
                self.timeline_cache.insert(number, v);
            }
            Err(e) => {
                self.last_error = Some(format!("Timeline #{}: {}", number, e));
            }
        }
    }

    pub fn on_linked_fetched(
        &mut self,
        number: u64,
        result: Result<Vec<LinkedPr>, String>,
    ) {
        if self.loading_linked_for == Some(number) {
            self.loading_linked_for = None;
        }
        match result {
            Ok(v) => {
                self.linked_cache.insert(number, v);
            }
            Err(e) => {
                self.last_error = Some(format!("Linked #{}: {}", number, e));
            }
        }
    }

    pub fn on_action_done(
        &mut self,
        number: u64,
        action: String,
        result: Result<(), String>,
    ) {
        if let Some(ed) = self.comment_editor.as_mut() {
            ed.submitting = false;
        }
        if let Some(c) = self.compose.as_mut() {
            c.submitting = false;
        }
        match result {
            Ok(()) => {
                self.last_action_toast = Some(action);
                self.last_error = None;
                // Editor closes on success.
                self.comment_editor = None;
                // Invalidate caches so the next open re-fetches.
                self.detail_cache.remove(&number);
                self.timeline_cache.remove(&number);
                self.linked_cache.remove(&number);
                if self.opened_issue_number == Some(number) {
                    self.spawn_detail_fetch(number);
                    if matches!(self.active_tab, Tab::Timeline) {
                        self.spawn_timeline_fetch(number);
                    }
                    if matches!(self.active_tab, Tab::Linked) {
                        self.spawn_linked_fetch(number);
                    }
                }
                // Re-list so the list view reflects the new state.
                if let Ok(v) = crate::github::issue::list_issues(&self.token, &self.coords) {
                    self.items = v;
                }
            }
            Err(e) => {
                self.last_error = Some(format!("{}: {}", action, e));
            }
        }
    }

    pub fn on_issue_created(&mut self, number: u64) {
        if let Some(c) = self.compose.as_mut() {
            c.submitting = false;
        }
        self.compose = None;
        self.mode = Mode::Detail;
        self.opened_issue_number = Some(number);
        self.active_tab = Tab::Conversation;
        self.conversation_scroll = 0;
        self.conversation_selected = 0;
        // Refresh the index and load the new issue's detail.
        if let Ok(v) = crate::github::issue::list_issues(&self.token, &self.coords) {
            self.items = v;
        }
        self.spawn_detail_fetch(number);
        self.last_action_toast = Some(format!("Created issue #{}", number));
    }

    pub fn on_compose_labels_picked(&mut self, labels: Vec<String>) {
        if let Some(c) = self.compose.as_mut() {
            c.labels = labels;
        }
    }

    pub fn on_compose_assignees_picked(&mut self, assignees: Vec<String>) {
        if let Some(c) = self.compose.as_mut() {
            c.assignees = assignees;
        }
    }

    pub fn on_compose_milestone_picked(&mut self, milestone: Option<u64>) {
        if let Some(c) = self.compose.as_mut() {
            c.milestone = milestone;
        }
    }

    pub fn footer_hint(&self) -> String {
        if self.comment_editor.is_some() {
            return "⌘ Ctrl+S:send▕▏Esc:cancel".into();
        }
        if self.reaction_picker.is_some() {
            return "⌘ ←→:nav▕▏↵:react▕▏Esc:cancel".into();
        }
        match self.mode {
            Mode::List => "⌘ ↑↓:nav▕▏↵:open▕▏n:new▕▏Tab:filter▕▏Esc:close".into(),
            Mode::Detail => {
                "⌘ ↑↓:nav▕▏c:comment▕▏+:react▕▏L/A/M:meta▕▏x:close▕▏o:reopen▕▏Esc:back".into()
            }
            Mode::Compose => "⌘ ↑↓:field▕▏Ctrl+S:submit▕▏Esc:cancel".into(),
        }
    }

    pub fn update_layout(&mut self, _area: Rect) {}
    pub fn prepare_graph_uploads(&mut self) {}
    pub fn clear_graph_images(&mut self) {}
    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        Vec::new()
    }
    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        Vec::new()
    }
    pub fn refresh(&mut self) {
        match crate::github::issue::list_issues(&self.token, &self.coords) {
            Ok(v) => {
                self.items = v;
                self.last_error = None;
            }
            Err(e) => {
                self.last_error = Some(format!("List issues: {}", e));
            }
        }
        if let Some(n) = self.opened_issue_number {
            self.detail_cache.remove(&n);
            self.spawn_detail_fetch(n);
        }
    }
    pub fn update_color_theme(&mut self, _theme: crate::color::ColorTheme) {}
    pub fn is_input_active(&self) -> bool {
        // Comment editor + Compose title/body capture raw keys; the
        // app routes Backspace/Delete only when this returns true.
        self.comment_editor.is_some()
            || (matches!(self.mode, Mode::Compose)
                && matches!(
                    self.compose.as_ref().map(|c| c.field),
                    Some(ComposeField::Title) | Some(ComposeField::Body)
                ))
    }

    // ─── Event dispatch ──────────────────────────────────────────

    pub fn handle_event(&mut self, evt: UserEventWithCount, key: KeyEvent) {
        if self.reaction_picker.is_some() {
            self.handle_event_reaction_picker(key);
            return;
        }
        if self.comment_editor.is_some() {
            // Mouse wheel inside the comment editor scrolls the
            // editor viewport (not the conversation behind it).
            match evt.event {
                UserEvent::ScrollUp => {
                    self.editor_scroll_viewport(-1);
                    return;
                }
                UserEvent::ScrollDown => {
                    self.editor_scroll_viewport(1);
                    return;
                }
                _ => {}
            }
            self.handle_event_comment_editor(key);
            return;
        }
        // Semantic event dispatch — gives us count-aware nav (e.g.
        // typing `5j` to move 5 rows down), mouse wheel, and a single
        // path that all bindings flow through.
        let n = evt.count.max(1) as i32;
        match (self.mode, evt.event) {
            (Mode::List, UserEvent::NavigateUp) => {
                self.list_move_hovered(-n);
                return;
            }
            (Mode::List, UserEvent::NavigateDown) => {
                self.list_move_hovered(n);
                return;
            }
            (Mode::List, UserEvent::PageUp) => {
                self.list_move_hovered(-10 * n);
                return;
            }
            (Mode::List, UserEvent::PageDown) => {
                self.list_move_hovered(10 * n);
                return;
            }
            (Mode::List, UserEvent::GoToTop) => {
                self.hovered = 0;
                return;
            }
            (Mode::List, UserEvent::GoToBottom) => {
                self.hovered = self.filtered_indices().len().saturating_sub(1);
                return;
            }
            (Mode::List, UserEvent::ScrollUp) => {
                self.list_scroll_offset = self.list_scroll_offset.saturating_sub(n as usize);
                return;
            }
            (Mode::List, UserEvent::ScrollDown) => {
                self.list_scroll_offset = self.list_scroll_offset.saturating_add(n as usize);
                return;
            }
            (Mode::List, UserEvent::Confirm) => {
                self.open_hovered();
                return;
            }
            (Mode::List, UserEvent::NavigateLeft) => {
                self.cycle_list_filter(-1);
                return;
            }
            (Mode::List, UserEvent::NavigateRight) => {
                self.cycle_list_filter(1);
                return;
            }
            (Mode::List, UserEvent::Cancel) => {
                self.tx.send(AppEvent::CloseIssues);
                return;
            }
            (Mode::Detail, UserEvent::NavigateUp) => {
                self.detail_move_selected(-n);
                return;
            }
            (Mode::Detail, UserEvent::NavigateDown) => {
                self.detail_move_selected(n);
                return;
            }
            (Mode::Detail, UserEvent::PageUp) => {
                self.detail_move_selected(-10 * n);
                return;
            }
            (Mode::Detail, UserEvent::PageDown) => {
                self.detail_move_selected(10 * n);
                return;
            }
            (Mode::Detail, UserEvent::GoToTop) => {
                self.conversation_selected = 0;
                self.conversation_scroll_to_selected = true;
                return;
            }
            (Mode::Detail, UserEvent::GoToBottom) => {
                self.conversation_selected =
                    self.conversation_comment_count.saturating_sub(1);
                self.conversation_scroll_to_selected = true;
                return;
            }
            (Mode::Detail, UserEvent::ScrollUp) => {
                self.conversation_scroll = self.conversation_scroll.saturating_sub(n as usize);
                return;
            }
            (Mode::Detail, UserEvent::ScrollDown) => {
                self.conversation_scroll = self.conversation_scroll.saturating_add(n as usize);
                return;
            }
            (Mode::Detail, UserEvent::Cancel) => {
                self.mode = Mode::List;
                self.opened_issue_number = None;
                self.active_tab = Tab::Conversation;
                return;
            }
            _ => {}
        }
        match self.mode {
            Mode::List => self.handle_event_list(key),
            Mode::Detail => self.handle_event_detail(key),
            Mode::Compose => self.handle_event_compose(key),
        }
    }

    fn list_move_hovered(&mut self, delta: i32) {
        let len = self.filtered_indices().len() as i32;
        if len == 0 {
            return;
        }
        let new = (self.hovered as i32 + delta).clamp(0, len - 1);
        self.hovered = new as usize;
    }

    fn detail_move_selected(&mut self, delta: i32) {
        let len = self.conversation_comment_count as i32;
        if len == 0 {
            return;
        }
        let new = (self.conversation_selected as i32 + delta).clamp(0, len - 1);
        self.conversation_selected = new as usize;
        self.conversation_scroll_to_selected = true;
    }

    fn cycle_list_filter(&mut self, delta: i32) {
        let order = [
            IssueListFilter::Open,
            IssueListFilter::Closed,
            IssueListFilter::All,
        ];
        let cur = order
            .iter()
            .position(|f| *f == self.list_filter)
            .unwrap_or(0) as i32;
        let len = order.len() as i32;
        let next = ((cur + delta) % len + len) % len;
        self.list_filter = order[next as usize];
        self.hovered = 0;
    }

    fn handle_event_list(&mut self, key: KeyEvent) {
        let filtered_len = self.filtered_indices().len();
        match key.code {
            KeyCode::Esc => {
                self.tx.send(AppEvent::CloseIssues);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.hovered > 0 {
                    self.hovered -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.hovered + 1 < filtered_len {
                    self.hovered += 1;
                }
            }
            KeyCode::Home | KeyCode::Char('g') => self.hovered = 0,
            KeyCode::End | KeyCode::Char('G') => {
                if filtered_len > 0 {
                    self.hovered = filtered_len - 1;
                }
            }
            KeyCode::Tab => {
                self.list_filter = match self.list_filter {
                    IssueListFilter::Open => IssueListFilter::Closed,
                    IssueListFilter::Closed => IssueListFilter::All,
                    IssueListFilter::All => IssueListFilter::Open,
                };
                self.hovered = 0;
            }
            KeyCode::BackTab => {
                self.list_filter = match self.list_filter {
                    IssueListFilter::Open => IssueListFilter::All,
                    IssueListFilter::Closed => IssueListFilter::Open,
                    IssueListFilter::All => IssueListFilter::Closed,
                };
                self.hovered = 0;
            }
            KeyCode::Enter => self.open_hovered(),
            KeyCode::Char('n') => self.start_compose(),
            _ => {}
        }
    }

    fn handle_event_detail(&mut self, key: KeyEvent) {
        let comment_count = self.conversation_comment_count.max(1);
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::List;
                self.opened_issue_number = None;
                self.active_tab = Tab::Conversation;
            }
            KeyCode::Up | KeyCode::Char('k') if matches!(self.active_tab, Tab::Conversation) => {
                if self.conversation_selected > 0 {
                    self.conversation_selected -= 1;
                    self.conversation_scroll_to_selected = true;
                }
            }
            KeyCode::Down | KeyCode::Char('j') if matches!(self.active_tab, Tab::Conversation) => {
                if self.conversation_selected + 1 < comment_count {
                    self.conversation_selected += 1;
                    self.conversation_scroll_to_selected = true;
                }
            }
            KeyCode::Home | KeyCode::Char('g') if matches!(self.active_tab, Tab::Conversation) => {
                self.conversation_selected = 0;
                self.conversation_scroll_to_selected = true;
            }
            KeyCode::End | KeyCode::Char('G') if matches!(self.active_tab, Tab::Conversation) => {
                self.conversation_selected = comment_count.saturating_sub(1);
                self.conversation_scroll_to_selected = true;
            }
            KeyCode::PageDown => {
                self.conversation_scroll = self.conversation_scroll.saturating_add(10);
            }
            KeyCode::PageUp => {
                self.conversation_scroll = self.conversation_scroll.saturating_sub(10);
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.conversation_scroll = self.conversation_scroll.saturating_add(5);
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.conversation_scroll = self.conversation_scroll.saturating_sub(5);
            }
            KeyCode::Tab => self.cycle_tab(1),
            KeyCode::BackTab => self.cycle_tab(-1),
            KeyCode::Char('c') => self.start_new_comment(),
            KeyCode::Char('e') => self.start_edit_own_comment(),
            KeyCode::Char('d') => self.confirm_delete_own_comment(),
            KeyCode::Char('+') => self.start_react(),
            KeyCode::Char('L') => self.start_labels_picker(),
            KeyCode::Char('A') => self.start_assignees_picker(),
            KeyCode::Char('M') => self.start_milestone_picker(),
            KeyCode::Char('x') => self.start_close(),
            KeyCode::Char('o') => self.start_reopen(),
            _ => {}
        }
    }

    fn cycle_tab(&mut self, delta: i32) {
        let cur = self.active_tab.index() as i32;
        let total = Tab::all().len() as i32;
        let next = ((cur + delta) % total + total) % total;
        if let Some(t) = Tab::from_index(next as usize) {
            self.set_active_tab(t);
        }
    }

    fn set_active_tab(&mut self, t: Tab) {
        self.active_tab = t;
        if let Some(n) = self.opened_issue_number {
            match t {
                Tab::Timeline if !self.timeline_cache.contains_key(&n) => {
                    self.spawn_timeline_fetch(n);
                }
                Tab::Linked if !self.linked_cache.contains_key(&n) => {
                    self.spawn_linked_fetch(n);
                }
                _ => {}
            }
        }
    }

    fn handle_event_compose(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(c) = self.compose.as_ref() else {
            return;
        };
        let field = c.field;
        let editing_text = matches!(field, ComposeField::Title | ComposeField::Body);
        let editing_body = matches!(field, ComposeField::Body);

        // Global escapes / submits.
        match key.code {
            KeyCode::Char('s') if ctrl => {
                self.submit_compose();
                return;
            }
            KeyCode::Esc => {
                self.compose = None;
                self.mode = Mode::List;
                return;
            }
            _ => {}
        }

        // Up/Down behaviour: when editing Body and there are multiple
        // logical rows on either side of the cursor, navigate within
        // it; otherwise (or in non-text fields) switch fields.
        if matches!(key.code, KeyCode::Up | KeyCode::Down) {
            if editing_body && self.compose_body_can_move_vertically(key.code) {
                self.compose_body_vertical(key.code);
                return;
            }
            if matches!(key.code, KeyCode::Up) {
                self.compose_field_prev();
            } else {
                self.compose_field_next();
            }
            return;
        }

        if !editing_text {
            // Labels / Assignees / Milestone: Enter opens the picker.
            if matches!(key.code, KeyCode::Enter) {
                self.compose_activate_field();
            }
            return;
        }

        // Text-editing keys on Title (single line) + Body (multi-line).
        match key.code {
            KeyCode::Backspace if ctrl => self.compose_field_delete_word_left(),
            KeyCode::Char('h') if ctrl => self.compose_field_delete_word_left(),
            KeyCode::Char('w') if ctrl => self.compose_field_delete_word_left(),
            KeyCode::Backspace => self.compose_field_delete_left(),
            KeyCode::Delete if ctrl => self.compose_field_delete_word_right(),
            KeyCode::Delete => self.compose_field_delete_right(),
            KeyCode::Left if ctrl => self.compose_field_word_left(),
            KeyCode::Right if ctrl => self.compose_field_word_right(),
            KeyCode::Left => self.compose_field_cursor_left(),
            KeyCode::Right => self.compose_field_cursor_right(),
            KeyCode::Home => self.compose_field_cursor_home(),
            KeyCode::End => self.compose_field_cursor_end(),
            KeyCode::Tab if editing_body => {
                for _ in 0..4 {
                    self.compose_field_insert_char(' ');
                }
            }
            KeyCode::Enter if editing_body => self.compose_field_insert_char('\n'),
            // Enter on Title submits (matches the natural "type title,
            // hit Enter" flow you'd expect on a single-line input).
            KeyCode::Enter => self.submit_compose(),
            KeyCode::Char(ch) if !ctrl => self.compose_field_insert_char(ch),
            _ => {}
        }
    }

    // ─── Compose field mutators ──────────────────────────────────

    /// Returns `(buf_ref, cursor_ref)` mutably for the focused text
    /// field, or `None` if the focus isn't a text field.
    fn compose_text_target(state: &mut ComposeState) -> Option<(&mut String, &mut usize)> {
        match state.field {
            ComposeField::Title => Some((&mut state.title, &mut state.title_cursor)),
            ComposeField::Body => Some((&mut state.body, &mut state.body_cursor)),
            _ => None,
        }
    }

    fn compose_field_insert_char(&mut self, ch: char) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if c.submitting {
            return;
        }
        if let Some((buf, cur)) = Self::compose_text_target(c) {
            buf.insert(*cur, ch);
            *cur += ch.len_utf8();
        }
    }

    fn compose_field_delete_left(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if c.submitting {
            return;
        }
        if let Some((buf, cur)) = Self::compose_text_target(c) {
            if *cur == 0 {
                return;
            }
            let mut new_cur = *cur - 1;
            while new_cur > 0 && !buf.is_char_boundary(new_cur) {
                new_cur -= 1;
            }
            buf.replace_range(new_cur..*cur, "");
            *cur = new_cur;
        }
    }

    fn compose_field_delete_right(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if c.submitting {
            return;
        }
        if let Some((buf, cur)) = Self::compose_text_target(c) {
            if *cur >= buf.len() {
                return;
            }
            let mut end = *cur + 1;
            while end < buf.len() && !buf.is_char_boundary(end) {
                end += 1;
            }
            buf.replace_range(*cur..end, "");
        }
    }

    fn compose_field_delete_word_left(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if c.submitting {
            return;
        }
        if let Some((buf, cur)) = Self::compose_text_target(c) {
            let new_cur = word_left_boundary(buf, *cur);
            buf.replace_range(new_cur..*cur, "");
            *cur = new_cur;
        }
    }

    fn compose_field_delete_word_right(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if c.submitting {
            return;
        }
        if let Some((buf, cur)) = Self::compose_text_target(c) {
            let end = word_right_boundary(buf, *cur);
            buf.replace_range(*cur..end, "");
        }
    }

    fn compose_field_cursor_left(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if let Some((buf, cur)) = Self::compose_text_target(c) {
            if *cur == 0 {
                return;
            }
            let mut new_cur = *cur - 1;
            while new_cur > 0 && !buf.is_char_boundary(new_cur) {
                new_cur -= 1;
            }
            *cur = new_cur;
        }
    }

    fn compose_field_cursor_right(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if let Some((buf, cur)) = Self::compose_text_target(c) {
            if *cur >= buf.len() {
                return;
            }
            let mut new_cur = *cur + 1;
            while new_cur < buf.len() && !buf.is_char_boundary(new_cur) {
                new_cur += 1;
            }
            *cur = new_cur;
        }
    }

    fn compose_field_cursor_home(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if let Some((buf, cur)) = Self::compose_text_target(c) {
            *cur = buf[..*cur].rfind('\n').map(|i| i + 1).unwrap_or(0);
        }
    }

    fn compose_field_cursor_end(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if let Some((buf, cur)) = Self::compose_text_target(c) {
            *cur = buf[*cur..]
                .find('\n')
                .map(|i| *cur + i)
                .unwrap_or(buf.len());
        }
    }

    fn compose_field_word_left(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if let Some((buf, cur)) = Self::compose_text_target(c) {
            *cur = word_left_boundary(buf, *cur);
        }
    }

    fn compose_field_word_right(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if let Some((buf, cur)) = Self::compose_text_target(c) {
            *cur = word_right_boundary(buf, *cur);
        }
    }

    /// `true` when an Up/Down key in the Body would move the cursor
    /// to another row instead of leaving the field. Lets the global
    /// Up/Down field-cycling kick in at first/last row only.
    fn compose_body_can_move_vertically(&self, key: KeyCode) -> bool {
        let Some(c) = self.compose.as_ref() else {
            return false;
        };
        if !matches!(c.field, ComposeField::Body) {
            return false;
        }
        let buf = &c.body;
        let cur = c.body_cursor.min(buf.len());
        match key {
            KeyCode::Up => buf[..cur].contains('\n'),
            KeyCode::Down => buf[cur..].contains('\n'),
            _ => false,
        }
    }

    fn compose_body_vertical(&mut self, key: KeyCode) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if !matches!(c.field, ComposeField::Body) {
            return;
        }
        let buf = c.body.clone();
        let cur = c.body_cursor.min(buf.len());
        let line_start = buf[..cur].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let col = cur - line_start;
        match key {
            KeyCode::Up => {
                if line_start == 0 {
                    c.body_cursor = 0;
                    return;
                }
                let prev_end = line_start - 1;
                let prev_start = buf[..prev_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let prev_len = prev_end - prev_start;
                c.body_cursor = prev_start + col.min(prev_len);
            }
            KeyCode::Down => {
                let line_end = buf[line_start..]
                    .find('\n')
                    .map(|i| line_start + i)
                    .unwrap_or(buf.len());
                if line_end >= buf.len() {
                    c.body_cursor = buf.len();
                    return;
                }
                let next_start = line_end + 1;
                let next_end = buf[next_start..]
                    .find('\n')
                    .map(|i| next_start + i)
                    .unwrap_or(buf.len());
                let next_len = next_end - next_start;
                c.body_cursor = next_start + col.min(next_len);
            }
            _ => {}
        }
    }

    fn compose_field_next(&mut self) {
        if let Some(c) = self.compose.as_mut() {
            c.field = c.field.next();
        }
    }
    fn compose_field_prev(&mut self) {
        if let Some(c) = self.compose.as_mut() {
            c.field = c.field.prev();
        }
    }

    fn compose_activate_field(&mut self) {
        let Some(c) = self.compose.as_ref() else {
            return;
        };
        match c.field {
            ComposeField::Labels => self.open_compose_labels_picker(),
            ComposeField::Assignees => self.open_compose_assignees_picker(),
            ComposeField::Milestone => self.open_compose_milestone_picker(),
            _ => {}
        }
    }

    // ─── Comment editor key dispatch ─────────────────────────────

    fn handle_event_comment_editor(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // Cursor-changing actions trigger viewport anchoring; pure
        // scrolling / Esc do not (lets manual scroll stand).
        let cursor_changed = match key.code {
            KeyCode::Esc => {
                self.comment_editor = None;
                false
            }
            KeyCode::Char('s') if ctrl => {
                self.submit_comment_editor();
                false
            }
            // Ctrl+Enter is the canonical "send" but most terminals
            // don't propagate the modifier with Enter — Ctrl+S is the
            // reliable shortcut. Both work where the terminal allows.
            KeyCode::Enter if ctrl => {
                self.submit_comment_editor();
                false
            }
            KeyCode::Enter => {
                self.editor_insert_char('\n');
                true
            }
            // Tab inserts 4 spaces — binding it to "send" is hostile
            // when the user is typing code / lists.
            KeyCode::Tab => {
                for _ in 0..4 {
                    self.editor_insert_char(' ');
                }
                true
            }
            // Ctrl+Backspace / Ctrl+H / Ctrl+W all delete a word left
            // (terminals + readline conventions overlap here).
            KeyCode::Backspace if ctrl => {
                self.editor_delete_word_left();
                true
            }
            KeyCode::Char('h') if ctrl => {
                self.editor_delete_word_left();
                true
            }
            KeyCode::Char('w') if ctrl => {
                self.editor_delete_word_left();
                true
            }
            KeyCode::Backspace => {
                self.editor_delete_left();
                true
            }
            KeyCode::Delete if ctrl => {
                self.editor_delete_word_right();
                true
            }
            KeyCode::Delete => {
                self.editor_delete_right();
                true
            }
            KeyCode::Left if ctrl => {
                self.editor_word_left();
                true
            }
            KeyCode::Right if ctrl => {
                self.editor_word_right();
                true
            }
            KeyCode::Left => {
                self.editor_cursor_left();
                true
            }
            KeyCode::Right => {
                self.editor_cursor_right();
                true
            }
            KeyCode::Up => {
                self.editor_cursor_up();
                true
            }
            KeyCode::Down => {
                self.editor_cursor_down();
                true
            }
            KeyCode::Home => {
                self.editor_cursor_home();
                true
            }
            KeyCode::End => {
                self.editor_cursor_end();
                true
            }
            KeyCode::PageUp => {
                let page = self
                    .comment_editor
                    .as_ref()
                    .map(|e| e.last_body_height.saturating_sub(1).max(1) as i32)
                    .unwrap_or(1);
                self.editor_scroll_viewport(-page);
                false
            }
            KeyCode::PageDown => {
                let page = self
                    .comment_editor
                    .as_ref()
                    .map(|e| e.last_body_height.saturating_sub(1).max(1) as i32)
                    .unwrap_or(1);
                self.editor_scroll_viewport(page);
                false
            }
            KeyCode::Char(ch) if !ctrl => {
                self.editor_insert_char(ch);
                true
            }
            _ => false,
        };
        if cursor_changed {
            self.editor_anchor_scroll_to_cursor();
        }
    }

    // ─── Editor mutators (shared between key handler + click) ────

    fn editor_insert_char(&mut self, ch: char) {
        if let Some(ed) = self.comment_editor.as_mut() {
            if ed.submitting {
                return;
            }
            ed.buffer.insert(ed.cursor, ch);
            ed.cursor += ch.len_utf8();
        }
    }

    fn editor_delete_left(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            if ed.submitting || ed.cursor == 0 {
                return;
            }
            let mut new_cur = ed.cursor - 1;
            while new_cur > 0 && !ed.buffer.is_char_boundary(new_cur) {
                new_cur -= 1;
            }
            ed.buffer.replace_range(new_cur..ed.cursor, "");
            ed.cursor = new_cur;
        }
    }

    fn editor_delete_right(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            if ed.submitting || ed.cursor >= ed.buffer.len() {
                return;
            }
            let mut end = ed.cursor + 1;
            while end < ed.buffer.len() && !ed.buffer.is_char_boundary(end) {
                end += 1;
            }
            ed.buffer.replace_range(ed.cursor..end, "");
        }
    }

    fn editor_delete_word_left(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            if ed.submitting {
                return;
            }
            let new_cursor = word_left_boundary(&ed.buffer, ed.cursor);
            ed.buffer.replace_range(new_cursor..ed.cursor, "");
            ed.cursor = new_cursor;
        }
    }

    fn editor_delete_word_right(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            if ed.submitting {
                return;
            }
            let end = word_right_boundary(&ed.buffer, ed.cursor);
            ed.buffer.replace_range(ed.cursor..end, "");
        }
    }

    fn editor_cursor_left(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            if ed.cursor == 0 {
                return;
            }
            let mut c = ed.cursor - 1;
            while c > 0 && !ed.buffer.is_char_boundary(c) {
                c -= 1;
            }
            ed.cursor = c;
        }
    }

    fn editor_cursor_right(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            if ed.cursor >= ed.buffer.len() {
                return;
            }
            let mut c = ed.cursor + 1;
            while c < ed.buffer.len() && !ed.buffer.is_char_boundary(c) {
                c += 1;
            }
            ed.cursor = c;
        }
    }

    fn editor_cursor_up(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            let buf = &ed.buffer;
            let cur_line_start = buf[..ed.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
            if cur_line_start == 0 {
                ed.cursor = 0;
                return;
            }
            let col = ed.cursor - cur_line_start;
            let prev_line_end = cur_line_start - 1;
            let prev_line_start = buf[..prev_line_end].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let prev_len = prev_line_end - prev_line_start;
            ed.cursor = prev_line_start + col.min(prev_len);
        }
    }

    fn editor_cursor_down(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            let buf = &ed.buffer;
            let cur_line_start = buf[..ed.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let col = ed.cursor - cur_line_start;
            let cur_line_end = buf[ed.cursor..]
                .find('\n')
                .map(|i| ed.cursor + i)
                .unwrap_or(buf.len());
            if cur_line_end >= buf.len() {
                ed.cursor = buf.len();
                return;
            }
            let next_line_start = cur_line_end + 1;
            let next_line_end = buf[next_line_start..]
                .find('\n')
                .map(|i| next_line_start + i)
                .unwrap_or(buf.len());
            let next_len = next_line_end - next_line_start;
            ed.cursor = next_line_start + col.min(next_len);
        }
    }

    fn editor_cursor_home(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            ed.cursor = ed.buffer[..ed.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
        }
    }

    fn editor_cursor_end(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            ed.cursor = ed.buffer[ed.cursor..]
                .find('\n')
                .map(|i| ed.cursor + i)
                .unwrap_or(ed.buffer.len());
        }
    }

    fn editor_word_left(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            ed.cursor = word_left_boundary(&ed.buffer, ed.cursor);
        }
    }

    fn editor_word_right(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            ed.cursor = word_right_boundary(&ed.buffer, ed.cursor);
        }
    }

    fn editor_anchor_scroll_to_cursor(&mut self) {
        let Some(ed) = self.comment_editor.as_mut() else {
            return;
        };
        let body = ed.last_body_height;
        if body == 0 {
            return;
        }
        let (_, cursor_row) = cursor_screen_pos(&ed.buffer, ed.cursor);
        if cursor_row < ed.scroll_offset {
            ed.scroll_offset = cursor_row;
        } else if cursor_row >= ed.scroll_offset + body {
            ed.scroll_offset = cursor_row + 1 - body;
        }
    }

    fn editor_scroll_viewport(&mut self, delta: i32) {
        let Some(ed) = self.comment_editor.as_mut() else {
            return;
        };
        let total_rows = if ed.buffer.is_empty() {
            1u16
        } else {
            ed.buffer.matches('\n').count() as u16 + 1
        };
        let body = ed.last_body_height.max(1);
        let max_offset = total_rows.saturating_sub(body);
        let new_offset = (ed.scroll_offset as i32 + delta).clamp(0, max_offset as i32);
        ed.scroll_offset = new_offset as u16;
    }

    fn editor_move_cursor_to_click(&mut self, col: u16, row: u16) {
        let Some(body) = self.editor_body_area else {
            return;
        };
        if !rect_contains(Some(body), col, row) {
            return;
        }
        let Some(ed) = self.comment_editor.as_mut() else {
            return;
        };
        if ed.submitting {
            return;
        }
        let row_in_body = row.saturating_sub(body.y) as usize;
        let col_in_body = col.saturating_sub(body.x) as usize;
        let target_row = ed.scroll_offset as usize + row_in_body;
        let mut current_row = 0usize;
        let mut line_start: usize = 0;
        for (byte_idx, ch) in ed.buffer.char_indices() {
            if current_row == target_row {
                break;
            }
            if ch == '\n' {
                current_row += 1;
                line_start = byte_idx + 1;
            }
        }
        if current_row < target_row {
            ed.cursor = ed.buffer.len();
            return;
        }
        let mut byte_pos = line_start;
        let mut col_walked = 0usize;
        for (offset, ch) in ed.buffer[line_start..].char_indices() {
            if ch == '\n' || col_walked >= col_in_body {
                byte_pos = line_start + offset;
                break;
            }
            col_walked += 1;
            byte_pos = line_start + offset + ch.len_utf8();
        }
        ed.cursor = byte_pos.min(ed.buffer.len());
    }

    // ─── Reaction picker ─────────────────────────────────────────

    fn handle_event_reaction_picker(&mut self, key: KeyEvent) {
        let Some(picker) = self.reaction_picker.as_mut() else {
            return;
        };
        let total = crate::github::pr::ReactionKind::all().len();
        match key.code {
            KeyCode::Esc => {
                self.reaction_picker = None;
            }
            KeyCode::Left | KeyCode::Up => {
                if picker.hovered > 0 {
                    picker.hovered -= 1;
                }
            }
            KeyCode::Right | KeyCode::Down => {
                if picker.hovered + 1 < total {
                    picker.hovered += 1;
                }
            }
            KeyCode::Home => picker.hovered = 0,
            KeyCode::End => picker.hovered = total - 1,
            KeyCode::Enter => {
                let chosen = crate::github::pr::ReactionKind::all()[picker.hovered];
                let target_idx = picker.target_idx;
                self.reaction_picker = None;
                self.submit_reaction(target_idx, chosen);
            }
            _ => {}
        }
    }

    fn start_react(&mut self) {
        let target = self.conversation_selected;
        self.reaction_picker = Some(ReactionPicker {
            target_idx: target,
            hovered: 0,
            overlay_rect: None,
            row_rects: Vec::new(),
        });
    }

    fn submit_reaction(
        &mut self,
        target_idx: usize,
        kind: crate::github::pr::ReactionKind,
    ) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();

        // Index 0 is the issue body itself; subsequent indices are
        // comments (in the order rendered in render_tab_conversation).
        if target_idx == 0 {
            std::thread::spawn(move || {
                let result =
                    crate::github::issue::add_issue_reaction(&token, &coords, number, kind);
                tx.send(AppEvent::IssueActionDone {
                    number,
                    action: format!("Reacted with {}", kind.emoji()),
                    result,
                });
            });
            return;
        }
        let detail = match self.detail_cache.get(&number) {
            Some(d) => d.clone(),
            None => return,
        };
        let Some(entry) = detail.conversation.get(target_idx - 1) else {
            return;
        };
        let Some(comment_id) = entry.id else {
            return;
        };
        std::thread::spawn(move || {
            let result = crate::github::pr::add_issue_comment_reaction(
                &token, &coords, comment_id, kind,
            );
            tx.send(AppEvent::IssueActionDone {
                number,
                action: format!("Reacted with {}", kind.emoji()),
                result,
            });
        });
    }

    // ─── Comment CRUD ────────────────────────────────────────────

    fn start_new_comment(&mut self) {
        if self.opened_issue_number.is_none() {
            return;
        }
        self.comment_editor = Some(CommentEditor {
            kind: CommentEditorKind::NewTopLevel,
            buffer: String::new(),
            cursor: 0,
            submitting: false,
            scroll_offset: 0,
            last_body_height: 0,
        });
    }

    fn start_edit_own_comment(&mut self) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        // Cannot edit the issue description card (index 0) via this
        // shortcut — issue body edit is rare; defer to future slice.
        if self.conversation_selected == 0 {
            return;
        }
        let Some(detail) = self.detail_cache.get(&number) else {
            return;
        };
        let Some(entry) = detail.conversation.get(self.conversation_selected - 1) else {
            return;
        };
        let is_me = self
            .me_login
            .as_deref()
            .map_or(false, |me| me == entry.author);
        let Some(comment_id) = entry.id else {
            return;
        };
        if !is_me {
            return;
        }
        self.comment_editor = Some(CommentEditor {
            kind: CommentEditorKind::EditTopLevel { comment_id },
            buffer: entry.body.clone(),
            cursor: entry.body.len(),
            submitting: false,
            scroll_offset: 0,
            last_body_height: 0,
        });
    }

    fn confirm_delete_own_comment(&mut self) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        if self.conversation_selected == 0 {
            return;
        }
        let Some(detail) = self.detail_cache.get(&number) else {
            return;
        };
        let Some(entry) = detail.conversation.get(self.conversation_selected - 1) else {
            return;
        };
        let is_me = self
            .me_login
            .as_deref()
            .map_or(false, |me| me == entry.author);
        let Some(comment_id) = entry.id else {
            return;
        };
        if !is_me {
            return;
        }
        // Fire directly — issues don't open a separate dialog (matches
        // PR behaviour where delete is a single keystroke).
        self.tx.send(AppEvent::DeleteIssueComment {
            issue_number: number,
            comment_id,
        });
    }

    fn submit_comment_editor(&mut self) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        let Some(ed) = self.comment_editor.as_mut() else {
            return;
        };
        if ed.submitting || ed.buffer.trim().is_empty() {
            return;
        }
        ed.submitting = true;
        let buffer = ed.buffer.clone();
        let kind = ed.kind.clone();
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let (label, result) = match kind {
                CommentEditorKind::NewTopLevel => (
                    "Comment posted".to_string(),
                    crate::github::issue::post_issue_comment(&token, &coords, number, &buffer),
                ),
                CommentEditorKind::EditTopLevel { comment_id } => (
                    "Comment edited".to_string(),
                    crate::github::issue::edit_issue_comment(&token, &coords, comment_id, &buffer),
                ),
            };
            tx.send(AppEvent::IssueActionDone {
                number,
                action: label,
                result,
            });
        });
    }

    // ─── Close / reopen ──────────────────────────────────────────

    fn start_close(&mut self) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        let title = self
            .detail_cache
            .get(&number)
            .map(|d| d.title.clone())
            .unwrap_or_default();
        self.tx.send(AppEvent::OpenDialog(
            crate::event::DialogKind::ConfirmCloseIssue {
                issue_number: number,
                issue_title: title,
            },
        ));
    }

    fn start_reopen(&mut self) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        self.tx.send(AppEvent::ReopenIssue {
            issue_number: number,
        });
    }

    // ─── Labels / Assignees / Milestone pickers ─────────────────

    fn start_labels_picker(&mut self) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        let detail = self.detail_cache.get(&number).cloned();
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let all_labels = match crate::github::issue::list_repo_labels(&token, &coords) {
                Ok(v) => v,
                Err(e) => {
                    tx.send(AppEvent::NotifyError(format!("Labels: {}", e)));
                    return;
                }
            };
            let (title, currently): (String, Vec<String>) = match detail {
                Some(d) => (
                    d.title,
                    d.labels.iter().map(|l| l.name.clone()).collect(),
                ),
                None => (String::new(), Vec::new()),
            };
            tx.send(AppEvent::OpenIssueLabelsPicker {
                issue_number: number,
                issue_title: title,
                all_labels,
                currently_on_issue: currently,
            });
        });
    }

    fn start_assignees_picker(&mut self) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        let detail = self.detail_cache.get(&number).cloned();
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let all_users = match crate::github::issue::list_repo_assignees(&token, &coords) {
                Ok(v) => v,
                Err(e) => {
                    tx.send(AppEvent::NotifyError(format!("Assignees: {}", e)));
                    return;
                }
            };
            let (title, currently): (String, Vec<String>) = match detail {
                Some(d) => (d.title, d.assignees),
                None => (String::new(), Vec::new()),
            };
            tx.send(AppEvent::OpenIssueAssigneesPicker {
                issue_number: number,
                issue_title: title,
                all_users,
                currently_assigned: currently,
            });
        });
    }

    fn start_milestone_picker(&mut self) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        let detail = self.detail_cache.get(&number).cloned();
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let all_milestones =
                match crate::github::issue::list_repo_milestones(&token, &coords) {
                    Ok(v) => v,
                    Err(e) => {
                        tx.send(AppEvent::NotifyError(format!("Milestones: {}", e)));
                        return;
                    }
                };
            let (title, currently): (String, Option<u64>) = match detail {
                Some(d) => {
                    let cur = d
                        .milestone
                        .as_ref()
                        .and_then(|m_title| {
                            all_milestones
                                .iter()
                                .find(|m| &m.title == m_title)
                                .map(|m| m.number)
                        });
                    (d.title, cur)
                }
                None => (String::new(), None),
            };
            tx.send(AppEvent::OpenIssueMilestonePicker {
                issue_number: number,
                issue_title: title,
                all_milestones,
                currently_set: currently,
            });
        });
    }

    // ─── Compose ─────────────────────────────────────────────────

    fn start_compose(&mut self) {
        let token = self.token.clone();
        let coords = self.coords.clone();
        // Pre-fetch labels/assignees/milestones synchronously — small
        // payloads, keeps the picker open instant. Surface failures
        // as warnings rather than blocking the compose flow.
        let labels = crate::github::issue::list_repo_labels(&token, &coords).unwrap_or_default();
        let assignees =
            crate::github::issue::list_repo_assignees(&token, &coords).unwrap_or_default();
        let milestones =
            crate::github::issue::list_repo_milestones(&token, &coords).unwrap_or_default();
        // Try to seed from the first available issue template (if any).
        let template_body =
            crate::github::issue::load_first_template(&coords).unwrap_or_default();
        self.compose = Some(ComposeState {
            title: String::new(),
            title_cursor: 0,
            body: template_body.clone(),
            body_cursor: template_body.len(),
            body_scroll: 0,
            body_last_height: 0,
            labels: Vec::new(),
            available_labels: labels,
            assignees: Vec::new(),
            available_assignees: assignees,
            milestone: None,
            available_milestones: milestones,
            field: ComposeField::Title,
            submitting: false,
        });
        self.mode = Mode::Compose;
    }

    fn submit_compose(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if c.title.trim().is_empty() || c.submitting {
            return;
        }
        c.submitting = true;
        let title = c.title.clone();
        let body = c.body.clone();
        let labels = c.labels.clone();
        let assignees = c.assignees.clone();
        let milestone = c.milestone;
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            match crate::github::issue::create_issue(
                &token, &coords, &title, &body, &labels, &assignees, milestone,
            ) {
                Ok(number) => tx.send(AppEvent::IssueCreated { number }),
                Err(e) => tx.send(AppEvent::NotifyError(format!("Create issue: {}", e))),
            }
        });
    }

    fn open_compose_labels_picker(&mut self) {
        let Some(c) = self.compose.as_ref() else {
            return;
        };
        let selected: Vec<bool> = c
            .available_labels
            .iter()
            .map(|l| c.labels.contains(&l.name))
            .collect();
        self.tx.send(AppEvent::OpenDialog(
            crate::event::DialogKind::IssueLabels {
                issue_number: 0,
                issue_title: "New issue".into(),
                all_labels: c.available_labels.clone(),
                selected,
                for_compose: true,
            },
        ));
    }

    fn open_compose_assignees_picker(&mut self) {
        let Some(c) = self.compose.as_ref() else {
            return;
        };
        let selected: Vec<bool> = c
            .available_assignees
            .iter()
            .map(|u| c.assignees.contains(u))
            .collect();
        let initial = selected.clone();
        self.tx.send(AppEvent::OpenDialog(
            crate::event::DialogKind::IssueAssignees {
                issue_number: 0,
                issue_title: "New issue".into(),
                all_users: c.available_assignees.clone(),
                selected,
                initial,
                for_compose: true,
            },
        ));
    }

    fn open_compose_milestone_picker(&mut self) {
        let Some(c) = self.compose.as_ref() else {
            return;
        };
        self.tx.send(AppEvent::OpenDialog(
            crate::event::DialogKind::IssueMilestone {
                issue_number: 0,
                issue_title: "New issue".into(),
                all_milestones: c.available_milestones.clone(),
                selected: c.milestone,
                for_compose: true,
            },
        ));
    }

    // ─── Click + mouse move ──────────────────────────────────────

    pub fn handle_click(&mut self, col: u16, row: u16) {
        // Comment editor takes priority — click inside it positions
        // the cursor; click outside dismisses nothing (so the user
        // can still scan the conversation without losing their draft).
        if self.comment_editor.is_some() {
            if rect_contains(self.editor_body_area, col, row) {
                self.editor_move_cursor_to_click(col, row);
                return;
            }
        }
        if self.reaction_picker.is_some() {
            let chosen = {
                let Some(picker) = self.reaction_picker.as_ref() else {
                    return;
                };
                picker
                    .row_rects
                    .iter()
                    .position(|r| rect_contains(Some(*r), col, row))
            };
            if let Some(i) = chosen {
                let kind = crate::github::pr::ReactionKind::all()[i];
                let target_idx = self.reaction_picker.as_ref().unwrap().target_idx;
                self.reaction_picker = None;
                self.submit_reaction(target_idx, kind);
            }
            return;
        }
        // Filter-tab clicks.
        for (filter, rect) in self.filter_tab_rects.clone() {
            if rect_contains(Some(rect), col, row) {
                self.list_filter = filter;
                self.hovered = 0;
                if matches!(self.mode, Mode::Detail) {
                    self.mode = Mode::List;
                    self.opened_issue_number = None;
                }
                return;
            }
        }
        // Detail tab-bar clicks.
        if matches!(self.mode, Mode::Detail) {
            for (tab, rect) in self.tab_bar_rects.clone() {
                if rect_contains(Some(rect), col, row) {
                    self.set_active_tab(tab);
                    return;
                }
            }
        }
        // Compose field clicks.
        if matches!(self.mode, Mode::Compose) {
            for (field, rect) in self.compose_field_rects.clone() {
                if rect_contains(Some(rect), col, row) {
                    if let Some(c) = self.compose.as_mut() {
                        c.field = field;
                    }
                    // Click on Labels / Assignees / Milestone also
                    // opens the picker — match what users expect.
                    match field {
                        ComposeField::Labels => self.open_compose_labels_picker(),
                        ComposeField::Assignees => self.open_compose_assignees_picker(),
                        ComposeField::Milestone => self.open_compose_milestone_picker(),
                        _ => {}
                    }
                    return;
                }
            }
        }
        if matches!(self.mode, Mode::List) {
            if let Some(area) = self.list_area {
                if rect_contains(Some(area), col, row) {
                    let local_row = row.saturating_sub(self.list_inner_y) as usize;
                    let idx = local_row + self.list_scroll_offset;
                    let filtered = self.filtered_indices();
                    if idx < filtered.len() {
                        self.hovered = idx;
                        self.open_hovered();
                    }
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        self.hovered_filter = None;
        for (filter, rect) in &self.filter_tab_rects {
            if rect_contains(Some(*rect), col, row) {
                self.hovered_filter = Some(*filter);
                break;
            }
        }
        self.hovered_tab = None;
        for (tab, rect) in &self.tab_bar_rects {
            if rect_contains(Some(*rect), col, row) {
                self.hovered_tab = Some(*tab);
                break;
            }
        }
        if matches!(self.mode, Mode::List) {
            if let Some(area) = self.list_area {
                if rect_contains(Some(area), col, row) {
                    let local_row = row.saturating_sub(self.list_inner_y) as usize;
                    let idx = local_row + self.list_scroll_offset;
                    let filtered_len = self.filtered_indices().len();
                    if idx < filtered_len {
                        self.hovered = idx;
                    }
                }
            }
        }
        // Compose: hovering a field row focuses it so the cursor +
        // visual highlight tracks the mouse. Matches PR's UX.
        if matches!(self.mode, Mode::Compose) {
            for (field, rect) in self.compose_field_rects.clone() {
                if rect_contains(Some(rect), col, row) {
                    if let Some(c) = self.compose.as_mut() {
                        if c.field != field {
                            c.field = field;
                        }
                    }
                    break;
                }
            }
        }
    }

    fn open_hovered(&mut self) {
        let filtered = self.filtered_indices();
        let Some(&item_idx) = filtered.get(self.hovered) else {
            return;
        };
        let number = self.items[item_idx].number;
        self.opened_issue_number = Some(number);
        self.mode = Mode::Detail;
        self.active_tab = Tab::Conversation;
        self.conversation_scroll = 0;
        self.conversation_selected = 0;
        self.conversation_scroll_to_selected = true;
        if !self.detail_cache.contains_key(&number) && self.loading_for != Some(number) {
            self.spawn_detail_fetch(number);
        }
    }

    fn spawn_detail_fetch(&mut self, number: u64) {
        self.loading_for = Some(number);
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = crate::github::issue::fetch_issue_detail(&token, &coords, number);
            tx.send(AppEvent::IssueDetailFetched { number, result });
        });
    }

    fn spawn_timeline_fetch(&mut self, number: u64) {
        self.loading_timeline_for = Some(number);
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = crate::github::issue::list_issue_timeline(&token, &coords, number);
            tx.send(AppEvent::IssueTimelineFetched { number, result });
        });
    }

    fn spawn_linked_fetch(&mut self, number: u64) {
        self.loading_linked_for = Some(number);
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = crate::github::issue::list_linked_prs(&token, &coords, number);
            tx.send(AppEvent::IssueLinkedFetched { number, result });
        });
    }

    // ─── Render ──────────────────────────────────────────────────

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.filter_tab_rects.clear();
        self.tab_bar_rects.clear();
        self.compose_field_rects.clear();
        self.comment_editor_cursor_pos = None;

        // Compose has its own full-screen layout, no header.
        if matches!(self.mode, Mode::Compose) {
            self.render_compose(f, area);
            self.place_terminal_cursor(f);
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(area);
        self.render_header(f, chunks[0]);
        if let Some(err) = self.last_error.clone() {
            let theme = &self.ctx.color_theme;
            let lines = vec![Line::from(Span::styled(
                err,
                Style::default()
                    .fg(theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            ))];
            f.render_widget(Paragraph::new(lines), chunks[1]);
            return;
        }
        match self.mode {
            Mode::List => self.render_list(f, chunks[1]),
            Mode::Detail => self.render_detail(f, chunks[1]),
            Mode::Compose => {}
        }
        if let Some(toast) = self.last_action_toast.clone() {
            let theme = &self.ctx.color_theme;
            // Render at bottom-right of the screen.
            let label = format!("  {}", toast);
            let lw = label.chars().count() as u16;
            if chunks[1].width > lw {
                let rect = Rect {
                    x: chunks[1].x + chunks[1].width - lw - 1,
                    y: chunks[1].y + chunks[1].height.saturating_sub(1),
                    width: lw,
                    height: 1,
                };
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        label,
                        Style::default().fg(theme.status_success_fg),
                    ))),
                    rect,
                );
            }
        }
        // Reaction overlay on top of the detail content.
        if self.reaction_picker.is_some() {
            self.render_reaction_picker_overlay(f, chunks[1]);
        }
        self.place_terminal_cursor(f);
    }

    /// Position the terminal cursor on the active text input surface
    /// (comment editor or compose Title/Body). Honours the user's
    /// `cursor_type` config — native cursor by default, virtual glyph
    /// painted into the buffer when so configured.
    fn place_terminal_cursor(&self, f: &mut Frame) {
        let Some((cx, cy)) = self.comment_editor_cursor_pos else {
            return;
        };
        match &self.ctx.ui_config.common.cursor_type {
            crate::config::CursorType::Native => {
                f.set_cursor_position((cx, cy));
            }
            crate::config::CursorType::Virtual(glyph) => {
                let style = Style::default().fg(self.ctx.color_theme.virtual_cursor_fg);
                f.buffer_mut().set_string(cx, cy, glyph, style);
            }
        }
    }

    fn render_header(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(
            " Issues ",
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw("  "));
        let mut col = area.x + spans.iter().map(|s| s.content.chars().count() as u16).sum::<u16>();
        for filter in [
            IssueListFilter::Open,
            IssueListFilter::Closed,
            IssueListFilter::All,
        ] {
            let count = self
                .items
                .iter()
                .filter(|i| filter.matches(i.state))
                .count();
            let label = format!(" {} {} ", filter.label(), count);
            let chip_w = label.chars().count() as u16;
            let active = filter == self.list_filter;
            let hovered = self.hovered_filter == Some(filter);
            let style = if active {
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else if hovered {
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.detail_label_fg)
            };
            spans.push(Span::styled(label.clone(), style));
            spans.push(Span::raw(" "));
            let rect = Rect {
                x: col,
                y: area.y,
                width: chip_w,
                height: 1,
            };
            self.filter_tab_rects.push((filter, rect));
            col = col.saturating_add(chip_w + 1);
        }
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "n:new",
            Style::default()
                .fg(theme.detail_label_fg)
                .add_modifier(Modifier::DIM),
        ));
        let line = Line::from(spans);
        f.render_widget(Paragraph::new(line), area);
    }

    fn render_list(&mut self, f: &mut Frame, area: Rect) {
        self.list_area = Some(area);
        self.list_inner_y = area.y;
        let theme = &self.ctx.color_theme;

        let filtered: Vec<&Issue> = self
            .filtered_indices()
            .into_iter()
            .map(|i| &self.items[i])
            .collect();

        if filtered.is_empty() {
            let line = Line::from(Span::styled(
                "  No issues match this filter.",
                Style::default().fg(theme.detail_label_fg),
            ));
            f.render_widget(Paragraph::new(line), area);
            return;
        }
        let visible_h = area.height as usize;
        if self.hovered < self.list_scroll_offset {
            self.list_scroll_offset = self.hovered;
        } else if visible_h > 0 && self.hovered >= self.list_scroll_offset + visible_h {
            self.list_scroll_offset = self.hovered + 1 - visible_h;
        }
        let max_num_width = filtered
            .iter()
            .map(|i| digits_count(i.number))
            .max()
            .unwrap_or(1);

        // Reserve column budgets so long titles get truncated with
        // ellipsis instead of wrapping or overflowing into chips.
        // Layout:  ▶_ ● _ #N __ TITLE __ [chips] __ 💬 N
        // fixed: caret(2) + icon(1) + sp(1) + #N(W+1) + sp(2) + sp(2) + comments(~6) = ~14 + W
        let avail = area.width as usize;
        let chips_cells: usize = filtered
            .iter()
            .map(|i| i.labels.iter().count() * 5) // 4-col chip + 1 space
            .max()
            .unwrap_or(0);
        let fixed = 2 + 1 + 1 + (max_num_width + 1) + 2 + 2 + 8;
        let title_budget = avail
            .saturating_sub(fixed + chips_cells)
            .max(20);

        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, issue) in filtered.iter().enumerate().skip(self.list_scroll_offset) {
            if lines.len() >= area.height as usize {
                break;
            }
            let is_hovered = i == self.hovered;
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.push(Span::styled(
                if is_hovered { "▶ " } else { "  " },
                Style::default().fg(theme.list_head_fg),
            ));
            spans.push(state_icon(theme, issue.state, issue.state_reason));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                format!("#{:width$}", issue.number, width = max_num_width),
                Style::default().fg(theme.detail_label_fg),
            ));
            spans.push(Span::raw("  "));
            let title_style = if is_hovered {
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            spans.push(Span::styled(
                fit_cell(&issue.title, title_budget),
                title_style,
            ));
            for label in &issue.labels {
                spans.push(Span::raw(" "));
                for s in short_label_chip_spans(label) {
                    spans.push(s);
                }
            }
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("💬 {}", issue.comments_count),
                Style::default().fg(theme.detail_label_fg),
            ));
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), area);
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        let cached = self.detail_cache.get(&number).cloned();
        let theme_label_fg = self.ctx.color_theme.detail_label_fg;

        // Layout mirrors the PR view exactly so the two pages share a
        // visual rhythm: sub-header ─ labels ─ divider ─ tabs ─ content.
        // Labels row collapses to 0 rows when the issue has none.
        let has_labels = cached
            .as_ref()
            .map(|d| !d.labels.is_empty())
            .unwrap_or(false);
        let labels_height: u16 = if has_labels { 1 } else { 0 };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(labels_height),
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(area);
        let sub_header_area = chunks[0];
        let labels_area = chunks[1];
        let divider_area = chunks[2];
        let tab_bar_area = chunks[3];
        let tab_content_area = chunks[4];
        self.tab_content_area = Some(tab_content_area);

        self.render_issue_sub_header(f, sub_header_area, cached.as_ref());
        if has_labels {
            self.render_issue_labels_row(f, labels_area, cached.as_ref());
        }
        // Divider — single `─` row spanning the width.
        {
            let theme = &self.ctx.color_theme;
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "─".repeat(divider_area.width as usize),
                    Style::default().fg(theme.divider_fg),
                ))),
                divider_area,
            );
        }
        self.render_tab_bar(f, tab_bar_area, number);

        let Some(detail) = cached.as_ref() else {
            let label = if self.loading_for == Some(number) {
                "  Loading issue…"
            } else {
                "  Issue not loaded."
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    label,
                    Style::default().fg(theme_label_fg),
                ))),
                tab_content_area,
            );
            return;
        };

        // Editor (when open) takes the bottom of the tab content.
        if self.comment_editor.is_some() {
            let editor_h = tab_content_area.height.min(10).max(5);
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(editor_h)])
                .split(tab_content_area);
            match self.active_tab {
                Tab::Conversation => self.render_tab_conversation(f, split[0], detail),
                Tab::Timeline => self.render_tab_timeline(f, split[0], number),
                Tab::Linked => self.render_tab_linked(f, split[0], number),
            }
            self.render_comment_editor(f, split[1]);
        } else {
            match self.active_tab {
                Tab::Conversation => self.render_tab_conversation(f, tab_content_area, detail),
                Tab::Timeline => self.render_tab_timeline(f, tab_content_area, number),
                Tab::Linked => self.render_tab_linked(f, tab_content_area, number),
            }
        }
    }

    fn render_issue_sub_header(
        &self,
        f: &mut Frame,
        area: Rect,
        detail: Option<&IssueDetail>,
    ) {
        let theme = &self.ctx.color_theme;
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::raw(" "));
        if let Some(d) = detail {
            spans.push(state_chip(theme, d.state, d.state_reason));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("#{}", d.number),
                Style::default()
                    .fg(theme.list_hash_fg)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw("  "));
            // Compute the budget left for the title once the trailing
            // metadata is reserved — keeps long titles from squashing
            // " opened by … · assigned: … · milestone: …" off-screen.
            let mut meta_text = format!("opened by {} · {}", d.author, d.opened_when);
            if !d.assignees.is_empty() {
                meta_text.push_str(&format!("  ·  assigned: {}", d.assignees.join(", ")));
            }
            if let Some(ms) = &d.milestone {
                meta_text.push_str(&format!("  ·  milestone: {}", ms));
            }
            let fixed = spans
                .iter()
                .map(|s| s.content.chars().count())
                .sum::<usize>();
            let meta_cells = meta_text.chars().count();
            let avail = area.width as usize;
            let title_budget = avail
                .saturating_sub(fixed + meta_cells + 4)
                .max(10);
            spans.push(Span::styled(
                fit_cell(&d.title, title_budget),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw("    "));
            spans.push(Span::styled(
                meta_text,
                Style::default().fg(theme.detail_label_fg),
            ));
        } else {
            spans.push(Span::styled(
                format!("#{}", self.opened_issue_number.unwrap_or(0)),
                Style::default().fg(theme.detail_label_fg),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_issue_labels_row(
        &self,
        f: &mut Frame,
        area: Rect,
        detail: Option<&IssueDetail>,
    ) {
        let Some(d) = detail else {
            return;
        };
        let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
        for (i, label) in d.labels.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            for s in label_chip_spans_local(label) {
                spans.push(s);
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_tab_bar(&mut self, f: &mut Frame, area: Rect, number: u64) {
        let theme = &self.ctx.color_theme;
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut col = area.x;
        for tab in Tab::all() {
            let count_text = match tab {
                Tab::Conversation => self
                    .detail_cache
                    .get(&number)
                    .map(|d| d.conversation.len() + 1)
                    .map(|n| format!(" {}", n))
                    .unwrap_or_default(),
                Tab::Timeline => self
                    .timeline_cache
                    .get(&number)
                    .map(|v| format!(" {}", v.len()))
                    .unwrap_or_default(),
                Tab::Linked => self
                    .linked_cache
                    .get(&number)
                    .map(|v| format!(" {}", v.len()))
                    .unwrap_or_default(),
            };
            let label = format!(" {}{} ", tab.label(), count_text);
            let chip_w = label.chars().count() as u16;
            let active = tab == self.active_tab;
            let hovered = self.hovered_tab == Some(tab);
            let style = if active {
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else if hovered {
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.detail_label_fg)
            };
            spans.push(Span::styled(label.clone(), style));
            spans.push(Span::raw(" "));
            let rect = Rect {
                x: col,
                y: area.y,
                width: chip_w,
                height: 1,
            };
            self.tab_bar_rects.push((tab, rect));
            col = col.saturating_add(chip_w + 1);
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_tab_conversation(&mut self, f: &mut Frame, area: Rect, detail: &IssueDetail) {
        let theme = &self.ctx.color_theme;
        let mut lines: Vec<Line<'static>> = Vec::new();
        self.conversation_comment_spans.clear();
        let me_login = self.me_login.clone();

        let is_me_top = me_login
            .as_deref()
            .map_or(false, |me| me == detail.author);
        let idx0_first = lines.len();
        let mut top_shortcuts: Vec<&'static str> = Vec::new();
        if self.conversation_selected == 0 {
            top_shortcuts.push("+:react");
        }
        push_comment_card(
            &mut lines,
            theme,
            area.width,
            CommentCardInput {
                author: &detail.author,
                action: CommentAction::OpenedIssue,
                when: &detail.opened_when,
                body: &detail.body,
                ancestor_gutters: &[],
                has_children: false,
                is_selected: self.conversation_selected == 0,
                is_me: is_me_top,
                inline_shortcuts: top_shortcuts,
                reactions: detail.reactions.clone(),
            },
        );
        self.conversation_comment_spans
            .push((0, idx0_first, lines.len().saturating_sub(1)));

        if detail.conversation.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "  No comments yet — press 'c' to add one.",
                Style::default().fg(theme.detail_label_fg),
            )));
            self.conversation_comment_count = 1;
        } else {
            lines.push(Line::from(""));
            let mut idx = 1usize;
            for (i, c) in detail.conversation.iter().enumerate() {
                if i > 0 {
                    lines.push(Line::from(""));
                }
                let first = lines.len();
                let is_me = me_login
                    .as_deref()
                    .map_or(false, |me| me == c.author);
                let selected = self.conversation_selected == idx;
                let mut shortcuts: Vec<&'static str> = Vec::new();
                if selected {
                    shortcuts.push("+:react");
                    if is_me {
                        shortcuts.push("e:edit");
                        shortcuts.push("d:delete");
                    }
                }
                push_comment_card(
                    &mut lines,
                    theme,
                    area.width,
                    CommentCardInput {
                        author: &c.author,
                        action: CommentAction::Commented,
                        when: &c.when,
                        body: &c.body,
                        ancestor_gutters: &[],
                        has_children: false,
                        is_selected: selected,
                        is_me,
                        inline_shortcuts: shortcuts,
                        reactions: c.reactions.clone(),
                    },
                );
                self.conversation_comment_spans
                    .push((idx, first, lines.len().saturating_sub(1)));
                idx += 1;
            }
            self.conversation_comment_count = idx;
        }
        if self.conversation_selected >= self.conversation_comment_count {
            self.conversation_selected = self.conversation_comment_count.saturating_sub(1);
        }
        if self.conversation_scroll_to_selected {
            self.ensure_selected_visible(area.height as usize);
            self.conversation_scroll_to_selected = false;
        }
        let scroll = self.conversation_scroll.min(lines.len().saturating_sub(1));
        let para = Paragraph::new(lines).scroll((scroll as u16, 0));
        f.render_widget(para, area);
    }

    fn render_tab_timeline(&mut self, f: &mut Frame, area: Rect, number: u64) {
        let theme = &self.ctx.color_theme;
        let Some(events) = self.timeline_cache.get(&number).cloned() else {
            let label = if self.loading_timeline_for == Some(number) {
                "  Loading timeline…"
            } else {
                "  Timeline not loaded yet."
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    label,
                    Style::default().fg(theme.detail_label_fg),
                ))),
                area,
            );
            return;
        };
        if events.is_empty() {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "  No timeline events yet.",
                    Style::default().fg(theme.detail_label_fg),
                ))),
                area,
            );
            return;
        }
        let mut lines: Vec<Line<'static>> = Vec::new();
        for ev in events {
            lines.push(timeline_event_line(theme, &ev));
        }
        f.render_widget(Paragraph::new(lines), area);
    }

    fn render_tab_linked(&mut self, f: &mut Frame, area: Rect, number: u64) {
        let theme = &self.ctx.color_theme;
        let Some(prs) = self.linked_cache.get(&number).cloned() else {
            let label = if self.loading_linked_for == Some(number) {
                "  Loading linked PRs…"
            } else {
                "  Linked PRs not loaded yet."
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    label,
                    Style::default().fg(theme.detail_label_fg),
                ))),
                area,
            );
            return;
        };
        if prs.is_empty() {
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "  No linked PRs.",
                    Style::default().fg(theme.detail_label_fg),
                ))),
                area,
            );
            return;
        }
        let max_num_w = prs.iter().map(|p| digits_count(p.number)).max().unwrap_or(1);
        let mut lines: Vec<Line<'static>> = Vec::new();
        for pr in prs {
            let state_color = match pr.state.as_str() {
                "open" => theme.status_success_fg,
                _ => theme.detail_label_fg,
            };
            // Reserve room: state(2) + #N(W+1) + sp(2) + sp(2) + by(~12)
            let fixed = 2 + (max_num_w + 1) + 2 + 2 + 3 + pr.author.chars().count();
            let title_budget = (area.width as usize).saturating_sub(fixed).max(10);
            lines.push(Line::from(vec![
                Span::styled("● ", Style::default().fg(state_color)),
                Span::styled(
                    format!("#{:width$}", pr.number, width = max_num_w),
                    Style::default().fg(theme.detail_label_fg),
                ),
                Span::raw("  "),
                Span::styled(
                    fit_cell(&pr.title, title_budget),
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("by {}", pr.author),
                    Style::default().fg(theme.detail_label_fg),
                ),
            ]));
        }
        f.render_widget(Paragraph::new(lines), area);
    }

    fn render_comment_editor(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let (kind, buffer, cursor_byte, submitting) = {
            let Some(ed) = self.comment_editor.as_ref() else {
                return;
            };
            (ed.kind.clone(), ed.buffer.clone(), ed.cursor, ed.submitting)
        };
        let title = kind.header();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.list_head_fg))
            .title(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    title.to_string(),
                    Style::default()
                        .fg(theme.list_head_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ]));
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.height == 0 || inner.width == 0 {
            return;
        }
        let footer_h: u16 = if submitting { 1 } else { 0 };
        let body_h = inner.height.saturating_sub(footer_h).max(1);
        let body_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: body_h,
        };
        self.editor_body_area = Some(body_area);

        let (cursor_col, cursor_row) = cursor_screen_pos(&buffer, cursor_byte);
        let total_rows = if buffer.is_empty() {
            1
        } else {
            buffer.matches('\n').count() as u16 + 1
        };
        let mut scroll = self
            .comment_editor
            .as_ref()
            .map(|e| e.scroll_offset)
            .unwrap_or(0);
        let max_scroll = total_rows.saturating_sub(body_h);
        if scroll > max_scroll {
            scroll = max_scroll;
        }
        if cursor_row < scroll {
            scroll = cursor_row;
        } else if cursor_row >= scroll + body_h {
            scroll = cursor_row + 1 - body_h;
        }
        if let Some(ed) = self.comment_editor.as_mut() {
            ed.scroll_offset = scroll;
            ed.last_body_height = body_h;
        }
        let value = Style::default().fg(theme.fg);
        let body_text: Vec<Line<'static>> = if buffer.is_empty() {
            vec![Line::from(Span::styled(
                "Type your comment…".to_string(),
                Style::default().fg(theme.detail_label_fg),
            ))]
        } else {
            buffer
                .split('\n')
                .skip(scroll as usize)
                .take(body_h as usize)
                .map(|l| Line::from(Span::styled(l.to_string(), value)))
                .collect()
        };
        f.render_widget(Paragraph::new(body_text), body_area);

        if submitting {
            let footer_area = Rect {
                x: inner.x,
                y: inner.y + body_h,
                width: inner.width,
                height: footer_h,
            };
            f.render_widget(
                Paragraph::new(Span::styled(
                    "  Sending…".to_string(),
                    Style::default().fg(theme.status_warn_fg),
                )),
                footer_area,
            );
        }
        let cy = body_area.y + cursor_row.saturating_sub(scroll);
        let cx = body_area.x + cursor_col;
        if !submitting && cx < body_area.x + body_area.width && cy < body_area.y + body_area.height
        {
            self.comment_editor_cursor_pos = Some((cx, cy));
        }
    }

    fn render_reaction_picker_overlay(&mut self, f: &mut Frame, area: Rect) {
        let Some(picker) = self.reaction_picker.as_mut() else {
            return;
        };
        let theme = &self.ctx.color_theme;
        let kinds = crate::github::pr::ReactionKind::all();
        let cell_w: u16 = 6;
        let width: u16 = (kinds.len() as u16 * cell_w).saturating_add(2);
        let height: u16 = 3;
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let rect = Rect::new(x, y, width, height);
        picker.overlay_rect = Some(rect);
        f.render_widget(ratatui::widgets::Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.list_head_fg))
            .title(Line::from(Span::styled(
                " React ".to_string(),
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        picker.row_rects.clear();
        let mut x_cursor = inner.x;
        for (i, kind) in kinds.iter().enumerate() {
            let is_selected = i == picker.hovered;
            let cell_rect = Rect::new(x_cursor, inner.y, cell_w, 1);
            picker.row_rects.push(cell_rect);
            let style = if is_selected {
                Style::default()
                    .fg(theme.fg)
                    .bg(theme.list_selected_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" {} ", kind.emoji()),
                    style,
                ))),
                cell_rect,
            );
            x_cursor = x_cursor.saturating_add(cell_w);
        }
    }

    fn render_compose(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let Some(c) = self.compose.as_ref() else {
            return;
        };
        // 3 sections: header, fields, body (rest)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Length(7), Constraint::Min(3)])
            .split(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    " New Issue ",
                    Style::default()
                        .fg(theme.list_head_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    "Ctrl+S to submit · Esc to cancel · ↑↓ to switch fields",
                    Style::default().fg(theme.detail_label_fg),
                ),
            ])),
            chunks[0],
        );

        // Title + Labels + Assignees + Milestone in a compact grid.
        let field_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Length(1),
            ])
            .split(chunks[1]);

        // Title row.
        let title_focused = matches!(c.field, ComposeField::Title);
        let title_rect = field_chunks[0];
        self.compose_field_rects.push((ComposeField::Title, title_rect));
        let title_label_style = if title_focused {
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.detail_label_fg)
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("Title", title_label_style)),
                Line::from(Span::styled(
                    if c.title.is_empty() && !title_focused {
                        "(required)".to_string()
                    } else {
                        c.title.clone()
                    },
                    if title_focused {
                        Style::default()
                            .fg(theme.fg)
                            .add_modifier(Modifier::UNDERLINED)
                    } else {
                        Style::default().fg(theme.fg)
                    },
                )),
            ]),
            title_rect,
        );
        if title_focused {
            let cx = title_rect.x
                + (c.title[..c.title_cursor.min(c.title.len())].chars().count() as u16);
            let cy = title_rect.y + 1;
            if cx < title_rect.x + title_rect.width && cy < title_rect.y + title_rect.height {
                self.comment_editor_cursor_pos = Some((cx, cy));
            }
        }

        // Labels row.
        let labels_focused = matches!(c.field, ComposeField::Labels);
        let labels_rect = field_chunks[1];
        self.compose_field_rects.push((ComposeField::Labels, labels_rect));
        let labels_label_style = if labels_focused {
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.detail_label_fg)
        };
        let mut labels_value_spans: Vec<Span<'static>> = Vec::new();
        if c.labels.is_empty() {
            labels_value_spans.push(Span::styled(
                "Enter to pick…",
                Style::default()
                    .fg(theme.detail_label_fg)
                    .add_modifier(Modifier::DIM),
            ));
        } else {
            for name in &c.labels {
                let label = c
                    .available_labels
                    .iter()
                    .find(|l| &l.name == name)
                    .cloned()
                    .unwrap_or_else(|| crate::github::pr::Label {
                        name: name.clone(),
                        color: None,
                        description: None,
                    });
                for s in label_chip_spans_local(&label) {
                    labels_value_spans.push(s);
                }
                labels_value_spans.push(Span::raw(" "));
            }
        }
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("Labels", labels_label_style)),
                Line::from(labels_value_spans),
            ]),
            labels_rect,
        );

        // Assignees row.
        let assignees_focused = matches!(c.field, ComposeField::Assignees);
        let assignees_rect = field_chunks[2];
        self.compose_field_rects
            .push((ComposeField::Assignees, assignees_rect));
        let assignees_label_style = if assignees_focused {
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.detail_label_fg)
        };
        let assignees_value = if c.assignees.is_empty() {
            "Enter to pick…".to_string()
        } else {
            c.assignees.join(", ")
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("Assignees", assignees_label_style)),
                Line::from(Span::styled(
                    assignees_value,
                    Style::default().fg(theme.fg),
                )),
            ]),
            assignees_rect,
        );

        // Milestone row (single line — label + value inline).
        let milestone_focused = matches!(c.field, ComposeField::Milestone);
        let milestone_rect = field_chunks[3];
        self.compose_field_rects
            .push((ComposeField::Milestone, milestone_rect));
        let milestone_label_style = if milestone_focused {
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.detail_label_fg)
        };
        let milestone_text = c
            .milestone
            .and_then(|n| {
                c.available_milestones
                    .iter()
                    .find(|m| m.number == n)
                    .map(|m| m.title.clone())
            })
            .unwrap_or_else(|| "None".to_string());
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("Milestone: ", milestone_label_style),
                Span::styled(milestone_text, Style::default().fg(theme.fg)),
            ])),
            milestone_rect,
        );

        // Body — the rest of the area.
        let body_focused = matches!(c.field, ComposeField::Body);
        let body_rect = chunks[2];
        self.compose_field_rects.push((ComposeField::Body, body_rect));
        let body_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if body_focused {
                theme.list_head_fg
            } else {
                theme.detail_label_fg
            }))
            .title(Line::from(Span::styled(
                " Body ",
                Style::default()
                    .fg(if body_focused {
                        theme.list_head_fg
                    } else {
                        theme.detail_label_fg
                    })
                    .add_modifier(Modifier::BOLD),
            )));
        let body_inner = body_block.inner(body_rect);
        f.render_widget(body_block, body_rect);
        if body_inner.height == 0 || body_inner.width == 0 {
            return;
        }
        let (cursor_col, cursor_row) = cursor_screen_pos(&c.body, c.body_cursor);
        let total_rows = if c.body.is_empty() {
            1
        } else {
            c.body.matches('\n').count() as u16 + 1
        };
        let mut scroll = c.body_scroll;
        let max_scroll = total_rows.saturating_sub(body_inner.height);
        if scroll > max_scroll {
            scroll = max_scroll;
        }
        if body_focused {
            if cursor_row < scroll {
                scroll = cursor_row;
            } else if cursor_row >= scroll + body_inner.height {
                scroll = cursor_row + 1 - body_inner.height;
            }
        }
        let body_str = c.body.clone();
        // NLL ends the immutable borrow of `c` after this last use,
        // so the mutable re-borrow below is fine.
        if let Some(cc) = self.compose.as_mut() {
            cc.body_scroll = scroll;
            cc.body_last_height = body_inner.height;
        }
        let body_text: Vec<Line<'static>> = if body_str.is_empty() {
            vec![Line::from(Span::styled(
                "Type the issue body…".to_string(),
                Style::default().fg(theme.detail_label_fg),
            ))]
        } else {
            body_str
                .split('\n')
                .skip(scroll as usize)
                .take(body_inner.height as usize)
                .map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(theme.fg))))
                .collect()
        };
        f.render_widget(Paragraph::new(body_text), body_inner);
        if body_focused {
            let cy = body_inner.y + cursor_row.saturating_sub(scroll);
            let cx = body_inner.x + cursor_col;
            if cx < body_inner.x + body_inner.width && cy < body_inner.y + body_inner.height {
                self.comment_editor_cursor_pos = Some((cx, cy));
            }
        }
    }

    // ─── Helpers ─────────────────────────────────────────────────

    fn ensure_selected_visible(&mut self, viewport_h: usize) {
        let Some(&(_, first, last)) = self
            .conversation_comment_spans
            .iter()
            .find(|(idx, _, _)| *idx == self.conversation_selected)
        else {
            return;
        };
        if first < self.conversation_scroll {
            self.conversation_scroll = first;
        } else if last >= self.conversation_scroll + viewport_h {
            self.conversation_scroll = last + 1 - viewport_h;
        }
    }

    fn filtered_indices(&self) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, i)| self.list_filter.matches(i.state))
            .map(|(i, _)| i)
            .collect()
    }
}

fn digits_count(n: u64) -> usize {
    if n == 0 {
        1
    } else {
        (n as f64).log10().floor() as usize + 1
    }
}

fn rect_contains(rect: Option<Rect>, col: u16, row: u16) -> bool {
    let Some(r) = rect else {
        return false;
    };
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

fn state_icon(
    theme: &crate::color::ColorTheme,
    state: IssueState,
    reason: Option<IssueStateReason>,
) -> Span<'static> {
    let (icon, fg) = match (state, reason) {
        (IssueState::Open, _) => ("●", theme.status_success_fg),
        (IssueState::Closed, Some(IssueStateReason::NotPlanned)) => ("◌", theme.detail_label_fg),
        (IssueState::Closed, _) => ("●", MERGED_PURPLE),
    };
    Span::styled(icon.to_string(), Style::default().fg(fg))
}

fn state_chip(
    theme: &crate::color::ColorTheme,
    state: IssueState,
    reason: Option<IssueStateReason>,
) -> Span<'static> {
    let (label, fg) = match (state, reason) {
        (IssueState::Open, _) => (" Open ", theme.status_success_fg),
        (IssueState::Closed, Some(IssueStateReason::NotPlanned)) => {
            (" Closed (not planned) ", theme.detail_label_fg)
        }
        (IssueState::Closed, _) => (" Closed ", MERGED_PURPLE),
    };
    Span::styled(
        label.to_string(),
        Style::default().fg(fg).add_modifier(Modifier::BOLD),
    )
}

fn timeline_event_line(
    theme: &crate::color::ColorTheme,
    ev: &TimelineEvent,
) -> Line<'static> {
    let actor = ev.actor.clone().unwrap_or_else(|| "?".into());
    let mut spans: Vec<Span<'static>> = Vec::new();
    let (icon, fg) = match &ev.kind {
        TimelineKind::Labeled { .. } | TimelineKind::Unlabeled { .. } => {
            ("🏷", theme.list_head_fg)
        }
        TimelineKind::Assigned { .. } | TimelineKind::Unassigned { .. } => {
            ("👤", theme.list_name_fg)
        }
        TimelineKind::Milestoned { .. } | TimelineKind::Demilestoned { .. } => {
            ("◎", theme.list_head_fg)
        }
        TimelineKind::Closed { .. } => ("●", MERGED_PURPLE),
        TimelineKind::Reopened => ("●", theme.status_success_fg),
        TimelineKind::Renamed { .. } => ("✎", theme.detail_label_fg),
        TimelineKind::CrossReferenced { .. } => ("↪", theme.list_head_fg),
        TimelineKind::Mentioned => ("@", theme.detail_label_fg),
        TimelineKind::Subscribed => ("☆", theme.detail_label_fg),
        TimelineKind::Pinned | TimelineKind::Unpinned => ("⚲", theme.detail_label_fg),
        TimelineKind::Other { .. } => ("·", theme.detail_label_fg),
    };
    spans.push(Span::styled(
        format!(" {} ", icon),
        Style::default().fg(fg),
    ));
    spans.push(Span::styled(
        actor,
        Style::default()
            .fg(theme.list_name_fg)
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw(" "));
    let action_text: String = match &ev.kind {
        TimelineKind::Labeled { name, color } => {
            // Render the label name as a chip via parse_hex_color.
            let chip_fg = color
                .as_deref()
                .and_then(parse_hex_color)
                .map(|(r, g, b)| {
                    let lum = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
                    if lum > 140.0 {
                        Color::Rgb(0, 0, 0)
                    } else {
                        Color::Rgb(255, 255, 255)
                    }
                })
                .unwrap_or(theme.fg);
            let bg = color
                .as_deref()
                .and_then(parse_hex_color)
                .map(|(r, g, b)| Color::Rgb(r, g, b))
                .unwrap_or(theme.bg);
            spans.push(Span::styled(
                "added label ".to_string(),
                Style::default().fg(theme.detail_label_fg),
            ));
            spans.push(Span::styled(
                format!(" {} ", name),
                Style::default()
                    .fg(chip_fg)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ));
            return Line::from({
                let mut s = spans;
                s.push(Span::raw("  "));
                s.push(Span::styled(
                    ev.when.clone(),
                    Style::default().fg(theme.list_date_fg),
                ));
                s
            });
        }
        TimelineKind::Unlabeled { name, .. } => format!("removed label {}", name),
        TimelineKind::Assigned { who } => format!("assigned {}", who),
        TimelineKind::Unassigned { who } => format!("unassigned {}", who),
        TimelineKind::Milestoned { title } => format!("added to milestone {}", title),
        TimelineKind::Demilestoned { title } => format!("removed from milestone {}", title),
        TimelineKind::Closed { reason } => match reason {
            Some(IssueStateReason::NotPlanned) => "closed as not planned".to_string(),
            _ => "closed this issue".to_string(),
        },
        TimelineKind::Reopened => "reopened this issue".to_string(),
        TimelineKind::Renamed { from, to } => format!("renamed '{}' → '{}'", from, to),
        TimelineKind::CrossReferenced {
            issue_number, title, ..
        } => match (issue_number, title) {
            (Some(n), Some(t)) => format!("cross-referenced #{} ({})", n, t),
            (Some(n), None) => format!("cross-referenced #{}", n),
            _ => "cross-referenced".to_string(),
        },
        TimelineKind::Mentioned => "was mentioned".to_string(),
        TimelineKind::Subscribed => "subscribed".to_string(),
        TimelineKind::Pinned => "pinned this issue".to_string(),
        TimelineKind::Unpinned => "unpinned this issue".to_string(),
        TimelineKind::Other { kind } => format!("event: {}", kind),
    };
    spans.push(Span::styled(
        action_text,
        Style::default().fg(theme.detail_label_fg),
    ));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        ev.when.clone(),
        Style::default().fg(theme.list_date_fg),
    ));
    Line::from(spans)
}

// Reserved imports for future polish — keeps the slice lean while
// signalling what's next.
#[allow(dead_code)]
fn _i_future() {
    let _ = render_markdown_body;
    let _ = wrap_styled_spans;
    let _ = build_body_prefix;
    let _ = build_junction_prefix;
    let _ = TREE_LEVEL_WIDTH;
}
