//! Pull Requests view — list + detail panels for the active repo's
//! GitHub PRs. Read-only in this iteration; review/merge actions live
//! in a follow-up.
//!
//! Data is fetched up-front (list) and lazily on selection change
//! (detail). Both calls block; the view shows a loading marker while
//! the request is in flight. Errors surface in a persistent banner
//! similar to the rebase apply error path.

use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use rustc_hash::FxHashMap;

/// GitHub's purple for merged PRs — matches the badge color on
/// github.com so the visual cue is instantly recognizable.
const MERGED_PURPLE: Color = Color::Rgb(0x89, 0x57, 0xe5);

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    github::{
        pr::{
            CheckConclusion, CheckStatus, ConversationEntry, ConversationKind, FileStatus,
            Mergeability, PullCommit, PullRequest, PullRequestDetail, PullState, ReviewState,
        },
        RepoCoords,
    },
};

#[derive(Debug)]
pub struct PullRequestsView<'a> {
    // Re-anchored on close so we can return to the commit list state we
    // came from. `None` if opened from a view without a commit list.
    commit_list_state: Option<crate::widget::commit_list::CommitListState<'a>>,
    coords: RepoCoords,
    token: String,
    /// GitHub login of the authenticated user — used to flag comments
    /// the user wrote themselves with a `(me)` suffix. `None` when the
    /// auth state didn't include a login.
    me_login: Option<String>,
    /// All PRs returned by `list_pull_requests` (state=all). The view
    /// filters this client-side through `list_filter` so flipping tabs
    /// doesn't re-hit the API.
    items: Vec<PullRequest>,
    /// Which subset of `items` the list currently shows.
    list_filter: PrListFilter,
    /// Hit-test rects for the filter tab bar — `(filter, screen rect)`
    /// captured during render. Click within a rect switches to that
    /// filter.
    filter_tab_rects: Vec<(PrListFilter, Rect)>,
    /// Filter tab currently under the mouse (visual feedback only).
    hovered_filter: Option<PrListFilter>,
    /// Per-PR detail cache. First open of a PR spawns a background fetch;
    /// subsequent visits hit this cache and render instantly.
    detail_cache: FxHashMap<u64, PullRequestDetail>,
    /// PR number whose fetch is currently in flight, if any. Drives the
    /// "Loading…" indicator in the detail panel.
    loading_for: Option<u64>,
    /// Index of the row under the keyboard cursor / mouse hover. Drives
    /// the row highlight only — opening the PR (loading its detail)
    /// requires an explicit Enter or click.
    hovered: usize,
    /// PR number currently displayed in the detail view, if any. Stored
    /// by number (not index) so it stays stable when the list re-orders
    /// or the filter changes. Marked with a leading triangle in the
    /// list when the opened PR is in the current filter.
    opened_pr_number: Option<u64>,
    /// List vs Detail mode. List is the index of PRs; Detail is the
    /// full GitHub-like multi-tab view for one PR.
    mode: Mode,
    /// Active tab when in Detail mode.
    active_tab: Tab,
    /// Per-tab scroll offsets and selections — kept independent so
    /// switching tabs preserves where the user was.
    conversation_scroll: usize,
    /// Index of the currently-selected comment in the Conversation tab.
    /// `0` is the PR description card; subsequent indices match the
    /// order entries are rendered (top-level → children depth-first).
    conversation_selected: usize,
    /// Set to `true` by keyboard nav (↑↓, PgUp/Dn, Home/End) to ask
    /// the next render to scroll the selected comment into view. Mouse
    /// hover deliberately does NOT set this — the viewport stays still
    /// while the cursor moves around.
    conversation_scroll_to_selected: bool,
    /// Active inline comment editor — opened by `c` (new comment), `r`
    /// (reply), or `e` (edit). When `Some`, all keypresses route to it
    /// until Ctrl+Enter (submit) or Esc (cancel).
    comment_editor: Option<CommentEditor>,
    /// Screen position where the editor's cursor was last drawn —
    /// captured each render so we can place the terminal cursor on top
    /// of the editor surface.
    comment_editor_cursor_pos: Option<(u16, u16)>,
    /// Body rect of the inline editor captured during render — used
    /// to translate clicks inside it into buffer cursor positions.
    editor_body_area: Option<Rect>,
    /// Logical line range of each comment in the conversation render —
    /// `(comment_idx, first_line, last_line)` — captured at render time
    /// so mouse hits and auto-scroll can resolve which card sits where.
    conversation_comment_spans: Vec<(usize, usize, usize)>,
    /// Computed once per render: how many comments make up the
    /// Conversation tab. Drives the keyboard nav clamping.
    conversation_comment_count: usize,
    commits_scroll: usize,
    commits_hovered: usize,
    checks_scroll: usize,
    checks_hovered: usize,
    files_scroll: usize,
    files_hovered: usize,
    last_error: Option<String>,
    /// Rects captured each frame for mouse hit-testing — `None` until the
    /// first render. Cleared at the top of each render pass.
    list_area: Option<Rect>,
    /// Per-tab bounding rect (the inner content area, not the tab bar).
    tab_content_area: Option<Rect>,
    /// Tab bar hit-testing — list of `(Tab, screen rect)` entries
    /// captured during render. Click within a rect switches to that tab.
    tab_bar_rects: Vec<(Tab, Rect)>,
    /// Tab currently under the mouse (gives visual feedback before click).
    hovered_tab: Option<Tab>,
    /// First inner row Y of the list panel (rendering coordinate) — used to
    /// translate a click row into a selection index. Captured during render.
    list_inner_y: u16,
    /// Top-of-list scroll offset, kept in sync with what we passed to ratatui
    /// so click hit-testing stays accurate when the list scrolls.
    list_scroll_offset: usize,
    ctx: Rc<AppContext>,
    tx: Sender,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Top-level PR list. Click/Enter on a row transitions to `Detail`.
    List,
    /// Single-PR detail view with the 4-tab layout.
    Detail,
}

/// Which subset of fetched PRs the list view currently shows. The view
/// fetches `state=all` once and filters client-side between these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrListFilter {
    Open,
    Merged,
    Closed,
    All,
}

impl PrListFilter {
    fn all() -> &'static [PrListFilter] {
        &[
            PrListFilter::Open,
            PrListFilter::Merged,
            PrListFilter::Closed,
            PrListFilter::All,
        ]
    }

    fn label(self) -> &'static str {
        match self {
            PrListFilter::Open => "Open",
            PrListFilter::Merged => "Merged",
            PrListFilter::Closed => "Closed",
            PrListFilter::All => "All",
        }
    }

    fn matches(self, pr: &PullRequest) -> bool {
        match self {
            PrListFilter::Open => matches!(pr.state, PullState::Open),
            PrListFilter::Merged => matches!(pr.state, PullState::Merged),
            // "Closed" means closed-without-merge — merged PRs have
            // their own tab, otherwise the two would overlap.
            PrListFilter::Closed => matches!(pr.state, PullState::Closed),
            PrListFilter::All => true,
        }
    }

    fn index(self) -> usize {
        Self::all().iter().position(|f| *f == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Conversation,
    Commits,
    Checks,
    Files,
}

/// State of an inline comment composer (new top-level comment, reply
/// to a review thread, or edit of an existing own comment). Active
/// when `comment_editor.is_some()` — the conversation reserves a few
/// rows at the bottom of the tab content area to render it, and all
/// keypresses route to the editor until Ctrl+Enter or Esc.
#[derive(Debug, Clone)]
struct CommentEditor {
    kind: CommentEditorKind,
    /// UTF-8 byte buffer of the message. Empty on a fresh new-comment;
    /// pre-loaded with the original body on edit.
    buffer: String,
    /// Cursor as a byte offset into `buffer`. Always lies on a char
    /// boundary; helpers maintain that invariant.
    cursor: usize,
    /// `true` while a POST/PATCH/DELETE is in flight — disables
    /// further keypresses and shows a "Sending…" footer.
    submitting: bool,
    /// Top logical row visible in the editor body. The render anchors
    /// this on the cursor when the cursor moves outside the viewport,
    /// but PgUp/PgDn and the mouse wheel adjust it independently so
    /// the user can browse the buffer without dragging the cursor.
    scroll_offset: u16,
    /// Body height captured at the last render. Used by PgUp/PgDn so
    /// they scroll by exactly one visible page.
    last_body_height: u16,
}

#[derive(Debug, Clone)]
enum CommentEditorKind {
    /// New top-level conversation comment (issues endpoint).
    NewTopLevel,
    /// New top-level comment pre-filled with a markdown blockquote of
    /// another comment's body — what GitHub's "Quote reply" does when
    /// the original isn't an inline review comment (which is the only
    /// kind it lets you natively thread).
    QuoteReply { quoted_author: String },
    /// Reply to an existing review-comment thread (pulls endpoint).
    Reply { parent_id: u64 },
    /// Edit of an own top-level conversation comment.
    EditIssue { comment_id: u64 },
    /// Edit of an own inline review comment.
    EditReview { comment_id: u64 },
    /// PR-level "Approve" review. Body optional.
    ApproveReview,
    /// PR-level "Request changes" review. Body required by GitHub.
    RequestChangesReview,
}

impl CommentEditorKind {
    fn header(&self, parent_author: Option<&str>) -> String {
        match self {
            CommentEditorKind::NewTopLevel => "Add a comment".to_string(),
            CommentEditorKind::QuoteReply { quoted_author } => {
                format!("Quote reply to @{}", quoted_author)
            }
            CommentEditorKind::Reply { .. } => match parent_author {
                Some(a) => format!("Reply to @{}", a),
                None => "Reply".to_string(),
            },
            CommentEditorKind::EditIssue { .. } | CommentEditorKind::EditReview { .. } => {
                "Edit comment".to_string()
            }
            CommentEditorKind::ApproveReview => "Approve PR (optional comment)".to_string(),
            CommentEditorKind::RequestChangesReview => {
                "Request changes (comment required)".to_string()
            }
        }
    }
}

impl Tab {
    fn all() -> [Tab; 4] {
        [Tab::Conversation, Tab::Commits, Tab::Checks, Tab::Files]
    }

    fn label(self) -> &'static str {
        match self {
            Tab::Conversation => "Conversation",
            Tab::Commits => "Commits",
            Tab::Checks => "Checks",
            Tab::Files => "Files",
        }
    }

    fn index(self) -> usize {
        match self {
            Tab::Conversation => 0,
            Tab::Commits => 1,
            Tab::Checks => 2,
            Tab::Files => 3,
        }
    }

    fn from_index(i: usize) -> Option<Self> {
        Self::all().get(i).copied()
    }
}

/// Per-column widths for the PR list. The "fit" columns (state, number,
/// title, author) are padded to align tabularly; head/base are *not*
/// padded — they sit flush against ` → ` so it always reads
/// `branch → branch` rather than `branch     → main`.
struct PrListColumns {
    state: usize,
    number: usize,
    title: usize,
    author: usize,
}

impl PrListColumns {
    /// Fixed visual overhead between columns:
    ///   "▶ " (2) + state + "  " (2) + number + "  " (2)
    ///   + title + "  " (2) + author + " · " (3) + head + " → " (3) + base
    const MARKER: usize = 2;
    const GAP: usize = 2;
    const DOT_SEP: usize = 3; // " · "
    const ARROW: usize = 3; // " → "

    fn compute(items: &[PullRequest], available_width: usize) -> Self {
        // Floors come from the header labels — otherwise a column would
        // shrink below its title's width.
        let state = items
            .iter()
            .map(|pr| match (pr.state, pr.draft) {
                (PullState::Open, true) => 5,
                (PullState::Open, false) => 4,
                (PullState::Merged, _) => 6,
                (PullState::Closed, _) => 6,
            })
            .max()
            .unwrap_or(4)
            .max("STATUS".chars().count());
        let number = items
            .iter()
            .map(|pr| 1 + digits(pr.number))
            .max()
            .unwrap_or(2);
        let author = items
            .iter()
            .map(|pr| pr.author.chars().count())
            .max()
            .unwrap_or(0)
            .max("AUTHOR".chars().count());
        let max_head = items
            .iter()
            .map(|pr| strip_head_owner(&pr.head_label).chars().count())
            .max()
            .unwrap_or(0);
        let max_base = items
            .iter()
            .map(|pr| pr.base_ref.chars().count())
            .max()
            .unwrap_or(0);
        let max_title = items
            .iter()
            .map(|pr| pr.title.chars().count())
            .max()
            .unwrap_or(0);

        // Title flexes between fixed overhead and the worst-case
        // unpadded head/base. If everything fits, title sits at its
        // natural max; otherwise it shrinks and ellipses to `…`.
        let fixed = Self::MARKER
            + state
            + Self::GAP
            + number
            + Self::GAP
            + Self::GAP
            + author
            + Self::DOT_SEP
            + max_head
            + Self::ARROW
            + max_base;
        let remaining = available_width.saturating_sub(fixed);
        let title = remaining.min(max_title.max(1)).max(1);

        Self {
            state,
            number,
            title,
            author,
        }
    }
}

fn digits(n: u64) -> usize {
    if n == 0 {
        1
    } else {
        (n as f64).log10().floor() as usize + 1
    }
}

/// Strip the `<user>:` prefix GitHub adds to `head_label` for forked
/// PRs — the author already appears in its own column, so showing
/// `alice:feature` next to an `alice` cell is just noise.
fn strip_head_owner(label: &str) -> &str {
    match label.split_once(':') {
        Some((_, branch)) => branch,
        None => label,
    }
}

/// Left-align `s` into a cell of exactly `width` columns. Truncates
/// with `…` if it overflows; right-pads with spaces otherwise.
fn fit_cell(s: &str, width: usize) -> String {
    let count = s.chars().count();
    if count == width {
        s.to_string()
    } else if count < width {
        let mut out = String::with_capacity(s.len() + (width - count));
        out.push_str(s);
        for _ in 0..(width - count) {
            out.push(' ');
        }
        out
    } else if width == 0 {
        String::new()
    } else {
        // Truncate to width - 1, then append `…`. Guarded for the
        // degenerate width=1 case where `…` itself takes the whole cell.
        let take = width.saturating_sub(1);
        let mut out: String = s.chars().take(take).collect();
        out.push('…');
        out
    }
}

impl<'a> PullRequestsView<'a> {
    pub fn new(
        commit_list_state: Option<crate::widget::commit_list::CommitListState<'a>>,
        coords: RepoCoords,
        token: String,
        items: Vec<PullRequest>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        let me_login = ctx
            .github_auth_state
            .login
            .clone()
            .filter(|s| !s.is_empty());
        let view = Self {
            commit_list_state,
            coords,
            token,
            me_login,
            items,
            list_filter: PrListFilter::Open,
            filter_tab_rects: Vec::new(),
            hovered_filter: None,
            detail_cache: FxHashMap::default(),
            loading_for: None,
            hovered: 0,
            opened_pr_number: None,
            mode: Mode::List,
            active_tab: Tab::Conversation,
            conversation_scroll: 0,
            conversation_selected: 0,
            conversation_scroll_to_selected: true,
            conversation_comment_spans: Vec::new(),
            conversation_comment_count: 1,
            comment_editor: None,
            comment_editor_cursor_pos: None,
            editor_body_area: None,
            commits_scroll: 0,
            commits_hovered: 0,
            checks_scroll: 0,
            checks_hovered: 0,
            files_scroll: 0,
            files_hovered: 0,
            last_error: None,
            list_area: None,
            tab_content_area: None,
            tab_bar_rects: Vec::new(),
            hovered_tab: None,
            list_inner_y: 0,
            list_scroll_offset: 0,
            ctx,
            tx,
        };
        // Do NOT auto-fetch detail on open — the user has to click or
        // press Enter on a row to load it. Keeps the initial open snappy
        // and avoids burning a request the user doesn't want.
        view
    }

