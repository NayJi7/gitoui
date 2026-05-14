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
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use rustc_hash::FxHashMap;

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    github::issue::{
        Issue, IssueDetail, IssueState, IssueStateReason, LinkedPr, TimelineEvent, TimelineKind,
    },
    github::pr::PullRequest,
    github::RepoCoords,
    view::pr::{
        build_body_prefix, build_junction_prefix, cursor_screen_pos, fit_cell,
        label_chip_spans_local, paint_login_avatar, parse_hex_color, push_comment_card,
        render_markdown_body, short_label_chip_spans, word_left_boundary, word_right_boundary,
        wrap_styled_spans, AvatarSlot, CommentAction, CommentCardInput, MERGED_PURPLE,
        TREE_LEVEL_WIDTH,
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
    /// Pending avatars to paint after the Conversation tab Paragraph
    /// renders. Drained at the end of the frame.
    conversation_avatar_slots: Vec<AvatarSlot>,
    /// Full set of avatars painted on the screen last frame. ONE
    /// vec for the whole view so cross-section transitions (list →
    /// detail, tab switches, scroll, …) automatically clear the
    /// avatars from the previous section.
    prev_painted_avatars: Vec<crate::view::pr::PaintedAvatar>,
    /// Accumulator filled by each render path during a frame.
    /// Drained and diffed once at the end of `render()`.
    pending_avatar_paints: Vec<(crate::view::pr::PaintedAvatar, Color)>,
    /// Clickable `#N` reference hit-boxes inside the Conversation
    /// tab. Captured during the post-render restyle pass; consumed
    /// by `handle_click` to navigate to the referenced item.
    conversation_ref_links: Vec<RefLink>,
    /// References tab state — `hovered` is the selected row,
    /// `count` is the total rows (used to clamp nav), `row_rects`
    /// hold the per-row hit-test rects captured at render time.
    references_hovered: usize,
    references_count: usize,
    references_row_rects: Vec<Rect>,
    /// Same idea for the issue list rows.
    list_avatar_slots: Vec<AvatarSlot>,
    /// Reaction picker overlay; takes key+mouse focus when present.
    reaction_picker: Option<ReactionPicker>,
    /// Floating `#` autocomplete popup. Active while the user is
    /// typing a `#NNN` mention inside the comment editor — proposes
    /// issues + PRs matching the prefix, picked into the buffer on
    /// Enter. Drives its own keypress dispatch while `Some`.
    mention_popup: Option<MentionPopup>,
    /// PR list cached for `#` autocomplete. Fetched lazily on the
    /// first time the popup needs to open — issues we already have
    /// in `self.items`. Empty until the background fetch lands.
    mention_pr_cache: Vec<PullRequest>,
    /// `true` while the background PR fetch for `#` autocomplete is
    /// in flight, so we don't fire a second one.
    mention_pr_loading: bool,
    /// In-session log of the viewer's reactions on each conversation
    /// entry — keyed by `(issue_number, target_idx)`. Stores both
    /// the kind (so the picker can highlight it red) and the
    /// returned reaction id (so a re-click can DELETE it).
    viewer_reactions: FxHashMap<(u64, usize), Vec<(crate::github::pr::ReactionKind, u64)>>,
    /// Compose-new-issue draft. `Some` only when `mode == Compose`.
    compose: Option<ComposeState>,
    compose_field_rects: Vec<(ComposeField, Rect)>,
    last_error: Option<String>,
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
    /// Bidirectional `#N` references — forward refs (this issue
    /// mentions #N in its body or comments) AND backward refs
    /// (other items mention this issue via cross-reference events).
    /// Renamed from "Linked" because it's broader than GitHub's
    /// official Development-sidebar "linked PRs" feature.
    References,
}

