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
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use rustc_hash::FxHashMap;

use crate::{
    app::AppContext,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    github::{
        pr::{
            CheckConclusion, CheckStatus, ConversationEntry, ConversationKind, FileStatus,
            PullCommit, PullRequest, PullRequestDetail, PullState, ReviewState,
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
    /// Open PRs returned by `list_pull_requests`. May be empty.
    items: Vec<PullRequest>,
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
    /// Index of the PR currently displayed in the detail view, if any.
    /// Marked with a leading triangle in the list. `None` until the user
    /// clicks or presses Enter on a row.
    opened: Option<usize>,
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
            detail_cache: FxHashMap::default(),
            loading_for: None,
            hovered: 0,
            opened: None,
            mode: Mode::List,
            active_tab: Tab::Conversation,
            conversation_scroll: 0,
            conversation_selected: 0,
            conversation_scroll_to_selected: true,
            conversation_comment_spans: Vec::new(),
            conversation_comment_count: 1,
            comment_editor: None,
            comment_editor_cursor_pos: None,
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
        // On the Conversation tab the footer is dynamic — `R:reply` only
        // appears when the selected card is an inline review comment
        // (GitHub doesn't allow replying to anything else), and
        // `e:edit` / `d:delete` only appear when the user owns the
        // selected comment. The shortcuts that don't apply stay
        // physically blocked in `handle_event_detail` too.
        if matches!(self.mode, Mode::Detail)
            && matches!(self.active_tab, Tab::Conversation)
        {
            let (reply_kind, can_modify_own) = match self.selected_conversation_entry() {
                Some(e) => {
                    let is_review = matches!(e.kind, ConversationKind::ReviewComment { .. });
                    let is_own = self
                        .me_login
                        .as_deref()
                        .map_or(false, |me| me == e.author);
                    let editable_kind = matches!(
                        e.kind,
                        ConversationKind::Comment
                            | ConversationKind::ReviewComment { .. }
                    );
                    let reply_kind = if is_review {
                        Some("R:reply")
                    } else {
                        // Any other selected card (PR description,
                        // top-level comment, review) gets a "quote
                        // reply" — a new top-level comment pre-filled
                        // with `> @author wrote: …`.
                        Some("R:quote reply")
                    };
                    (reply_kind, is_own && editable_kind)
                }
                None => (None, false),
            };
            let mut parts: Vec<&str> = vec!["c:comment"];
            if let Some(r) = reply_kind {
                parts.push(r);
            }
            if can_modify_own {
                parts.push("e:edit");
                parts.push("d:delete");
            }
            parts.push("r:reload");
            return format!("⌘ {}", parts.join("▕▏"));
        }

        let parts: Vec<&str> = match self.mode {
            Mode::List => vec!["r:reload"],
            // Non-Conversation tabs (Commits / Checks / Files) currently
            // have no view-level actions beyond the basics — leave the
            // footer empty so we don't clutter it with universal hints
            // (⇆ / ↑↓ / Esc) that already live in muscle memory.
            Mode::Detail => vec!["r:reload"],
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
        let Some(pr) = self.items.get(self.hovered) else {
            return;
        };
        let number = pr.number;
        self.opened = Some(self.hovered);
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
                self.hovered = self.hovered.min(self.items.len().saturating_sub(1));
                self.opened = prev_number
                    .and_then(|n| self.items.iter().position(|p| p.number == n));
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
            UserEvent::GoToBottom => self.hovered = self.items.len().saturating_sub(1),
            UserEvent::ScrollUp => self.scroll_list(-1),
            UserEvent::ScrollDown => self.scroll_list(1),
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
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Esc => {
                    self.comment_editor = None;
                }
                // Ctrl+Enter is the canonical "send" combo but many
                // terminals don't propagate the modifier with Enter
                // (they send a plain `\r`). Ctrl+S is the reliable
                // fallback that always reaches us.
                KeyCode::Char('s') if ctrl => self.submit_comment_editor(),
                KeyCode::Enter if ctrl => {
                    self.submit_comment_editor();
                }
                KeyCode::Enter => self.editor_insert_char('\n'),
                // Tab inserts indentation. We can't bind Tab to "send"
                // because users genuinely need it in code/text blocks.
                KeyCode::Tab => {
                    for _ in 0..4 {
                        self.editor_insert_char(' ');
                    }
                }
                // Ctrl+Backspace AND Ctrl+H (the legacy ASCII alias many
                // terminals send instead of Ctrl+Backspace) AND Ctrl+W
                // (readline convention) all delete the word to the left.
                KeyCode::Backspace if ctrl => self.editor_delete_word_left(),
                KeyCode::Char('h') if ctrl => self.editor_delete_word_left(),
                KeyCode::Char('w') if ctrl => self.editor_delete_word_left(),
                KeyCode::Backspace => self.editor_delete_left(),
                KeyCode::Delete if ctrl => self.editor_delete_word_right(),
                KeyCode::Delete => self.editor_delete_right(),
                KeyCode::Left if ctrl => self.editor_word_left(),
                KeyCode::Right if ctrl => self.editor_word_right(),
                KeyCode::Left => self.editor_cursor_left(),
                KeyCode::Right => self.editor_cursor_right(),
                KeyCode::Up => self.editor_cursor_up(),
                KeyCode::Down => self.editor_cursor_down(),
                KeyCode::Home => self.editor_cursor_home(),
                KeyCode::End => self.editor_cursor_end(),
                KeyCode::Char(c) if !ctrl => self.editor_insert_char(c),
                _ => {}
            }
            return;
        }

        // ── Conversation-tab action shortcuts (no editor open).
        //    Lowercase `r` is reserved for view-level `reload` (handled
        //    above in handle_event); reply moves to uppercase `R` so
        //    both stay accessible.
        //    For `R` we match the uppercase glyph regardless of modifier
        //    — different terminals attach SHIFT, NONE, or even both to
        //    uppercase letters, so we just trust the produced char.
        if matches!(self.active_tab, Tab::Conversation) {
            let plain = key.modifiers == KeyModifiers::NONE;
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
        self.opened.and_then(|i| self.items.get(i)).map(|p| p.number)
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
        });
        // The conversation pane shrinks to make room for the editor;
        // we DON'T want any pending "scroll to selected" flag to fire
        // and re-anchor the view away from where the user was reading.
        self.conversation_scroll_to_selected = false;
    }

    fn start_reply(&mut self) {
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
                });
            }
            _ => {
                // GitHub-style quote reply: each line of the original
                // becomes `> <line>` (blank lines stay as `> ` so the
                // quote keeps its paragraph breaks), no `@author wrote:`
                // attribution, two blank lines after the quote so the
                // cursor lands on a fresh paragraph below it.
                let author = entry.author.clone();
                let body = entry.body.clone();
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
                });
            }
        }
        self.conversation_scroll_to_selected = false;
    }

    fn start_edit(&mut self) {
        let Some(entry) = self.selected_conversation_entry() else {
            return;
        };
        // Edit is only available on own comments — flagged on the
        // entry's author against the cached `me_login`.
        let is_me = self
            .me_login
            .as_deref()
            .map_or(false, |me| me == entry.author);
        if !is_me {
            self.tx.send(AppEvent::NotifyInfo(
                "You can only edit comments you wrote.".into(),
            ));
            return;
        }
        let kind = match (&entry.kind, entry.id) {
            (ConversationKind::Comment, Some(id)) => CommentEditorKind::EditIssue {
                comment_id: id,
            },
            (ConversationKind::ReviewComment { .. }, Some(id)) => {
                CommentEditorKind::EditReview { comment_id: id }
            }
            _ => {
                self.tx.send(AppEvent::NotifyInfo(
                    "This comment is not editable here.".into(),
                ));
                return;
            }
        };
        let buffer = entry.body.clone();
        let cursor = buffer.len();
        self.comment_editor = Some(CommentEditor {
            kind,
            buffer,
            cursor,
            submitting: false,
        });
        self.conversation_scroll_to_selected = false;
    }

    fn confirm_delete_comment(&mut self) {
        let Some(entry) = self.selected_conversation_entry() else {
            return;
        };
        let is_me = self
            .me_login
            .as_deref()
            .map_or(false, |me| me == entry.author);
        if !is_me {
            self.tx.send(AppEvent::NotifyInfo(
                "You can only delete comments you wrote.".into(),
            ));
            return;
        }
        let (id, is_review) = match (&entry.kind, entry.id) {
            (ConversationKind::Comment, Some(id)) => (id, false),
            (ConversationKind::ReviewComment { .. }, Some(id)) => (id, true),
            _ => {
                self.tx.send(AppEvent::NotifyInfo(
                    "This comment is not deletable.".into(),
                ));
                return;
            }
        };
        // Fire the delete without an additional confirm prompt — the
        // toast on completion + the destructive nature of the key (`d`)
        // are the explicit signal. We re-fetch on success to refresh
        // the thread.
        let Some(number) = self.opened_number() else {
            return;
        };
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = if is_review {
                crate::github::pr::delete_review_comment(&token, &coords, id)
            } else {
                crate::github::pr::delete_issue_comment(&token, &coords, id)
            };
            tx.send(AppEvent::PullRequestActionDone {
                number,
                action: "Comment deleted".into(),
                result,
            });
        });
    }

    fn submit_comment_editor(&mut self) {
        let Some(editor) = self.comment_editor.as_ref() else {
            return;
        };
        let body = editor.buffer.trim().to_string();
        if body.is_empty() {
            self.tx.send(AppEvent::NotifyWarn(
                "Comment body is empty — nothing to send.".into(),
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

    pub fn handle_click(&mut self, col: u16, row: u16) {
        match self.mode {
            Mode::List => {
                if let Some(idx) = self.row_at_list(row, col) {
                    if idx < self.items.len() {
                        self.hovered = idx;
                        self.open_hovered();
                    }
                }
            }
            Mode::Detail => {
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
                if let Some(idx) = self.row_at_list(row, col) {
                    if idx < self.items.len() && idx != self.hovered {
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
        if idx < self.items.len() {
            Some(idx)
        } else {
            None
        }
    }

    fn move_hovered(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let max = self.items.len() as i32 - 1;
        self.hovered = (self.hovered as i32 + delta).clamp(0, max) as usize;
    }

    /// Pan the visible list window by `delta` rows without touching the
    /// hovered cursor. Mouse wheel calls this.
    fn scroll_list(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let max = self.items.len().saturating_sub(1) as i32;
        let new = (self.list_scroll_offset as i32 + delta).clamp(0, max) as usize;
        self.list_scroll_offset = new;
    }

    // ---------- rendering ----------

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Clear hit-test rects from the previous frame so a stale rect
        // from a different layout never matches a click.
        self.list_area = None;
        self.tab_content_area = None;
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
        let theme = &self.ctx.color_theme;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(Span::styled(
                " Open PRs ",
                Style::default()
                    .fg(theme.fg)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        f.render_widget(block, area);
        self.list_area = Some(area);
        self.list_inner_y = inner.y;
        if self.items.is_empty() {
            self.list_scroll_offset = 0;
            let empty = Paragraph::new(Span::styled(
                "No open pull requests.",
                Style::default().fg(theme.detail_label_fg),
            ))
            .alignment(Alignment::Center);
            f.render_widget(empty, inner);
            return;
        }
        // Keyboard nav (↑↓ etc.) re-anchors the scroll so the hovered
        // row stays on-screen. Mouse-wheel scroll, on the other hand,
        // moves the viewport independently and may leave `hovered`
        // outside the visible window — that's by design.
        let visible = inner.height as usize;
        if self.hovered >= self.list_scroll_offset + visible {
            self.list_scroll_offset = self.hovered + 1 - visible;
        } else if self.hovered < self.list_scroll_offset {
            self.list_scroll_offset = self.hovered;
        }
        let max_offset = self.items.len().saturating_sub(visible);
        if self.list_scroll_offset > max_offset {
            self.list_scroll_offset = max_offset;
        }

        let items: Vec<ListItem<'static>> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, pr)| {
                // Triangle marker follows the keyboard / mouse cursor
                // (hovered), not the previously-opened PR — the user
                // sees at a glance which row will open on Enter/click.
                let is_marked = self.hovered == i;
                ListItem::new(self.format_pr_row(pr, is_marked))
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
        f.render_stateful_widget(list, inner, &mut state);
    }

    fn format_pr_row(&self, pr: &PullRequest, is_opened: bool) -> Line<'static> {
        let theme = &self.ctx.color_theme;
        // State word — colored text instead of a filled chip so it sits
        // alongside the commit-list style of plain coloured spans.
        let state_span = match (pr.state, pr.draft) {
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
                    .fg(theme.list_ref_branch_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            (PullState::Closed, _) => Span::styled(
                "CLOSED",
                Style::default()
                    .fg(theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            ),
        };
        // Base ref always renders as a REMOTE branch (it's the GitHub
        // target — the local repo may not even have it checked out, so
        // `list_ref_remote_branch_fg` matches what the graph would use).
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
                format!("#{}", pr.number),
                Style::default()
                    .fg(theme.list_hash_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                pr.title.clone(),
                Style::default().fg(theme.list_commit_message_fg),
            ),
            Span::raw("  "),
            Span::styled(pr.author.clone(), Style::default().fg(theme.list_name_fg)),
            Span::raw("  → "),
            Span::styled(
                pr.base_ref.clone(),
                Style::default()
                    .fg(theme.list_ref_remote_branch_fg)
                    .add_modifier(Modifier::BOLD),
            ),
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
        let Some(editor) = self.comment_editor.as_ref() else {
            return;
        };
        // Header: title + parent author hint for replies.
        let parent_author = match &editor.kind {
            CommentEditorKind::Reply { parent_id } => self
                .opened_detail()
                .and_then(|d| {
                    d.conversation
                        .iter()
                        .find(|e| e.id == Some(*parent_id))
                        .map(|e| e.author.as_str())
                }),
            _ => None,
        };
        let title = editor.kind.header(parent_author);

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

        // Split the inner area into body + footer (single-line hint).
        let body_height = inner.height.saturating_sub(1).max(1);
        let body_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: body_height,
        };
        let footer_area = Rect {
            x: inner.x,
            y: inner.y + body_height,
            width: inner.width,
            height: 1,
        };

        // Body: render each logical line as-is, clipped to body_height.
        let value = Style::default().fg(theme.fg);
        let body_text = if editor.buffer.is_empty() {
            // Placeholder hint so the box doesn't look broken when empty.
            vec![Span::styled(
                match &editor.kind {
                    CommentEditorKind::NewTopLevel => "Type your comment…",
                    CommentEditorKind::Reply { .. } => "Type your reply…",
                    _ => "Type your edits…",
                }
                .to_string(),
                Style::default().fg(theme.detail_label_fg),
            )]
        } else {
            // We render the raw buffer verbatim (no markdown — author is
            // still composing, no need to pre-render).
            editor
                .buffer
                .lines()
                .take(body_height as usize)
                .map(|l| Span::styled(l.to_string(), value))
                .collect()
        };
        let lines: Vec<Line<'static>> = body_text
            .into_iter()
            .map(|s| Line::from(s))
            .collect();
        let para = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });
        f.render_widget(para, body_area);

        // Footer: shortcuts hint OR "Sending…" while a write is in flight.
        // We list Ctrl+S first since Ctrl+Enter doesn't survive in most
        // terminals (modifier gets stripped from the Enter sequence).
        let footer_text = if editor.submitting {
            "  Sending…".to_string()
        } else {
            "  Ctrl+S:send   Esc:cancel".to_string()
        };
        let footer_style = if editor.submitting {
            Style::default().fg(theme.status_warn_fg)
        } else {
            Style::default().fg(theme.detail_label_fg)
        };
        f.render_widget(
            Paragraph::new(Span::styled(footer_text, footer_style)),
            footer_area,
        );

        // Place the terminal cursor on the editor surface at the
        // logical (col, row) corresponding to the buffer cursor.
        let (col, row) = cursor_screen_pos(&editor.buffer, editor.cursor);
        let cx = body_area.x + col;
        let cy = body_area.y + row;
        // Clamp to the body area.
        if !editor.submitting
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
        if let Some(detail) = detail {
            spans.push(state_word_span(theme, detail.state, detail.draft));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                format!("#{}", detail.number),
                Style::default()
                    .fg(theme.list_hash_fg)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                truncate(&detail.title, 80),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                detail.author.clone(),
                Style::default().fg(theme.list_name_fg),
            ));
            spans.push(Span::raw("  "));
            spans.push(Span::styled(
                detail.head_label.clone(),
                Style::default()
                    .fg(theme.list_ref_branch_fg)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(" → ", Style::default().fg(theme.detail_label_fg)));
            spans.push(Span::styled(
                detail.base_ref.clone(),
                Style::default()
                    .fg(theme.list_ref_remote_branch_fg)
                    .add_modifier(Modifier::BOLD),
            ));
        } else if let Some(n) = number {
            spans.push(Span::styled(
                format!("Loading PR #{}…", n),
                Style::default().fg(theme.detail_label_fg),
            ));
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
                when: "",
                body: &detail.body,
                ancestor_gutters: &[],
                has_children: false,
                is_selected: self.conversation_selected == pr_idx,
                is_me: pr_is_me,
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

        let items: Vec<ListItem<'static>> = detail
            .commit_list
            .iter()
            .map(|c| ListItem::new(commit_row(theme, c)))
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
        let items: Vec<ListItem<'static>> = detail
            .files
            .iter()
            .map(|f| ListItem::new(file_line(theme, f)))
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
    let after_title = card_width.saturating_sub(consumed).saturating_sub(2);
    top.push(Span::styled(format!(" {}┐", "─".repeat(after_title)), border_style));
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
                .fg(theme.list_ref_branch_fg)
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

fn commit_row(theme: &crate::color::ColorTheme, c: &PullCommit) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            c.short_sha.clone(),
            Style::default()
                .fg(theme.list_hash_fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            c.subject.clone(),
            Style::default().fg(theme.list_commit_message_fg),
        ),
        Span::raw("  "),
        Span::styled(c.author.clone(), Style::default().fg(theme.list_name_fg)),
        Span::raw("  "),
        Span::styled(c.date.clone(), Style::default().fg(theme.list_date_fg)),
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

fn file_line(theme: &crate::color::ColorTheme, f: &crate::github::pr::PullFile) -> Line<'static> {
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
        Span::styled(f.filename.clone(), Style::default().fg(theme.fg)),
        Span::raw("  "),
        Span::styled(
            format!("+{}", f.additions),
            Style::default().fg(theme.detail_file_change_add_fg),
        ),
        Span::raw(" "),
        Span::styled(
            format!("-{}", f.deletions),
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
fn render_markdown_body(
    body: &str,
    theme: &crate::color::ColorTheme,
    inner_width: usize,
) -> Vec<Vec<Span<'static>>> {
    let stripped = strip_html(body);
    let mut out: Vec<Vec<Span<'static>>> = Vec::new();
    let mut prev_blank = true; // suppress leading blank rows
    let mut in_code_block = false;

    let normal = Style::default().fg(theme.fg);
    let label = Style::default().fg(theme.detail_label_fg);
    let code_style = Style::default().fg(theme.list_hash_fg);
    let h2_style = Style::default()
        .fg(theme.list_head_fg)
        .add_modifier(Modifier::BOLD);
    let h3_style = Style::default()
        .fg(theme.fg)
        .add_modifier(Modifier::BOLD);

    for raw_line in stripped.lines() {
        let line = raw_line.trim_end();

        // Code-block fence — toggle state, swallow the line.
        if line.trim().starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            for w in wrap_text(line, inner_width) {
                out.push(vec![Span::styled(w, code_style)]);
            }
            prev_blank = false;
            continue;
        }

        if line.trim().is_empty() {
            if !prev_blank {
                out.push(vec![]);
            }
            prev_blank = true;
            continue;
        }
        prev_blank = false;

        // Horizontal rule
        let t = line.trim();
        if t == "---" || t == "***" || t == "___" {
            out.push(vec![Span::styled(
                "─".repeat(inner_width.max(4)),
                Style::default().fg(theme.divider_fg),
            )]);
            continue;
        }

        // Headings
        if let Some(rest) = strip_heading(line, "### ") {
            for w in wrap_text(rest, inner_width) {
                out.push(vec![Span::styled(w, h3_style)]);
            }
            continue;
        }
        if let Some(rest) = strip_heading(line, "## ") {
            for w in wrap_text(rest, inner_width) {
                out.push(vec![Span::styled(w, h2_style)]);
            }
            continue;
        }
        if let Some(rest) = strip_heading(line, "# ") {
            for w in wrap_text(rest, inner_width) {
                out.push(vec![Span::styled(w, h2_style)]);
            }
            continue;
        }

        // Blockquote
        if let Some(rest) = line.strip_prefix("> ") {
            let inline = parse_inline_markdown(rest, theme);
            let avail = inner_width.saturating_sub(2);
            for line_spans in wrap_styled_spans(inline, avail) {
                let mut row = vec![Span::styled("│ ".to_string(), label)];
                row.extend(line_spans);
                out.push(row);
            }
            continue;
        }

        // List item (`-`, `*`, or indented sub-bullet)
        let (prefix, content, prefix_width) = if let Some(rest) = line.strip_prefix("- ") {
            (Some("• "), rest, 2)
        } else if let Some(rest) = line.strip_prefix("* ") {
            (Some("• "), rest, 2)
        } else if let Some(rest) = line.strip_prefix("  - ") {
            (Some("  ◦ "), rest, 4)
        } else if let Some(rest) = line.strip_prefix("  * ") {
            (Some("  ◦ "), rest, 4)
        } else {
            (None, line, 0)
        };

        let inline = parse_inline_markdown(content, theme);
        let avail = inner_width.saturating_sub(prefix_width);
        let wrapped = wrap_styled_spans(inline, avail);
        for (i, line_spans) in wrapped.into_iter().enumerate() {
            let mut row = Vec::new();
            if let Some(p) = prefix {
                let s = if i == 0 {
                    p.to_string()
                } else {
                    " ".repeat(prefix_width)
                };
                row.push(Span::styled(s, label));
            }
            row.extend(line_spans);
            out.push(row);
        }
    }

    // Trim trailing blank rows for cleaner cards.
    while out.last().map_or(false, |row| {
        row.iter()
            .all(|s| s.content.as_ref().trim().is_empty())
    }) {
        out.pop();
    }

    let _ = normal;
    out
}

fn strip_heading<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    // Strip the heading marker but only when it's at the start. We don't
    // bother with closing `#` on the right (rare in GitHub markdown).
    line.strip_prefix(marker)
}

/// Inline markdown for a single content string. Produces a flat list of
/// styled spans honouring **bold**, *italic*, `inline code`, and
/// `[label](url)` links (label only).
fn parse_inline_markdown(
    text: &str,
    theme: &crate::color::ColorTheme,
) -> Vec<Span<'static>> {
    let normal = Style::default().fg(theme.fg);
    let bold = normal.add_modifier(Modifier::BOLD);
    let italic = normal.add_modifier(Modifier::ITALIC);
    let code = Style::default().fg(theme.list_hash_fg);
    let link = Style::default()
        .fg(theme.list_head_fg)
        .add_modifier(Modifier::UNDERLINED);

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut chars = text.chars().peekable();

    let flush_buf = |buf: &mut String, spans: &mut Vec<Span<'static>>, style: Style| {
        if !buf.is_empty() {
            spans.push(Span::styled(std::mem::take(buf), style));
        }
    };

    while let Some(c) = chars.next() {
        match c {
            // **bold**
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                flush_buf(&mut buf, &mut spans, normal);
                let mut bold_text = String::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        closed = true;
                        break;
                    }
                    bold_text.push(c);
                }
                if closed {
                    spans.push(Span::styled(bold_text, bold));
                } else {
                    // Unclosed → render as plain text with the leading **
                    buf.push_str("**");
                    buf.push_str(&bold_text);
                }
            }
            // *italic* / _italic_
            '*' | '_' => {
                // Only treat as italic when the closing delimiter is
                // found before whitespace breaks the run.
                let delim = c;
                flush_buf(&mut buf, &mut spans, normal);
                let mut italic_text = String::new();
                let mut closed = false;
                let saved_iter = chars.clone();
                while let Some(c) = chars.next() {
                    if c == delim {
                        closed = true;
                        break;
                    }
                    italic_text.push(c);
                }
                if closed && !italic_text.is_empty() {
                    spans.push(Span::styled(italic_text, italic));
                } else {
                    // Rewind: treat as literal.
                    chars = saved_iter;
                    buf.push(delim);
                }
            }
            // `inline code`
            '`' => {
                flush_buf(&mut buf, &mut spans, normal);
                let mut code_text = String::new();
                let mut closed = false;
                let saved_iter = chars.clone();
                while let Some(c) = chars.next() {
                    if c == '`' {
                        closed = true;
                        break;
                    }
                    code_text.push(c);
                }
                if closed {
                    spans.push(Span::styled(code_text, code));
                } else {
                    chars = saved_iter;
                    buf.push('`');
                }
            }
            // [label](url)
            '[' => {
                let saved_iter = chars.clone();
                let mut label_text = String::new();
                let mut found_close = false;
                while let Some(c) = chars.next() {
                    if c == ']' {
                        found_close = true;
                        break;
                    }
                    label_text.push(c);
                }
                if found_close && chars.peek() == Some(&'(') {
                    chars.next(); // consume '('
                    let mut url = String::new();
                    let mut closed_paren = false;
                    while let Some(c) = chars.next() {
                        if c == ')' {
                            closed_paren = true;
                            break;
                        }
                        url.push(c);
                    }
                    if closed_paren {
                        flush_buf(&mut buf, &mut spans, normal);
                        let _ = url;
                        spans.push(Span::styled(label_text, link));
                        continue;
                    }
                }
                // Not a link — restore.
                chars = saved_iter;
                buf.push('[');
            }
            _ => buf.push(c),
        }
    }
    flush_buf(&mut buf, &mut spans, normal);
    spans
}

/// Word-wrap a plain string into multiple lines of at most `max_width`
/// display columns. Wraps at whitespace boundaries; falls back to a
/// hard cut if a single token exceeds the width.
fn wrap_text(s: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![s.to_string()];
    }
    if console::measure_text_width(s) <= max_width {
        return vec![s.to_string()];
    }
    let mut lines: Vec<String> = vec![String::new()];
    let mut cur_w = 0usize;
    for token in split_into_tokens(s) {
        let tw = console::measure_text_width(&token);
        let is_space = token.chars().all(char::is_whitespace);
        if cur_w + tw > max_width && cur_w > 0 {
            lines.push(String::new());
            cur_w = 0;
            if is_space {
                continue;
            }
        }
        if tw > max_width {
            // Token alone is wider than the line — hard split it.
            for ch in token.chars() {
                let cw = console::measure_text_width(&ch.to_string());
                if cur_w + cw > max_width && cur_w > 0 {
                    lines.push(String::new());
                    cur_w = 0;
                }
                lines.last_mut().unwrap().push(ch);
                cur_w += cw;
            }
        } else {
            lines.last_mut().unwrap().push_str(&token);
            cur_w += tw;
        }
    }
    lines
}