    /// Receive a background fetch result and update the cache. The view's
    /// next render picks up the new data automatically.
    pub fn on_detail_fetched(
        &mut self,
        number: u64,
        result: Result<PullRequestDetail, String>,
    ) {
        if self.loading_for == Some(number) {
            self.loading_for = None;
        }
        match result {
            Ok(d) => {
                self.detail_cache.insert(number, d);
                self.last_error = None;
            }
            Err(e) => {
                self.last_error = Some(format!("Fetch PR #{}: {}", number, e));
            }
        }
    }

    pub fn take_list_state(
        &mut self,
    ) -> Option<crate::widget::commit_list::CommitListState<'a>> {
        self.commit_list_state.take()
    }

    pub fn footer_hint(&self) -> String {
        // When the inline editor is open, the footer is owned by it.
        if self.comment_editor.is_some() {
            let parts = if self
                .comment_editor
                .as_ref()
                .map_or(false, |e| e.submitting)
            {
                vec!["Sending…", "Esc:cancel"]
            } else {
                vec!["Ctrl+S:send", "Esc:cancel"]
            };
            return format!("⌘ {}", parts.join("▕▏"));
        }
        // Footer holds the GLOBAL actions only. Card-specific shortcuts
        // (R:reply / R:quote reply / e:edit / d:delete) live inside the
        // selected comment's top border — that way the footer stays
        // calm and the available actions are visually attached to the
        // card they target.
        let parts: Vec<&str> = match self.mode {
            Mode::List => vec!["r:reload"],
            Mode::Detail => {
                let mut p = vec!["c:comment"];
                p.push("a:approve");
                p.push("x:request changes");
                p.push("m:merge");
                p.push("o:open in web");
                p.push("r:reload");
                p
            }
        };
        format!("⌘ {}", parts.join("▕▏"))
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
        self.reload();
    }
    pub fn update_color_theme(&mut self, _theme: crate::color::ColorTheme) {}

    /// True when the inline comment editor is open. Tells the app key
    /// router to deliver raw key events (Backspace, Delete, etc.) here
    /// instead of treating them as UserEvent shortcuts.
    pub fn is_input_active(&self) -> bool {
        self.comment_editor.is_some()
    }

    // ---------- data ----------

    /// Open the currently-hovered PR — sets `opened`, transitions into
    /// Detail mode (full-screen tabbed view), resets per-tab scroll, and
    /// fires a background fetch on cache miss.
    fn open_hovered(&mut self) {
        let Some(idx) = self.hovered_item_index() else {
            return;
        };
        let number = self.items[idx].number;
        self.opened_pr_number = Some(number);
        self.mode = Mode::Detail;
        self.active_tab = Tab::Conversation;
        self.conversation_scroll = 0;
        self.commits_scroll = 0;
        self.commits_hovered = 0;
        self.checks_scroll = 0;
        self.checks_hovered = 0;
        self.files_scroll = 0;
        self.files_hovered = 0;
        if self.detail_cache.contains_key(&number) {
            return;
        }
        if self.loading_for == Some(number) {
            return;
        }
        self.spawn_detail_fetch(number);
    }

    /// Return from Detail mode back to the PR list. `opened` is kept so
    /// the triangle indicator stays on the row.
    fn back_to_list(&mut self) {
        self.mode = Mode::List;
    }

    fn spawn_detail_fetch(&mut self, number: u64) {
        self.loading_for = Some(number);
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = crate::github::pr::fetch_pull_request_detail(
                &token, &coords, number,
            );
            tx.send(AppEvent::PullRequestDetailFetched { number, result });
        });
    }

    /// Reload the PR list AND, when we're sitting in Detail mode, refresh
    /// the currently-opened PR's detail. We preserve `opened` by PR
    /// NUMBER (not by list index, which may have shifted) so the view
    /// stays anchored on the same PR after the list re-orders.
    fn reload(&mut self) {
        let prev_number = self.opened_number();
        match crate::github::pr::list_pull_requests(&self.token, &self.coords) {
            Ok(items) => {
                self.items = items;
                // Keep the opened PR by NUMBER — survives filter changes
                // and list re-orders. If the PR no longer exists, clear.
                self.opened_pr_number = prev_number
                    .filter(|n| self.items.iter().any(|p| p.number == *n));
                self.clamp_hovered();
                self.detail_cache.clear();
                self.loading_for = None;
                // Detail mode: re-fetch the opened PR's payload. If the
                // PR no longer exists (merged / closed by someone else),
                // bounce back to the list.
                if matches!(self.mode, Mode::Detail) {
                    if let Some(number) = self.opened_number() {
                        self.spawn_detail_fetch(number);
                    } else {
                        self.mode = Mode::List;
                    }
                }
                self.last_error = None;
            }
            Err(e) => {
                self.last_error = Some(format!("Reload PRs: {}", e));
            }
        }
    }

    // ---------- events ----------

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};

        // While the inline editor is open, EVERY key must route to it —
        // no view-level shortcuts (`r` reload, `c` comment, etc.) may
        // hijack the keystroke. Otherwise the user can't type `r`/`c`
        // in their message.
        if self.comment_editor.is_some() {
            match self.mode {
                Mode::List => self.handle_event_list(event_with_count, key),
                Mode::Detail => self.handle_event_detail(event_with_count, key),
            }
            return;
        }

        // `r` reloads — works in both modes when no input is active.
        if key.code == KeyCode::Char('r') && key.modifiers == KeyModifiers::NONE {
            self.reload();
            return;
        }

        match self.mode {
            Mode::List => self.handle_event_list(event_with_count, key),
            Mode::Detail => self.handle_event_detail(event_with_count, key),
        }
        let _ = KeyModifiers::NONE;
    }

    fn handle_event_list(
        &mut self,
        event_with_count: UserEventWithCount,
        _key: KeyEvent,
    ) {
        match event_with_count.event {
            UserEvent::Cancel | UserEvent::Close => {
                self.tx.send(AppEvent::ClosePullRequests);
            }
            UserEvent::Confirm => self.open_hovered(),
            UserEvent::NavigateUp => self.move_hovered(-1),
            UserEvent::NavigateDown => self.move_hovered(1),
            UserEvent::PageUp => self.move_hovered(-10),
            UserEvent::PageDown => self.move_hovered(10),
            UserEvent::GoToTop => self.hovered = 0,
            UserEvent::GoToBottom => self.hovered = self.filtered_len().saturating_sub(1),
            UserEvent::ScrollUp => self.scroll_list(-1),
            UserEvent::ScrollDown => self.scroll_list(1),
            // ←/→ cycle the [Open, Merged, Closed, All] tabs.
            UserEvent::NavigateLeft => {
                let filters = PrListFilter::all();
                let idx = self.list_filter.index();
                let prev = (idx + filters.len() - 1) % filters.len();
                self.set_filter(filters[prev]);
            }
            UserEvent::NavigateRight => {
                let filters = PrListFilter::all();
                let idx = self.list_filter.index();
                let next = (idx + 1) % filters.len();
                self.set_filter(filters[next]);
            }
            _ => {}
        }
    }

    fn handle_event_detail(
        &mut self,
        event_with_count: UserEventWithCount,
        key: KeyEvent,
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};

        // ── Inline comment editor takes all input until Ctrl+Enter / Esc.
        if self.comment_editor.is_some() {
            // While a submission is in flight, only Esc cancels — every
            // other key is swallowed.
            if self
                .comment_editor
                .as_ref()
                .map_or(false, |e| e.submitting)
            {
                if matches!(key.code, KeyCode::Esc) {
                    self.comment_editor = None;
                }
                return;
            }
            // Mouse wheel scrolls the editor viewport without dragging
            // the cursor — useful for skimming earlier or later parts
            // of the buffer while typing somewhere else.
            match event_with_count.event {
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
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            // `true` when the dispatched action moved the cursor or
            // edited the buffer — we anchor the viewport to the cursor
            // afterwards so typing always stays in view. Manual scroll
            // (PgUp/PgDn, wheel) and non-mutating keys leave the scroll
            // exactly where the user put it.
            let cursor_changed = match key.code {
                KeyCode::Esc => {
                    self.comment_editor = None;
                    false
                }
                // Ctrl+Enter is the canonical "send" combo but many
                // terminals don't propagate the modifier with Enter
                // (they send a plain `\r`). Ctrl+S is the reliable
                // fallback that always reaches us.
                KeyCode::Char('s') if ctrl => {
                    self.submit_comment_editor();
                    false
                }
                KeyCode::Enter if ctrl => {
                    self.submit_comment_editor();
                    false
                }
                KeyCode::Enter => {
                    self.editor_insert_char('\n');
                    true
                }
                // Tab inserts indentation. We can't bind Tab to "send"
                // because users genuinely need it in code/text blocks.
                KeyCode::Tab => {
                    for _ in 0..4 {
                        self.editor_insert_char(' ');
                    }
                    true
                }
                // Ctrl+Backspace AND Ctrl+H (the legacy ASCII alias many
                // terminals send instead of Ctrl+Backspace) AND Ctrl+W
                // (readline convention) all delete the word to the left.
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
                // PgUp / PgDn scroll the viewport without dragging
                // the cursor — handy for browsing earlier or later
                // sections while typing further along.
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
                KeyCode::Char(c) if !ctrl => {
                    self.editor_insert_char(c);
                    true
                }
                _ => false,
            };
            if cursor_changed {
                self.editor_anchor_scroll_to_cursor();
            }
            return;
        }

        // ── PR-level action shortcuts (work on any tab once a PR is opened).
        //    a:approve  x:request changes  m:merge  o:open in browser
        let plain = key.modifiers == KeyModifiers::NONE;
        match key.code {
            KeyCode::Char('a') if plain => {
                self.start_approve();
                return;
            }
            KeyCode::Char('x') if plain => {
                self.start_request_changes();
                return;
            }
            KeyCode::Char('m') if plain => {
                self.start_merge();
                return;
            }
            KeyCode::Char('o') if plain => {
                self.open_in_browser();
                return;
            }
            _ => {}
        }

        // ── Conversation-tab action shortcuts (no editor open).
        //    Lowercase `r` is reserved for view-level `reload` (handled
        //    above in handle_event); reply moves to uppercase `R` so
        //    both stay accessible.
        //    For `R` we match the uppercase glyph regardless of modifier
        //    — different terminals attach SHIFT, NONE, or even both to
        //    uppercase letters, so we just trust the produced char.
        if matches!(self.active_tab, Tab::Conversation) {
            match key.code {
                KeyCode::Char('c') if plain => {
                    self.start_new_comment();
                    return;
                }
                KeyCode::Char('R') => {
                    self.start_reply();
                    return;
                }
                KeyCode::Char('e') if plain => {
                    self.start_edit();
                    return;
                }
                KeyCode::Char('d') if plain => {
                    self.confirm_delete_comment();
                    return;
                }
                _ => {}
            }
        }

        match event_with_count.event {
            UserEvent::Cancel | UserEvent::Close => self.back_to_list(),
            // Left/Right move between tabs (in addition to 1-4 hotkeys).
            UserEvent::NavigateLeft => {
                let prev =
                    (self.active_tab.index() + Tab::all().len() - 1) % Tab::all().len();
                if let Some(t) = Tab::from_index(prev) {
                    self.active_tab = t;
                }
            }
            UserEvent::NavigateRight => {
                let next = (self.active_tab.index() + 1) % Tab::all().len();
                if let Some(t) = Tab::from_index(next) {
                    self.active_tab = t;
                }
            }
            UserEvent::NavigateUp => self.tab_nav(-1),
            UserEvent::NavigateDown => self.tab_nav(1),
            UserEvent::PageUp => self.tab_nav(-10),
            UserEvent::PageDown => self.tab_nav(10),
            UserEvent::GoToTop => self.tab_goto_top(),
            UserEvent::GoToBottom => self.tab_goto_bottom(),
            UserEvent::ScrollUp => self.tab_scroll(-1),
            UserEvent::ScrollDown => self.tab_scroll(1),
            _ => {}
        }
    }

    /// Move the per-tab hovered cursor / scroll. Used by ↑↓ + PgUp/Dn.
    fn tab_nav(&mut self, delta: i32) {
        match self.active_tab {
            Tab::Conversation => {
                let max = self.conversation_comment_count.saturating_sub(1);
                self.conversation_selected =
                    adjust_index(self.conversation_selected, delta, max);
                self.conversation_scroll_to_selected = true;
            }
            Tab::Commits => {
                let max = self.opened_commits_len().saturating_sub(1);
                self.commits_hovered = adjust_index(self.commits_hovered, delta, max);
            }
            Tab::Checks => {
                let max = self.opened_checks_len().saturating_sub(1);
                self.checks_hovered = adjust_index(self.checks_hovered, delta, max);
            }
            Tab::Files => {
                let max = self.opened_files_len().saturating_sub(1);
                self.files_hovered = adjust_index(self.files_hovered, delta, max);
            }
        }
    }

    fn tab_scroll(&mut self, delta: i32) {
        // Mouse wheel — doesn't move the per-tab cursor, just pans the
        // visible window (or scrolls the body for the Conversation tab
        // which has no hovered cursor).
        match self.active_tab {
            Tab::Conversation => {
                self.conversation_scroll = adjust_scroll(self.conversation_scroll, delta)
            }
            Tab::Commits => self.commits_scroll = adjust_scroll(self.commits_scroll, delta),
            Tab::Checks => self.checks_scroll = adjust_scroll(self.checks_scroll, delta),
            Tab::Files => self.files_scroll = adjust_scroll(self.files_scroll, delta),
        }
    }

    fn tab_goto_top(&mut self) {
        match self.active_tab {
            Tab::Conversation => {
                self.conversation_selected = 0;
                self.conversation_scroll = 0;
                self.conversation_scroll_to_selected = true;
            }
            Tab::Commits => {
                self.commits_hovered = 0;
                self.commits_scroll = 0;
            }
            Tab::Checks => {
                self.checks_hovered = 0;
                self.checks_scroll = 0;
            }
            Tab::Files => {
                self.files_hovered = 0;
                self.files_scroll = 0;
            }
        }
    }

    fn tab_goto_bottom(&mut self) {
        match self.active_tab {
            Tab::Conversation => {
                self.conversation_selected =
                    self.conversation_comment_count.saturating_sub(1);
                self.conversation_scroll_to_selected = true;
            }
            Tab::Commits => self.commits_hovered = self.opened_commits_len().saturating_sub(1),
            Tab::Checks => self.checks_hovered = self.opened_checks_len().saturating_sub(1),
            Tab::Files => self.files_hovered = self.opened_files_len().saturating_sub(1),
        }
    }

    fn opened_commits_len(&self) -> usize {
        self.opened_detail().map(|d| d.commit_list.len()).unwrap_or(0)
    }
    fn opened_checks_len(&self) -> usize {
        self.opened_detail().map(|d| d.check_runs.len()).unwrap_or(0)
    }
    fn opened_files_len(&self) -> usize {
        self.opened_detail().map(|d| d.files.len()).unwrap_or(0)
    }
    fn opened_detail(&self) -> Option<&PullRequestDetail> {
        let number = self.opened_number()?;
        self.detail_cache.get(&number)
    }
    fn opened_number(&self) -> Option<u64> {
        self.opened_pr_number
    }

    /// Indices into `self.items` of PRs matching the active filter,
    /// preserving the API's sort order.
    fn filtered_indices(&self) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(i, pr)| if self.list_filter.matches(pr) { Some(i) } else { None })
            .collect()
    }

    /// Number of PRs visible under the active filter.
    fn filtered_len(&self) -> usize {
        self.items
            .iter()
            .filter(|pr| self.list_filter.matches(pr))
            .count()
    }

    /// Snap `hovered` to a valid filtered-list index after the filter
    /// changes or items are reloaded.
    fn clamp_hovered(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.hovered = 0;
        } else if self.hovered >= len {
            self.hovered = len - 1;
        }
    }

    /// Resolve `self.hovered` (filtered index) to an index into
    /// `self.items`, if the filter is non-empty.
    fn hovered_item_index(&self) -> Option<usize> {
        self.filtered_indices().get(self.hovered).copied()
    }

    /// Look up the conversation entry at `conversation_selected`. Index 0
    /// is the PR description (no `ConversationEntry`); subsequent indices
    /// walk through the conversation feed in render order so we can map
    /// the selected card back to its underlying data.
    fn selected_conversation_entry(&self) -> Option<&ConversationEntry> {
        let idx = self.conversation_selected;
        if idx == 0 {
            return None; // PR description, no entry
        }
        let detail = self.opened_detail()?;
        let mut counter = 1usize;
        // Re-walk the same render order push_thread uses (top-level
        // first, then DFS into children) — must match exactly.
        let by_id: FxHashMap<u64, &ConversationEntry> = detail
            .conversation
            .iter()
            .filter_map(|e| e.id.map(|id| (id, e)))
            .collect();
        let mut children_of: FxHashMap<u64, Vec<&ConversationEntry>> = FxHashMap::default();
        for entry in &detail.conversation {
            if let Some(p) = entry.parent_id {
                children_of.entry(p).or_default().push(entry);
            }
        }
        let top_level: Vec<&ConversationEntry> = detail
            .conversation
            .iter()
            .filter(|e| match e.parent_id {
                None => true,
                Some(p) => !by_id.contains_key(&p),
            })
            .collect();
        for entry in top_level {
            if let Some(found) = walk_for_index(entry, &children_of, &mut counter, idx) {
                return Some(found);
            }
        }
        None
    }

    // ────────────────────── Comment authoring ──────────────────────

    fn start_new_comment(&mut self) {
        if self.opened_number().is_none() {
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
        // The conversation pane shrinks to make room for the editor;
        // we DON'T want any pending "scroll to selected" flag to fire
        // and re-anchor the view away from where the user was reading.
        self.conversation_scroll_to_selected = false;
    }

    fn start_reply(&mut self) {
        // Special case: the PR description card (index 0) has no
        // `ConversationEntry` but GitHub's web UI lets you quote-reply
        // it via the `…` menu on the card. We mirror that by pulling
        // the body + author straight off the detail and opening a
        // pre-filled new-top-level editor.
        if self.conversation_selected == 0 {
            let Some(detail) = self.opened_detail() else {
                self.tx.send(AppEvent::NotifyInfo(
                    "PR is still loading — try again in a moment.".into(),
                ));
                return;
            };
            let author = detail.author.clone();
            let body = detail.body.clone();
            self.open_quote_reply_editor(author, body);
            return;
        }
        let Some(entry) = self.selected_conversation_entry() else {
            self.tx.send(AppEvent::NotifyInfo(
                "Select a comment to reply to.".into(),
            ));
            return;
        };
        // Two flavours:
        //   - Inline review comment → direct reply via /pulls/{n}/comments
        //     (GitHub natively threads it under the parent).
        //   - Anything else → "quote reply": new top-level comment
        //     pre-filled with a markdown blockquote of the original.
        match (&entry.kind, entry.id) {
            (ConversationKind::ReviewComment { .. }, Some(parent_id)) => {
                self.comment_editor = Some(CommentEditor {
                    kind: CommentEditorKind::Reply { parent_id },
                    buffer: String::new(),
                    cursor: 0,
                    submitting: false,
                    scroll_offset: 0,
                    last_body_height: 0,
                });
            }
            _ => {
                let author = entry.author.clone();
                let body = entry.body.clone();
                self.open_quote_reply_editor(author, body);
            }
        }
        self.conversation_scroll_to_selected = false;
    }

    /// Open a new-top-level editor pre-filled with a GitHub-style
    /// quote of `body` (each line prefixed with `> `, blank lines
    /// preserved as `> ` so paragraph breaks survive). Cursor lands
    /// two blank lines below the quote — ready for the user's reply.
    fn open_quote_reply_editor(&mut self, author: String, body: String) {
        let mut quoted = String::new();
        if body.is_empty() {
            quoted.push_str("> \n");
        } else {
            for line in body.lines() {
                if line.is_empty() {
                    quoted.push_str("> \n");
                } else {
                    quoted.push_str("> ");
                    quoted.push_str(line);
                    quoted.push('\n');
                }
            }
        }
        quoted.push_str("\n\n");
        let cursor = quoted.len();
        self.comment_editor = Some(CommentEditor {
            kind: CommentEditorKind::QuoteReply {
                quoted_author: author,
            },
            buffer: quoted,
            cursor,
            submitting: false,
            scroll_offset: 0,
            last_body_height: 0,
        });
        self.conversation_scroll_to_selected = false;
    }

    fn start_edit(&mut self) {
        let Some(entry) = self.selected_conversation_entry() else {
            return;
        };
        // Edit is only available on own comments — flagged on the
        // entry's author against the cached `me_login`. The footer
        // shortcuts already hide `e:edit` on non-own comments, so an
        // explicit keypress here is silently ignored (no toast).
        let is_me = self
            .me_login
            .as_deref()
            .map_or(false, |me| me == entry.author);
        if !is_me {
            return;
        }
        let kind = match (&entry.kind, entry.id) {
            (ConversationKind::Comment, Some(id)) => CommentEditorKind::EditIssue {
                comment_id: id,
            },
            (ConversationKind::ReviewComment { .. }, Some(id)) => {
                CommentEditorKind::EditReview { comment_id: id }
            }
            _ => return,
        };
        let buffer = entry.body.clone();
        let cursor = buffer.len();
        self.comment_editor = Some(CommentEditor {
            kind,
            buffer,
            cursor,
            submitting: false,
            scroll_offset: 0,
            last_body_height: 0,
        });
        self.conversation_scroll_to_selected = false;
    }

    // ────────────────────── PR-level review actions ──────────────────────

    fn start_approve(&mut self) {
        if self.opened_number().is_none() {
            return;
        }
        self.comment_editor = Some(CommentEditor {
            kind: CommentEditorKind::ApproveReview,
            buffer: String::new(),
            cursor: 0,
            submitting: false,
            scroll_offset: 0,
            last_body_height: 0,
        });
        self.conversation_scroll_to_selected = false;
    }

    fn start_request_changes(&mut self) {
        if self.opened_number().is_none() {
            return;
        }
        self.comment_editor = Some(CommentEditor {
            kind: CommentEditorKind::RequestChangesReview,
            buffer: String::new(),
            cursor: 0,
            submitting: false,
            scroll_offset: 0,
            last_body_height: 0,
        });
        self.conversation_scroll_to_selected = false;
    }

    fn start_merge(&mut self) {
        let Some(number) = self.opened_number() else {
            return;
        };
        // Delegate to the standard dialog system so the user picks the
        // merge method + can tweak the commit title/message before
        // confirming. `DialogKind::MergePullRequest` is wired in app.rs
        // to call back via `merge_pull_request` on confirm.
        let title = self
            .opened_detail()
            .map(|d| d.title.clone())
            .unwrap_or_default();
        let body = self
            .opened_detail()
            .map(|d| d.body.clone())
            .unwrap_or_default();
        self.tx.send(AppEvent::OpenDialog(
            crate::event::DialogKind::MergePullRequest {
                number,
                pr_title: title,
                pr_body: body,
            },
        ));
    }

    fn open_in_browser(&mut self) {
        let Some(number) = self.opened_number() else {
            return;
        };
        let url = format!(
            "https://github.com/{}/{}/pull/{}",
            self.coords.owner, self.coords.repo, number
        );
        // Best-effort: spawn xdg-open / open / start in a detached
        // process. Surface the URL in a toast either way so the user
        // can copy it manually if the launcher isn't installed.
        match crate::external::open_url(&url) {
            Ok(()) => self
                .tx
                .send(AppEvent::NotifyInfo(format!("Opened {}", url))),
            Err(e) => self
                .tx
                .send(AppEvent::NotifyError(format!("Open browser: {}", e))),
        }
    }

    fn confirm_delete_comment(&mut self) {
        let Some(entry) = self.selected_conversation_entry() else {
            return;
        };
        // Delete is only available on own comments. The footer shortcuts
        // already hide `d:delete` on non-own comments, so an explicit
        // keypress here is silently ignored (no toast).
        let is_me = self
            .me_login
            .as_deref()
            .map_or(false, |me| me == entry.author);
        if !is_me {
            return;
        }
        let (id, is_review) = match (&entry.kind, entry.id) {
            (ConversationKind::Comment, Some(id)) => (id, false),
            (ConversationKind::ReviewComment { .. }, Some(id)) => (id, true),
            _ => return,
        };
        let Some(number) = self.opened_number() else {
            return;
        };
        let author = entry.author.clone();
        let body_preview = entry.body.clone();
        // Hand off to the standard confirm-dialog system. The dialog's
        // OK button maps to `GitAction::DeletePrComment`, which app.rs
        // intercepts and runs through `delete_pr_comment` (background
        // thread + `PullRequestActionDone` result).
        self.tx.send(AppEvent::OpenDialog(
            crate::event::DialogKind::ConfirmDeleteComment {
                pr_number: number,
                comment_id: id,
                is_review,
                author,
                body_preview,
            },
        ));
    }

    fn submit_comment_editor(&mut self) {
        let Some(editor) = self.comment_editor.as_ref() else {
            return;
        };
        let body = editor.buffer.trim().to_string();
        // ApproveReview is the only kind that accepts an empty body —
        // everything else (regular comment, reply, edit, RequestChanges)
        // refuses to send.
        let allow_empty = matches!(editor.kind, CommentEditorKind::ApproveReview);
        if body.is_empty() && !allow_empty {
            self.tx.send(AppEvent::NotifyWarn(
                "Body is empty — nothing to send.".into(),
            ));
            return;
        }
        let Some(number) = self.opened_number() else {
            return;
        };
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        let kind = editor.kind.clone();
        let action_label: String = match &kind {
            CommentEditorKind::NewTopLevel => "Comment posted".into(),
            CommentEditorKind::QuoteReply { .. } => "Quote reply posted".into(),
            CommentEditorKind::Reply { .. } => "Reply posted".into(),
            CommentEditorKind::EditIssue { .. } | CommentEditorKind::EditReview { .. } => {
                "Comment updated".into()
            }
            CommentEditorKind::ApproveReview => "Approved".into(),
            CommentEditorKind::RequestChangesReview => "Changes requested".into(),
        };
        // Flip into "sending" state so the editor renders a spinner
        // and discards further input until the result arrives.
        if let Some(e) = self.comment_editor.as_mut() {
            e.submitting = true;
        }
        std::thread::spawn(move || {
            let result = match kind {
                // Both new-comment and quote-reply post to the same
                // issue-comments endpoint — quote reply is just a
                // pre-filled body convenience.
                CommentEditorKind::NewTopLevel
                | CommentEditorKind::QuoteReply { .. } => {
                    crate::github::pr::post_issue_comment(&token, &coords, number, &body)
                }
                CommentEditorKind::Reply { parent_id } => {
                    crate::github::pr::post_review_comment_reply(
                        &token, &coords, number, &body, parent_id,
                    )
                }
                CommentEditorKind::EditIssue { comment_id } => {
                    crate::github::pr::patch_issue_comment(&token, &coords, comment_id, &body)
                }
                CommentEditorKind::EditReview { comment_id } => {
                    crate::github::pr::patch_review_comment(&token, &coords, comment_id, &body)
                }
                CommentEditorKind::ApproveReview => {
                    let body_opt = if body.is_empty() {
                        None
                    } else {
                        Some(body.as_str())
                    };
                    crate::github::pr::submit_review(
                        &token,
                        &coords,
                        number,
                        crate::github::pr::ReviewVerdict::Approve,
                        body_opt,
                    )
                }
                CommentEditorKind::RequestChangesReview => {
                    crate::github::pr::submit_review(
                        &token,
                        &coords,
                        number,
                        crate::github::pr::ReviewVerdict::RequestChanges,
                        Some(body.as_str()),
                    )
                }
            };
            tx.send(AppEvent::PullRequestActionDone {
                number,
                action: action_label,
                result,
            });
        });
    }

    /// Called when a background write action returns. Closes the editor
    /// on success, surfaces the message via toast, and invalidates the
    /// per-PR detail cache so a refresh picks up the new state.
    pub fn on_action_done(
        &mut self,
        number: u64,
        action: String,
        result: Result<(), String>,
    ) {
        match result {
            Ok(()) => {
                self.tx.send(AppEvent::NotifySuccess(action));
                self.comment_editor = None;
                self.detail_cache.remove(&number);
                self.loading_for = None;
                if self.opened_number() == Some(number) {
                    self.spawn_detail_fetch(number);
                }
            }
            Err(e) => {
                self.tx
                    .send(AppEvent::NotifyError(format!("{}: {}", action, e)));
                if let Some(ed) = self.comment_editor.as_mut() {
                    ed.submitting = false;
                }
            }
        }
    }

    // ────────────────────── Inline-editor key handlers ──────────────────────

    fn editor_insert_char(&mut self, c: char) {
        if let Some(ed) = self.comment_editor.as_mut() {
            let cursor = ed.cursor;
            ed.buffer.insert(cursor, c);
            ed.cursor += c.len_utf8();
        }
    }

    fn editor_delete_left(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            if ed.cursor == 0 {
                return;
            }
            // Walk back one char boundary.
            let mut new_cursor = ed.cursor - 1;
            while new_cursor > 0 && !ed.buffer.is_char_boundary(new_cursor) {
                new_cursor -= 1;
            }
            ed.buffer.replace_range(new_cursor..ed.cursor, "");
            ed.cursor = new_cursor;
        }
    }

    fn editor_delete_right(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            if ed.cursor >= ed.buffer.len() {
                return;
            }
            let mut new_end = ed.cursor + 1;
            while new_end < ed.buffer.len() && !ed.buffer.is_char_boundary(new_end) {
                new_end += 1;
            }
            ed.buffer.replace_range(ed.cursor..new_end, "");
        }
    }

    fn editor_delete_word_left(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
            let new_cursor = word_left_boundary(&ed.buffer, ed.cursor);
            ed.buffer.replace_range(new_cursor..ed.cursor, "");
            ed.cursor = new_cursor;
        }
    }

    fn editor_delete_word_right(&mut self) {
        if let Some(ed) = self.comment_editor.as_mut() {
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
            // Move one line up, preserving the visual column.
            let buf = &ed.buffer;
            let cur_line_start = buf[..ed.cursor].rfind('\n').map(|i| i + 1).unwrap_or(0);
            if cur_line_start == 0 {
                ed.cursor = 0;
                return;
            }
            let col = ed.cursor - cur_line_start;
            let prev_line_end = cur_line_start - 1; // the '\n' itself
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

    /// Move the editor's buffer cursor to the byte offset that
    /// corresponds to a click at `(col, row)` inside the editor body.
    /// Out-of-line clicks land on the closest valid position
    /// (clamped to line length / buffer length).
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
        // Walk to the start of `target_row` in the buffer, then walk
        // along the line up to `col_in_body` chars.
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
            // Click was below the last line — clamp to buffer end.
            ed.cursor = ed.buffer.len();
            return;
        }
        // Walk chars from line_start until we hit col_in_body or `\n`
        // or the end of the buffer.
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

    /// Push `scroll_offset` just enough to keep the cursor inside the
    /// last-known viewport. Called from every cursor-mutating helper
    /// so typing / arrow-key navigation always reveals the cursor —
    /// but unlike a render-time anchor, this leaves a previously-set
    /// manual scroll alone whenever the cursor is still visible.
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

    /// Scroll the editor viewport up by one page without moving the
    /// cursor. The cursor's logical position stays put — the user is
    /// browsing the buffer, not navigating.
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

    pub fn handle_click(&mut self, col: u16, row: u16) {
        match self.mode {
            Mode::List => {
                // Filter tab bar click → switch filter.
                let filter_hit = self
                    .filter_tab_rects
                    .iter()
                    .find(|(_, rect)| rect_contains(Some(*rect), col, row))
                    .map(|(f, _)| *f);
                if let Some(f) = filter_hit {
                    self.set_filter(f);
                    return;
                }
                if let Some(idx) = self.row_at_list(row, col) {
                    if idx < self.filtered_len() {
                        self.hovered = idx;
                        self.open_hovered();
                    }
                }
            }
            Mode::Detail => {
                // Inline editor click → reposition the buffer cursor.
                // Routed first because the editor sits on top of the
                // conversation pane.
                if self.comment_editor.is_some()
                    && rect_contains(self.editor_body_area, col, row)
                {
                    self.editor_move_cursor_to_click(col, row);
                    return;
                }
                // Tab bar click → switch tab.
                for (tab, rect) in &self.tab_bar_rects {
                    if rect_contains(Some(*rect), col, row) {
                        self.active_tab = *tab;
                        return;
                    }
                }
                // Click inside a tab's row-based content → set hovered
                // for that tab. Files / Commits / Checks all use the
                // same "rows of items" model.
                if let Some(area) = self.tab_content_area {
                    if rect_contains(Some(area), col, row) {
                        if let Some(idx) = self.row_at_tab(row, area) {
                            self.set_tab_hovered(idx);
                        }
                    }
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        // While the inline editor is open, the user is typing — mouse
        // drift should not change selection or focus. Block all hover
        // updates until the editor closes.
        if self.comment_editor.is_some() {
            return;
        }
        match self.mode {
            Mode::List => {
                // Filter tab hover (visual feedback only — click switches).
                let mut new_hover: Option<PrListFilter> = None;
                for (filter, rect) in &self.filter_tab_rects {
                    if rect_contains(Some(*rect), col, row) {
                        new_hover = Some(*filter);
                        break;
                    }
                }
                if new_hover != self.hovered_filter {
                    self.hovered_filter = new_hover;
                }
                if let Some(idx) = self.row_at_list(row, col) {
                    if idx < self.filtered_len() && idx != self.hovered {
                        self.hovered = idx;
                    }
                }
            }
            Mode::Detail => {
                // Tab bar hover (visual feedback only — click switches).
                let mut new_hover: Option<Tab> = None;
                for (tab, rect) in &self.tab_bar_rects {
                    if rect_contains(Some(*rect), col, row) {
                        new_hover = Some(*tab);
                        break;
                    }
                }
                self.hovered_tab = new_hover;
                // Tab content hover sets the per-tab hovered row.
                if let Some(area) = self.tab_content_area {
                    if rect_contains(Some(area), col, row) {
                        if let Some(idx) = self.row_at_tab(row, area) {
                            self.set_tab_hovered(idx);
                        }
                    }
                }
            }
        }
    }

    fn row_at_list(&self, row: u16, col: u16) -> Option<usize> {
        if !rect_contains(self.list_area, col, row) {
            return None;
        }
        if row < self.list_inner_y {
            return None;
        }
        let idx = self.list_scroll_offset + (row - self.list_inner_y) as usize;
        if idx < self.items.len() {
            Some(idx)
        } else {
            None
        }
    }

    fn row_at_tab(&self, row: u16, area: Rect) -> Option<usize> {
        if row < area.y {
            return None;
        }
        let visible_row = (row - area.y) as usize;
        // Conversation tab uses per-comment spans (each card has a
        // variable line range), so the lookup is different from the
        // simple row-indexed list tabs.
        if matches!(self.active_tab, Tab::Conversation) {
            let logical = self.conversation_scroll + visible_row;
            return self
                .conversation_comment_spans
                .iter()
                .find(|(_, first, last)| logical >= *first && logical <= *last)
                .map(|(idx, _, _)| *idx);
        }
        let scroll = match self.active_tab {
            Tab::Commits => self.commits_scroll,
            Tab::Checks => self.checks_scroll,
            Tab::Files => self.files_scroll,
            Tab::Conversation => return None,
        };
        let idx = scroll + visible_row;
        let max = match self.active_tab {
            Tab::Commits => self.opened_commits_len(),
            Tab::Checks => self.opened_checks_len(),
            Tab::Files => self.opened_files_len(),
            Tab::Conversation => 0,
        };
        if idx < max {
            Some(idx)
        } else {
            None
        }
    }

    fn set_tab_hovered(&mut self, idx: usize) {
        match self.active_tab {
            Tab::Commits => self.commits_hovered = idx,
            Tab::Checks => self.checks_hovered = idx,
            Tab::Files => self.files_hovered = idx,
            // Conversation tab uses its own per-comment hit-test rather
            // than row-indexed list addressing — see `row_at_tab`.
            Tab::Conversation => self.conversation_selected = idx,
        }
    }

    /// Translate a screen row into a list `selected` index, accounting for
    /// the current scroll offset. Returns None when the click is outside
    /// the list's content rows (e.g. on the border).
    fn row_at(&self, row: u16) -> Option<usize> {
        if row < self.list_inner_y {
            return None;
        }
        let visible_row = row.saturating_sub(self.list_inner_y) as usize;
        let idx = self.list_scroll_offset + visible_row;
        if idx < self.filtered_len() {
            Some(idx)
        } else {
            None
        }
    }

    fn move_hovered(&mut self, delta: i32) {
        let len = self.filtered_len();
        if len == 0 {
            return;
        }
        let max = len as i32 - 1;
        self.hovered = (self.hovered as i32 + delta).clamp(0, max) as usize;
    }

    /// Pan the visible list window by `delta` rows without touching the
    /// hovered cursor. Mouse wheel calls this.
    fn scroll_list(&mut self, delta: i32) {
        let len = self.filtered_len();
        if len == 0 {
            return;
        }
        let max = len.saturating_sub(1) as i32;
        let new = (self.list_scroll_offset as i32 + delta).clamp(0, max) as usize;
        self.list_scroll_offset = new;
    }

    /// Switch the active list filter and snap the cursor / scroll back
    /// to the top of the new subset.
    fn set_filter(&mut self, filter: PrListFilter) {
        if self.list_filter == filter {
            return;
        }
        self.list_filter = filter;
        self.hovered = 0;
        self.list_scroll_offset = 0;
    }

    // ---------- rendering ----------

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Clear hit-test rects from the previous frame so a stale rect
        // from a different layout never matches a click.
        self.list_area = None;
        self.tab_content_area = None;
        self.editor_body_area = None;
        self.tab_bar_rects.clear();

        let banner_height: u16 = if self.last_error.is_some() && area.height > 6 {
            3
        } else {
            0
        };
        let [header_area, banner_area, body_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(banner_height),
            Constraint::Min(0),
        ])
        .areas(area);
        self.render_header(f, header_area);
        if banner_height > 0 {
            self.render_error_banner(f, banner_area);
        }

        match self.mode {
            Mode::List => self.render_list(f, body_area),
            Mode::Detail => self.render_detail_mode(f, body_area),
        }

        // Place the terminal cursor on the inline comment editor when
        // it's the active input surface — same convention as the rebase
        // reword editor.
        if let Some((cx, cy)) = self.comment_editor_cursor_pos {
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
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let title = Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "⊙ ",
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Pull Requests ",
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            // Repo coords are an identifier, not a branch — use the
            // hash/identifier token so the name reads as "named ref" and
            // doesn't compete with the green/red branch convention below.
            Span::styled(
                format!("{}/{}", self.coords.owner, self.coords.repo),
                Style::default()
                    .fg(theme.list_hash_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} open", self.items.len()),
                Style::default().fg(theme.detail_label_fg),
            ),
        ]);
        let divider = Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(theme.divider_fg),
        ));
        f.render_widget(Paragraph::new(vec![title, divider]), area);
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

    fn render_list(&mut self, f: &mut Frame, area: Rect) {
        // Build the filter tabs as the block's title — they replace
        // the static "Pull Requests" label so the panel stays compact.
        let (title_spans, tab_rects) = self.build_filter_title(area);
        self.filter_tab_rects = tab_rects;

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.ctx.color_theme.divider_fg))
            .title(Line::from(title_spans));
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.list_area = Some(area);
        // 1-row breathing gap below the filter tabs. We deliberately
        // keep the full inner width so the List widget can paint the
        // selection bg all the way to the panel border — the per-row
        // right margin is created by reserving 1 col in the column
        // budget (see RIGHT_MARGIN below).
        let body_area = Rect::new(
            inner.x,
            inner.y.saturating_add(1),
            inner.width,
            inner.height.saturating_sub(1),
        );
        self.list_inner_y = body_area.y;
        let theme = &self.ctx.color_theme;

        // Filtered slice for both rendering and clamp logic.
        let filtered: Vec<&PullRequest> = self
            .items
            .iter()
            .filter(|pr| self.list_filter.matches(pr))
            .collect();

        if filtered.is_empty() {
            self.list_scroll_offset = 0;
            let msg = match self.list_filter {
                PrListFilter::Open => "No open pull requests.",
                PrListFilter::Merged => "No merged pull requests.",
                PrListFilter::Closed => "No closed pull requests.",
                PrListFilter::All => "No pull requests in this repo.",
            };
            let empty = Paragraph::new(Span::styled(
                msg,
                Style::default().fg(theme.detail_label_fg),
            ))
            .alignment(Alignment::Center);
            f.render_widget(empty, body_area);
            return;
        }
        // Keyboard nav (↑↓ etc.) re-anchors the scroll so the hovered
        // row stays on-screen. Mouse-wheel scroll, on the other hand,
        // moves the viewport independently and may leave `hovered`
        // outside the visible window — that's by design.
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

        // Reserve 2 cols on the right for the row margin — the row's
        // trailing `Span::raw("  ")` lands there, and the List widget
        // also paints the selection bg into it, so the highlight
        // extends visually to the panel border.
        const RIGHT_MARGIN: usize = 2;
        let col_budget = (body_area.width as usize).saturating_sub(RIGHT_MARGIN);
        let owned_filtered: Vec<PullRequest> =
            filtered.iter().map(|pr| (*pr).clone()).collect();
        let cols = PrListColumns::compute(&owned_filtered, col_budget);
        let items: Vec<ListItem<'static>> = filtered
            .iter()
            .enumerate()
            .map(|(i, pr)| {
                // Triangle marker follows the keyboard / mouse cursor
                // (hovered) so the user can see at a glance which row
                // will open on Enter / click.
                let is_marked = self.hovered == i;
                ListItem::new(self.format_pr_row(pr, is_marked, &cols))
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.hovered));
        *state.offset_mut() = self.list_scroll_offset;
        // Only paint the background on the selected row — the per-span
        // foregrounds (sha → list_hash_fg, author → list_name_fg, date →
        // list_date_fg, etc.) stay intact instead of being squashed into
        // a single list_selected_fg. Bold modifier still helps it pop.
        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme.list_selected_bg)
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, body_area, &mut state);
    }

    /// Build the filter-tabs title row that sits on the top border of
    /// the PR list panel. Returns the styled spans (to pass into
    /// `Block::title`) plus per-tab screen rects (for click
    /// hit-testing). Left-aligned titles in ratatui start at
    /// `area.x + 1` (just after the rounded corner).
    fn build_filter_title(
        &self,
        area: Rect,
    ) -> (Vec<Span<'static>>, Vec<(PrListFilter, Rect)>) {
        let theme = &self.ctx.color_theme;
        let counts: Vec<usize> = PrListFilter::all()
            .iter()
            .map(|f| self.items.iter().filter(|pr| f.matches(pr)).count())
            .collect();

        // 3-col gap between tabs so they read as distinct categories.
        const TAB_GAP: u16 = 3;
        let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
        let mut rects: Vec<(PrListFilter, Rect)> = Vec::new();
        let mut cursor_x: u16 = area.x.saturating_add(2); // border (1) + leading space (1)
        for (i, filter) in PrListFilter::all().iter().enumerate() {
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
            if i + 1 < PrListFilter::all().len() {
                spans.push(Span::raw(" ".repeat(TAB_GAP as usize)));
                cursor_x = cursor_x.saturating_add(TAB_GAP);
            }
        }
        spans.push(Span::raw(" "));
        (spans, rects)
    }

    fn format_pr_row(
        &self,
        pr: &PullRequest,
        is_opened: bool,
        cols: &PrListColumns,
    ) -> Line<'static> {
        let theme = &self.ctx.color_theme;
        // Each cell is pre-padded to its column width so rows align
        // tabularly across the list, regardless of value length.
        let (state_text, state_color) = match (pr.state, pr.draft) {
            (PullState::Open, true) => ("DRAFT", theme.detail_label_fg),
            (PullState::Open, false) => ("OPEN", theme.status_success_fg),
            (PullState::Merged, _) => ("MERGED", MERGED_PURPLE),
            (PullState::Closed, _) => ("CLOSED", theme.status_error_fg),
        };
        let state_span = Span::styled(
            fit_cell(state_text, cols.state),
            Style::default().fg(state_color).add_modifier(Modifier::BOLD),
        );
        // Leading `▶` flags the PR whose detail is currently shown.
        let marker = if is_opened {
            Span::styled(
                "▶ ",
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            Span::raw("  ")
        };
        Line::from(vec![
            marker,
            state_span,
            Span::raw("  "),
            Span::styled(
                fit_cell(&format!("#{}", pr.number), cols.number),
                Style::default()
                    .fg(theme.list_hash_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                fit_cell(&pr.title, cols.title),
                Style::default().fg(theme.list_commit_message_fg),
            ),
            Span::raw("  "),
            Span::styled(
                fit_cell(&pr.author, cols.author),
                Style::default().fg(theme.list_name_fg),
            ),
            Span::styled(" · ", Style::default().fg(theme.detail_label_fg)),
            // Head branch (local-style green) → base branch (remote-style
            // red), matching the convention used in the detail sub-header.
            // Not padded so the arrow always reads `branch → branch`
            // instead of `branch       → main`. Fork prefix `user:` is
            // stripped — the author column already shows who opened it.
            Span::styled(
                strip_head_owner(&pr.head_label).to_string(),
                Style::default()
                    .fg(theme.list_ref_branch_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" → ", Style::default().fg(theme.detail_label_fg)),
            Span::styled(
                pr.base_ref.clone(),
                Style::default()
                    .fg(theme.list_ref_remote_branch_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            // 2-col breathing room on the right so branches don't sit
            // flush against the panel's right border.
            Span::raw("  "),
        ])
    }


    fn render_detail_mode(&mut self, f: &mut Frame, area: Rect) {
        let current_number = self.opened_number();
        let cached_detail =
            current_number.and_then(|n| self.detail_cache.get(&n)).cloned();
        let theme_label_fg = self.ctx.color_theme.detail_label_fg;

        // ── Layout: sub-header (PR title+meta) ─ tab bar ─ tab content
        let [sub_header_area, tab_bar_area, tab_content_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .areas(area);
        self.tab_content_area = Some(tab_content_area);

        self.render_pr_sub_header(f, sub_header_area, cached_detail.as_ref(), current_number);
        self.render_tab_bar(f, tab_bar_area);

        let Some(detail) = cached_detail.as_ref() else {
            let label = if current_number.is_some()
                && self.loading_for == current_number
            {
                "  Fetching from GitHub…"
            } else {
                "  No PR opened."
            };
            let p = Paragraph::new(Span::styled(
                label,
                Style::default().fg(theme_label_fg),
            ));
            f.render_widget(p, tab_content_area);
            return;
        };

        match self.active_tab {
            Tab::Conversation => self.render_tab_conversation(f, tab_content_area, detail),
            Tab::Commits => self.render_tab_commits(f, tab_content_area, detail),
            Tab::Checks => self.render_tab_checks(f, tab_content_area, detail),
            Tab::Files => self.render_tab_files(f, tab_content_area, detail),
        }
    }

    /// Sub-header below the global header — PR number, title, state chip,
    /// branches, stats. Equivalent to the top portion of a GitHub PR page.
    /// Inline comment editor reserved at the bottom of the Conversation
    /// tab. 1-row header (title + author hint), N body rows, 1-row footer
    /// (Ctrl+Enter / Esc hints + "Sending…" while a submission is in flight).
    fn render_comment_editor(
        &mut self,
        f: &mut Frame,
        area: Rect,
        theme: &crate::color::ColorTheme,
    ) {
        // Pull the data we need from the editor up-front so we can
        // hand it back as `&mut` later to update the scroll offset
        // without fighting the borrow checker.
        let (kind, buffer, cursor_byte, submitting) = {
            let Some(editor) = self.comment_editor.as_ref() else {
                return;
            };
            (
                editor.kind.clone(),
                editor.buffer.clone(),
                editor.cursor,
                editor.submitting,
            )
        };
        let parent_author_owned: Option<String> = match &kind {
            CommentEditorKind::Reply { parent_id } => self
                .opened_detail()
                .and_then(|d| {
                    d.conversation
                        .iter()
                        .find(|e| e.id == Some(*parent_id))
                        .map(|e| e.author.clone())
                }),
            _ => None,
        };
        let title = kind.header(parent_author_owned.as_deref());

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.list_head_fg))
            .title(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    title,
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

        // Whole inner area goes to the body — Ctrl+S / Esc shortcuts
        // live in the app footer so we don't burn an editor row on a
        // hint that's already visible at the bottom of the screen.
        // When a submit is in flight we still want a 1-row strip for
        // the "Sending…" status; otherwise the editor reclaims it.
        let footer_height: u16 = if submitting { 1 } else { 0 };
        let body_height = inner.height.saturating_sub(footer_height).max(1);
        let body_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: body_height,
        };
        // Capture for mouse hit-testing — clicks inside translate
        // back to a buffer cursor position.
        self.editor_body_area = Some(body_area);
        let footer_area = Rect {
            x: inner.x,
            y: inner.y + body_height,
            width: inner.width,
            height: footer_height,
        };

        // Compute the cursor's logical (col, row) in the buffer first
        // so we can scroll the body window to keep it visible.
        let (cursor_col, cursor_row) = cursor_screen_pos(&buffer, cursor_byte);

        // Total logical row count — number of `\n` in the buffer plus 1
        // (we don't add a trailing empty line for a final `\n`).
        let total_rows = if buffer.is_empty() {
            1
        } else {
            buffer.matches('\n').count() as u16 + 1
        };

        // Read the scroll offset as-is — auto-anchoring lives in the
        // cursor-mutating helpers (so manual wheel/PgUp/PgDn don't get
        // snapped back to the cursor on the next render).
        let mut scroll_offset: u16 = self
            .comment_editor
            .as_ref()
            .map(|e| e.scroll_offset)
            .unwrap_or(0);
        let max_scroll = total_rows.saturating_sub(body_height);
        if scroll_offset > max_scroll {
            scroll_offset = max_scroll;
        }
        if let Some(ed) = self.comment_editor.as_mut() {
            ed.scroll_offset = scroll_offset;
            ed.last_body_height = body_height;
        }

        // Body: render the buffer's logical lines starting from the
        // scroll offset, up to body_height. Wrap is OFF so screen rows
        // map 1:1 with logical rows — keeps cursor positioning trivial.
        let value = Style::default().fg(theme.fg);
        let body_text: Vec<Line<'static>> = if buffer.is_empty() {
            vec![Line::from(Span::styled(
                match &kind {
                    CommentEditorKind::NewTopLevel => "Type your comment…",
                    CommentEditorKind::Reply { .. } => "Type your reply…",
                    _ => "Type your edits…",
                }
                .to_string(),
                Style::default().fg(theme.detail_label_fg),
            ))]
        } else {
            buffer
                .split('\n')
                .skip(scroll_offset as usize)
                .take(body_height as usize)
                .map(|l| Line::from(Span::styled(l.to_string(), value)))
                .collect()
        };
        f.render_widget(Paragraph::new(body_text), body_area);

        // In-editor footer is only used to surface the in-flight
        // status — Ctrl+S / Esc hints already sit in the app footer.
        if submitting {
            f.render_widget(
                Paragraph::new(Span::styled(
                    "  Sending…".to_string(),
                    Style::default().fg(theme.status_warn_fg),
                )),
                footer_area,
            );
        }

        // Place the terminal cursor on the editor surface — translate
        // the logical row into the viewport's row by subtracting the
        // current scroll offset.
        let visible_row = cursor_row.saturating_sub(scroll_offset);
        let cx = body_area.x + cursor_col;
        let cy = body_area.y + visible_row;
        if !submitting
            && cx < body_area.x + body_area.width
            && cy < body_area.y + body_area.height
        {
            self.comment_editor_cursor_pos = Some((cx, cy));
        }
    }

    fn render_pr_sub_header(
        &self,
        f: &mut Frame,
        area: Rect,
        detail: Option<&PullRequestDetail>,
        number: Option<u64>,
    ) {
        let theme = &self.ctx.color_theme;
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::raw("  "));
        let mut badge_spans: Vec<Span<'static>> = Vec::new();
        if let Some(detail) = detail {
            // Compute the badge first so we can reserve room for it on
            // the right and let the title flex/ellipsis to fit. Without
            // this, a long title pushes the badge off-screen entirely.
            badge_spans = mergeability_badge(theme, detail.mergeability);
            let badge_width: usize = badge_spans
                .iter()
                .map(|s| console::measure_text_width(s.content.as_ref()))
                .sum();

            // Build the non-title spans separately so we can size the
            // title relative to actual terminal width.
            let state_span = state_word_span(theme, detail.state, detail.draft);
            let num_span = Span::styled(
                format!("#{}", detail.number),
                Style::default()
                    .fg(theme.list_hash_fg)
                    .add_modifier(Modifier::BOLD),
            );
            let by_span = Span::styled(
                "by ",
                Style::default().fg(theme.detail_label_fg),
            );
            let author_span = Span::styled(
                detail.author.clone(),
                Style::default().fg(theme.list_name_fg),
            );
            // Same fork-prefix strip as the PR list — the author name
            // already appears as its own span, no need to repeat it.
            let head_span = Span::styled(
                strip_head_owner(&detail.head_label).to_string(),
                Style::default()
                    .fg(theme.list_ref_branch_fg)
                    .add_modifier(Modifier::BOLD),
            );
            let arrow_span =
                Span::styled(" → ", Style::default().fg(theme.detail_label_fg));
            let base_span = Span::styled(
                detail.base_ref.clone(),
                Style::default()
                    .fg(theme.list_ref_remote_branch_fg)
                    .add_modifier(Modifier::BOLD),
            );

            // Fixed overhead = leading "  " (2) + state + "  " (2) + #num
            //   + "  " (2) + [title here] + " · " (3) + "by " (3) + author
            //   + " · " (3) + head + " → " (3) + base + min_gap (2)
            //   + badge + trailing (2).
            let trailing_pad = 2;
            let min_gap = 2;
            let measure = |s: &Span<'_>| console::measure_text_width(s.content.as_ref());
            let fixed_overhead = 2
                + measure(&state_span)
                + 2
                + measure(&num_span)
                + 2
                + 3
                + measure(&by_span)
                + measure(&author_span)
                + 3
                + measure(&head_span)
                + 3
                + measure(&base_span)
                + min_gap
                + badge_width
                + trailing_pad;
            let title_budget = (area.width as usize).saturating_sub(fixed_overhead).max(8);
            let title_truncated = truncate(&detail.title, title_budget);

            let dot_sep = || {
                Span::styled(" · ", Style::default().fg(theme.detail_label_fg))
            };

            spans.push(state_span);
            spans.push(Span::raw("  "));
            spans.push(num_span);
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                title_truncated,
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ));
            spans.push(dot_sep());
            spans.push(by_span);
            spans.push(author_span);
            spans.push(dot_sep());
            spans.push(head_span);
            spans.push(arrow_span);
            spans.push(base_span);
        } else if let Some(n) = number {
            spans.push(Span::styled(
                format!("Loading PR #{}…", n),
                Style::default().fg(theme.detail_label_fg),
            ));
        }

        // Right-align the mergeability badge: measure both sides, pad
        // with spaces in between so the badge sits on the right edge.
        let left_width: usize = spans
            .iter()
            .map(|s| console::measure_text_width(s.content.as_ref()))
            .sum();
        let badge_width: usize = badge_spans
            .iter()
            .map(|s| console::measure_text_width(s.content.as_ref()))
            .sum();
        let total = area.width as usize;
        let trailing_pad = 2;
        let gap = total
            .saturating_sub(left_width)
            .saturating_sub(badge_width)
            .saturating_sub(trailing_pad);
        if gap > 0 && badge_width > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
            spans.extend(badge_spans);
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

    /// Tab bar: clickable list of [Conversation, Commits, Checks, Files]
    /// with the active one highlighted (HEAD-accent fg + underlined).
    fn render_tab_bar(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let detail = self.opened_detail().cloned();
        let counts = [
            detail.as_ref().map(|d| d.conversation.len()).unwrap_or(0),
            detail.as_ref().map(|d| d.commit_list.len()).unwrap_or(0),
            detail.as_ref().map(|d| d.check_runs.len()).unwrap_or(0),
            detail.as_ref().map(|d| d.files.len()).unwrap_or(0),
        ];

        // Render each tab as `  Label N  ` so click hit-testing can map
        // raw column → tab. We accumulate the start column as we go.
        let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
        let mut cursor_x: u16 = area.x + 2; // skip the leading 2-space indent
        let tabs = Tab::all();
        for (i, tab) in tabs.iter().enumerate() {
            let is_active = *tab == self.active_tab;
            let is_hovered = self.hovered_tab == Some(*tab) && !is_active;
            let label = format!("{} {}", tab.label(), counts[i]);
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

    fn render_tab_conversation(
        &mut self,
        f: &mut Frame,
        area: Rect,
        detail: &PullRequestDetail,
    ) {
        // Reserve the bottom rows for the inline editor when it's open.
        self.comment_editor_cursor_pos = None;
        let editor_height: u16 = if self.comment_editor.is_some() {
            // Header + body (≥ 6 visible rows, up to 10) + footer.
            // 1 row above the editor stays empty as a visual breather.
            let body_lines = self
                .comment_editor
                .as_ref()
                .map(|e| e.buffer.lines().count().max(6).min(10))
                .unwrap_or(6) as u16;
            (3 + body_lines).min(area.height.saturating_sub(2)).max(8)
        } else {
            0
        };
        let gap_height: u16 = if editor_height > 0 { 1 } else { 0 };
        let [conv_area, _gap, editor_area] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(gap_height),
            Constraint::Length(editor_height),
        ])
        .areas(area);

        // Clone the theme once: we need a long-lived snapshot while
        // also calling `&mut self` methods (push_thread, scroll fixups)
        // further down.
        let theme = self.ctx.color_theme.clone();
        let theme = &theme;

        if editor_height > 0 {
            self.render_comment_editor(f, editor_area, theme);
        }

        let area = conv_area;
        let mut lines: Vec<Line<'static>> = Vec::new();
        // Reset per-frame span tracking so mouse hits use the layout
        // we're about to lay down, not a stale one.
        self.conversation_comment_spans.clear();

        let pr_idx = 0usize;
        let pr_first = lines.len();
        let pr_is_me = self
            .me_login
            .as_deref()
            .map_or(false, |me| me == detail.author);
        // 1. PR description rendered as the first "card" (top-level, no
        //    parent → empty ancestor gutters, and no descending line
        //    since the conversation feed is not its "child" tree).
        push_comment_card(
            &mut lines,
            theme,
            area.width,
            CommentCardInput {
                author: &detail.author,
                action: CommentAction::Opened,
                when: &detail.opened_when,
                body: &detail.body,
                ancestor_gutters: &[],
                has_children: false,
                is_selected: self.conversation_selected == pr_idx,
                is_me: pr_is_me,
                // PR description card: quote-reply only (no in-place
                // edit / delete shortcuts here).
                inline_shortcuts: vec!["R:quote reply"],
            },
        );
        self.conversation_comment_spans
            .push((pr_idx, pr_first, lines.len().saturating_sub(1)));

        // 2. Build a parent → children map so review-comment threads
        //    can be rendered indented under their parent. Issue comments
        //    and reviews always live at the top level.
        let by_id: FxHashMap<u64, &ConversationEntry> = detail
            .conversation
            .iter()
            .filter_map(|e| e.id.map(|id| (id, e)))
            .collect();
        let mut children_of: FxHashMap<u64, Vec<&ConversationEntry>> = FxHashMap::default();
        for entry in &detail.conversation {
            if let Some(p) = entry.parent_id {
                children_of.entry(p).or_default().push(entry);
            }
        }

        // 3. Top-level pass: anything without a parent_id, OR a reply
        //    whose parent we couldn't find (defensive — show it flat).
        let top_level: Vec<&ConversationEntry> = detail
            .conversation
            .iter()
            .filter(|e| match e.parent_id {
                None => true,
                Some(p) => !by_id.contains_key(&p),
            })
            .collect();

        if top_level.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No comments yet.",
                Style::default().fg(theme.detail_label_fg),
            )));
            self.conversation_comment_count = 1;
        } else {
            // 1 blank row between the PR description and the first
            // top-level comment, then 1 blank row between each pair of
            // unrelated top-level threads. Inside a thread (parent +
            // replies), no blank rows — the connector line wires them.
            lines.push(Line::from(""));
            let mut next_idx = 1usize; // 0 was the PR description
            for (i, entry) in top_level.iter().enumerate() {
                if i > 0 {
                    lines.push(Line::from(""));
                }
                self.push_thread(
                    &mut lines,
                    theme,
                    area.width,
                    entry,
                    &children_of,
                    &[],
                    &mut next_idx,
                );
            }
            self.conversation_comment_count = next_idx;
        }
        // Clamp selection if comments shrank below the previous index.
        if self.conversation_selected >= self.conversation_comment_count {
            self.conversation_selected =
                self.conversation_comment_count.saturating_sub(1);
        }
        // Auto-scroll the viewport only when the selection moved via
        // keyboard (or programmatic action). Mouse hover changes the
        // selection without nudging the scroll — see how the flag is
        // set in `tab_nav` / `tab_goto_*` but never in `handle_mouse_move`.
        if self.conversation_scroll_to_selected {
            self.ensure_selected_comment_visible(area.height as usize);
            self.conversation_scroll_to_selected = false;
        }

        let scroll = self.conversation_scroll.min(lines.len().saturating_sub(1));
        let para = Paragraph::new(lines).scroll((scroll as u16, 0));
        f.render_widget(para, area);
    }

    /// Recursively render a comment and its replies. Replies indent one
    /// level under the parent; a T-shaped dashed connector (`┊╌╌╌`)
    /// bridges each reply's top-left to the parent's gutter, and the
    /// parent's gutter only descends as long as more replies are coming.
    /// A leaf comment with no replies has zero descending line.
    fn push_thread(
        &mut self,
        lines: &mut Vec<Line<'static>>,
        theme: &crate::color::ColorTheme,
        width: u16,
        entry: &ConversationEntry,
        children_of: &FxHashMap<u64, Vec<&ConversationEntry>>,
        ancestor_gutters: &[bool],
        next_idx: &mut usize,
    ) {
        let action = match &entry.kind {
            ConversationKind::Comment => CommentAction::Commented,
            ConversationKind::Review { state } => CommentAction::Review(*state),
            ConversationKind::ReviewComment { file, line } => {
                CommentAction::ReviewComment {
                    file: file.clone(),
                    line: *line,
                }
            }
        };
        let replies = entry
            .id
            .and_then(|id| children_of.get(&id))
            .cloned()
            .unwrap_or_default();
        let has_children = !replies.is_empty();
        let my_idx = *next_idx;
        *next_idx += 1;
        let first_line = lines.len();
        let is_me = self
            .me_login
            .as_deref()
            .map_or(false, |me| me == entry.author);
        // Build the inline shortcut chips for THIS card:
        //  - reply variant depends on whether the card is a file-level
        //    review comment (true reply) vs a top-level entry (quote
        //    reply, GitHub doesn't natively thread those)
        //  - edit / delete only for own editable kinds (review entries
        //    aren't editable from the API).
        let mut shortcuts: Vec<&'static str> = Vec::new();
        let is_review_comment = matches!(
            entry.kind,
            ConversationKind::ReviewComment { .. }
        );
        if is_review_comment {
            shortcuts.push("R:reply");
        } else {
            shortcuts.push("R:quote reply");
        }
        let editable_kind = matches!(
            entry.kind,
            ConversationKind::Comment | ConversationKind::ReviewComment { .. }
        );
        if is_me && editable_kind {
            shortcuts.push("e:edit");
            shortcuts.push("d:delete");
        }
        push_comment_card(
            lines,
            theme,
            width,
            CommentCardInput {
                author: &entry.author,
                action,
                when: &entry.when,
                body: &entry.body,
                ancestor_gutters,
                has_children,
                is_selected: self.conversation_selected == my_idx,
                is_me,
                inline_shortcuts: shortcuts,
            },
        );
        self.conversation_comment_spans
            .push((my_idx, first_line, lines.len().saturating_sub(1)));

        // Recurse into replies, threading a new gutter at this card's
        // depth. The gutter is "open" for all-but-last children so the
        // line keeps descending; on the last child it terminates with `└`.
        if has_children {
            // No gap row between a card and its replies — boxes sit
            // tight against each other and the descending `│` runs
            // through every row (top border, body, bend) on the children
            // side, providing visual continuity without any blank line.
            for (i, child) in replies.iter().enumerate() {
                let is_last = i == replies.len() - 1;
                let mut new_gutters: Vec<bool> = ancestor_gutters.to_vec();
                new_gutters.push(!is_last);
                self.push_thread(
                    lines,
                    theme,
                    width,
                    child,
                    children_of,
                    &new_gutters,
                    next_idx,
                );
            }
        }
    }

    /// Adjust `conversation_scroll` so the selected card sits inside
    /// the visible viewport. Called after a render lays down spans.
    fn ensure_selected_comment_visible(&mut self, visible_height: usize) {
        if visible_height == 0 {
            return;
        }
        let Some((_, first, last)) = self
            .conversation_comment_spans
            .iter()
            .find(|(idx, _, _)| *idx == self.conversation_selected)
        else {
            return;
        };
        let (first, last) = (*first, *last);
        if last >= self.conversation_scroll + visible_height {
            self.conversation_scroll = last + 1 - visible_height;
        }
        if first < self.conversation_scroll {
            self.conversation_scroll = first;
        }
    }

    fn render_tab_commits(
        &mut self,
        f: &mut Frame,
        area: Rect,
        detail: &PullRequestDetail,
    ) {
        let theme = &self.ctx.color_theme;
        if detail.commit_list.is_empty() {
            let p = Paragraph::new(Span::styled(
                "  No commits in this PR.",
                Style::default().fg(theme.detail_label_fg),
            ));
            f.render_widget(p, area);
            return;
        }
        // Anchor scroll so the hovered row stays in view.
        let visible = area.height as usize;
        if self.commits_hovered >= self.commits_scroll + visible {
            self.commits_scroll = self.commits_hovered + 1 - visible;
        } else if self.commits_hovered < self.commits_scroll {
            self.commits_scroll = self.commits_hovered;
        }
        let max_offset = detail.commit_list.len().saturating_sub(visible);
        if self.commits_scroll > max_offset {
            self.commits_scroll = max_offset;
        }

        let cols = CommitColumns::compute(&detail.commit_list, area.width as usize);
        let items: Vec<ListItem<'static>> = detail
            .commit_list
            .iter()
            .map(|c| ListItem::new(commit_row(theme, c, &cols)))
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.commits_hovered));
        *state.offset_mut() = self.commits_scroll;
        // Only paint the background on the selected row — the per-span
        // foregrounds (sha → list_hash_fg, author → list_name_fg, date →
        // list_date_fg, etc.) stay intact instead of being squashed into
        // a single list_selected_fg. Bold modifier still helps it pop.
        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme.list_selected_bg)
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, area, &mut state);
    }

    fn render_tab_checks(
        &mut self,
        f: &mut Frame,
        area: Rect,
        detail: &PullRequestDetail,
    ) {
        let theme = &self.ctx.color_theme;
        if detail.check_runs.is_empty() {
            let p = Paragraph::new(Span::styled(
                "  No checks configured.",
                Style::default().fg(theme.detail_label_fg),
            ));
            f.render_widget(p, area);
            return;
        }
        let visible = area.height as usize;
        if self.checks_hovered >= self.checks_scroll + visible {
            self.checks_scroll = self.checks_hovered + 1 - visible;
        } else if self.checks_hovered < self.checks_scroll {
            self.checks_scroll = self.checks_hovered;
        }
        let items: Vec<ListItem<'static>> = detail
            .check_runs
            .iter()
            .map(|c| ListItem::new(check_row(theme, c)))
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.checks_hovered));
        *state.offset_mut() = self.checks_scroll;
        // Only paint the background on the selected row — the per-span
        // foregrounds (sha → list_hash_fg, author → list_name_fg, date →
        // list_date_fg, etc.) stay intact instead of being squashed into
        // a single list_selected_fg. Bold modifier still helps it pop.
        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme.list_selected_bg)
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, area, &mut state);
    }

    fn render_tab_files(
        &mut self,
        f: &mut Frame,
        area: Rect,
        detail: &PullRequestDetail,
    ) {
        let theme = &self.ctx.color_theme;
        if detail.files.is_empty() {
            let p = Paragraph::new(Span::styled(
                "  No file changes.",
                Style::default().fg(theme.detail_label_fg),
            ));
            f.render_widget(p, area);
            return;
        }
        let visible = area.height as usize;
        if self.files_hovered >= self.files_scroll + visible {
            self.files_scroll = self.files_hovered + 1 - visible;
        } else if self.files_hovered < self.files_scroll {
            self.files_scroll = self.files_hovered;
        }
        let cols = FileColumns::compute(&detail.files, area.width as usize);
        let items: Vec<ListItem<'static>> = detail
            .files
            .iter()
            .map(|f| ListItem::new(file_line(theme, f, &cols)))
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.files_hovered));
        *state.offset_mut() = self.files_scroll;
        // Only paint the background on the selected row — the per-span
        // foregrounds (sha → list_hash_fg, author → list_name_fg, date →
        // list_date_fg, etc.) stay intact instead of being squashed into
        // a single list_selected_fg. Bold modifier still helps it pop.
        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme.list_selected_bg)
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, area, &mut state);
    }
}