impl Tab {
    fn all() -> [Tab; 3] {
        [Tab::Conversation, Tab::Timeline, Tab::References]
    }
    fn label(self) -> &'static str {
        match self {
            Tab::Conversation => "Conversation",
            Tab::Timeline => "Timeline",
            Tab::References => "References",
        }
    }
    fn index(self) -> usize {
        match self {
            Tab::Conversation => 0,
            Tab::Timeline => 1,
            Tab::References => 2,
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

/// One row of the References tab — combines forward refs (this
/// issue mentions #N) and backward refs (#N mentioned this issue
/// via timeline cross-references).
#[derive(Debug, Clone)]
struct ReferenceRow {
    number: u64,
    title: String,
    is_pr: bool,
    state_label: String,
    direction: RefDirection,
}

#[derive(Debug, Clone, Copy)]
enum RefDirection {
    /// This issue's body or comments mention the row's `#N`.
    Forward,
    /// The row's `#N` mentions this issue (cross-referenced event).
    Backward,
}

/// Clickable `#N` hit-box inside the Conversation tab. Captured
/// during render, consumed by `handle_click` to dispatch the
/// cross-view navigation. `pub(crate)` so the PR view can reuse
/// the same shape for its own conversation refs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RefLink {
    /// Logical line index in the conversation Paragraph buffer
    /// (pre-scroll). Combined with `area.y + line - scroll` to get
    /// the screen row.
    pub line: usize,
    /// Column offset where the `#` lands inside the line.
    pub col_start: u16,
    /// Column just past the last digit (exclusive).
    pub col_end: u16,
    pub number: u64,
    pub is_pr: bool,
}

/// One candidate inside the `#` autocomplete popup. `pub(crate)` so
/// the PR view can drive the same popup widget with its own data.
#[derive(Debug, Clone)]
pub(crate) struct MentionItem {
    pub number: u64,
    pub title: String,
    pub kind: MentionKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MentionKind {
    Issue,
    Pr,
}

/// Which text surface the popup is anchored to — read/write paths
/// branch on this so the same popup machinery works for both the
/// comment editor (Detail mode) and the compose Body field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MentionTarget {
    CommentEditor,
    ComposeBody,
}

/// Floating popup that proposes issues + PRs to insert as a `#NNN`
/// reference. The active text surface routes its key events here
/// while the popup is `Some`. `pub(crate)` so the PR view can hold
/// its own instance and reuse the rendering helper.
#[derive(Debug, Clone)]
pub(crate) struct MentionPopup {
    pub target: MentionTarget,
    /// Byte offset of the `#` token inside the target buffer —
    /// drives the replacement range on pick.
    pub anchor: usize,
    /// Chars typed after the `#` (the filter query).
    pub query: String,
    /// Pre-filtered, sorted candidates the popup currently shows.
    /// Recomputed every time `query` changes.
    pub filtered: Vec<MentionItem>,
    pub hovered: usize,
    /// Row offset inside `filtered` of the topmost rendered item —
    /// drives the scroll window so `hovered` stays in view.
    pub scroll: usize,
    /// Body height captured at the last render — used by the nav
    /// helpers to decide when to scroll the viewport.
    pub last_visible: u16,
    pub overlay_rect: Option<Rect>,
    pub row_rects: Vec<Rect>,
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
        // Order MUST match the visual layout in `render_compose`
        // (Title → Labels → Assignees → Milestone → Body) so ↑/↓
        // moves through fields the way the user reads them.
        &[
            ComposeField::Title,
            ComposeField::Labels,
            ComposeField::Assignees,
            ComposeField::Milestone,
            ComposeField::Body,
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
            conversation_avatar_slots: Vec::new(),
            prev_painted_avatars: Vec::new(),
            pending_avatar_paints: Vec::new(),
            conversation_ref_links: Vec::new(),
            references_hovered: 0,
            references_count: 0,
            references_row_rects: Vec::new(),
            list_avatar_slots: Vec::new(),
            reaction_picker: None,
            mention_popup: None,
            mention_pr_cache: Vec::new(),
            viewer_reactions: FxHashMap::default(),
            mention_pr_loading: false,
            compose: None,
            compose_field_rects: Vec::new(),
            last_error: None,
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
                let _ = action;
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
                    if matches!(self.active_tab, Tab::References) {
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
        if self.mention_popup.is_some() {
            return "⌘ ↑↓:nav▕▏↵:pick▕▏Esc:cancel".into();
        }
        if self.comment_editor.is_some() {
            return "⌘ #:mention▕▏Ctrl+S:send▕▏Esc:cancel".into();
        }
        if self.reaction_picker.is_some() {
            return "⌘ ←→:nav▕▏↵:react▕▏Esc:cancel".into();
        }
        match self.mode {
            Mode::List => "⌘ n:new▕▏r:reload".into(),
            Mode::Detail => {
                // `x` is now the unified state-change key (close
                // when open, reopen when closed) — mirrors the PR
                // view's `Ctrl+X`/`x` convention. `o` is freed up
                // for "open in web".
                let state = self
                    .opened_issue_number
                    .and_then(|n| self.detail_cache.get(&n))
                    .map(|d| d.state);
                let mutate = match state {
                    Some(IssueState::Open) => "x:close",
                    Some(IssueState::Closed) => "x:reopen",
                    None => "",
                };
                // Card-local actions (`R:quote reply`, `+:react`,
                // `e:edit`, `d:delete`, `↵:open ref`) live in the
                // selected comment's top border now — keeps the
                // footer scannable and parity with PR view.
                let mut parts = vec![
                    "c:comment",
                    "l:labels",
                    "a:assignees",
                    "m:milestone",
                ];
                if !mutate.is_empty() {
                    parts.push(mutate);
                }
                parts.push("o:open in web");
                parts.push("r:reload");
                format!("⌘ {}", parts.join("▕▏"))
            }
            Mode::Compose => "⌘ ↑↓:field▕▏↵:edit/pick▕▏Ctrl+S:submit".into(),
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
        // Mention popup intercepts keys before the editor — its own
        // handler passes char/backspace through to update the query.
        if self.mention_popup.is_some() {
            // Mouse wheel scrolls the popup viewport without moving
            // the selector — separate from ↑↓ which moves the
            // selector and only scrolls when it leaves the window.
            match evt.event {
                UserEvent::ScrollUp => {
                    self.mention_popup_scroll(-1);
                    return;
                }
                UserEvent::ScrollDown => {
                    self.mention_popup_scroll(1);
                    return;
                }
                _ => {}
            }
            self.handle_event_mention_popup(key);
            return;
        }
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
        // Compose body scroll — mouse wheel pans the body viewport
        // when focus is on the Body field. Keeps the cursor in
        // place (we just shift body_scroll); a subsequent ↑/↓ keys
        // moves the cursor as usual and re-anchors the viewport.
        if matches!(self.mode, Mode::Compose) {
            match evt.event {
                UserEvent::ScrollUp => {
                    self.compose_body_scroll_viewport(-3);
                    return;
                }
                UserEvent::ScrollDown => {
                    self.compose_body_scroll_viewport(3);
                    return;
                }
                _ => {}
            }
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
            // ←/→ still cycle the filter — the binding stays
            // active, just not advertised in the footer (the user
            // wanted a cleaner hint with only the canonical
            // shortcuts).
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
            (Mode::List, UserEvent::Refresh) => {
                self.refresh();
                return;
            }
            (Mode::Detail, UserEvent::NavigateUp) => {
                if matches!(self.active_tab, Tab::References) {
                    self.references_move(-n);
                } else {
                    self.detail_move_selected(-n);
                }
                return;
            }
            (Mode::Detail, UserEvent::NavigateDown) => {
                if matches!(self.active_tab, Tab::References) {
                    self.references_move(n);
                } else {
                    self.detail_move_selected(n);
                }
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
            // ←/→ cycle Conversation / Timeline / Linked tabs (not
            // advertised in the footer to stay clean — same pattern
            // as the list filters). We deliberately gate this on
            // the raw key being an arrow so the `l` and `h` letter
            // bindings still reach `handle_event_detail` for the
            // labels picker etc.
            (Mode::Detail, UserEvent::NavigateLeft)
                if matches!(key.code, KeyCode::Left) =>
            {
                self.cycle_tab(-1);
                return;
            }
            (Mode::Detail, UserEvent::NavigateRight)
                if matches!(key.code, KeyCode::Right) =>
            {
                self.cycle_tab(1);
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
            // Tab no longer cycles filter — ←/→ owns that (via the
            // UserEvent dispatch in `handle_event` above). Tab can
            // be reused for ref-list or whatever the global binding
            // says, falling through here unhandled.
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
            // Tab switching is mouse-click only — no keyboard binding.
            KeyCode::Char('c') => self.start_new_comment(),
            // `r` reloads the detail (parity with the PR view).
            // `R` (Shift+R) is the quote-reply binding — same
            // convention as the PR conversation cards.
            KeyCode::Char('r') => self.refresh(),
            KeyCode::Char('R') => self.start_quote_reply(),
            // Shift+N: open Compose pre-filled with a back-reference
            // to the currently-selected entry. Mirrors GitHub's
            // "Reference in new issue" sub-menu action.
            KeyCode::Char('N') => self.start_reference_in_new_issue(),
            // Enter on the Conversation tab follows the FIRST `#N`
            // reference inside the currently-selected comment card.
            // Click on a specific ref also works for picking among
            // multiple — this shortcut is the keyboard-driven
            // fallback that GitHub web doesn't have.
            KeyCode::Enter if matches!(self.active_tab, Tab::Conversation) => {
                self.follow_first_ref_in_selected_card();
            }
            // Enter on the References tab opens the selected ref.
            KeyCode::Enter if matches!(self.active_tab, Tab::References) => {
                self.follow_selected_reference();
            }
            KeyCode::Char('e') => self.start_edit_own_comment(),
            KeyCode::Char('d') => self.confirm_delete_own_comment(),
            KeyCode::Char('+') => self.start_react(),
            // Lowercase letter actions for label/assignee/milestone.
            // `l` (= navigate_right global binding) and `a` (= stage)
            // both reach us as raw chars because they fired
            // UserEvents that the dispatch above no longer claims.
            KeyCode::Char('l') => self.start_labels_picker(),
            KeyCode::Char('a') => self.start_assignees_picker(),
            KeyCode::Char('m') => self.start_milestone_picker(),
            // `x` unifies close + reopen — picks the right one from
            // the currently-opened issue's state. `o` opens the issue
            // in the user's browser.
            KeyCode::Char('x') => self.start_close_or_reopen(),
            KeyCode::Char('o') => self.open_in_browser(),
            _ => {}
        }
    }

    fn start_close_or_reopen(&mut self) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        match self.detail_cache.get(&number).map(|d| d.state) {
            Some(IssueState::Open) => self.start_close(),
            Some(IssueState::Closed) => self.start_reopen(),
            None => {}
        }
    }

    fn open_in_browser(&mut self) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        let url = format!(
            "https://github.com/{}/{}/issues/{}",
            self.coords.owner, self.coords.repo, number
        );
        match crate::external::open_url(&url) {
            Ok(()) => self
                .tx
                .send(AppEvent::NotifyInfo(format!("Opened {}", url))),
            Err(e) => self
                .tx
                .send(AppEvent::NotifyError(format!("Open browser: {}", e))),
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
                Tab::References if !self.linked_cache.contains_key(&n) => {
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
                self.compose_body_anchor_to_cursor();
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
            // Labels / Assignees / Milestone: Enter on the focused
            // row opens its picker — the only entry point, so the
            // form stays predictable.
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
        // After any body-focused key, anchor the scroll viewport to
        // the cursor so typing past the bottom row brings new text
        // into view. Mouse-wheel scroll bypasses this path (it goes
        // through the dedicated `ScrollUp/Down` intercept earlier),
        // so wheel-scrolling no longer gets reverted on the next
        // render — that was the source of the "scroll doesn't stick"
        // and "hover scrolls to bottom" bugs.
        if editing_body {
            self.compose_body_anchor_to_cursor();
        }
    }

    /// Re-anchor `body_scroll` so the body cursor stays inside the
    /// last-rendered viewport. Called from cursor-mutating actions
    /// (typing, arrow keys, click, vertical nav) — NOT from the
    /// render path. Mouse-wheel scrolling never calls this, which is
    /// why the user's wheel scroll stays where they put it.
    fn compose_body_anchor_to_cursor(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if !matches!(c.field, ComposeField::Body) {
            return;
        }
        let view = c.body_last_height;
        if view == 0 {
            return;
        }
        let (_, cursor_row) = cursor_screen_pos(&c.body, c.body_cursor);
        if cursor_row < c.body_scroll {
            c.body_scroll = cursor_row;
        } else if cursor_row >= c.body_scroll + view {
            c.body_scroll = cursor_row + 1 - view;
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
        // Detect a `#` typed inside the Body field BEFORE we mutate
        // anything else — we want the anchor offset to point at the
        // `#` we're about to write. Title doesn't open the popup
        // (we'd cover the field's underline) — only Body does.
        let opened_popup_anchor: Option<usize> = {
            let Some(c) = self.compose.as_ref() else {
                return;
            };
            if c.submitting {
                return;
            }
            if ch == '#' && matches!(c.field, ComposeField::Body) {
                Some(c.body_cursor)
            } else {
                None
            }
        };
        if let Some(c) = self.compose.as_mut() {
            if let Some((buf, cur)) = Self::compose_text_target(c) {
                buf.insert(*cur, ch);
                *cur += ch.len_utf8();
            }
        }
        if let Some(anchor) = opened_popup_anchor {
            self.open_mention_popup(anchor, MentionTarget::ComposeBody);
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

    /// Pan the compose Body viewport by `delta` rows without
    /// touching the cursor. Clamped to the buffer's row count vs.
    /// the last-rendered body height.
    fn compose_body_scroll_viewport(&mut self, delta: i32) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        let total_rows = if c.body.is_empty() {
            1
        } else {
            c.body.matches('\n').count() as u16 + 1
        };
        let view = c.body_last_height.max(1);
        let max_scroll = total_rows.saturating_sub(view);
        let new = (c.body_scroll as i32 + delta).clamp(0, max_scroll as i32);
        c.body_scroll = new as u16;
    }

    /// Translate a click inside the compose Body rect into a byte
    /// offset in `state.body` and seat the cursor there. Mirrors
    /// `editor_move_cursor_to_click` for the comment editor.
    fn compose_body_move_cursor_to_click(&mut self, click_col: u16, click_row: u16) {
        // The first stored `(ComposeField::Body, rect)` is the
        // bordered block; the click might land on the border, in
        // which case we still snap to the nearest inner cell.
        let body_rect = match self
            .compose_field_rects
            .iter()
            .find(|(f, _)| matches!(f, ComposeField::Body))
            .map(|(_, r)| *r)
        {
            Some(r) => r,
            None => return,
        };
        // Inner area = rect minus its 1-cell border on each side.
        let inner_x = body_rect.x.saturating_add(1);
        let inner_y = body_rect.y.saturating_add(1);
        let inner_right = body_rect.x + body_rect.width.saturating_sub(1);
        let inner_bottom = body_rect.y + body_rect.height.saturating_sub(1);
        let col_in_view = click_col
            .saturating_sub(inner_x)
            .min(inner_right.saturating_sub(inner_x));
        let row_in_view = click_row
            .saturating_sub(inner_y)
            .min(inner_bottom.saturating_sub(inner_y).saturating_sub(1));
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        let scroll = c.body_scroll as usize;
        let logical_line = scroll + row_in_view as usize;
        // Walk lines to find the start byte of the target line.
        let buf = c.body.clone();
        let mut line_start: usize = 0;
        let mut current_line: usize = 0;
        for (offset, ch) in buf.char_indices() {
            if current_line == logical_line {
                break;
            }
            if ch == '\n' {
                current_line += 1;
                line_start = offset + ch.len_utf8();
            }
        }
        if current_line < logical_line {
            // Click below the last line — drop cursor at end.
            c.body_cursor = buf.len();
            c.field = ComposeField::Body;
            return;
        }
        // Walk chars from line_start until we hit col_in_view or `\n`.
        let mut byte_pos = line_start;
        let mut col_walked: usize = 0;
        for (offset, ch) in buf[line_start..].char_indices() {
            if ch == '\n' || col_walked >= col_in_view as usize {
                byte_pos = line_start + offset;
                break;
            }
            col_walked += 1;
            byte_pos = line_start + offset + ch.len_utf8();
        }
        c.body_cursor = byte_pos.min(buf.len());
        c.field = ComposeField::Body;
        // Drop the borrow before calling the anchor helper.
        drop(c);
        self.compose_body_anchor_to_cursor();
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
                // Typing a `#` opens the autocomplete anchored at
                // the char we just inserted — `cursor` points to
                // just past it, so anchor = cursor - 1.
                if ch == '#' {
                    if let Some(ed) = self.comment_editor.as_ref() {
                        let anchor = ed.cursor.saturating_sub(1);
                        self.open_mention_popup(anchor, MentionTarget::CommentEditor);
                    }
                }
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

    pub fn on_mention_prs_fetched(
        &mut self,
        result: Result<Vec<PullRequest>, String>,
    ) {
        self.mention_pr_loading = false;
        if let Ok(prs) = result {
            self.mention_pr_cache = prs;
            // Refresh the popup's filtered list if it's open so the
            // PRs surface immediately after the fetch lands.
            if self.mention_popup.is_some() {
                self.refresh_mention_filter();
            }
        }
    }

    fn spawn_mention_prs_fetch(&mut self) {
        if self.mention_pr_loading || !self.mention_pr_cache.is_empty() {
            return;
        }
        self.mention_pr_loading = true;
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = crate::github::pr::list_pull_requests(&token, &coords);
            tx.send(AppEvent::IssueMentionPrsFetched { result });
        });
    }

    /// Combine issues + cached PRs into a flat list of mention
    /// candidates, sorted by number descending.
    fn mention_universe(&self) -> Vec<MentionItem> {
        let mut out: Vec<MentionItem> = Vec::new();
        for issue in &self.items {
            out.push(MentionItem {
                number: issue.number,
                title: issue.title.clone(),
                kind: MentionKind::Issue,
            });
        }
        for pr in &self.mention_pr_cache {
            out.push(MentionItem {
                number: pr.number,
                title: pr.title.clone(),
                kind: MentionKind::Pr,
            });
        }
        out.sort_by(|a, b| b.number.cmp(&a.number));
        out
    }

    fn filter_mention_items(query: &str, all: &[MentionItem]) -> Vec<MentionItem> {
        filter_mention_items_pub(query, all)
    }

    fn refresh_mention_filter(&mut self) {
        let universe = self.mention_universe();
        if let Some(p) = self.mention_popup.as_mut() {
            p.filtered = Self::filter_mention_items(&p.query, &universe);
            if p.hovered >= p.filtered.len() {
                p.hovered = p.filtered.len().saturating_sub(1);
            }
        }
    }

    /// Open the `#` autocomplete popup anchored at `anchor` inside
    /// `target`'s buffer.
    fn open_mention_popup(&mut self, anchor: usize, target: MentionTarget) {
        self.spawn_mention_prs_fetch();
        let universe = self.mention_universe();
        let filtered = Self::filter_mention_items("", &universe);
        self.mention_popup = Some(MentionPopup {
            target,
            anchor,
            query: String::new(),
            filtered,
            hovered: 0,
            scroll: 0,
            last_visible: 0,
            overlay_rect: None,
            row_rects: Vec::new(),
        });
    }

    fn close_mention_popup(&mut self) {
        self.mention_popup = None;
    }

    /// Mouse-wheel scroll inside the mention popup. Moves the
    /// viewport (`scroll`) without touching the selector
    /// (`hovered`) — gives the user a real "list slides under
    /// the cursor" feel, separate from ↑↓ keyboard nav.
    fn mention_popup_scroll(&mut self, delta: i32) {
        let Some(p) = self.mention_popup.as_mut() else {
            return;
        };
        let vis = p.last_visible.max(1) as usize;
        let max_scroll = p.filtered.len().saturating_sub(vis);
        let new = (p.scroll as i32 + delta).max(0) as usize;
        p.scroll = new.min(max_scroll);
    }

    /// Insert the selected `#N` into the editor buffer, replacing
    /// the `#query` chunk that opened the popup. Closes the popup.
    fn pick_mention(&mut self) {
        let (target, anchor, query_len, number) = {
            let Some(p) = self.mention_popup.as_ref() else {
                return;
            };
            let Some(item) = p.filtered.get(p.hovered) else {
                return;
            };
            (p.target, p.anchor, p.query.chars().count(), item.number)
        };
        self.mention_popup = None;
        let replacement = format!("#{}", number);
        // Clamp the splice range — an inconsistent (anchor, buf)
        // snapshot would otherwise panic `replace_range` with
        // "begin > end" mid-edit.
        match target {
            MentionTarget::CommentEditor => {
                let Some(ed) = self.comment_editor.as_mut() else {
                    return;
                };
                let start = anchor.min(ed.buffer.len());
                let end = walk_chars(&ed.buffer, start.saturating_add(1), query_len)
                    .min(ed.buffer.len())
                    .max(start);
                ed.buffer.replace_range(start..end, &replacement);
                ed.cursor = start + replacement.len();
            }
            MentionTarget::ComposeBody => {
                let Some(c) = self.compose.as_mut() else {
                    return;
                };
                let start = anchor.min(c.body.len());
                let end = walk_chars(&c.body, start.saturating_add(1), query_len)
                    .min(c.body.len())
                    .max(start);
                c.body.replace_range(start..end, &replacement);
                c.body_cursor = start + replacement.len();
            }
        }
    }

    fn handle_event_mention_popup(&mut self, key: KeyEvent) {
        // Esc → close, Enter → pick, ↑↓ → nav. Everything else
        // (including chars and backspace) flows through to the
        // editor so the query keeps tracking what the user typed.
        match key.code {
            KeyCode::Esc => {
                self.close_mention_popup();
                return;
            }
            KeyCode::Enter => {
                self.pick_mention();
                return;
            }
            KeyCode::Up => {
                if let Some(p) = self.mention_popup.as_mut() {
                    if p.hovered > 0 {
                        p.hovered -= 1;
                    }
                    // Keep hovered inside the viewport.
                    if p.hovered < p.scroll {
                        p.scroll = p.hovered;
                    }
                }
                return;
            }
            KeyCode::Down => {
                if let Some(p) = self.mention_popup.as_mut() {
                    if p.hovered + 1 < p.filtered.len() {
                        p.hovered += 1;
                    }
                    let vis = p.last_visible.max(1) as usize;
                    if p.hovered >= p.scroll + vis {
                        p.scroll = p.hovered + 1 - vis;
                    }
                }
                return;
            }
            _ => {}
        }
        // Pass through to whichever surface the popup is anchored
        // on, then re-derive the query from the resulting buffer
        // state.
        let target = self
            .mention_popup
            .as_ref()
            .map(|p| p.target)
            .unwrap_or(MentionTarget::CommentEditor);
        match target {
            MentionTarget::CommentEditor => self.handle_event_comment_editor(key),
            MentionTarget::ComposeBody => self.handle_event_compose(key),
        }
        self.update_mention_query_from_editor();
    }

    /// Re-read the chars between `anchor` (the `#`) and the cursor
    /// in the editor buffer, rebuild `query` + filter. Closes the
    /// popup if the buffer no longer starts with `#` at the anchor
    /// (e.g. user backspaced past the `#`).
    fn update_mention_query_from_editor(&mut self) {
        // Snapshot the bits we need under an immutable borrow, then
        // mutate `self` after the borrow ends. The (buf, cursor)
        // pair comes from the target the popup is anchored on.
        let snapshot: (String, bool) = {
            let Some(popup) = self.mention_popup.as_ref() else {
                return;
            };
            let target = popup.target;
            let anchor = popup.anchor;
            let (buf_ref, cursor_val): (&str, usize) = match target {
                MentionTarget::CommentEditor => {
                    let Some(ed) = self.comment_editor.as_ref() else {
                        self.mention_popup = None;
                        return;
                    };
                    (ed.buffer.as_str(), ed.cursor)
                }
                MentionTarget::ComposeBody => {
                    let Some(c) = self.compose.as_ref() else {
                        self.mention_popup = None;
                        return;
                    };
                    (c.body.as_str(), c.body_cursor)
                }
            };
            let buf = buf_ref;
            let editor_cursor = cursor_val;
            // Bail out if the anchor lost its `#` (user backspaced
            // through it) or the cursor moved before the anchor.
            if anchor >= buf.len()
                || !buf[anchor..].starts_with('#')
                || editor_cursor <= anchor
            {
                (String::new(), true)
            } else {
                let slice = &buf[anchor + 1..editor_cursor.min(buf.len())];
                let mut q = String::new();
                let mut closed = false;
                for ch in slice.chars() {
                    if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                        q.push(ch);
                    } else {
                        // Non-mention char between the anchor and
                        // the cursor → the mention ended; close.
                        closed = true;
                        break;
                    }
                }
                (q, closed)
            }
        };
        let (new_query, should_close) = snapshot;
        if should_close {
            self.close_mention_popup();
            return;
        }
        if let Some(p) = self.mention_popup.as_mut() {
            p.query = new_query;
        }
        self.refresh_mention_filter();
    }

    fn references_move(&mut self, delta: i32) {
        let len = self.references_count as i32;
        if len == 0 {
            return;
        }
        let new = (self.references_hovered as i32 + delta).clamp(0, len - 1);
        self.references_hovered = new as usize;
    }

    fn follow_selected_reference(&mut self) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        let refs = self.collect_references(number);
        let Some(r) = refs.get(self.references_hovered) else {
            return;
        };
        self.follow_reference(r.number, r.is_pr);
    }

    /// Find the first captured `#N` reference whose line falls
    /// inside the currently-selected comment's span and follow it.
    /// No-op when the card has no refs.
    fn follow_first_ref_in_selected_card(&mut self) {
        let Some(&(_, first, last)) = self
            .conversation_comment_spans
            .iter()
            .find(|(idx, _, _)| *idx == self.conversation_selected)
        else {
            return;
        };
        let Some(link) = self
            .conversation_ref_links
            .iter()
            .find(|l| l.line >= first && l.line <= last)
            .copied()
        else {
            return;
        };
        self.follow_reference(link.number, link.is_pr);
    }

    /// Navigate to a `#N` reference — issues swap the open issue
    /// in-place (cheap), PRs dispatch a cross-view event that
    /// re-anchors the App on the PR view.
    fn follow_reference(&mut self, number: u64, is_pr: bool) {
        if is_pr {
            // Closing the comment editor and any popups first so the
            // user lands cleanly on the PR detail.
            self.comment_editor = None;
            self.mention_popup = None;
            self.tx.send(AppEvent::OpenPullRequestDetail { number });
            return;
        }
        // Same-view navigation: pivot the current Issues view onto
        // the new issue. Keeps Esc → list semantics intact since we
        // never leave Mode::Detail.
        self.opened_issue_number = Some(number);
        self.active_tab = Tab::Conversation;
        self.conversation_scroll = 0;
        self.conversation_selected = 0;
        self.conversation_scroll_to_selected = true;
        if !self.detail_cache.contains_key(&number) && self.loading_for != Some(number) {
            self.spawn_detail_fetch(number);
        }
        if !self.timeline_cache.contains_key(&number)
            && self.loading_timeline_for != Some(number)
        {
            self.spawn_timeline_fetch(number);
        }
        if !self.linked_cache.contains_key(&number)
            && self.loading_linked_for != Some(number)
        {
            self.spawn_linked_fetch(number);
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
        // Fire-and-forget fetch of the viewer's reactions on the
        // target entry so the picker can tag them red even on a
        // session that didn't apply them locally. Result lands via
        // `on_viewer_reactions_fetched`.
        self.spawn_viewer_reactions_fetch(target);
    }

    fn spawn_viewer_reactions_fetch(&self, target_idx: usize) {
        let Some(number) = self.opened_issue_number else {
            return;
        };
        let Some(me) = self.me_login.clone() else {
            return;
        };
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        if target_idx == 0 {
            std::thread::spawn(move || {
                let reactions = crate::github::pr::list_my_issue_reactions(
                    &token, &coords, number, &me,
                )
                .unwrap_or_default();
                tx.send(AppEvent::IssueViewerReactionsFetched {
                    issue_number: number,
                    target_idx,
                    reactions,
                });
            });
            return;
        }
        let comment_id = self
            .detail_cache
            .get(&number)
            .and_then(|d| d.conversation.get(target_idx - 1))
            .and_then(|e| e.id);
        let Some(cid) = comment_id else {
            return;
        };
        std::thread::spawn(move || {
            let reactions = crate::github::pr::list_my_issue_comment_reactions(
                &token, &coords, cid, &me,
            )
            .unwrap_or_default();
            tx.send(AppEvent::IssueViewerReactionsFetched {
                issue_number: number,
                target_idx,
                reactions,
            });
        });
    }

    pub fn on_viewer_reactions_fetched(
        &mut self,
        issue_number: u64,
        target_idx: usize,
        reactions: Vec<(crate::github::pr::ReactionKind, u64)>,
    ) {
        self.viewer_reactions
            .insert((issue_number, target_idx), reactions);
    }

    /// Lookup of reactions the viewer has placed on a given
    /// conversation entry (issue body when `target_idx == 0`,
    /// otherwise comment index `target_idx - 1`). Backed by the
    /// in-session `viewer_reactions` map populated by
    /// `submit_reaction`.
    fn viewer_reactions_for(
        &self,
        target_idx: usize,
    ) -> Vec<crate::github::pr::ReactionKind> {
        let Some(number) = self.opened_issue_number else {
            return Vec::new();
        };
        self.viewer_reactions
            .get(&(number, target_idx))
            .map(|v| v.iter().map(|(k, _)| *k).collect())
            .unwrap_or_default()
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

        // Toggle: if the viewer already reacted with this kind, hit
        // DELETE on the recorded reaction id instead of POSTing a
        // duplicate. Matches GitHub web's behaviour where clicking a
        // chip you already own removes the reaction.
        let existing_id: Option<u64> = self
            .viewer_reactions
            .get(&(number, target_idx))
            .and_then(|v| v.iter().find(|(k, _)| *k == kind).map(|(_, id)| *id));

        if target_idx == 0 {
            if let Some(rid) = existing_id {
                std::thread::spawn(move || {
                    let result = crate::github::pr::delete_issue_reaction(
                        &token, &coords, number, rid,
                    );
                    if result.is_ok() {
                        tx.send(AppEvent::IssueReactionRemoved {
                            issue_number: number,
                            target_idx,
                            kind,
                        });
                    } else if let Err(e) = result {
                        tx.send(AppEvent::NotifyError(format!("Unreact: {}", e)));
                    }
                });
                return;
            }
            std::thread::spawn(move || {
                match crate::github::issue::add_issue_reaction(&token, &coords, number, kind)
                {
                    Ok(reaction_id) => {
                        tx.send(AppEvent::IssueReactionApplied {
                            issue_number: number,
                            target_idx,
                            kind,
                            reaction_id,
                        });
                    }
                    Err(e) => {
                        tx.send(AppEvent::NotifyError(format!("React: {}", e)));
                    }
                }
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
        if let Some(rid) = existing_id {
            std::thread::spawn(move || {
                let result = crate::github::pr::delete_issue_comment_reaction(
                    &token, &coords, comment_id, rid,
                );
                if result.is_ok() {
                    tx.send(AppEvent::IssueReactionRemoved {
                        issue_number: number,
                        target_idx,
                        kind,
                    });
                } else if let Err(e) = result {
                    tx.send(AppEvent::NotifyError(format!("Unreact: {}", e)));
                }
            });
            return;
        }
        std::thread::spawn(move || {
            match crate::github::pr::add_issue_comment_reaction(&token, &coords, comment_id, kind)
            {
                Ok(reaction_id) => {
                    tx.send(AppEvent::IssueReactionApplied {
                        issue_number: number,
                        target_idx,
                        kind,
                        reaction_id,
                    });
                }
                Err(e) => {
                    tx.send(AppEvent::NotifyError(format!("React: {}", e)));
                }
            }
        });
    }

    pub fn on_reaction_applied(
        &mut self,
        issue_number: u64,
        target_idx: usize,
        kind: crate::github::pr::ReactionKind,
        reaction_id: u64,
    ) {
        self.viewer_reactions
            .entry((issue_number, target_idx))
            .or_default()
            .push((kind, reaction_id));
        // Re-fetch the detail so the chip count updates.
        self.spawn_detail_fetch(issue_number);
    }

    pub fn on_reaction_removed(
        &mut self,
        issue_number: u64,
        target_idx: usize,
        kind: crate::github::pr::ReactionKind,
    ) {
        if let Some(v) = self.viewer_reactions.get_mut(&(issue_number, target_idx)) {
            v.retain(|(k, _)| *k != kind);
        }
        self.spawn_detail_fetch(issue_number);
    }

    // ─── Comment CRUD ────────────────────────────────────────────

    /// Quote-reply: opens a fresh top-level comment pre-filled with
    /// the selected comment's body wrapped in a markdown blockquote,
    /// followed by an empty line where the user types their reply.
    /// GitHub web does the same thing — issue comments don't thread
    /// natively so a quoted top-level comment is the closest match.
    fn start_quote_reply(&mut self) {
        // Warm up the PR cache for the `#` autocomplete so the popup
        // has results ready the moment the user types `#` inside the
        // reply.
        self.spawn_mention_prs_fetch();
        let Some(number) = self.opened_issue_number else {
            return;
        };
        let Some(detail) = self.detail_cache.get(&number).cloned() else {
            return;
        };
        // Source = the description card (idx 0) or one of the
        // chronological comments. Skip if there's nothing selected
        // or the entry has no body.
        let (author, body) = if self.conversation_selected == 0 {
            (detail.author.clone(), detail.body.clone())
        } else {
            let Some(entry) = detail
                .conversation
                .get(self.conversation_selected - 1)
            else {
                return;
            };
            (entry.author.clone(), entry.body.clone())
        };
        if body.trim().is_empty() {
            return;
        }
        // Wrap each body line in `> ` so it renders as a Markdown
        // blockquote on GitHub. Trailing blank line + cursor at end
        // for the user to type the reply.
        let quoted: String = body
            .lines()
            .map(|l| format!("> {}", l))
            .collect::<Vec<_>>()
            .join("\n");
        let buffer = format!("@{} wrote:\n{}\n\n", author, quoted);
        let cursor = buffer.len();
        self.comment_editor = Some(CommentEditor {
            kind: CommentEditorKind::NewTopLevel,
            buffer,
            cursor,
            submitting: false,
            scroll_offset: 0,
            last_body_height: 0,
        });
    }

    fn start_new_comment(&mut self) {
        if self.opened_issue_number.is_none() {
            return;
        }
        // Warm up the `#` autocomplete PR cache.
        self.spawn_mention_prs_fetch();
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
        self.spawn_mention_prs_fetch();
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
        let Some(detail) = self.detail_cache.get(&number) else {
            return;
        };
        // No-op when the issue is already closed — the keybinding
        // shouldn't even surface in that case (see `footer_hint`)
        // but guard here too so a stale shortcut doesn't fire.
        if matches!(detail.state, IssueState::Closed) {
            return;
        }
        let title = detail.title.clone();
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
        let Some(detail) = self.detail_cache.get(&number) else {
            return;
        };
        // Symmetric guard to `start_close` — never POST a reopen on
        // an already-open issue.
        if matches!(detail.state, IssueState::Open) {
            return;
        }
        let title = detail.title.clone();
        self.tx.send(AppEvent::OpenDialog(
            crate::event::DialogKind::ConfirmReopenIssue {
                issue_number: number,
                issue_title: title,
            },
        ));
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

    /// "Reference in new issue" — opens Compose pre-filled with a
    /// back-reference to the currently-selected entry (issue body or
    /// comment). The new issue body is a markdown blockquote of the
    /// source plus a `Re: #N` line so the auto-detected cross-ref
    /// shows up in both directions once submitted.
    fn start_reference_in_new_issue(&mut self) {
        let Some(orig_number) = self.opened_issue_number else {
            return;
        };
        let Some(detail) = self.detail_cache.get(&orig_number).cloned() else {
            return;
        };
        // Source = selected card. idx 0 is the issue body itself.
        let (orig_author, source_body) = if self.conversation_selected == 0 {
            (detail.author.clone(), detail.body.clone())
        } else {
            let Some(entry) = detail.conversation.get(self.conversation_selected - 1)
            else {
                return;
            };
            (entry.author.clone(), entry.body.clone())
        };
        let title = format!("Follow-up to #{}: {}", orig_number, detail.title);
        let quoted: String = if source_body.trim().is_empty() {
            String::new()
        } else {
            source_body
                .lines()
                .map(|l| format!("> {}", l))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let body = if quoted.is_empty() {
            format!("Continuing from #{}.\n\n", orig_number)
        } else {
            format!(
                "Continuing from #{} (@{} wrote):\n{}\n\n",
                orig_number, orig_author, quoted
            )
        };
        let body_cursor = body.len();
        let title_cursor = title.len();
        // Same picker pre-loads as start_compose so labels /
        // assignees / milestone are ready without an extra wait.
        let token = self.token.clone();
        let coords = self.coords.clone();
        let labels =
            crate::github::issue::list_repo_labels(&token, &coords).unwrap_or_default();
        let assignees =
            crate::github::issue::list_repo_assignees(&token, &coords).unwrap_or_default();
        let milestones =
            crate::github::issue::list_repo_milestones(&token, &coords).unwrap_or_default();
        self.compose = Some(ComposeState {
            title,
            title_cursor,
            body,
            body_cursor,
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
            crate::github::issue::load_first_template(&self.ctx.repo_path).unwrap_or_default();
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
        // Mention popup wins all clicks while open. Click on a row
        // picks the mention; click anywhere else closes the popup.
        if self.mention_popup.is_some() {
            let hit = {
                let Some(p) = self.mention_popup.as_ref() else {
                    return;
                };
                p.row_rects
                    .iter()
                    .position(|r| rect_contains(Some(*r), col, row))
            };
            if let Some(i) = hit {
                if let Some(p) = self.mention_popup.as_mut() {
                    p.hovered = i;
                }
                self.pick_mention();
            } else if !rect_contains(
                self.mention_popup.as_ref().and_then(|p| p.overlay_rect),
                col,
                row,
            ) {
                self.close_mention_popup();
            }
            return;
        }
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
        // Inline `#N` reference click — only relevant in the
        // Conversation tab. Resolve the screen (col, row) back to a
        // logical (line, col) inside the Paragraph buffer and check
        // if any captured link covers that position.
        if matches!(self.mode, Mode::Detail)
            && matches!(self.active_tab, Tab::Conversation)
        {
            if let Some(area) = self.tab_content_area {
                if rect_contains(Some(area), col, row) {
                    let logical_line =
                        (row.saturating_sub(area.y) as usize) + self.conversation_scroll;
                    let local_col = col.saturating_sub(area.x);
                    let target = self
                        .conversation_ref_links
                        .iter()
                        .find(|l| {
                            l.line == logical_line
                                && local_col >= l.col_start
                                && local_col < l.col_end
                        })
                        .copied();
                    if let Some(link) = target {
                        self.follow_reference(link.number, link.is_pr);
                        return;
                    }
                }
            }
        }
        // References tab row click — first click selects, second
        // (or click on already-selected) follows. Matches the way
        // double-clicks would behave on a list widget while still
        // working over a single click for power users.
        if matches!(self.mode, Mode::Detail)
            && matches!(self.active_tab, Tab::References)
        {
            let hit = self
                .references_row_rects
                .iter()
                .position(|r| rect_contains(Some(*r), col, row));
            if let Some(i) = hit {
                if i == self.references_hovered {
                    self.follow_selected_reference();
                } else {
                    self.references_hovered = i;
                }
                return;
            }
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
                    match field {
                        // Click on Labels / Assignees / Milestone
                        // also opens the picker.
                        ComposeField::Labels => self.open_compose_labels_picker(),
                        ComposeField::Assignees => self.open_compose_assignees_picker(),
                        ComposeField::Milestone => self.open_compose_milestone_picker(),
                        // Click inside the Body block snaps the
                        // cursor to the clicked character — same
                        // UX as the comment editor.
                        ComposeField::Body => {
                            self.compose_body_move_cursor_to_click(col, row);
                        }
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
        // Mention popup is modal: hover inside it highlights the
        // matching row, hover outside is ignored (so the comment
        // selection behind the popup doesn't shift).
        if self.mention_popup.is_some() {
            if let Some(p) = self.mention_popup.as_mut() {
                if let Some(i) = p
                    .row_rects
                    .iter()
                    .position(|r| rect_contains(Some(*r), col, row))
                {
                    p.hovered = i;
                }
            }
            return;
        }
        // Reaction picker is modal: hover over a cell updates the
        // highlighted emoji so the click handler reacts with it.
        // Hover outside leaves the picker's selection alone (won't
        // bump the comment behind, which would feel wrong).
        if self.reaction_picker.is_some() {
            if let Some(picker) = self.reaction_picker.as_mut() {
                if let Some(i) = picker
                    .row_rects
                    .iter()
                    .position(|r| rect_contains(Some(*r), col, row))
                {
                    picker.hovered = i;
                }
            }
            return;
        }
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
        // Detail mode + References tab: hover updates the selected
        // row so the user can navigate by mouse without clicking.
        if matches!(self.mode, Mode::Detail)
            && matches!(self.active_tab, Tab::References)
        {
            if let Some(i) = self
                .references_row_rects
                .iter()
                .position(|r| rect_contains(Some(*r), col, row))
            {
                self.references_hovered = i;
            }
        }
        // Detail mode + Conversation tab: hovering a comment card
        // bumps the selection to that card, matching PR's UX.
        // Hovering deliberately does NOT auto-scroll — the user
        // expects mouse hover to leave the viewport alone.
        if matches!(self.mode, Mode::Detail)
            && matches!(self.active_tab, Tab::Conversation)
        {
            if let Some(area) = self.tab_content_area {
                if rect_contains(Some(area), col, row) {
                    let logical = (row.saturating_sub(area.y) as usize)
                        + self.conversation_scroll;
                    for &(idx, first, last) in &self.conversation_comment_spans {
                        if logical >= first && logical <= last {
                            self.conversation_selected = idx;
                            // No scroll_to_selected — hover never
                            // moves the viewport.
                            break;
                        }
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
        self.open_detail_for(number);
    }

    /// Programmatic entry point used by cross-view navigation
    /// (`OpenIssueDetail`) — opens the issue detail without
    /// requiring a prior list selection. The issue may not exist
    /// in `self.items` yet; the fetch will surface a 404 if so.
    pub fn open_by_number(&mut self, number: u64) {
        self.open_detail_for(number);
    }

    fn open_detail_for(&mut self, number: u64) {
        self.opened_issue_number = Some(number);
        self.mode = Mode::Detail;
        self.active_tab = Tab::Conversation;
        self.conversation_scroll = 0;
        self.conversation_selected = 0;
        self.conversation_scroll_to_selected = true;
        // Fire all four fetches eagerly: detail / timeline / linked
        // populate the tab counts before the user has to switch tabs,
        // and the PR cache primes the `#N` reference resolver so
        // refs in the body card (e.g. `#1`) render as proper links on
        // the very first frame — without this, the resolver runs
        // against an empty `mention_pr_cache` and the ref text shows
        // as plain `#N` until some later action triggers the fetch.
        if !self.detail_cache.contains_key(&number) && self.loading_for != Some(number) {
            self.spawn_detail_fetch(number);
        }
        if !self.timeline_cache.contains_key(&number)
            && self.loading_timeline_for != Some(number)
        {
            self.spawn_timeline_fetch(number);
        }
        if !self.linked_cache.contains_key(&number)
            && self.loading_linked_for != Some(number)
        {
            self.spawn_linked_fetch(number);
        }
        self.spawn_mention_prs_fetch();
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
        // Clear hit-test rects from the previous frame so a stale
        // rect from a different layout never matches a click.
        self.filter_tab_rects.clear();
        self.tab_bar_rects.clear();
        self.compose_field_rects.clear();
        self.comment_editor_cursor_pos = None;
        self.list_area = None;
        self.tab_content_area = None;
        // Per-frame avatar accumulator — flushed once at the end
        // via the diff helper so cross-section transitions clear
        // stale placements correctly.
        self.pending_avatar_paints.clear();

        // Compose keeps the same top header as the list/detail
        // views (⊙ Issues OWNER/REPO …) — only the body area below
        // the header switches to the compose form. Parity with the
        // PR view's chrome.
        if matches!(self.mode, Mode::Compose) {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(0)])
                .split(area);
            self.render_top_header(f, chunks[0]);
            self.render_compose(f, chunks[1]);
            // `#` mention popup also fires from compose Body — paint
            // it on top so the picker is visible during issue creation.
            if self.mention_popup.is_some() {
                self.render_mention_popup(f, area);
            }
            self.flush_pending_avatars(f);
            self.place_terminal_cursor(f);
            return;
        }

        // Same vertical structure as the PR view: 2-row top header
        // (title + divider), optional 3-row error banner, then the
        // body — which is either the bordered list panel or the
        // detail layout.
        let banner_height: u16 =
            if self.last_error.is_some() && area.height > 6 { 3 } else { 0 };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(banner_height),
                Constraint::Min(0),
            ])
            .split(area);
        self.render_top_header(f, chunks[0]);
        if banner_height > 0 {
            self.render_error_banner(f, chunks[1]);
        }
        match self.mode {
            Mode::List => self.render_list(f, chunks[2]),
            Mode::Detail => self.render_detail(f, chunks[2]),
            Mode::Compose => {}
        }
        // Reaction overlay on top of the detail content.
        if self.reaction_picker.is_some() {
            self.render_reaction_picker_overlay(f, chunks[2]);
        }
        // `#` mention popup — drawn last so it floats above the
        // editor + reaction picker (the latter shouldn't coexist
        // but z-order is correct anyway).
        if self.mention_popup.is_some() {
            self.render_mention_popup(f, chunks[2]);
        }
        self.flush_pending_avatars(f);
        self.place_terminal_cursor(f);
    }

    /// Drains `pending_avatar_paints` and runs the single-pass
    /// diff against `prev_painted_avatars`: stale slots (from a
    /// section that doesn't render this frame) get a clear-cell
    /// emitted at their old position, unchanged slots are marked
    /// skip (no protocol re-emit), new slots are painted fresh.
    fn flush_pending_avatars(&mut self, f: &mut Frame) {
        let pending = std::mem::take(&mut self.pending_avatar_paints);
        let prev = std::mem::take(&mut self.prev_painted_avatars);
        // Build the list of "occluding" rects — overlays drawn this
        // frame whose visual region must not show any avatar from
        // the cards beneath. Avatars sitting inside any of them get
        // dropped from `pending`; the diff then sees them as "no
        // longer current" and emits a clear command, so the popup
        // body is uncluttered.
        let mut occluders: Vec<Rect> = Vec::new();
        if let Some(p) = self.mention_popup.as_ref() {
            if let Some(r) = p.overlay_rect {
                occluders.push(r);
            }
        }
        if let Some(p) = self.reaction_picker.as_ref() {
            if let Some(r) = p.overlay_rect {
                occluders.push(r);
            }
        }
        let pending: Vec<(crate::view::pr::PaintedAvatar, Color)> = pending
            .into_iter()
            .filter(|(pa, _)| {
                !occluders
                    .iter()
                    .any(|r| rect_contains(Some(*r), pa.screen_x, pa.screen_y))
            })
            .collect();
        self.prev_painted_avatars =
            crate::view::pr::paint_avatars_with_diff(f, &self.ctx, &prev, pending);
    }

    /// Position the terminal cursor on the active text input surface
    /// (comment editor or compose Title/Body). Honours the user's
    /// `cursor_type` config — native cursor by default, virtual glyph
    /// painted into the buffer when so configured.
    fn place_terminal_cursor(&self, f: &mut Frame) {
        let Some((cx, cy)) = self.comment_editor_cursor_pos else {
            return;
        };
        // Suppress the terminal cursor when the mention popup is up —
        // a blinking cursor sitting on top of the popup body reads as
        // a glitch. The popup intercepts every keystroke anyway, so
        // the editor cursor isn't actionable while it's open.
        if let Some(p) = self.mention_popup.as_ref() {
            if let Some(rect) = p.overlay_rect {
                if cx >= rect.x
                    && cx < rect.x + rect.width
                    && cy >= rect.y
                    && cy < rect.y + rect.height
                {
                    return;
                }
            }
        }
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

    /// Top header — single line "⊙ Issues OWNER/REPO  N open" plus a
    /// thin divider beneath. Mirrors `PullRequestsView::render_header`
    /// so the two pages share an identical entry surface.
    fn render_top_header(&self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let open_count = self
            .items
            .iter()
            .filter(|i| matches!(i.state, IssueState::Open))
            .count();
        let mut left: Vec<Span<'static>> = vec![
            Span::raw("  "),
            // `◉` (fisheye) = filled dot on a ring, mirrors GitHub's
            // open-issue indicator. Green matches the `Open` state
            // colour the rest of the view already uses.
            Span::styled(
                "◉ ",
                Style::default()
                    .fg(theme.status_success_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Issues ",
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}/{}", self.coords.owner, self.coords.repo),
                Style::default()
                    .fg(crate::view::pr::REPO_TEAL)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} open", open_count),
                Style::default().fg(theme.detail_label_fg),
            ),
        ];
        // Right-aligned ` GitHub` mark — same gate as the PR header.
        if self.ctx.ui_config.common.nerd_font {
            const GH_TAG: &str = "\u{f09b} GitHub";
            const RIGHT_PAD: usize = 2;
            let left_w: usize = left
                .iter()
                .map(|s| console::measure_text_width(s.content.as_ref()))
                .sum();
            let tag_w = console::measure_text_width(GH_TAG);
            let total_w = area.width as usize;
            if total_w > left_w + tag_w + RIGHT_PAD {
                let fill = total_w - left_w - tag_w - RIGHT_PAD;
                left.push(Span::raw(" ".repeat(fill)));
                left.push(Span::styled(
                    GH_TAG.to_string(),
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                ));
            }
        }
        let divider = Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(theme.divider_fg),
        ));
        f.render_widget(
            Paragraph::new(vec![Line::from(left), divider]),
            area,
        );
    }

    fn render_error_banner(&self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let Some(msg) = self.last_error.as_ref() else {
            return;
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.status_error_fg))
            .title(Line::from(Span::styled(
                " ✗ GitHub fetch failed ",
                Style::default()
                    .fg(theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let p = Paragraph::new(Span::styled(
            msg.clone(),
            Style::default().fg(theme.status_error_fg),
        ));
        f.render_widget(p, inner);
    }

    /// Build the filter-tabs title row that sits on the top border of
    /// the Issues list panel — strict mirror of PR's
    /// `build_filter_title`. Returns styled spans + per-tab screen
    /// rects for click hit-testing.
    fn build_filter_title(
        &self,
        area: Rect,
    ) -> (Vec<Span<'static>>, Vec<(IssueListFilter, Rect)>) {
        let theme = &self.ctx.color_theme;
        let filters = [
            IssueListFilter::Open,
            IssueListFilter::Closed,
            IssueListFilter::All,
        ];
        let counts: Vec<usize> = filters
            .iter()
            .map(|f| {
                self.items
                    .iter()
                    .filter(|i| f.matches(i.state))
                    .count()
            })
            .collect();
        const TAB_GAP: u16 = 3;
        let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
        let mut rects: Vec<(IssueListFilter, Rect)> = Vec::new();
        let mut cursor_x: u16 = area.x.saturating_add(2);
        for (i, filter) in filters.iter().enumerate() {
            let is_active = *filter == self.list_filter;
            let is_hovered = self.hovered_filter == Some(*filter) && !is_active;
            let label = format!("{} {}", filter.label(), counts[i]);
            let fg = if is_active {
                theme.list_head_fg
            } else if is_hovered {
                theme.fg
            } else {
                theme.detail_label_fg
            };
            let mut style = Style::default().fg(fg);
            if is_active {
                style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            } else if is_hovered {
                style = style.add_modifier(Modifier::BOLD);
            }
            let label_len = label.chars().count() as u16;
            rects.push((*filter, Rect::new(cursor_x, area.y, label_len, 1)));
            spans.push(Span::styled(label, style));
            cursor_x = cursor_x.saturating_add(label_len);
            if i + 1 < filters.len() {
                spans.push(Span::raw(" ".repeat(TAB_GAP as usize)));
                cursor_x = cursor_x.saturating_add(TAB_GAP);
            }
        }
        spans.push(Span::raw(" "));
        (spans, rects)
    }

    fn render_list(&mut self, f: &mut Frame, area: Rect) {
        // Filter tabs as the block title — identical to the PR list.
        let (title_spans, tab_rects) = self.build_filter_title(area);
        self.filter_tab_rects = tab_rects;

        let theme_for_block = self.ctx.color_theme.divider_fg;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme_for_block))
            .title(Line::from(title_spans));
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.list_area = Some(area);
        // 1-row gap below the filter tabs so the first row breathes.
        let body_area = Rect::new(
            inner.x,
            inner.y.saturating_add(1),
            inner.width,
            inner.height.saturating_sub(1),
        );
        self.list_inner_y = body_area.y;
        let theme = &self.ctx.color_theme;

        let filtered_indices = self.filtered_indices();
        let filtered: Vec<&Issue> = filtered_indices
            .iter()
            .map(|i| &self.items[*i])
            .collect();

        if filtered.is_empty() {
            self.list_scroll_offset = 0;
            let msg = match self.list_filter {
                IssueListFilter::Open => "No open issues.",
                IssueListFilter::Closed => "No closed issues.",
                IssueListFilter::All => "No issues in this repo.",
            };
            f.render_widget(
                Paragraph::new(Span::styled(
                    msg,
                    Style::default().fg(theme.detail_label_fg),
                ))
                .alignment(Alignment::Center),
                body_area,
            );
            return;
        }

        // Keyboard scroll anchoring — same logic as PR list.
        let visible = body_area.height as usize;
        if visible > 0 && self.hovered >= self.list_scroll_offset + visible {
            self.list_scroll_offset = self.hovered + 1 - visible;
        } else if self.hovered < self.list_scroll_offset {
            self.list_scroll_offset = self.hovered;
        }
        let max_offset = filtered.len().saturating_sub(visible);
        if self.list_scroll_offset > max_offset {
            self.list_scroll_offset = max_offset;
        }

        const RIGHT_MARGIN: usize = 2;
        let col_budget = (body_area.width as usize).saturating_sub(RIGHT_MARGIN);
        let owned_filtered: Vec<Issue> = filtered.iter().map(|i| (*i).clone()).collect();
        let cols = IssueListColumns::compute(&owned_filtered, col_budget);
        let row_width = body_area.width as usize;
        self.list_avatar_slots.clear();
        let avatars_on = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        let mut local_slots: Vec<AvatarSlot> = Vec::new();
        let items: Vec<ListItem<'static>> = filtered
            .iter()
            .enumerate()
            .map(|(i, issue)| {
                let is_marked = self.hovered == i;
                let (line, avatar_col) =
                    format_issue_row(theme, issue, is_marked, &cols, row_width, avatars_on);
                if let Some(col) = avatar_col {
                    if i >= self.list_scroll_offset
                        && visible > 0
                        && i < self.list_scroll_offset + visible
                    {
                        local_slots.push(AvatarSlot {
                            login: issue.author.clone(),
                            line: i - self.list_scroll_offset,
                            col,
                            is_selected: is_marked,
                        });
                    }
                }
                ListItem::new(line)
            })
            .collect();
        self.list_avatar_slots = local_slots;
        let mut state = ListState::default();
        state.select(Some(self.hovered));
        *state.offset_mut() = self.list_scroll_offset;
        // No widget-level highlight — `format_issue_row` paints the
        // selection bg per-span so label chips keep their colours.
        let list = List::new(items)
            .highlight_style(Style::default().add_modifier(Modifier::BOLD));
        f.render_stateful_widget(list, body_area, &mut state);

        // Push issue-list avatar intents into the per-frame
        // accumulator. The trailing diff pass in `render()` handles
        // clearing stale slots and skip-emitting unchanged ones.
        let slots = std::mem::take(&mut self.list_avatar_slots);
        let theme_bg = self.ctx.color_theme.bg;
        let sel_bg = self.ctx.color_theme.list_selected_bg;
        for slot in &slots {
            let screen_y = body_area.y + slot.line as u16;
            if screen_y >= body_area.y + body_area.height {
                continue;
            }
            let screen_x = body_area.x + slot.col;
            let bg = if slot.is_selected { sel_bg } else { theme_bg };
            self.pending_avatar_paints.push((
                crate::view::pr::PaintedAvatar {
                    login: slot.login.clone(),
                    screen_x,
                    screen_y,
                    is_selected: slot.is_selected,
                },
                bg,
            ));
        }
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
                Tab::References => self.render_tab_references(f, split[0], number),
            }
            self.render_comment_editor(f, split[1]);
        } else {
            match self.active_tab {
                Tab::Conversation => self.render_tab_conversation(f, tab_content_area, detail),
                Tab::Timeline => self.render_tab_timeline(f, tab_content_area, number),
                Tab::References => self.render_tab_references(f, tab_content_area, number),
            }
        }
    }

    fn render_issue_sub_header(
        &mut self,
        f: &mut Frame,
        area: Rect,
        detail: Option<&IssueDetail>,
    ) {
        let theme = &self.ctx.color_theme;
        let avatars_on = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::raw(" "));
        // Avatars captured as we go; painted after the Paragraph
        // renders. `(login, col_within_area)` pairs.
        let mut avatar_paints: Vec<(String, u16)> = Vec::new();
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
            // Conservative title budget — we don't know exact meta
            // width here without measuring, so estimate generously.
            // `avatar_pad_each` is 3 cells per author/assignee when
            // avatars are enabled, 0 otherwise; matches the actual
            // pad we'll push below.
            let pad: usize = if avatars_on { 3 } else { 0 };
            let meta_estimate = "opened by ".len()
                + d.author.chars().count()
                + " · ".len()
                + d.opened_when.chars().count()
                + pad
                + if d.assignees.is_empty() {
                    0
                } else {
                    "  ·  assigned: ".len()
                        + d.assignees.iter().map(|a| a.chars().count() + pad).sum::<usize>()
                        + (d.assignees.len().saturating_sub(1)) * 2
                }
                + d.milestone
                    .as_ref()
                    .map(|m| "  ·  milestone: ".len() + m.chars().count())
                    .unwrap_or(0);
            let fixed = spans
                .iter()
                .map(|s| s.content.chars().count())
                .sum::<usize>();
            let avail = area.width as usize;
            let title_budget = avail
                .saturating_sub(fixed + meta_estimate + 4)
                .max(10);
            spans.push(Span::styled(
                fit_cell(&d.title, title_budget),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw("    "));

            let dim = Style::default().fg(theme.detail_label_fg);
            spans.push(Span::styled("opened by ".to_string(), dim));
            if avatars_on {
                let author_col: u16 = spans
                    .iter()
                    .map(|s| s.content.chars().count() as u16)
                    .sum();
                spans.push(Span::raw("   "));
                avatar_paints.push((d.author.clone(), author_col));
            }
            spans.push(Span::styled(
                d.author.clone(),
                Style::default().fg(theme.list_name_fg),
            ));
            spans.push(Span::styled(" · ".to_string(), dim));
            // Date matches the commit-list date colour — keeps the
            // visual language consistent across views.
            spans.push(Span::styled(
                d.opened_when.clone(),
                Style::default().fg(theme.list_date_fg),
            ));

            if !d.assignees.is_empty() {
                spans.push(Span::styled("  ·  assigned: ".to_string(), dim));
                for (i, a) in d.assignees.iter().enumerate() {
                    if i > 0 {
                        spans.push(Span::styled(", ".to_string(), dim));
                    }
                    if avatars_on {
                        let col: u16 = spans
                            .iter()
                            .map(|s| s.content.chars().count() as u16)
                            .sum();
                        spans.push(Span::raw("   "));
                        avatar_paints.push((a.clone(), col));
                    }
                    spans.push(Span::styled(
                        a.clone(),
                        Style::default().fg(theme.list_name_fg),
                    ));
                }
            }
            if let Some(ms) = &d.milestone {
                spans.push(Span::styled("  ·  milestone: ".to_string(), dim));
                spans.push(Span::styled(ms.clone(), dim));
            }
        } else {
            spans.push(Span::styled(
                format!("#{}", self.opened_issue_number.unwrap_or(0)),
                Style::default().fg(theme.detail_label_fg),
            ));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
        let theme_bg = self.ctx.color_theme.bg;
        for (login, col) in avatar_paints {
            self.pending_avatar_paints.push((
                crate::view::pr::PaintedAvatar {
                    login,
                    screen_x: area.x + col,
                    screen_y: area.y,
                    is_selected: false,
                },
                theme_bg,
            ));
        }
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
        // Per-tab counts only render once the issue detail (and the
        // tab's own data) has loaded — empty counters would lie
        // about the state of an in-flight tab.
        let detail = self.detail_cache.get(&number).cloned();
        let convo_count = detail.as_ref().map(|d| d.conversation.len() + 1);
        let timeline_count = self.timeline_cache.get(&number).map(|v| v.len());
        // References = forward (local #N scan) + backward (timeline
        // cross-references), so the count has to come from the
        // combined collector — not just the API-cached LinkedPr
        // list, which only covers what GitHub search managed to
        // index. Only show the number once the issue detail itself
        // has loaded (otherwise scanning the empty body is just 0).
        let references_count: Option<usize> = if detail.is_some() {
            Some(self.collect_references(number).len())
        } else {
            None
        };

        let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
        let mut cursor_x: u16 = area.x + 2;
        let tabs = Tab::all();
        for (i, tab) in tabs.iter().enumerate() {
            let is_active = *tab == self.active_tab;
            let is_hovered = self.hovered_tab == Some(*tab) && !is_active;
            let count = match tab {
                Tab::Conversation => convo_count,
                Tab::Timeline => timeline_count,
                Tab::References => references_count,
            };
            // Label width matches the visible glyphs exactly — no
            // padding spaces around it, so the UNDERLINED modifier
            // stays glued to the word.
            let label = match count {
                Some(c) => format!("{} {}", tab.label(), c),
                None => tab.label().to_string(),
            };
            let fg = if is_active {
                theme.list_head_fg
            } else if is_hovered {
                theme.fg
            } else {
                theme.detail_label_fg
            };
            let mut style = Style::default().fg(fg).add_modifier(Modifier::BOLD);
            if is_active {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            let width = label.chars().count() as u16;
            let rect = Rect {
                x: cursor_x,
                y: area.y,
                width,
                height: 1,
            };
            self.tab_bar_rects.push((*tab, rect));
            spans.push(Span::styled(label, style));
            cursor_x += width;
            if i + 1 < tabs.len() {
                spans.push(Span::raw("    "));
                cursor_x += 4;
            }
        }
        let divider = Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(theme.divider_fg),
        ));
        f.render_widget(
            Paragraph::new(vec![Line::from(spans), divider]),
            area,
        );
    }

    fn render_tab_conversation(&mut self, f: &mut Frame, area: Rect, detail: &IssueDetail) {
        let theme = &self.ctx.color_theme;
        let avatars_on = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        let mut lines: Vec<Line<'static>> = Vec::new();
        self.conversation_comment_spans.clear();
        // Avatar slots are populated as we push cards; cleared each
        // frame so stale entries from a previous detail don't paint
        // over the new layout.
        self.conversation_avatar_slots.clear();
        let me_login = self.me_login.clone();

        let is_me_top = me_login
            .as_deref()
            .map_or(false, |me| me == detail.author);
        let idx0_first = lines.len();
        // Pre-compute whether the body has at least one resolvable
        // `#N` ref so we can advertise the `↵:open ref` shortcut on the
        // selected card.
        let issue_set: rustc_hash::FxHashSet<u64> =
            self.items.iter().map(|i| i.number).collect();
        let pr_set: rustc_hash::FxHashSet<u64> =
            self.mention_pr_cache.iter().map(|p| p.number).collect();
        let body_has_ref = extract_hash_refs(&detail.body)
            .into_iter()
            .any(|n| n != detail.number && (issue_set.contains(&n) || pr_set.contains(&n)));
        let mut top_shortcuts: Vec<&'static str> = Vec::new();
        if self.conversation_selected == 0 {
            // Issue body card: quote-reply only (no in-place edit
            // since the issue body itself isn't editable from this
            // surface), plus react.
            top_shortcuts.push("R:quote reply");
            top_shortcuts.push("+:react");
            if body_has_ref {
                top_shortcuts.push("↵:open ref");
            }
        }
        let top_layout = push_comment_card(
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
                avatar_login: if avatars_on {
                    Some(&detail.author)
                } else {
                    None
                },
            },
        );
        if let Some(col) = top_layout.avatar_slot {
            self.conversation_avatar_slots.push(AvatarSlot {
                login: detail.author.clone(),
                line: top_layout.top_line,
                col,
                is_selected: self.conversation_selected == 0,
            });
        }
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
                    // Issue comments are flat (no `in_reply_to_id`),
                    // so every reply path is a quote-reply, never an
                    // inline reply — label accordingly.
                    shortcuts.push("R:quote reply");
                    shortcuts.push("+:react");
                    let has_ref = extract_hash_refs(&c.body).into_iter().any(|n| {
                        n != detail.number
                            && (issue_set.contains(&n) || pr_set.contains(&n))
                    });
                    if has_ref {
                        shortcuts.push("↵:open ref");
                    }
                    if is_me {
                        shortcuts.push("e:edit");
                        shortcuts.push("d:delete");
                    }
                }
                let layout = push_comment_card(
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
                        avatar_login: if avatars_on {
                            Some(&c.author)
                        } else {
                            None
                        },
                    },
                );
                if let Some(col) = layout.avatar_slot {
                    self.conversation_avatar_slots.push(AvatarSlot {
                        login: c.author.clone(),
                        line: layout.top_line,
                        col,
                        is_selected: selected,
                    });
                }
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
        // Post-process every line to recolour `#N` references that
        // resolve to a known issue or PR in our local caches and
        // also capture the click hit-boxes — GitHub web renders
        // these as underlined hyperlinks.
        let issue_set: rustc_hash::FxHashSet<u64> =
            self.items.iter().map(|i| i.number).collect();
        let pr_set: rustc_hash::FxHashSet<u64> =
            self.mention_pr_cache.iter().map(|p| p.number).collect();
        let issue_fg: Color = self.ctx.color_theme.status_success_fg;
        let pr_fg: Color = self.ctx.color_theme.list_hash_fg;
        self.conversation_ref_links.clear();
        let mut links: Vec<RefLink> = Vec::new();
        let lines: Vec<Line<'static>> = lines
            .into_iter()
            .enumerate()
            .map(|(line_idx, line)| {
                restyle_and_track_hash_refs(
                    line_idx,
                    line,
                    &|n| {
                        if pr_set.contains(&n) {
                            Some((pr_fg, true))
                        } else if issue_set.contains(&n) {
                            Some((issue_fg, false))
                        } else {
                            None
                        }
                    },
                    &mut links,
                )
            })
            .collect();
        self.conversation_ref_links = links;

        let scroll = self.conversation_scroll.min(lines.len().saturating_sub(1));
        let para = Paragraph::new(lines).scroll((scroll as u16, 0));
        f.render_widget(para, area);

        // Push conversation avatars into the per-frame accumulator.
        let slots = std::mem::take(&mut self.conversation_avatar_slots);
        let theme_bg = self.ctx.color_theme.bg;
        for slot in &slots {
            if slot.line < scroll {
                continue;
            }
            let row_in_area = (slot.line - scroll) as u16;
            if row_in_area >= area.height {
                continue;
            }
            let screen_y = area.y + row_in_area;
            let screen_x = area.x + slot.col;
            self.pending_avatar_paints.push((
                crate::view::pr::PaintedAvatar {
                    login: slot.login.clone(),
                    screen_x,
                    screen_y,
                    is_selected: false,
                },
                theme_bg,
            ));
        }
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
        // `(login, col, row)` triples — the timeline lays out events
        // top-down, one per row, so row == index into `events`.
        let avatars_on = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        let mut paints: Vec<(String, u16, u16)> = Vec::new();
        for (row, ev) in events.iter().enumerate() {
            let (line, actor_col) = timeline_event_line(theme, ev, avatars_on);
            if let (Some(col), Some(actor)) = (actor_col, &ev.actor) {
                if !actor.is_empty() {
                    paints.push((actor.clone(), col, row as u16));
                }
            }
            lines.push(line);
        }
        f.render_widget(Paragraph::new(lines), area);
        let theme_bg = self.ctx.color_theme.bg;
        for (login, col, row) in paints {
            if row >= area.height {
                continue;
            }
            self.pending_avatar_paints.push((
                crate::view::pr::PaintedAvatar {
                    login,
                    screen_x: area.x + col,
                    screen_y: area.y + row,
                    is_selected: false,
                },
                theme_bg,
            ));
        }
    }

    /// Collect every #N reference touching this issue, both
    /// directions: forward (this issue mentions #N) and backward
    /// (something else mentioned this issue → timeline cross-ref).
    /// Deduplicated by (number, is_pr). Returned in a stable order:
    /// references first sorted by number descending.
    fn collect_references(&self, number: u64) -> Vec<ReferenceRow> {
        let mut seen: rustc_hash::FxHashSet<(u64, bool)> =
            rustc_hash::FxHashSet::default();
        let mut out: Vec<ReferenceRow> = Vec::new();

        // Forward refs: parse the issue body + every comment for
        // `#N` patterns, then resolve against local caches.
        if let Some(detail) = self.detail_cache.get(&number) {
            let mut hay = String::new();
            hay.push_str(&detail.body);
            hay.push('\n');
            for c in &detail.conversation {
                hay.push_str(&c.body);
                hay.push('\n');
            }
            for n in extract_hash_refs(&hay) {
                if n == number {
                    continue;
                }
                if let Some(pr) = self.mention_pr_cache.iter().find(|p| p.number == n) {
                    if seen.insert((n, true)) {
                        out.push(ReferenceRow {
                            number: n,
                            title: pr.title.clone(),
                            is_pr: true,
                            state_label: match pr.state {
                                crate::github::pr::PullState::Open => "OPEN".into(),
                                crate::github::pr::PullState::Closed => "CLOSED".into(),
                                crate::github::pr::PullState::Merged => "MERGED".into(),
                            },
                            direction: RefDirection::Forward,
                        });
                    }
                } else if let Some(iss) = self.items.iter().find(|i| i.number == n) {
                    if seen.insert((n, false)) {
                        out.push(ReferenceRow {
                            number: n,
                            title: iss.title.clone(),
                            is_pr: false,
                            state_label: match iss.state {
                                IssueState::Open => "OPEN".into(),
                                IssueState::Closed => "CLOSED".into(),
                            },
                            direction: RefDirection::Forward,
                        });
                    }
                }
            }
        }

        // Backward refs: timeline cross-referenced events whose
        // source is an issue/PR mentioning the current one. The
        // `pull_request` flag inside the source payload distinguishes
        // PRs from issues — but the github::issue layer flattens
        // this away. As a heuristic, look up the number in our
        // caches (PR cache wins).
        if let Some(events) = self.timeline_cache.get(&number) {
            for ev in events {
                if let TimelineKind::CrossReferenced {
                    issue_number,
                    title,
                } = &ev.kind
                {
                    let Some(n) = issue_number else { continue };
                    if *n == number {
                        continue;
                    }
                    let is_pr =
                        self.mention_pr_cache.iter().any(|p| p.number == *n);
                    if seen.insert((*n, is_pr)) {
                        let state_label = if is_pr {
                            self.mention_pr_cache
                                .iter()
                                .find(|p| p.number == *n)
                                .map(|p| match p.state {
                                    crate::github::pr::PullState::Open => "OPEN".into(),
                                    crate::github::pr::PullState::Closed => "CLOSED".into(),
                                    crate::github::pr::PullState::Merged => "MERGED".into(),
                                })
                                .unwrap_or_else(|| "?".into())
                        } else {
                            self.items
                                .iter()
                                .find(|i| i.number == *n)
                                .map(|i| match i.state {
                                    IssueState::Open => "OPEN".into(),
                                    IssueState::Closed => "CLOSED".into(),
                                })
                                .unwrap_or_else(|| "?".into())
                        };
                        out.push(ReferenceRow {
                            number: *n,
                            title: title.clone().unwrap_or_default(),
                            is_pr,
                            state_label,
                            direction: RefDirection::Backward,
                        });
                    }
                }
            }
        }

        out.sort_by(|a, b| b.number.cmp(&a.number));
        out
    }

    fn render_tab_references(&mut self, f: &mut Frame, area: Rect, number: u64) {
        let theme = &self.ctx.color_theme;
        let refs = self.collect_references(number);

        if refs.is_empty() {
            let label = if self.loading_linked_for == Some(number)
                || self.loading_timeline_for == Some(number)
            {
                "  Loading references…"
            } else {
                "  No references — write `#N` in a comment to link an issue or PR."
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    label,
                    Style::default().fg(theme.detail_label_fg),
                ))),
                area,
            );
            // Clean up stale state so a click stops triggering on
            // last-frame rects.
            self.references_row_rects.clear();
            self.references_count = 0;
            return;
        }

        // Clamp selection inside the new list.
        if self.references_hovered >= refs.len() {
            self.references_hovered = refs.len().saturating_sub(1);
        }
        self.references_count = refs.len();

        let max_num_w = refs.iter().map(|r| digits_count(r.number)).max().unwrap_or(1);
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut row_rects: Vec<Rect> = Vec::new();
        for (i, r) in refs.iter().enumerate() {
            let is_sel = i == self.references_hovered;
            let kind_fg = if r.is_pr {
                theme.list_hash_fg
            } else {
                theme.status_success_fg
            };
            let state_fg = match r.state_label.as_str() {
                "OPEN" => theme.status_success_fg,
                "CLOSED" => theme.status_error_fg,
                "MERGED" => MERGED_PURPLE,
                _ => theme.detail_label_fg,
            };
            let dir_glyph = match r.direction {
                RefDirection::Forward => "→",
                RefDirection::Backward => "←",
            };
            let dir_tooltip = match r.direction {
                // Direction is informational: forward = the active
                // issue references this row; backward = this row
                // referenced the active issue.
                RefDirection::Forward => "refs",
                RefDirection::Backward => "ref'd",
            };
            let mut spans: Vec<Span<'static>> = vec![
                Span::styled(
                    if is_sel { "▶ " } else { "  " },
                    Style::default().fg(theme.list_head_fg),
                ),
                Span::styled(
                    format!(" {} ", dir_glyph),
                    Style::default().fg(theme.detail_label_fg),
                ),
                Span::styled(
                    if r.is_pr { "PR  " } else { "ISS " },
                    Style::default().fg(kind_fg).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:>w$}", r.state_label, w = 6),
                    Style::default().fg(state_fg).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("#{:width$}", r.number, width = max_num_w),
                    Style::default()
                        .fg(theme.list_hash_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
            ];
            let fixed = spans.iter().map(|s| s.content.chars().count()).sum::<usize>()
                + dir_tooltip.chars().count()
                + 4;
            let title_budget = (area.width as usize).saturating_sub(fixed).max(10);
            spans.push(Span::styled(
                fit_cell(&r.title, title_budget),
                Style::default()
                    .fg(theme.fg)
                    .add_modifier(if is_sel { Modifier::BOLD } else { Modifier::empty() }),
            ));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                dir_tooltip.to_string(),
                Style::default()
                    .fg(theme.detail_label_fg)
                    .add_modifier(Modifier::DIM),
            ));
            // Selection bg painted manually per-span so the leading
            // markers still get their own fg.
            if is_sel {
                let sel_bg = theme.list_selected_bg;
                for span in spans.iter_mut() {
                    if span.style.bg.is_none() {
                        span.style = span.style.bg(sel_bg);
                    }
                }
            }
            lines.push(Line::from(spans));
            row_rects.push(Rect::new(area.x, area.y + i as u16, area.width, 1));
        }
        f.render_widget(Paragraph::new(lines), area);
        self.references_row_rects = row_rects;
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
        let mention_fg = theme.list_hash_fg;
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
                .map(|l| editor_line_with_mentions(l, value, mention_fg))
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
        // Read picker fields up-front so we can call view helpers
        // afterwards without holding a `&mut` borrow on `self`.
        let target_idx = {
            let Some(picker) = self.reaction_picker.as_ref() else {
                return;
            };
            picker.target_idx
        };
        let mine: Vec<crate::github::pr::ReactionKind> =
            self.viewer_reactions_for(target_idx);
        let theme = &self.ctx.color_theme;
        let kinds = crate::github::pr::ReactionKind::all();
        // Pick the worst-case visible width across all eight emoji
        // so every cell is laid out to the same column budget —
        // some glyphs (notably 🚀) measure as 1 cell in some
        // terminals and 2 in others, which breaks alignment when
        // we hard-code a single width.
        // Render the whole row in ONE Paragraph instead of one
        // widget per cell — ratatui's double-width "continuation"
        // markers carry across spans within a single Paragraph, so
        // glyphs no longer overwrite each other. Cell hit-test rects
        // are computed math-side from cell_w.
        let cell_w: u16 = 5;
        let Some(picker) = self.reaction_picker.as_mut() else {
            return;
        };
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
        // Manual buffer-level rendering so we can force the "skip
        // continuation" flag on every emoji cell — unicode-width
        // measures ❤️ as 1 col but terminals draw it as 2, and
        // without manual skip the next cell's space overwrites the
        // emoji's 2nd visual half (which was making 🚀 vanish).
        let buf = f.buffer_mut();
        for (i, kind) in kinds.iter().enumerate() {
            let is_selected = i == picker.hovered;
            let is_mine = mine.contains(kind);
            let cell_rect = Rect::new(x_cursor, inner.y, cell_w, 1);
            picker.row_rects.push(cell_rect);
            // A "mine" emoji always stays red — when also hovered
            // we use a deeper red so the cursor position still reads
            // distinctly without losing the "you reacted with this"
            // signal.
            const MINE_RED: Color = Color::Rgb(0xB0, 0x32, 0x32);
            const MINE_HOVER_RED: Color = Color::Rgb(0x7A, 0x1F, 0x1F);
            let (bg, fg, modif) = if is_selected && is_mine {
                (MINE_HOVER_RED, theme.fg, Modifier::BOLD)
            } else if is_selected {
                (theme.list_selected_bg, theme.fg, Modifier::BOLD)
            } else if is_mine {
                (MINE_RED, theme.fg, Modifier::BOLD)
            } else {
                (theme.bg, theme.fg, Modifier::empty())
            };
            let cell_style = Style::default().fg(fg).bg(bg).add_modifier(modif);
            // Layout: ` E_ _ ` where E spans cols 1-2 (emoji is 2
            // cells visually), cols 0, 3, 4 hold padding spaces. The
            // continuation cell at col 2 is explicitly skipped so
            // ratatui's renderer doesn't overwrite the emoji's
            // second visual half.
            for col in 0..cell_w {
                let abs_x = x_cursor + col;
                if abs_x >= buf.area.x + buf.area.width {
                    break;
                }
                let cell = &mut buf[(abs_x, inner.y)];
                cell.reset();
                cell.set_symbol(" ");
                cell.set_style(cell_style);
                cell.set_skip(false);
            }
            if cell_w >= 3 && x_cursor + 1 < buf.area.x + buf.area.width {
                let emoji_cell = &mut buf[(x_cursor + 1, inner.y)];
                emoji_cell.set_symbol(kind.emoji());
                emoji_cell.set_style(cell_style);
                if x_cursor + 2 < buf.area.x + buf.area.width {
                    let cont_cell = &mut buf[(x_cursor + 2, inner.y)];
                    cont_cell.set_symbol("");
                    cont_cell.set_skip(true);
                    cont_cell.set_style(cell_style);
                }
            }
            x_cursor = x_cursor.saturating_add(cell_w);
        }
    }

    /// Floating `#` mention popup. Sits just above the editor (or
    /// below, when there's no room above), 1 row per candidate +
    /// borders. Shows up to 8 issues / PRs matching the query.
    fn render_mention_popup(&mut self, f: &mut Frame, area: Rect) {
        let editor_rect = self.editor_body_area.unwrap_or(area);
        let theme = self.ctx.color_theme.clone();
        if let Some(popup) = self.mention_popup.as_mut() {
            paint_mention_popup(f, area, editor_rect, popup, &theme);
        }
    }

    #[allow(dead_code)]
    fn render_mention_popup_legacy(&mut self, f: &mut Frame, area: Rect) {
        let Some(popup) = self.mention_popup.as_ref() else {
            return;
        };
        if popup.filtered.is_empty() {
            return;
        }
        let theme = &self.ctx.color_theme;
        // Cap visible rows so a giant repo doesn't blow the popup
        // off-screen. Scroll handles the rest.
        const MAX_VISIBLE: u16 = 8;
        let rows = (popup.filtered.len() as u16).min(MAX_VISIBLE);
        let height = rows + 2; // borders
        // Width budget: 4 cells for the kind + " #N  " + truncated
        // title. Cap at 60 to keep the popup compact.
        let max_title_w: u16 = popup
            .filtered
            .iter()
            .map(|m| m.title.chars().count() as u16)
            .max()
            .unwrap_or(20);
        let widest_num = popup
            .filtered
            .iter()
            .map(|m| (m.number.to_string().chars().count() + 1) as u16)
            .max()
            .unwrap_or(3);
        let want_width =
            2 + 5 /*[ISS]/[PR ]*/ + 1 + widest_num + 2 + max_title_w + 2 + 2;
        let width = want_width.min(60).min(area.width.saturating_sub(2));
        // Anchor near the editor. Without a precise cursor rect
        // exposed here, place the popup centered horizontally over
        // the editor area but vertically above the editor body so
        // the user can still see what they're typing.
        let editor_rect = self.editor_body_area.unwrap_or(area);
        let x = editor_rect.x.saturating_add(2).min(
            area.x + area.width.saturating_sub(width),
        );
        // Prefer ABOVE the editor — `editor_rect.y` minus our
        // height. Fall back to BELOW if there's no room above.
        let y = if editor_rect.y >= height {
            editor_rect.y - height
        } else {
            (editor_rect.y + editor_rect.height).min(
                area.y + area.height.saturating_sub(height),
            )
        };
        let rect = Rect::new(x, y, width, height);
        f.render_widget(ratatui::widgets::Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.list_head_fg))
            .title(Line::from(Span::styled(
                " Mention ".to_string(),
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(rect);
        f.render_widget(block, rect);

        // Reset stored hit-test rects + the overlay rect so a click
        // outside the popup doesn't accidentally hit a stale entry.
        let mut row_rects: Vec<Rect> = Vec::new();
        let scroll = popup.scroll.min(popup.filtered.len().saturating_sub(1));
        let visible = rows as usize;
        for (visible_idx, (i, item)) in popup
            .filtered
            .iter()
            .enumerate()
            .skip(scroll)
            .take(visible)
            .enumerate()
        {
            let row_rect = Rect::new(
                inner.x,
                inner.y + visible_idx as u16,
                inner.width,
                1,
            );
            // Stored at logical index `i` so the click handler can
            // still map a clicked rect to the underlying item even
            // when scrolled. We pad earlier rects with zero-size to
            // keep `row_rects[i]` aligned.
            while row_rects.len() < i {
                row_rects.push(Rect::new(0, 0, 0, 0));
            }
            row_rects.push(row_rect);
            let is_hovered = i == popup.hovered;
            let (kind_label, kind_fg) = match item.kind {
                MentionKind::Issue => ("ISS", theme.status_success_fg),
                MentionKind::Pr => ("PR ", theme.list_hash_fg),
            };
            let bg = if is_hovered {
                theme.list_selected_bg
            } else {
                theme.bg
            };
            let num = format!("#{}", item.number);
            // fit_cell-style truncation on title to keep the row
            // inside `inner.width`.
            let used = 1 + 3 + 1 + num.chars().count() + 2;
            let title_budget = (inner.width as usize).saturating_sub(used + 2).max(4);
            let title = fit_cell(&item.title, title_budget);
            let spans = vec![
                Span::styled(
                    " ",
                    Style::default().bg(bg),
                ),
                Span::styled(
                    kind_label.to_string(),
                    Style::default().fg(kind_fg).bg(bg).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    " ",
                    Style::default().bg(bg),
                ),
                Span::styled(
                    num,
                    Style::default()
                        .fg(theme.list_hash_fg)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "  ".to_string(),
                    Style::default().bg(bg),
                ),
                Span::styled(
                    title,
                    Style::default().fg(theme.fg).bg(bg),
                ),
            ];
            f.render_widget(
                Paragraph::new(Line::from(spans)).style(Style::default().bg(bg)),
                row_rect,
            );
        }
        if let Some(p) = self.mention_popup.as_mut() {
            p.overlay_rect = Some(rect);
            p.row_rects = row_rects;
            p.last_visible = rows;
        }
    }

    /// Full-screen "Create new Issue" form — mirrors the PR
    /// `render_compose_mode` layout exactly so the two pages share
    /// the same visual rhythm (sub-header / divider / single-pane
    /// Compose block, with each field aligned via `Label: value`).
    fn render_compose(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        self.compose_field_rects.clear();

        // Slim sub-header (1 row) + divider (1) + compose body — same
        // structure as PR compose. The `NEW · Create new Issue` line
        // hints that this is a write surface even with the standard
        // top header still visible above.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(area);
        let sub_header_area = chunks[0];
        let divider_area = chunks[1];
        let body_area = chunks[2];

        let sub_header = Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "NEW".to_string(),
                Style::default()
                    .fg(theme.status_success_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "Create new Issue".to_string(),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
        ]);
        f.render_widget(Paragraph::new(sub_header), sub_header_area);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "─".repeat(divider_area.width as usize),
                Style::default().fg(theme.divider_fg),
            ))),
            divider_area,
        );

        // ── Compose block (full width — no preview pane since there's
        //    no head/base diff to show like the PR view). Same Block
        //    chrome as PR's compose form for visual parity.
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    "Compose".to_string(),
                    Style::default()
                        .fg(theme.list_head_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ]));
        let inner = block.inner(body_area);
        f.render_widget(block, body_area);

        let Some(state) = self.compose.as_ref().cloned() else {
            return;
        };

        const INDENT: u16 = 2;
        const LABEL_WIDTH: u16 = 11; // "Assignees: " etc.

        let label_style = Style::default().fg(theme.detail_label_fg);
        let focus_indicator = |focused: bool| {
            if focused {
                Span::styled(
                    "▸ ".to_string(),
                    Style::default()
                        .fg(theme.list_head_fg)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw("  ".to_string())
            }
        };

        // Generic single-row Label:value render. Used for Title,
        // Labels, Assignees, Milestone. Returns (rect, value_start_x).
        let render_row =
            |f: &mut Frame, y: u16, focused: bool, label: &'static str, value: Vec<Span<'static>>| -> (Rect, u16) {
                let rect = Rect::new(inner.x, y, inner.width, 1);
                let value_x = inner.x + INDENT + 2 + LABEL_WIDTH + 1;
                let mut spans = vec![
                    Span::raw(" ".repeat(INDENT as usize)),
                    focus_indicator(focused),
                    Span::styled(
                        format!("{:<w$}", label, w = LABEL_WIDTH as usize),
                        label_style,
                    ),
                    Span::raw(" "),
                ];
                spans.extend(value);
                f.render_widget(Paragraph::new(Line::from(spans)), rect);
                (rect, value_x)
            };

        let mut y = inner.y;

        // ── Title row — text input with focus-only underline.
        let title_focused = matches!(state.field, ComposeField::Title);
        let title_value: Vec<Span<'static>> = if state.title.is_empty() {
            vec![Span::styled(
                "(type a title — required)".to_string(),
                Style::default().fg(theme.detail_label_fg),
            )]
        } else {
            vec![Span::styled(
                state.title.clone(),
                Style::default().fg(theme.fg),
            )]
        };
        let (title_rect, title_value_x) =
            render_row(f, y, title_focused, "Title:", title_value);
        self.compose_field_rects
            .push((ComposeField::Title, title_rect));
        // Underline under the typed text when focused — same trick
        // PR uses for its title input.
        if title_focused {
            let underline_y = (title_rect.y + 1).min(inner.y + inner.height - 1);
            let underline_w = inner
                .width
                .saturating_sub(INDENT + 2 + LABEL_WIDTH + 1 + 2);
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "─".repeat(underline_w as usize),
                    Style::default().fg(theme.list_head_fg),
                ))),
                Rect::new(title_value_x, underline_y, underline_w, 1),
            );
        }
        y += 2;

        // ── Labels row — chips inline.
        let labels_focused = matches!(state.field, ComposeField::Labels);
        let labels_value: Vec<Span<'static>> = if state.labels.is_empty() {
            vec![Span::styled(
                "(none — Enter to pick)".to_string(),
                Style::default().fg(theme.detail_label_fg),
            )]
        } else {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (i, name) in state.labels.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::raw(" "));
                }
                let label = state
                    .available_labels
                    .iter()
                    .find(|l| &l.name == name)
                    .cloned()
                    .unwrap_or_else(|| crate::github::pr::Label {
                        name: name.clone(),
                        color: None,
                        description: None,
                    });
                spans.extend(label_chip_spans_local(&label));
            }
            spans
        };
        let (labels_rect, _) = render_row(f, y, labels_focused, "Labels:", labels_value);
        self.compose_field_rects
            .push((ComposeField::Labels, labels_rect));
        y += 2;

        // ── Assignees row — comma-joined logins, with avatars when
        //    enabled. Avatar painted as a post-pass overlay.
        let assignees_focused = matches!(state.field, ComposeField::Assignees);
        let avatars_on = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        let mut assignee_paints: Vec<(String, u16)> = Vec::new();
        let assignees_value: Vec<Span<'static>> = if state.assignees.is_empty() {
            vec![Span::styled(
                "(none — Enter to pick)".to_string(),
                Style::default().fg(theme.detail_label_fg),
            )]
        } else {
            let mut spans: Vec<Span<'static>> = Vec::new();
            // Track running col offset inside the VALUE column so
            // avatar paints land in the right cell.
            let value_x = inner.x + INDENT + 2 + LABEL_WIDTH + 1;
            let mut col_in_value: u16 = 0;
            for (i, a) in state.assignees.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(
                        ", ".to_string(),
                        Style::default().fg(theme.detail_label_fg),
                    ));
                    col_in_value = col_in_value.saturating_add(2);
                }
                if avatars_on {
                    spans.push(Span::raw("   "));
                    assignee_paints.push((a.clone(), value_x + col_in_value));
                    col_in_value = col_in_value.saturating_add(3);
                }
                spans.push(Span::styled(
                    a.clone(),
                    Style::default().fg(theme.list_name_fg),
                ));
                col_in_value =
                    col_in_value.saturating_add(a.chars().count() as u16);
            }
            spans
        };
        let (assignees_rect, _) =
            render_row(f, y, assignees_focused, "Assignees:", assignees_value);
        self.compose_field_rects
            .push((ComposeField::Assignees, assignees_rect));
        // Avatars overlay — diff-tracked via the view-wide
        // accumulator (flushed at the end of `render()`).
        let theme_bg = theme.bg;
        for (login, x) in assignee_paints {
            self.pending_avatar_paints.push((
                crate::view::pr::PaintedAvatar {
                    login,
                    screen_x: x,
                    screen_y: assignees_rect.y,
                    is_selected: false,
                },
                theme_bg,
            ));
        }
        y += 2;

        // ── Milestone row — single selected value or "(none)".
        let milestone_focused = matches!(state.field, ComposeField::Milestone);
        let milestone_value: Vec<Span<'static>> = match state.milestone {
            Some(n) => match state
                .available_milestones
                .iter()
                .find(|m| m.number == n)
            {
                Some(m) => vec![Span::styled(
                    m.title.clone(),
                    Style::default().fg(theme.fg),
                )],
                None => vec![Span::styled(
                    format!("#{}", n),
                    Style::default().fg(theme.detail_label_fg),
                )],
            },
            None => vec![Span::styled(
                "(none — Enter to pick)".to_string(),
                Style::default().fg(theme.detail_label_fg),
            )],
        };
        let (milestone_rect, _) =
            render_row(f, y, milestone_focused, "Milestone:", milestone_value);
        self.compose_field_rects
            .push((ComposeField::Milestone, milestone_rect));
        y += 2;

        // ── Body label + bordered text area beneath, exactly like
        //    PR's compose body. Fills the remaining vertical space.
        let body_focused = matches!(state.field, ComposeField::Body);
        let body_label_line = Line::from(vec![
            Span::raw(" ".repeat(INDENT as usize)),
            focus_indicator(body_focused),
            Span::styled("Body:".to_string(), label_style),
        ]);
        f.render_widget(
            Paragraph::new(body_label_line),
            Rect::new(inner.x, y, inner.width, 1),
        );
        y += 1;
        let body_block_height = inner
            .y
            .saturating_add(inner.height)
            .saturating_sub(y)
            .max(3);
        let body_block_rect = Rect::new(
            inner.x + INDENT + 2,
            y,
            inner.width.saturating_sub(INDENT + 2 + 2),
            body_block_height,
        );
        let body_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if body_focused {
                theme.list_head_fg
            } else {
                theme.divider_fg
            }));
        let body_inner = body_block.inner(body_block_rect);
        f.render_widget(body_block, body_block_rect);
        self.compose_field_rects
            .push((ComposeField::Body, body_block_rect));
        if body_inner.height == 0 || body_inner.width == 0 {
            return;
        }
        let (cursor_col, cursor_row) = cursor_screen_pos(&state.body, state.body_cursor);
        let total_rows = if state.body.is_empty() {
            1
        } else {
            state.body.matches('\n').count() as u16 + 1
        };
        // Render-time scroll handling: ONLY clamp against the
        // current max — do NOT re-anchor on the cursor here. The
        // anchor lives in `compose_body_anchor_to_cursor` and is
        // called by cursor-mutating actions only. Re-anchoring at
        // render time would snap `body_scroll` back to the cursor
        // on every frame, undoing any mouse-wheel scroll the user
        // just did and making hover feel like it scrolls to the
        // bottom.
        let _ = cursor_row;
        let mut scroll = state.body_scroll;
        let max_scroll = total_rows.saturating_sub(body_inner.height);
        if scroll > max_scroll {
            scroll = max_scroll;
        }
        let body_str = state.body.clone();
        if let Some(cc) = self.compose.as_mut() {
            cc.body_scroll = scroll;
            cc.body_last_height = body_inner.height;
        }
        let base_style = Style::default().fg(theme.fg);
        let mention_fg = theme.list_hash_fg;
        let body_text: Vec<Line<'static>> = if body_str.is_empty() {
            vec![Line::from(Span::styled(
                "(type a description — supports markdown)".to_string(),
                Style::default().fg(theme.detail_label_fg),
            ))]
        } else {
            body_str
                .split('\n')
                .skip(scroll as usize)
                .take(body_inner.height as usize)
                .map(|l| editor_line_with_mentions(l, base_style, mention_fg))
                .collect()
        };
        f.render_widget(Paragraph::new(body_text), body_inner);

        // ── Cursor on the active text field ────────────────────────
        match state.field {
            ComposeField::Title => {
                let cx = title_value_x
                    + state.title[..state.title_cursor.min(state.title.len())]
                        .chars()
                        .count() as u16;
                if cx < inner.x + inner.width {
                    self.comment_editor_cursor_pos = Some((cx, title_rect.y));
                }
            }
            ComposeField::Body if body_focused => {
                // Only place the terminal cursor when its logical
                // row is INSIDE the visible scroll window — without
                // this guard `saturating_sub(scroll)` makes the
                // cursor visually jump to row 0 of the body whenever
                // we scroll past it, which the user perceives as
                // "the scroll moved my cursor".
                if cursor_row >= scroll
                    && cursor_row < scroll + body_inner.height
                {
                    let cx = body_inner.x + cursor_col;
                    let cy = body_inner.y + (cursor_row - scroll);
                    if cx < body_inner.x + body_inner.width {
                        self.comment_editor_cursor_pos = Some((cx, cy));
                    }
                }
            }
            _ => {}
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

/// Scan a free-form text blob for `#N` references and return the
/// list of resolved numbers (deduplicated, order-preserved). Same
/// left/right boundary rules as the comment styling — `foo#42` and
/// `#42abc` are NOT matched, only clean references.
/// Walk `count` chars forward from `start` byte offset inside `s`,
/// returning the byte offset just past the last walked char. Used
/// by the `#` autocomplete to splice in the chosen number over the
/// `#query` chunk the user typed.
fn walk_chars(s: &str, start: usize, count: usize) -> usize {
    let mut pos = start;
    let mut walked = 0;
    while pos < s.len() && walked < count {
        pos += 1;
        while pos < s.len() && !s.is_char_boundary(pos) {
            pos += 1;
        }
        walked += 1;
    }
    pos
}

pub(crate) fn extract_hash_refs(text: &str) -> Vec<u64> {
    let bytes = text.as_bytes();
    let mut out: Vec<u64> = Vec::new();
    let mut seen: rustc_hash::FxHashSet<u64> = rustc_hash::FxHashSet::default();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let left_ok = i == 0
                || matches!(
                    bytes[i - 1],
                    b' ' | b'\t' | b'\n' | b'(' | b'[' | b','
                        | b'.' | b':' | b';' | b'<' | b'>'
                );
            if left_ok {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    let right_ok =
                        j == bytes.len() || !bytes[j].is_ascii_alphanumeric();
                    if right_ok {
                        if let Ok(n) = text[i + 1..j].parse::<u64>() {
                            if seen.insert(n) {
                                out.push(n);
                            }
                        }
                        i = j;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

/// Walk a `Line`, recolour any `#N` reference resolvable via the
/// `resolver`, and append a `RefLink` to `links` for each match so
/// the caller can hit-test clicks. `line_idx` is the logical line
/// in the enclosing Paragraph buffer (pre-scroll), preserved into
/// the captured links. Recolour uses UNDERLINED + BOLD so the user
/// reads the chip as a hyperlink.
pub(crate) fn restyle_and_track_hash_refs<F>(
    line_idx: usize,
    line: Line<'static>,
    resolver: &F,
    links: &mut Vec<RefLink>,
) -> Line<'static>
where
    F: Fn(u64) -> Option<(Color, bool /* is_pr */)>,
{
    let mut out: Vec<Span<'static>> = Vec::new();
    // Track the running column position across spans so the link
    // rects line up with the rendered text.
    let mut col: u16 = 0;
    for span in line.spans {
        let span_text = span.content.to_string();
        let mut new_spans = restyle_hash_refs_in_span_collect(
            &span_text,
            span.style,
            resolver,
            line_idx,
            col,
            links,
        );
        for s in new_spans.drain(..) {
            col = col.saturating_add(s.content.chars().count() as u16);
            out.push(s);
        }
    }
    Line::from(out)
}

pub(crate) fn restyle_hash_refs_in_span_collect<F>(
    text: &str,
    original: Style,
    resolver: &F,
    line_idx: usize,
    span_col_start: u16,
    links: &mut Vec<RefLink>,
) -> Vec<Span<'static>>
where
    F: Fn(u64) -> Option<(Color, bool)>,
{
    let bytes = text.as_bytes();
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut chunk_start: usize = 0;
    let mut chunk_start_col: u16 = span_col_start;
    let mut cur_col: u16 = span_col_start;
    let mut i: usize = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            // Same boundary rules as before — left and right
            // boundaries gate clean refs.
            let left_ok = i == 0
                || matches!(
                    bytes[i - 1],
                    b' ' | b'\t' | b'\n' | b'(' | b'[' | b','
                        | b'.' | b':' | b';' | b'<' | b'>'
                );
            if left_ok {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    let right_ok =
                        j == bytes.len() || !bytes[j].is_ascii_alphanumeric();
                    if right_ok {
                        if let Ok(n) = text[i + 1..j].parse::<u64>() {
                            if let Some((color, is_pr)) = resolver(n) {
                                // Push the chunk before the match.
                                if i > chunk_start {
                                    let pre = &text[chunk_start..i];
                                    let pre_chars = pre.chars().count() as u16;
                                    out.push(Span::styled(
                                        pre.to_string(),
                                        original,
                                    ));
                                    cur_col = cur_col.saturating_add(pre_chars);
                                }
                                // Capture the link rect — col_end is
                                // exclusive so a click on the last
                                // digit still hits.
                                let ref_text = &text[i..j];
                                let ref_chars = ref_text.chars().count() as u16;
                                links.push(RefLink {
                                    line: line_idx,
                                    col_start: cur_col,
                                    col_end: cur_col + ref_chars,
                                    number: n,
                                    is_pr,
                                });
                                out.push(Span::styled(
                                    ref_text.to_string(),
                                    Style::default()
                                        .fg(color)
                                        .add_modifier(
                                            Modifier::BOLD | Modifier::UNDERLINED,
                                        ),
                                ));
                                cur_col = cur_col.saturating_add(ref_chars);
                                chunk_start = j;
                                chunk_start_col = cur_col;
                                i = j;
                                continue;
                            }
                        }
                    }
                }
            }
        }
        i += 1;
    }
    let _ = chunk_start_col;
    if chunk_start == 0 && out.is_empty() {
        return vec![Span::styled(text.to_string(), original)];
    }
    if chunk_start < bytes.len() {
        out.push(Span::styled(text[chunk_start..].to_string(), original));
    }
    out
}

/// Build a styled line for the comment / compose editor where any
/// `#N` substring is highlighted in `mention_fg` + UNDERLINED, while
/// the rest of the text uses `base`. Lighter version of
/// `restyle_hash_refs_in_span_collect` — no link tracking, no
/// resolver: it colours every well-formed `#N` because the editor
/// can't know yet if the reference will resolve.
pub(crate) fn filter_mention_items_pub(
    query: &str,
    all: &[MentionItem],
) -> Vec<MentionItem> {
    const MAX_ITEMS: usize = 10;
    if query.is_empty() {
        let mut issues: Vec<MentionItem> = all
            .iter()
            .filter(|m| matches!(m.kind, MentionKind::Issue))
            .cloned()
            .collect();
        let mut prs: Vec<MentionItem> = all
            .iter()
            .filter(|m| matches!(m.kind, MentionKind::Pr))
            .cloned()
            .collect();
        issues.sort_by(|a, b| b.number.cmp(&a.number));
        prs.sort_by(|a, b| b.number.cmp(&a.number));
        let half = MAX_ITEMS / 2;
        let mut out: Vec<MentionItem> = Vec::new();
        out.extend(issues.iter().take(half).cloned());
        out.extend(prs.iter().take(half).cloned());
        if out.len() < MAX_ITEMS {
            let need = MAX_ITEMS - out.len();
            let extra: Vec<MentionItem> = issues
                .iter()
                .chain(prs.iter())
                .filter(|m| !out.iter().any(|o| o.kind == m.kind && o.number == m.number))
                .take(need)
                .cloned()
                .collect();
            out.extend(extra);
        }
        out.sort_by(|a, b| b.number.cmp(&a.number));
        return out;
    }
    let q = query.to_lowercase();
    let qn = q.parse::<u64>().ok();
    let mut scored: Vec<(u32, &MentionItem)> = all
        .iter()
        .filter_map(|item| {
            let num_str = item.number.to_string();
            if let Some(n) = qn {
                if item.number == n {
                    return Some((200, item));
                }
            }
            if num_str.starts_with(&q) {
                return Some((100, item));
            }
            if num_str.contains(&q) {
                return Some((50, item));
            }
            let title_low = item.title.to_lowercase();
            if title_low.starts_with(&q) {
                return Some((20, item));
            }
            if title_low.contains(&q) {
                return Some((10, item));
            }
            None
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.number.cmp(&a.1.number)));
    scored
        .into_iter()
        .take(MAX_ITEMS)
        .map(|(_, m)| m.clone())
        .collect()
}

/// Render the `#` mention popup into `area`, anchored near
/// `editor_rect` (above when there's room, below otherwise).
/// Updates `popup.overlay_rect`, `popup.row_rects`, and
/// `popup.last_visible` so the caller's mouse/keyboard handlers
/// can hit-test the rendered rows. Shared between Issues and PRs.
pub(crate) fn paint_mention_popup(
    f: &mut Frame,
    area: Rect,
    editor_rect: Rect,
    popup: &mut MentionPopup,
    theme: &crate::color::ColorTheme,
) {
    if popup.filtered.is_empty() {
        return;
    }
    const MAX_VISIBLE: u16 = 8;
    let rows = (popup.filtered.len() as u16).min(MAX_VISIBLE);
    let height = rows + 2;
    let max_title_w: u16 = popup
        .filtered
        .iter()
        .map(|m| m.title.chars().count() as u16)
        .max()
        .unwrap_or(20);
    let widest_num = popup
        .filtered
        .iter()
        .map(|m| (m.number.to_string().chars().count() + 1) as u16)
        .max()
        .unwrap_or(3);
    let want_width = 2 + 5 + 1 + widest_num + 2 + max_title_w + 2 + 2;
    let width = want_width.min(60).min(area.width.saturating_sub(2));
    let x = editor_rect
        .x
        .saturating_add(2)
        .min(area.x + area.width.saturating_sub(width));
    let y = if editor_rect.y >= height {
        editor_rect.y - height
    } else {
        (editor_rect.y + editor_rect.height)
            .min(area.y + area.height.saturating_sub(height))
    };
    let rect = Rect::new(x, y, width, height);
    // Clear ONE extra column to the left of the popup before
    // drawing it: wide emojis (👍 👎 😀 …) painted on the card
    // underneath can have their anchor cell sitting at `x - 1`
    // with their continuation cell at `x`. `Clear` on the popup's
    // own rect wipes the continuation marker but leaves the
    // anchor, so the terminal still draws 2 visual cells for the
    // emoji — its 2nd cell bleeds into the popup's left border.
    // Extending the clear by 1 col wipes the anchor too.
    if x > 0 {
        let pre_rect = Rect::new(x - 1, y, 1, height);
        f.render_widget(ratatui::widgets::Clear, pre_rect);
    }
    f.render_widget(ratatui::widgets::Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.list_head_fg))
        .title(Line::from(Span::styled(
            " Mention ".to_string(),
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD),
        )));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut row_rects: Vec<Rect> = Vec::new();
    let scroll = popup.scroll.min(popup.filtered.len().saturating_sub(1));
    let visible = rows as usize;
    for (visible_idx, (i, item)) in popup
        .filtered
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .enumerate()
    {
        let row_rect = Rect::new(inner.x, inner.y + visible_idx as u16, inner.width, 1);
        while row_rects.len() < i {
            row_rects.push(Rect::new(0, 0, 0, 0));
        }
        row_rects.push(row_rect);
        let is_hovered = i == popup.hovered;
        let (kind_label, kind_fg) = match item.kind {
            MentionKind::Issue => ("ISS", theme.status_success_fg),
            MentionKind::Pr => ("PR ", theme.list_hash_fg),
        };
        let bg = if is_hovered { theme.list_selected_bg } else { theme.bg };
        let num = format!("#{}", item.number);
        let used = 1 + 3 + 1 + num.chars().count() + 2;
        let title_budget = (inner.width as usize).saturating_sub(used + 2).max(4);
        let title = fit_cell(&item.title, title_budget);
        let spans = vec![
            Span::styled(" ", Style::default().bg(bg)),
            Span::styled(
                kind_label.to_string(),
                Style::default().fg(kind_fg).bg(bg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ", Style::default().bg(bg)),
            Span::styled(
                num,
                Style::default()
                    .fg(theme.list_hash_fg)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ".to_string(), Style::default().bg(bg)),
            Span::styled(title, Style::default().fg(theme.fg).bg(bg)),
        ];
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(bg)),
            row_rect,
        );
    }
    popup.overlay_rect = Some(rect);
    popup.row_rects = row_rects;
    popup.last_visible = rows;
}