/// Same as `wrap_text` but for an already-styled span list. Preserves
/// each span's style across wraps.
fn wrap_styled_spans(
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

/// Best-effort HTML → plain text for PR / comment bodies. GitHub
/// Flavoured Markdown allows inline HTML (especially `<a href>` from
/// suggestion footers and tip blocks) which renders as literal tag
/// soup in a TUI. We:
/// - Replace `<a href="X">label</a>` with just `label`.
/// - Replace `<br>` / `<br/>` with `\n`.
/// - Strip every other `<…>` tag.
/// - Leave HTML entities like `&amp;` alone for now (rare in commit/PR bodies).
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '<' {
            out.push(c);
            continue;
        }
        // Find the matching '>' in the remaining string.
        let rest = &s[i + 1..];
        let close = match rest.find('>') {
            Some(c) => c,
            None => {
                out.push('<');
                continue;
            }
        };
        let tag_inner = &rest[..close];
        let lower = tag_inner.to_ascii_lowercase();
        let stripped = lower.trim();
        // <br> / <br/> become real line breaks.
        if stripped == "br" || stripped == "br/" || stripped == "br /" {
            out.push('\n');
        }
        // Skip past the '>' — consume chars until we pass that byte index.
        // `i + 1 + close + 1` is the absolute byte position just after '>'.
        let target = i + 1 + close + 1;
        while let Some(&(j, _)) = chars.peek() {
            if j >= target {
                break;
            }
            chars.next();
        }
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