/// One comment "card" in the Conversation tab: a full 4-sided box
/// (`┌─┐ │ └─┘`) with the author / action / timestamp baked into the
/// top border. Replies sit indented to the right of their parent; the
/// connector is a real T-shaped dashed line (`┊╌╌╌`) that descends from
/// the parent's bottom and bends right to dock at each reply's top-left.
/// A card with no replies emits no descending line at all.
struct CommentCardInput<'a> {
    author: &'a str,
    action: CommentAction,
    when: &'a str,
    body: &'a str,
    /// `ancestor_gutters[i] == true` means the gutter at depth i still
    /// has children coming below; we draw `│` there. `false` means the
    /// gutter is closed (we draw spaces). The LAST entry is the parent's
    /// gutter and dictates the bend on the junction row.
    ancestor_gutters: &'a [bool],
    /// True when at least one reply will be rendered directly under this
    /// card. Drives the bottom-left corner of the box: `├` (attaches the
    /// descending line) vs the standard `└` (closes the box cleanly).
    has_children: bool,
    /// When `true`, this card is the currently-selected comment in the
    /// Conversation tab — the box border switches to the head accent so
    /// the user can see which card has focus.
    is_selected: bool,
    /// `true` when `author` matches the authenticated GitHub login —
    /// we append a small `(me)` chip after the name.
    is_me: bool,
    /// Action shortcuts to surface inline in the top border when this
    /// card is selected (e.g. ["R:reply", "e:edit", "d:delete"]).
    /// Empty for cards that have no card-scoped actions (PR description,
    /// reviews) or when not selected.
    inline_shortcuts: Vec<&'static str>,
}