pub(crate) fn editor_line_with_mentions(
    text: &str,
    base: Style,
    mention_fg: Color,
) -> Line<'static> {
    let bytes = text.as_bytes();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut chunk_start: usize = 0;
    let mut i: usize = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let left_ok = i == 0
                || matches!(
                    bytes[i - 1],
                    b' ' | b'\t' | b'\n' | b'(' | b'[' | b','
                        | b'.' | b':' | b';' | b'<' | b'>'
                );
            if left_ok {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > i + 1 {
                    let right_ok =
                        j == bytes.len() || !bytes[j].is_ascii_alphanumeric();
                    if right_ok {
                        if i > chunk_start {
                            spans.push(Span::styled(
                                text[chunk_start..i].to_string(),
                                base,
                            ));
                        }
                        spans.push(Span::styled(
                            text[i..j].to_string(),
                            Style::default()
                                .fg(mention_fg)
                                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                        ));
                        chunk_start = j;
                        i = j;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    if chunk_start < bytes.len() {
        spans.push(Span::styled(text[chunk_start..].to_string(), base));
    }
    if spans.is_empty() {
        spans.push(Span::styled(text.to_string(), base));
    }
    Line::from(spans)
}

/// Column budgets for an aligned Issue list row — mirrors PR's
/// `PrListColumns`. Each cell is pre-padded to its column width so
/// rows align tabularly regardless of value length.
struct IssueListColumns {
    state: usize,
    number: usize,
    title: usize,
    labels: usize,
    author: usize,
}

const ISSUE_MAX_INLINE_LABELS: usize = 3;
const ISSUE_LABEL_CHIP_WIDTH: usize = 4;

impl IssueListColumns {
    const MARKER: usize = 2;
    const GAP: usize = 2;

    fn compute(items: &[Issue], available_width: usize) -> Self {
        // State column: " Open " (6) / " Closed " (8) / " Closed (np) " (~14)
        // We use compact "OPEN" / "CLOSED" / "CLOSED·" — minimal width 6.
        let state = items
            .iter()
            .map(|i| match (i.state, i.state_reason) {
                (IssueState::Closed, Some(IssueStateReason::NotPlanned)) => 8,
                (IssueState::Closed, _) => 6,
                (IssueState::Open, _) => 4,
            })
            .max()
            .unwrap_or(4)
            .max("STATE".chars().count());
        let number = items
            .iter()
            .map(|i| 1 + digits_count(i.number))
            .max()
            .unwrap_or(2);
        let author = items
            .iter()
            .map(|i| i.author.chars().count())
            .max()
            .unwrap_or(0)
            .max("AUTHOR".chars().count());
        let max_title = items
            .iter()
            .map(|i| i.title.chars().count())
            .max()
            .unwrap_or(0);
        let max_chips = items
            .iter()
            .map(|i| i.labels.len().min(ISSUE_MAX_INLINE_LABELS))
            .max()
            .unwrap_or(0);
        let labels = max_chips * ISSUE_LABEL_CHIP_WIDTH;
        let labels_gap = if labels > 0 { 1 } else { 0 };
        // 💬 N column — "💬" is 2 visual cells in most terminals; allow
        // 2 (icon) + 1 (space) + up to 4 digits = 7.
        const COMMENTS_COL: usize = 7;
        let fixed = Self::MARKER
            + state
            + Self::GAP
            + number
            + Self::GAP
            + labels_gap
            + labels
            + Self::GAP
            + author
            + Self::GAP
            + COMMENTS_COL;
        let remaining = available_width.saturating_sub(fixed);
        let title = remaining.min(max_title.max(1)).max(1);
        Self { state, number, title, labels, author }
    }
}

/// Build one aligned row for the Issue list. Returns `(line,
/// avatar_col)` — `avatar_col` is `Some(col)` only when avatars are
/// enabled (3-cell pad reserved before author). Mirrors PR's
/// `format_pr_row` including the per-span row selection bg.
fn format_issue_row(
    theme: &crate::color::ColorTheme,
    issue: &Issue,
    is_selected: bool,
    cols: &IssueListColumns,
    row_width: usize,
    avatars_on: bool,
) -> (Line<'static>, Option<u16>) {
    let (state_text, state_color) = match (issue.state, issue.state_reason) {
        (IssueState::Open, _) => ("OPEN", theme.status_success_fg),
        (IssueState::Closed, Some(IssueStateReason::NotPlanned)) => {
            ("CLOSED·NP", theme.detail_label_fg)
        }
        (IssueState::Closed, _) => ("CLOSED", MERGED_PURPLE),
    };
    let state_span = Span::styled(
        fit_cell(state_text, cols.state),
        Style::default().fg(state_color).add_modifier(Modifier::BOLD),
    );
    let marker = if is_selected {
        Span::styled(
            "▶ ",
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw("  ")
    };
    let mut row: Vec<Span<'static>> = vec![
        marker,
        state_span,
        Span::raw("  "),
        Span::styled(
            fit_cell(&format!("#{}", issue.number), cols.number),
            Style::default()
                .fg(theme.list_hash_fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            fit_cell(&issue.title, cols.title),
            Style::default().fg(theme.list_commit_message_fg),
        ),
    ];
    if cols.labels > 0 {
        row.push(Span::raw(" "));
        let mut chips_width = 0usize;
        for lab in issue.labels.iter().take(ISSUE_MAX_INLINE_LABELS) {
            row.extend(short_label_chip_spans(lab));
            chips_width += ISSUE_LABEL_CHIP_WIDTH;
        }
        if chips_width < cols.labels {
            row.push(Span::raw(" ".repeat(cols.labels - chips_width)));
        }
    }
    // Push the 2-cell pre-author gap first, then (when avatars are
    // on) reserve a 3-cell pad and record its starting column for
    // the caller to paint the avatar after the List renders.
    row.push(Span::raw("  "));
    let avatar_col: Option<u16> = if avatars_on {
        let pos: u16 = row
            .iter()
            .map(|s| s.content.chars().count() as u16)
            .sum();
        row.push(Span::raw("   "));
        Some(pos)
    } else {
        None
    };
    row.push(Span::styled(
        fit_cell(&issue.author, cols.author),
        Style::default().fg(theme.list_name_fg),
    ));
    row.push(Span::raw("  "));
    row.push(Span::styled(
        format!("💬 {}", issue.comments_count),
        Style::default().fg(theme.detail_label_fg),
    ));

    // Selection bg painted per-span — leave chips alone so their
    // colours stay intact. Trailing fill stretches the highlight to
    // the row's right edge.
    if is_selected {
        let sel_bg = theme.list_selected_bg;
        for span in row.iter_mut() {
            if span.style.bg.is_none() {
                span.style = span.style.bg(sel_bg);
            }
        }
        let content_width: usize = row
            .iter()
            .map(|s| s.content.chars().count())
            .sum();
        if content_width < row_width {
            row.push(Span::styled(
                " ".repeat(row_width - content_width),
                Style::default().bg(sel_bg),
            ));
        }
    }
    (Line::from(row), avatar_col)
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

/// Returns `(line, actor_col)` — `Some(col)` when the 3-cell pad
/// was reserved (avatars enabled), `None` otherwise so the row
/// stays tight.
fn timeline_event_line(
    theme: &crate::color::ColorTheme,
    ev: &TimelineEvent,
    avatars_on: bool,
) -> (Line<'static>, Option<u16>) {
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
    // Fixed 5-cell icon zone (`" {icon}<pad>"`) so EVERY timeline
    // event lines up the avatar at the same buffer column, no
    // matter whether the icon glyph measures 1 cell (`🏷` in some
    // terminals, `●`, `✎`, …) or 2 cells (`👤`). Without this
    // padding the rows end up offset by 1, which is exactly the
    // misalignment we saw on the `assigned` row.
    let icon_width = console::measure_text_width(icon) as u16;
    let trailing_pad = 5u16.saturating_sub(1 + icon_width);
    let icon_str = format!(" {}{}", icon, " ".repeat(trailing_pad as usize));
    spans.push(Span::styled(icon_str, Style::default().fg(fg)));
    // Avatar zone is 3 cells (2 image + 1 padding) starting at the
    // fixed icon-zone end (col 5).
    let actor_col: Option<u16> = if avatars_on {
        spans.push(Span::raw("   "));
        Some(5)
    } else {
        None
    };
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
            return (
                Line::from({
                    let mut s = spans;
                    s.push(Span::raw("  "));
                    s.push(Span::styled(
                        ev.when.clone(),
                        Style::default().fg(theme.list_date_fg),
                    ));
                    s
                }),
                actor_col,
            );
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
    (Line::from(spans), actor_col)
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