enum CommentAction {
    Opened,
    Commented,
    Review(ReviewState),
    ReviewComment { file: String, line: Option<u64> },
}

/// Column width of one tree level (`┊   `): dashed gutter + 3-space pad.
const TREE_LEVEL_WIDTH: u16 = 4;

// Tree-level slot is 4 cols wide. We offset the vertical line by 1 col
// to the right within that slot so the connector floats next to the
// parent's box border instead of pretending to attach to it. The bend
// sits on the FIRST BODY row of the child rather than its top border —
// the horizontal merges into the child box via a `┤` at its left edge.
fn gutter_segment_open() -> &'static str {
    " │  "
}
fn gutter_segment_closed() -> &'static str {
    "    "
}
fn gutter_segment_bend(more_siblings_below: bool) -> &'static str {
    // Trailing space so the bend's horizontal does NOT touch the
    // child's box border — keeps the connector visibly disconnected
    // from the box (avoids the colour clash that the `┤` merge made).
    if more_siblings_below {
        " ├─ "
    } else {
        " └─ "
    }
}

fn build_body_prefix(
    theme: &crate::color::ColorTheme,
    ancestor_gutters: &[bool],
) -> Vec<Span<'static>> {
    // Connector lines render in `divider_fg` (the dim grey) so they
    // sit visually below the box borders (which use `detail_label_fg`).
    let connector = Style::default().fg(theme.divider_fg);
    ancestor_gutters
        .iter()
        .map(|open| {
            let seg = if *open {
                gutter_segment_open()
            } else {
                gutter_segment_closed()
            };
            Span::styled(seg.to_string(), connector)
        })
        .collect()
}

fn build_junction_prefix(
    theme: &crate::color::ColorTheme,
    ancestor_gutters: &[bool],
) -> Vec<Span<'static>> {
    let connector = Style::default().fg(theme.divider_fg);
    let n = ancestor_gutters.len();
    ancestor_gutters
        .iter()
        .enumerate()
        .map(|(i, open)| {
            let seg = if i == n - 1 {
                gutter_segment_bend(*open)
            } else if *open {
                gutter_segment_open()
            } else {
                gutter_segment_closed()
            };
            Span::styled(seg.to_string(), connector)
        })
        .collect()
}

fn push_comment_card(
    lines: &mut Vec<Line<'static>>,
    theme: &crate::color::ColorTheme,
    available_width: u16,
    input: CommentCardInput<'_>,
) {
    let label = Style::default().fg(theme.detail_label_fg);
    let value = Style::default().fg(theme.fg);
    let name_style = Style::default()
        .fg(theme.list_name_fg)
        .add_modifier(Modifier::BOLD);
    let date_style = Style::default().fg(theme.list_date_fg);
    // Selected comments get the head accent on every box border so the
    // selection reads at a glance. Non-selected stay in the neutral
    // `detail_label_fg`.
    let border_style = if input.is_selected {
        Style::default()
            .fg(theme.list_head_fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.detail_label_fg)
    };
    let me_chip = Style::default().fg(theme.list_head_fg);

    let depth = input.ancestor_gutters.len();
    let prefix_width = depth * TREE_LEVEL_WIDTH as usize;
    let card_width = (available_width as usize)
        .saturating_sub(prefix_width)
        .saturating_sub(1)
        .max(20);

    // Action span baked into the top border.
    let action_span: Span<'static> = match &input.action {
        CommentAction::Opened => Span::styled(" opened this PR", label),
        CommentAction::Commented => Span::styled(" commented", label),
        CommentAction::Review(state) => match state {
            ReviewState::Approved => Span::styled(
                " approved",
                Style::default()
                    .fg(theme.status_success_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            ReviewState::ChangesRequested => Span::styled(
                " requested changes",
                Style::default()
                    .fg(theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            ReviewState::Commented => Span::styled(" commented", label),
            ReviewState::Other => Span::styled(" reviewed", label),
        },
        CommentAction::ReviewComment { file, line } => {
            let suffix = match line {
                Some(l) => format!(" on {}:{}", file, l),
                None => format!(" on {}", file),
            };
            Span::styled(suffix, label)
        }
    };
    let when_span = if input.when.is_empty() {
        None
    } else {
        Some(Span::styled(format!("  {}", input.when), date_style))
    };

    // ── Top border with inlined title: ┌─ title ─...─┐
    //    The vertical line continues PAST the top border at col 1 of
    //    the parent's gutter slot (no bend on the top row); the bend
    //    happens one row down, on the card's first body row.
    //
    //    Even for the LAST sibling, the descending line still reaches
    //    this card's top border from above — we force the immediate
    //    parent's gutter "open" just for this row so the `│` is there.
    //    (Below the bend row, the gutter takes its real "last" status.)
    let mut top_gutters = input.ancestor_gutters.to_vec();
    if let Some(last) = top_gutters.last_mut() {
        *last = true;
    }
    let mut top: Vec<Span<'static>> = build_body_prefix(theme, &top_gutters);
    top.push(Span::styled("┌─ ".to_string(), border_style));
    let mut consumed = 3; // "┌─ "
    consumed += console::measure_text_width(input.author);
    top.push(Span::styled(input.author.to_string(), name_style));
    if input.is_me {
        let me_label = " (me)";
        consumed += console::measure_text_width(me_label);
        top.push(Span::styled(me_label.to_string(), me_chip));
    }
    consumed += console::measure_text_width(action_span.content.as_ref());
    top.push(action_span);
    if let Some(w) = when_span {
        consumed += console::measure_text_width(w.content.as_ref());
        top.push(w);
    }

    // Inline shortcut chips on the right side of the top border, only
    // when the card is selected. `╶╴` (two short mid-line horizontals,
    // U+2576 + U+2574) keeps the 2-column footprint of the footer's
    // `▕▏` but sits at the line's midpoint instead of full height —
    // reads as a low-key separator that matches the box-drawing family.
    let shortcut_text = if input.is_selected && !input.inline_shortcuts.is_empty() {
        format!(" {} ", input.inline_shortcuts.join("╶╴"))
    } else {
        String::new()
    };
    let shortcut_width = console::measure_text_width(&shortcut_text);
    let after_title = card_width
        .saturating_sub(consumed)
        .saturating_sub(2) // " " before title + "┐"
        .saturating_sub(shortcut_width);
    top.push(Span::styled(
        format!(" {}", "─".repeat(after_title)),
        border_style,
    ));
    if !shortcut_text.is_empty() {
        // Chip uses a dimmer fg so it doesn't compete with the selected
        // border colour, but still BOLD so the keys read clearly.
        top.push(Span::styled(
            shortcut_text,
            Style::default()
                .fg(theme.detail_label_fg)
                .add_modifier(Modifier::BOLD),
        ));
    }
    top.push(Span::styled("┐".to_string(), border_style));
    lines.push(Line::from(top));

    // ── Body rows. For child cards (`depth > 0`), the FIRST body row
    //    carries the bend: prefix uses `build_junction_prefix` and the
    //    card's leftmost `│` is replaced with `┤` so the horizontal
    //    visually merges into the box. Subsequent rows return to the
    //    normal vertical-only gutter.
    let inner_width = card_width.saturating_sub(4);
    let body_lines: Vec<Vec<Span<'static>>> = if input.body.is_empty() {
        vec![vec![Span::styled("(no body)".to_string(), label)]]
    } else {
        render_markdown_body(input.body, theme, inner_width)
    };
    let is_child = depth > 0;
    for (i, line_spans) in body_lines.into_iter().enumerate() {
        let used: usize = line_spans
            .iter()
            .map(|s| console::measure_text_width(s.content.as_ref()))
            .sum();
        let pad = inner_width.saturating_sub(used);
        let is_bend_row = is_child && i == 0;
        let mut row = if is_bend_row {
            build_junction_prefix(theme, input.ancestor_gutters)
        } else {
            build_body_prefix(theme, input.ancestor_gutters)
        };
        // Box left stays `│` even on the bend row — the connector
        // doesn't merge into the box (user prefers visibly disconnected
        // over a colour mismatch where ─ and │/┤ are styled differently).
        row.push(Span::styled("│ ".to_string(), border_style));
        row.extend(line_spans);
        row.push(Span::styled(format!("{} │", " ".repeat(pad)), border_style));
        lines.push(Line::from(row));
    }
    let _ = value;
    let _ = input.has_children;
    let bottom_dashes = "─".repeat(card_width.saturating_sub(2));
    let mut bot = build_body_prefix(theme, input.ancestor_gutters);
    bot.push(Span::styled(format!("└{}┘", bottom_dashes), border_style));
    lines.push(Line::from(bot));
}

/// Compact "ready to merge" / "conflicts" / etc. badge rendered at the
/// right edge of the PR sub-header. Mirrors the colour cues GitHub uses
/// in its own merge-box: green for ready, red for blockers, yellow for
/// warnings, grey for terminal / unknown states.
fn mergeability_badge(
    theme: &crate::color::ColorTheme,
    state: Mergeability,
) -> Vec<Span<'static>> {
    let (label, fg) = match state {
        Mergeability::Ready => ("● Ready to merge", theme.status_success_fg),
        Mergeability::ChecksFailing => ("● Checks failing", theme.status_error_fg),
        Mergeability::Conflicts => ("● Merge conflicts", theme.status_error_fg),
        Mergeability::Blocked => ("● Review required", theme.status_warn_fg),
        Mergeability::Behind => ("● Base is ahead", theme.status_warn_fg),
        Mergeability::Draft => ("● Draft", theme.detail_label_fg),
        Mergeability::Merged => ("● Merged", MERGED_PURPLE),
        Mergeability::Closed => ("● Closed", theme.detail_label_fg),
        Mergeability::Unknown => ("● Checking…", theme.detail_label_fg),
    };
    // Wrap in parens so the badge reads as parenthetical status info
    // instead of competing visually with the title.
    let paren_style = Style::default().fg(theme.detail_label_fg);
    vec![
        Span::styled("(".to_string(), paren_style),
        Span::styled(
            label.to_string(),
            Style::default().fg(fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(")".to_string(), paren_style),
    ]
}

fn state_word_span(
    theme: &crate::color::ColorTheme,
    state: PullState,
    draft: bool,
) -> Span<'static> {
    match (state, draft) {
        (PullState::Open, true) => Span::styled(
            "DRAFT",
            Style::default()
                .fg(theme.detail_label_fg)
                .add_modifier(Modifier::BOLD),
        ),
        (PullState::Open, false) => Span::styled(
            "OPEN",
            Style::default()
                .fg(theme.status_success_fg)
                .add_modifier(Modifier::BOLD),
        ),
        (PullState::Merged, _) => Span::styled(
            "MERGED",
            Style::default()
                .fg(MERGED_PURPLE)
                .add_modifier(Modifier::BOLD),
        ),
        (PullState::Closed, _) => Span::styled(
            "CLOSED",
            Style::default()
                .fg(theme.status_error_fg)
                .add_modifier(Modifier::BOLD),
        ),
    }
}

/// Per-column widths for the Commits tab. Sha is fixed by hash length,
/// author and date are sized to the longest value, subject takes the
/// rest (ellipsis when narrow).
struct CommitColumns {
    sha: usize,
    subject: usize,
    author: usize,
    date: usize,
}

impl CommitColumns {
    fn compute(items: &[PullCommit], available_width: usize) -> Self {
        let sha = items
            .iter()
            .map(|c| c.short_sha.chars().count())
            .max()
            .unwrap_or(7);
        let author = items
            .iter()
            .map(|c| c.author.chars().count())
            .max()
            .unwrap_or(0);
        let date = items
            .iter()
            .map(|c| c.date.chars().count())
            .max()
            .unwrap_or(0);
        let max_subject = items
            .iter()
            .map(|c| c.subject.chars().count())
            .max()
            .unwrap_or(0);
        // leading "  " (2) + sha + "  " (2) + subject + "  " (2)
        //   + author + "  " (2) + date + trailing (2)
        let fixed = 2 + sha + 2 + 2 + author + 2 + date + 2;
        let subject = available_width
            .saturating_sub(fixed)
            .min(max_subject.max(1))
            .max(1);
        Self {
            sha,
            subject,
            author,
            date,
        }
    }
}

fn commit_row(
    theme: &crate::color::ColorTheme,
    c: &PullCommit,
    cols: &CommitColumns,
) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            fit_cell(&c.short_sha, cols.sha),
            Style::default()
                .fg(theme.list_hash_fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            fit_cell(&c.subject, cols.subject),
            Style::default().fg(theme.list_commit_message_fg),
        ),
        Span::raw("  "),
        Span::styled(
            fit_cell(&c.author, cols.author),
            Style::default().fg(theme.list_name_fg),
        ),
        Span::raw("  "),
        Span::styled(
            fit_cell(&c.date, cols.date),
            Style::default().fg(theme.list_date_fg),
        ),
    ])
}

fn check_row(theme: &crate::color::ColorTheme, c: &crate::github::pr::CheckRunDetail) -> Line<'static> {
    let (icon, color) = match (c.status, c.conclusion) {
        (CheckStatus::Completed, Some(CheckConclusion::Success))
        | (CheckStatus::Completed, Some(CheckConclusion::Neutral))
        | (CheckStatus::Completed, Some(CheckConclusion::Skipped)) => (
            "✓",
            theme.status_success_fg,
        ),
        (CheckStatus::Completed, Some(CheckConclusion::Failure))
        | (CheckStatus::Completed, Some(CheckConclusion::TimedOut))
        | (CheckStatus::Completed, Some(CheckConclusion::Cancelled))
        | (CheckStatus::Completed, Some(CheckConclusion::ActionRequired)) => (
            "✗",
            theme.status_error_fg,
        ),
        (CheckStatus::Queued, _) | (CheckStatus::InProgress, _) => (
            "⏳",
            theme.status_warn_fg,
        ),
        _ => ("?", theme.detail_label_fg),
    };
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            icon.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(c.name.clone(), Style::default().fg(theme.fg)),
    ])
}

fn ci_summary_spans(theme: &crate::color::ColorTheme, ci: &crate::github::pr::CiSummary) -> Span<'static> {
    if ci.total == 0 {
        return Span::styled(
            "— no checks".to_string(),
            Style::default().fg(theme.detail_label_fg),
        );
    }
    let (label, color) = if ci.failure > 0 {
        ("✗ failing", theme.status_error_fg)
    } else if ci.pending > 0 {
        ("⏳ running", theme.status_warn_fg)
    } else {
        ("✓ passing", theme.status_success_fg)
    };
    Span::styled(
        format!("{} {}/{}", label, ci.success, ci.total),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

/// Per-column widths for the Files tab. Filename flexes; additions and
/// deletions pad to the widest count so `+N -M` stacks tidily.
struct FileColumns {
    filename: usize,
    additions: usize,
    deletions: usize,
}

impl FileColumns {
    fn compute(items: &[crate::github::pr::PullFile], available_width: usize) -> Self {
        let additions = items
            .iter()
            .map(|f| 1 + digits(f.additions))
            .max()
            .unwrap_or(2);
        let deletions = items
            .iter()
            .map(|f| 1 + digits(f.deletions))
            .max()
            .unwrap_or(2);
        let max_filename = items
            .iter()
            .map(|f| f.filename.chars().count())
            .max()
            .unwrap_or(0);
        // leading "    " (4) + tag (1) + "  " (2) + filename + "  " (2)
        //   + additions + " " (1) + deletions + trailing (2)
        let fixed = 4 + 1 + 2 + 2 + additions + 1 + deletions + 2;
        let filename = available_width
            .saturating_sub(fixed)
            .min(max_filename.max(1))
            .max(1);
        Self {
            filename,
            additions,
            deletions,
        }
    }
}

fn file_line(
    theme: &crate::color::ColorTheme,
    f: &crate::github::pr::PullFile,
    cols: &FileColumns,
) -> Line<'static> {
    // Re-use the commit-detail palette for file-change tags so a PR's
    // file list reads the same way as the local commit's diff list.
    let (tag, tag_color) = match f.status {
        FileStatus::Added => ("A", theme.detail_file_change_add_fg),
        FileStatus::Modified => ("M", theme.detail_file_change_modify_fg),
        FileStatus::Removed => ("D", theme.detail_file_change_delete_fg),
        FileStatus::Renamed => ("R", theme.detail_file_change_move_fg),
        FileStatus::Other => ("?", theme.detail_label_fg),
    };
    Line::from(vec![
        Span::raw("    "),
        Span::styled(
            tag.to_string(),
            Style::default().fg(tag_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            fit_cell(&f.filename, cols.filename),
            Style::default().fg(theme.fg),
        ),
        Span::raw("  "),
        Span::styled(
            fit_cell(&format!("+{}", f.additions), cols.additions),
            Style::default().fg(theme.detail_file_change_add_fg),
        ),
        Span::raw(" "),
        Span::styled(
            fit_cell(&format!("-{}", f.deletions), cols.deletions),
            Style::default().fg(theme.detail_file_change_delete_fg),
        ),
    ])
}

/// Compute (column, row) within the editor body for a buffer cursor at
/// the given byte offset. Lines are split on `\n`; `col` is the
/// visible-width count of the prefix in the current logical line.
fn cursor_screen_pos(buf: &str, byte_cursor: usize) -> (u16, u16) {
    let safe_cursor = byte_cursor.min(buf.len());
    let prefix = &buf[..safe_cursor];
    let row = prefix.chars().filter(|c| *c == '\n').count() as u16;
    let last_line_start = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let col = console::measure_text_width(&prefix[last_line_start..]) as u16;
    (col, row)
}

/// Recursively walk a parent + its replies (DFS, in render order) and
/// return the entry at the given 1-based selection index. The PR
/// description is index 0 (handled by the caller); top-level entries
/// start at 1 and increment by 1 per visited entry.
fn walk_for_index<'a>(
    entry: &'a ConversationEntry,
    children_of: &FxHashMap<u64, Vec<&'a ConversationEntry>>,
    counter: &mut usize,
    target: usize,
) -> Option<&'a ConversationEntry> {
    if *counter == target {
        return Some(entry);
    }
    *counter += 1;
    if let Some(id) = entry.id {
        if let Some(replies) = children_of.get(&id) {
            for child in replies {
                if let Some(found) = walk_for_index(child, children_of, counter, target) {
                    return Some(found);
                }
            }
        }
    }
    None
}

/// Byte index of the next word-boundary to the LEFT of `cursor` —
/// skips trailing whitespace then alphanumerics. Mirrors Ctrl+Backspace
/// in modern editors.
fn word_left_boundary(buf: &str, mut cursor: usize) -> usize {
    let bytes = buf.as_bytes();
    while cursor > 0 {
        let prev = prev_char_boundary(buf, cursor);
        if !bytes[prev].is_ascii_whitespace() {
            break;
        }
        cursor = prev;
    }
    while cursor > 0 {
        let prev = prev_char_boundary(buf, cursor);
        if !is_word_byte(bytes[prev]) {
            break;
        }
        cursor = prev;
    }
    cursor
}

fn word_right_boundary(buf: &str, mut cursor: usize) -> usize {
    let bytes = buf.as_bytes();
    let len = buf.len();
    while cursor < len {
        let next = next_char_boundary(buf, cursor);
        if !is_word_byte(bytes[cursor]) {
            break;
        }
        cursor = next;
    }
    while cursor < len {
        let next = next_char_boundary(buf, cursor);
        if !bytes[cursor].is_ascii_whitespace() {
            break;
        }
        cursor = next;
    }
    cursor
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn prev_char_boundary(buf: &str, mut i: usize) -> usize {
    if i == 0 {
        return 0;
    }
    i -= 1;
    while i > 0 && !buf.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(buf: &str, mut i: usize) -> usize {
    let len = buf.len();
    if i >= len {
        return len;
    }
    i += 1;
    while i < len && !buf.is_char_boundary(i) {
        i += 1;
    }
    i
}

fn adjust_index(current: usize, delta: i32, max: usize) -> usize {
    if max == 0 && delta != 0 {
        return 0;
    }
    let new = current as i32 + delta;
    new.clamp(0, max as i32) as usize
}

fn adjust_scroll(current: usize, delta: i32) -> usize {
    let new = current as i32 + delta;
    new.max(0) as usize
}

fn rect_contains(rect: Option<Rect>, col: u16, row: u16) -> bool {
    let Some(r) = rect else {
        return false;
    };
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// Render a markdown body string into a stack of styled lines ready
/// to be placed inside a card. Handles:
/// - Headings (`# `, `## `, `### `) — bold + accent color
/// - Bold `**text**` and italic `*text*` / `_text_`
/// - Inline code `` `code` `` (in hash color)
/// - Fenced code blocks (triple-backtick)
/// - Bullet lists (`-` / `*`) with `•` glyph
/// - Blockquotes (`> text`) with a left gutter
/// - Horizontal rules (`---`, `***`, `___`)
/// - Links `[label](url)` — just the label, underlined accent
/// - Word-wrap every visual line at `inner_width` so long paragraphs
///   flow naturally instead of being chopped with an ellipsis.
/// - Collapses runs of blank lines down to a single blank line.
/// Render a markdown body to a sequence of wrapped, styled lines fit
/// for the PR comment cards. Backed by `pulldown-cmark` so we get
/// CommonMark + GFM (task lists, strikethrough, tables) for free.
pub(crate) fn render_markdown_body(
    body: &str,
    theme: &crate::color::ColorTheme,
    inner_width: usize,
) -> Vec<Vec<Span<'static>>> {
    use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    opts.insert(Options::ENABLE_TABLES);

    let parser = Parser::new_ext(body, opts);

    let normal = Style::default().fg(theme.fg);
    let label = Style::default().fg(theme.detail_label_fg);
    let code_style = Style::default().fg(theme.list_hash_fg);
    // Heading hierarchy — terminals can't size text, so we lean on
    // colour + modifiers to telegraph the level. H1 also gets an
    // underline so it visually outweighs H2 at a glance.
    let h1_style = Style::default()
        .fg(theme.list_head_fg)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let h2_style = Style::default()
        .fg(theme.list_head_fg)
        .add_modifier(Modifier::BOLD);
    let h3_style = Style::default()
        .fg(theme.fg)
        .add_modifier(Modifier::BOLD);
    let h_other_style = Style::default()
        .fg(theme.detail_label_fg)
        .add_modifier(Modifier::BOLD);
    let link_style = Style::default()
        .fg(theme.list_head_fg)
        .add_modifier(Modifier::UNDERLINED);
    let check_done_style = Style::default().fg(theme.status_success_fg);

    let mut out: Vec<Vec<Span<'static>>> = Vec::new();
    let mut inline: Vec<Span<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = vec![normal];
    // None = bullet; Some(n) = ordered with the next-number-to-emit.
    let mut list_stack: Vec<Option<u64>> = Vec::new();
    let mut bq_depth: usize = 0;
    let mut in_code_block: bool = false;
    let mut code_buf: String = String::new();
    let mut pending_marker: Option<(String, Style)> = None;

    let push_blank_separator = |out: &mut Vec<Vec<Span<'static>>>| {
        if out.is_empty() {
            return;
        }
        if let Some(last) = out.last() {
            if last
                .iter()
                .all(|s| s.content.as_ref().trim().is_empty())
            {
                return;
            }
        }
        out.push(Vec::new());
    };

    let measure_prefix = |prefix: &[Span<'static>]| -> usize {
        prefix
            .iter()
            .map(|s| console::measure_text_width(s.content.as_ref()))
            .sum()
    };

    let build_first_prefix =
        |bq_depth: usize, list_stack: &[Option<u64>], marker: &Option<(String, Style)>| {
            let mut p: Vec<Span<'static>> = Vec::new();
            for _ in 0..bq_depth {
                p.push(Span::styled("│ ".to_string(), label));
            }
            if !list_stack.is_empty() {
                // Continuation indentation for outer nesting levels.
                for _ in 0..(list_stack.len() - 1) {
                    p.push(Span::raw("  ".to_string()));
                }
                if let Some((text, style)) = marker {
                    p.push(Span::styled(text.clone(), *style));
                } else {
                    p.push(Span::raw("  ".to_string()));
                }
            }
            p
        };

    let build_cont_prefix = |bq_depth: usize, list_depth: usize| {
        let mut p: Vec<Span<'static>> = Vec::new();
        for _ in 0..bq_depth {
            p.push(Span::styled("│ ".to_string(), label));
        }
        for _ in 0..list_depth {
            p.push(Span::raw("  ".to_string()));
        }
        p
    };

    let mut flush_inline =
        |out: &mut Vec<Vec<Span<'static>>>,
         inline: &mut Vec<Span<'static>>,
         bq_depth: usize,
         list_stack: &[Option<u64>],
         pending_marker: &mut Option<(String, Style)>| {
            if inline.is_empty() {
                return;
            }
            let first_prefix =
                build_first_prefix(bq_depth, list_stack, pending_marker);
            let cont_prefix = build_cont_prefix(bq_depth, list_stack.len());
            let avail = inner_width.saturating_sub(measure_prefix(&first_prefix));
            let wrapped = wrap_styled_spans(std::mem::take(inline), avail.max(1));
            for (i, line_spans) in wrapped.into_iter().enumerate() {
                let mut row: Vec<Span<'static>> = if i == 0 {
                    first_prefix.clone()
                } else {
                    cont_prefix.clone()
                };
                row.extend(line_spans);
                out.push(row);
            }
            *pending_marker = None;
        };

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => {
                    let s = match level {
                        HeadingLevel::H1 => h1_style,
                        HeadingLevel::H2 => h2_style,
                        HeadingLevel::H3 => h3_style,
                        _ => h_other_style,
                    };
                    style_stack.push(s);
                }
                Tag::BlockQuote(_) => {
                    bq_depth += 1;
                }
                Tag::CodeBlock(_) => {
                    in_code_block = true;
                    code_buf.clear();
                }
                Tag::List(start) => {
                    list_stack.push(start);
                }
                Tag::Item => {
                    let depth = list_stack.len();
                    let marker_text = match list_stack.last() {
                        Some(Some(n)) => format!("{}. ", n),
                        Some(None) => match depth {
                            1 => "• ".to_string(),
                            2 => "◦ ".to_string(),
                            _ => "▪ ".to_string(),
                        },
                        None => "• ".to_string(),
                    };
                    pending_marker = Some((marker_text, label));
                }
                Tag::Emphasis => {
                    // Most terminal fonts lack italic glyphs, so we
                    // pin a distinct accent colour on top of the
                    // ITALIC modifier — that way `*foo*` reads
                    // differently from `**foo**` and from normal
                    // text no matter what the terminal supports.
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or(normal)
                        .fg(theme.list_date_fg)
                        .add_modifier(Modifier::ITALIC);
                    style_stack.push(s);
                }
                Tag::Strong => {
                    // Bold gets the head-accent colour in addition to
                    // the BOLD modifier — terminals that don't thicken
                    // text still surface bold via hue.
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or(normal)
                        .fg(theme.list_head_fg)
                        .add_modifier(Modifier::BOLD);
                    style_stack.push(s);
                }
                Tag::Strikethrough => {
                    let s = style_stack
                        .last()
                        .copied()
                        .unwrap_or(normal)
                        .add_modifier(Modifier::CROSSED_OUT);
                    style_stack.push(s);
                }
                Tag::Link { .. } => {
                    style_stack.push(link_style);
                }
                Tag::Image { .. } => {
                    style_stack.push(label);
                    inline.push(Span::styled("[image: ".to_string(), label));
                }
                // Tables, footnotes, html blocks, definition lists, etc.
                // are passed through transparently: their inline `Text`
                // events still hit the buffer, just without special
                // block decoration.
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => {
                    flush_inline(
                        &mut out,
                        &mut inline,
                        bq_depth,
                        &list_stack,
                        &mut pending_marker,
                    );
                    if list_stack.is_empty() {
                        push_blank_separator(&mut out);
                    }
                }
                TagEnd::Heading(_) => {
                    flush_inline(
                        &mut out,
                        &mut inline,
                        bq_depth,
                        &list_stack,
                        &mut pending_marker,
                    );
                    style_stack.pop();
                    push_blank_separator(&mut out);
                }
                TagEnd::BlockQuote(_) => {
                    bq_depth = bq_depth.saturating_sub(1);
                }
                TagEnd::CodeBlock => {
                    let buf = std::mem::take(&mut code_buf);
                    let prefix = build_cont_prefix(bq_depth, list_stack.len());
                    let avail = inner_width
                        .saturating_sub(measure_prefix(&prefix))
                        .max(1);
                    for raw_line in buf.lines() {
                        let wrapped = wrap_styled_spans(
                            vec![Span::styled(raw_line.to_string(), code_style)],
                            avail,
                        );
                        for piece in wrapped {
                            let mut row = prefix.clone();
                            row.extend(piece);
                            out.push(row);
                        }
                    }
                    in_code_block = false;
                    push_blank_separator(&mut out);
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                    if list_stack.is_empty() {
                        push_blank_separator(&mut out);
                    }
                }
                TagEnd::Item => {
                    flush_inline(
                        &mut out,
                        &mut inline,
                        bq_depth,
                        &list_stack,
                        &mut pending_marker,
                    );
                    if let Some(Some(n)) = list_stack.last_mut() {
                        *n += 1;
                    }
                    pending_marker = None;
                }
                TagEnd::Emphasis
                | TagEnd::Strong
                | TagEnd::Strikethrough
                | TagEnd::Link => {
                    style_stack.pop();
                }
                TagEnd::Image => {
                    style_stack.pop();
                    inline.push(Span::styled("]".to_string(), label));
                }
                _ => {}
            },
            Event::Text(s) => {
                if in_code_block {
                    code_buf.push_str(&s);
                } else {
                    let style = style_stack.last().copied().unwrap_or(normal);
                    inline.push(Span::styled(s.into_string(), style));
                }
            }
            Event::Code(s) => {
                inline.push(Span::styled(s.into_string(), code_style));
            }
            Event::Html(_) | Event::InlineHtml(_) => {
                // Skip raw HTML — looks ugly in a TUI and most GitHub
                // markdown only uses it for collapsible sections we
                // can't render anyway.
            }
            Event::SoftBreak => {
                // GitHub Flavored Markdown (in comments/PR bodies)
                // treats a single newline as a hard line break. We
                // follow the same convention so what users see in the
                // composer matches what hits the wire.
                flush_inline(
                    &mut out,
                    &mut inline,
                    bq_depth,
                    &list_stack,
                    &mut pending_marker,
                );
            }
            Event::HardBreak => {
                flush_inline(
                    &mut out,
                    &mut inline,
                    bq_depth,
                    &list_stack,
                    &mut pending_marker,
                );
            }
            Event::Rule => {
                let divider = Style::default().fg(theme.divider_fg);
                out.push(vec![Span::styled(
                    "─".repeat(inner_width.max(4)),
                    divider,
                )]);
            }
            Event::TaskListMarker(checked) => {
                let (text, style) = if checked {
                    ("[x] ".to_string(), check_done_style)
                } else {
                    ("[ ] ".to_string(), label)
                };
                inline.push(Span::styled(text, style));
            }
            Event::FootnoteReference(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => {}
        }
    }

    // Flush anything still buffered (rare — happens on malformed input
    // where a paragraph is never closed).
    flush_inline(
        &mut out,
        &mut inline,
        bq_depth,
        &list_stack,
        &mut pending_marker,
    );

    // Trim trailing blank rows for cleaner cards.
    while out.last().map_or(false, |row| {
        row.iter()
            .all(|s| s.content.as_ref().trim().is_empty())
    }) {
        out.pop();
    }

    out
}

/// Word-wrap a styled span list into multiple visual lines fitting
/// within `max_width` display columns. Each span's style is preserved
/// across wraps.
pub(crate) fn wrap_styled_spans(
    spans: Vec<Span<'static>>,
    max_width: usize,
) -> Vec<Vec<Span<'static>>> {
    if max_width == 0 {
        return vec![spans];
    }
    let total: usize = spans
        .iter()
        .map(|s| console::measure_text_width(s.content.as_ref()))
        .sum();
    if total <= max_width {
        return vec![spans];
    }
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut cur_w = 0usize;
    for span in spans {
        let style = span.style;
        let content_owned = span.content.to_string();
        for token in split_into_tokens(&content_owned) {
            let tw = console::measure_text_width(&token);
            let is_space = token.chars().all(char::is_whitespace);
            if cur_w + tw > max_width && cur_w > 0 {
                lines.push(Vec::new());
                cur_w = 0;
                if is_space {
                    continue;
                }
            }
            if tw > max_width {
                // Hard-split a single oversized token.
                let mut piece = String::new();
                let mut piece_w = 0usize;
                for ch in token.chars() {
                    let cw = console::measure_text_width(&ch.to_string());
                    if cur_w + piece_w + cw > max_width && (cur_w + piece_w) > 0 {
                        if !piece.is_empty() {
                            lines
                                .last_mut()
                                .unwrap()
                                .push(Span::styled(std::mem::take(&mut piece), style));
                            piece_w = 0;
                        }
                        lines.push(Vec::new());
                        cur_w = 0;
                    }
                    piece.push(ch);
                    piece_w += cw;
                }
                if !piece.is_empty() {
                    cur_w += piece_w;
                    lines
                        .last_mut()
                        .unwrap()
                        .push(Span::styled(piece, style));
                }
            } else {
                lines.last_mut().unwrap().push(Span::styled(token, style));
                cur_w += tw;
            }
        }
    }
    lines
}

fn split_into_tokens(s: &str) -> Vec<String> {
    // Alternating runs of whitespace / non-whitespace.
    let mut out = Vec::new();
    let mut current = String::new();
    let mut prev_space: Option<bool> = None;
    for c in s.chars() {
        let is_space = c.is_whitespace();
        match prev_space {
            None => {
                current.push(c);
                prev_space = Some(is_space);
            }
            Some(p) if p == is_space => current.push(c),
            Some(_) => {
                out.push(std::mem::take(&mut current));
                current.push(c);
                prev_space = Some(is_space);
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut)
    }
}
