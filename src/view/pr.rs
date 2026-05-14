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

/// GitHub's purple for merged PRs + closed-as-completed issues —
/// matches the badge color on
/// github.com so the visual cue is instantly recognizable.
pub(crate) const MERGED_PURPLE: Color = Color::Rgb(0x89, 0x57, 0xe5);

/// Teal accent reserved for the `owner/repo` identifier in the
/// top header of the PR + Issues views. Distinct from every other
/// in-use palette token (orange brand, green/red status, purple
/// merged, blue branches) so the repo label reads as its own kind
/// of token instead of being mistaken for a hash, a branch, or a
/// label chip.
pub(crate) const REPO_TEAL: Color = Color::Rgb(0x4f, 0xc7, 0xb8);

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
    /// GitHub avatars to paint after the Conversation tab Paragraph
    /// renders. Populated during render, drained at end of frame.
    conversation_avatar_slots: Vec<AvatarSlot>,
    /// Same as `conversation_avatar_slots` but for the PR list rows.
    list_avatar_slots: Vec<AvatarSlot>,
    /// Full set of avatars painted on the screen last frame. ONE
    /// vec for the whole view so cross-section transitions (list →
    /// detail, commits → conversation, …) automatically clear the
    /// avatars from the previous section: they end up in `prev`
    /// but not in `current`, so the diff emits clear cells for
    /// every stale position.
    prev_painted_avatars: Vec<PaintedAvatar>,
    /// Accumulator filled by each render path during a frame.
    /// Drained and diffed against `prev_painted_avatars` at the
    /// very end of `render()`. Always cleared at frame start.
    pending_avatar_paints: Vec<(PaintedAvatar, Color)>,
    /// Body rect of the inline editor captured during render — used
    /// to translate clicks inside it into buffer cursor positions.
    editor_body_area: Option<Rect>,
    /// Logical line range of each comment in the conversation render —
    /// `(comment_idx, first_line, last_line)` — captured at render time
    /// so mouse hits and auto-scroll can resolve which card sits where.
    conversation_comment_spans: Vec<(usize, usize, usize)>,
    /// Clickable `#N` hit-boxes inside the conversation, captured at
    /// render time. Combined with `conversation_scroll` to resolve
    /// screen coordinates → ref number.
    conversation_ref_links: Vec<crate::view::issue::RefLink>,
    /// Cache of recently-fetched issue numbers for this repo, used to
    /// resolve `#N` references in PR comments. Populated lazily when
    /// a PR is opened so the colouring + click hit-test know whether
    /// a number is an issue or a PR.
    mention_issue_numbers: rustc_hash::FxHashSet<u64>,
    /// True once an issue-number fetch has been spawned or completed
    /// — guards against duplicate background calls.
    mention_issues_fetched: bool,
    /// Floating `#` autocomplete popup state, shared shape with the
    /// Issues view via `crate::view::issue`.
    mention_popup: Option<crate::view::issue::MentionPopup>,
    /// Snapshot of issues + PRs (title + number) used to feed the
    /// mention popup's filter. Built on-demand from `self.items` and
    /// a separate issue fetch.
    mention_issue_titles: rustc_hash::FxHashMap<u64, String>,
    /// In-session log of the viewer's reactions on each conversation
    /// entry — keyed by `(pr_number, target_idx)`. Same shape as the
    /// Issues view: stores `(kind, reaction_id)` so the picker can
    /// highlight my chips red AND a second click can DELETE.
    viewer_reactions:
        rustc_hash::FxHashMap<(u64, usize), Vec<(crate::github::pr::ReactionKind, u64)>>,
    /// Computed once per render: how many comments make up the
    /// Conversation tab. Drives the keyboard nav clamping.
    conversation_comment_count: usize,
    commits_scroll: usize,
    commits_hovered: usize,
    checks_scroll: usize,
    checks_hovered: usize,
    files_scroll: usize,
    files_hovered: usize,
    /// Index of the file the user drilled into within the Files tab.
    /// `Some(i)` swaps the file list for a full diff view of that
    /// entry; Esc clears it.
    files_drilldown: Option<usize>,
    /// Vertical scroll within the drilled-down file's patch.
    files_drilldown_scroll: usize,
    /// SHA of the commit the user drilled into within the Commits
    /// tab. `Some` swaps the commit list for the commit's full
    /// message + per-file diff; Esc clears it.
    commits_drilldown: Option<String>,
    /// Per-commit detail cache (SHA → fetched payload). First click
    /// spawns a background fetch; subsequent ones are instant.
    commit_detail_cache: FxHashMap<String, crate::github::pr::CommitDetail>,
    /// Set of commit SHAs whose detail fetch is currently in flight
    /// — drives the "Loading…" placeholder.
    commit_detail_loading: rustc_hash::FxHashSet<String>,
    /// Vertical scroll within the drilled-down commit's diff.
    commits_drilldown_scroll: usize,
    /// Active draft when `mode == Mode::Compose`. Lazily built on
    /// `start_compose_pr` from the current local HEAD; cleared on
    /// cancel or successful submission.
    compose: Option<ComposeState>,
    /// Click hit-test rects for the compose-form fields — populated
    /// during render, consumed by `handle_click`.
    compose_field_rects: Vec<(ComposeField, Rect)>,
    /// Active branch-picker overlay (drawn on top of the compose
    /// form). When `Some`, all key + click events route to it.
    branch_picker: Option<BranchPicker>,
    /// Active reaction-picker overlay (drawn on top of the comment
    /// the user fired `+` on). When `Some`, key events route to it.
    reaction_picker: Option<ReactionPicker>,
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
    /// Full-screen "compose a new pull request" form with a live
    /// preview of the commits / files that will land on creation.
    /// Entered with `n` from the list, exited via `Esc` (cancel) or
    /// `Ctrl+S` (submit).
    Compose,
}

/// One field of the compose form — drives `↑↓` cycling, the cursor
/// indicator, and which key handler runs for typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposeField {
    Head,
    Base,
    Title,
    Body,
    Labels,
    Draft,
}

impl ComposeField {
    fn all() -> &'static [ComposeField] {
        &[
            ComposeField::Head,
            ComposeField::Base,
            ComposeField::Title,
            ComposeField::Body,
            ComposeField::Labels,
            ComposeField::Draft,
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

/// Small inline picker for the 8 GitHub reactions. Stays visually
/// close to its target comment instead of floating mid-screen so
/// the user sees what they're reacting to.
#[derive(Debug, Clone)]
struct ReactionPicker {
    /// Conversation index of the comment being reacted to — feeds
    /// back into `selected_conversation_entry` on confirm.
    target_idx: usize,
    /// 0..8 — which of the eight reactions is currently focused.
    hovered: usize,
    overlay_rect: Option<Rect>,
    row_rects: Vec<Rect>,
}

/// Scrollable single-select branch picker — opens as an overlay
/// when the user activates `Head:` or `Base:` in the compose form,
/// closes back to the form on Enter (selects) or Esc (cancels).
#[derive(Debug, Clone)]
struct BranchPicker {
    target_field: ComposeField,
    branches: Vec<BranchPickerEntry>,
    hovered: usize,
    scroll: usize,
    overlay_rect: Option<Rect>,
    row_rects: Vec<Rect>,
    /// Inner height captured at the last render — used by keyboard
    /// nav to keep the hovered row inside the viewport.
    visible_height: usize,
}

#[derive(Debug, Clone)]
struct BranchPickerEntry {
    name: String,
    kind: BranchKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BranchKind {
    Local,
    Remote,
}

/// In-progress draft for `Mode::Compose` — every field is editable,
/// the preview pane is refreshed in the background each time `head`
/// or `base` change so the user sees what will land on submit.
#[derive(Debug, Clone)]
struct ComposeState {
    head: String,
    base: String,
    title: String,
    body: String,
    draft: bool,
    focused: ComposeField,
    /// Byte cursor inside the field currently being edited (we re-
    /// use the same field for whichever of head/base/title/body has
    /// focus — they're never edited simultaneously).
    cursor: usize,
    body_scroll: u16,
    /// Height of the body block's inner area captured at the last
    /// render. Drives mouse-wheel + cursor-anchor clamping without
    /// having to re-derive the layout outside the render path.
    body_last_height: u16,
    /// Labels the user picked in the compose form — applied right
    /// after the PR is created (REST has no `labels` field on the
    /// create endpoint).
    labels: Vec<crate::github::pr::Label>,
    /// Reviewers the user picked in the compose form — applied
    /// right after the PR is created (same reason as labels).
    reviewers: Vec<String>,
    /// Preview metadata fetched from `git log base..head` — `None`
    /// until the first computation finishes.
    preview: Option<ComposePreview>,
    preview_loading: bool,
    submitting: bool,
}

#[derive(Debug, Clone)]
struct ComposePreview {
    commits: Vec<crate::github::pr::PullCommit>,
    files: Vec<crate::github::pr::PullFile>,
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
    /// Width of the labels column — sized to the worst-case label
    /// count across visible PRs so rows align. A label chip is
    /// `LABEL_CHIP_WIDTH` cols wide (2 letters + 2 padding spaces).
    labels: usize,
    author: usize,
}

/// How many label chips to show inline in the list, max. Beyond
/// this the row would crowd the title — anyone needing all labels
/// can drill into the PR detail to see them.
const MAX_INLINE_LABELS: usize = 3;
/// Width of a single short label chip (` XX `). Two letters of the
/// label name, padded with one space on each side so the bg colour
/// reads as a chip rather than text.
const LABEL_CHIP_WIDTH: usize = 4;

impl PrListColumns {
    /// Fixed visual overhead between columns:
    ///   "▶ " (2) + state + "  " (2) + number + "  " (2)
    ///   + title + " " (1) + labels + "  " (2) + author + " · " (3)
    ///   + head + " → " (3) + base
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

        // Labels column = (worst-case chip count × chip width). When
        // no visible PR has labels the column collapses to 0.
        let max_chips_per_row = items
            .iter()
            .map(|pr| pr.labels.len().min(MAX_INLINE_LABELS))
            .max()
            .unwrap_or(0);
        let labels = max_chips_per_row * LABEL_CHIP_WIDTH;
        // 1-col gap before the labels chunk (only if the chunk exists).
        let labels_gap: usize = if labels > 0 { 1 } else { 0 };

        // Title flexes between fixed overhead and the worst-case
        // unpadded head/base. If everything fits, title sits at its
        // natural max; otherwise it shrinks and ellipses to `…`.
        let fixed = Self::MARKER
            + state
            + Self::GAP
            + number
            + Self::GAP
            + labels_gap
            + labels
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
            labels,
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
pub(crate) fn fit_cell(s: &str, width: usize) -> String {
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
            conversation_ref_links: Vec::new(),
            mention_issue_numbers: rustc_hash::FxHashSet::default(),
            mention_issues_fetched: false,
            mention_popup: None,
            mention_issue_titles: rustc_hash::FxHashMap::default(),
            viewer_reactions: rustc_hash::FxHashMap::default(),
            conversation_comment_count: 1,
            comment_editor: None,
            comment_editor_cursor_pos: None,
            conversation_avatar_slots: Vec::new(),
            list_avatar_slots: Vec::new(),
            prev_painted_avatars: Vec::new(),
            pending_avatar_paints: Vec::new(),
            editor_body_area: None,
            commits_scroll: 0,
            commits_hovered: 0,
            checks_scroll: 0,
            checks_hovered: 0,
            files_scroll: 0,
            files_hovered: 0,
            files_drilldown: None,
            files_drilldown_scroll: 0,
            commits_drilldown: None,
            commit_detail_cache: FxHashMap::default(),
            commit_detail_loading: rustc_hash::FxHashSet::default(),
            commits_drilldown_scroll: 0,
            compose: None,
            compose_field_rects: Vec::new(),
            branch_picker: None,
            reaction_picker: None,
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

    /// Re-enter Detail mode pointed at `pr_number`, used by the
    /// app when bouncing back from a CommitDetail or DiffView that
    /// was launched from this PR.
    pub fn reopen_pr(&mut self, pr_number: u64) {
        self.opened_pr_number = Some(pr_number);
        self.mode = Mode::Detail;
        self.active_tab = Tab::Conversation;
        self.files_drilldown = None;
        self.commits_drilldown = None;
        if !self.detail_cache.contains_key(&pr_number) {
            self.spawn_detail_fetch(pr_number);
        }
    }

    /// Open the given PR's detail view — dispatched from cross-view
    /// nav (e.g. clicking a `#N` reference in the Issues view).
    /// Also re-anchors the list cursor so returning with Esc lands
    /// the user on the row they navigated through.
    pub fn open_by_number(&mut self, pr_number: u64) {
        if let Some(idx) = self.items.iter().position(|p| p.number == pr_number) {
            self.hovered = idx;
        }
        self.opened_pr_number = Some(pr_number);
        self.mode = Mode::Detail;
        self.active_tab = Tab::Conversation;
        self.files_drilldown = None;
        self.commits_drilldown = None;
        if !self.detail_cache.contains_key(&pr_number) {
            self.spawn_detail_fetch(pr_number);
        }
        self.spawn_mention_issue_numbers_fetch();
    }

    /// Background fetch of every issue number in this repo so the
    /// PR view can resolve `#N` references in comments (issue vs PR).
    /// Idempotent — once kicked off it never retries.
    fn spawn_mention_issue_numbers_fetch(&mut self) {
        if self.mention_issues_fetched {
            return;
        }
        self.mention_issues_fetched = true;
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let issues: Vec<(u64, String)> = crate::github::issue::list_issues(&token, &coords)
                .map(|v| v.into_iter().map(|i| (i.number, i.title)).collect())
                .unwrap_or_default();
            tx.send(AppEvent::PrMentionIssuesFetched { issues });
        });
    }

    pub fn on_mention_issues_fetched(&mut self, issues: Vec<(u64, String)>) {
        self.mention_issue_numbers = issues.iter().map(|(n, _)| *n).collect();
        self.mention_issue_titles = issues.into_iter().collect();
    }

    fn mention_universe(&self) -> Vec<crate::view::issue::MentionItem> {
        use crate::view::issue::{MentionItem, MentionKind};
        let mut out: Vec<MentionItem> = Vec::new();
        for (n, title) in &self.mention_issue_titles {
            out.push(MentionItem {
                number: *n,
                title: title.clone(),
                kind: MentionKind::Issue,
            });
        }
        for pr in &self.items {
            out.push(MentionItem {
                number: pr.number,
                title: pr.title.clone(),
                kind: MentionKind::Pr,
            });
        }
        out.sort_by(|a, b| b.number.cmp(&a.number));
        out
    }

    fn open_mention_popup(&mut self, anchor: usize, target: crate::view::issue::MentionTarget) {
        use crate::view::issue::{filter_mention_items_pub, MentionPopup};
        self.spawn_mention_issue_numbers_fetch();
        let universe = self.mention_universe();
        let filtered = filter_mention_items_pub("", &universe);
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

    fn refresh_mention_filter(&mut self) {
        use crate::view::issue::filter_mention_items_pub;
        let universe = self.mention_universe();
        if let Some(p) = self.mention_popup.as_mut() {
            p.filtered = filter_mention_items_pub(&p.query, &universe);
            if p.hovered >= p.filtered.len() {
                p.hovered = p.filtered.len().saturating_sub(1);
            }
        }
    }

    fn update_mention_query_from_editor(&mut self) {
        use crate::view::issue::MentionTarget;
        let (target, anchor, cursor, buf): (MentionTarget, usize, usize, String) = {
            let Some(p) = self.mention_popup.as_ref() else {
                return;
            };
            match p.target {
                MentionTarget::CommentEditor => {
                    let Some(ed) = self.comment_editor.as_ref() else {
                        return;
                    };
                    (p.target, p.anchor, ed.cursor, ed.buffer.clone())
                }
                MentionTarget::ComposeBody => {
                    let Some(c) = self.compose.as_ref() else {
                        return;
                    };
                    (p.target, p.anchor, c.cursor, c.body.clone())
                }
            }
        };
        let _ = target;
        // Sanity: the `#` must still be at the anchor and the cursor
        // must sit AFTER it. `cursor <= anchor` covers ← past the `#`
        // and selecting/clicking before it — without this stricter
        // bound, `buf[anchor + 1..cursor]` below panics with
        // "begin > end" the moment the user presses ←.
        if buf.as_bytes().get(anchor).copied() != Some(b'#') || cursor <= anchor {
            self.close_mention_popup();
            return;
        }
        // Query is the chars between `#` (exclusive) and cursor.
        // Belt-and-suspenders clamp: even with the guards above, an
        // inconsistent (cursor, buf.len()) snapshot mid-edit would
        // panic the slice. Force ordered, in-bounds indices.
        let start = (anchor + 1).min(buf.len());
        let end = cursor.min(buf.len()).max(start);
        let query: String = buf[start..end]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        let query_end = anchor + 1 + query.len();
        if query_end < cursor {
            // Cursor moved past the typed query — drop the popup.
            self.close_mention_popup();
            return;
        }
        if let Some(p) = self.mention_popup.as_mut() {
            p.query = query;
        }
        self.refresh_mention_filter();
    }

    fn pick_mention(&mut self) {
        use crate::view::issue::MentionTarget;
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
        // Clamp the splice range to ordered, in-bounds indices.
        // Without this, an inconsistent (anchor, buf) snapshot
        // (e.g. anchor past end after a multi-key edit) panics
        // `replace_range` with "begin > end".
        match target {
            MentionTarget::CommentEditor => {
                let Some(ed) = self.comment_editor.as_mut() else {
                    return;
                };
                let start = anchor.min(ed.buffer.len());
                let end = (anchor + 1 + query_len).min(ed.buffer.len()).max(start);
                ed.buffer.replace_range(start..end, &replacement);
                ed.cursor = start + replacement.len();
            }
            MentionTarget::ComposeBody => {
                let Some(c) = self.compose.as_mut() else {
                    return;
                };
                let start = anchor.min(c.body.len());
                let end = (anchor + 1 + query_len).min(c.body.len()).max(start);
                c.body.replace_range(start..end, &replacement);
                c.cursor = start + replacement.len();
            }
        }
    }

    fn handle_event_mention_popup(&mut self, key: KeyEvent) {
        use ratatui::crossterm::event::KeyCode;
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
        // Pass-through to the surface the popup is anchored on, then
        // re-derive the query from the resulting buffer state.
        use crate::view::issue::MentionTarget;
        use ratatui::crossterm::event::KeyModifiers;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let target = self
            .mention_popup
            .as_ref()
            .map(|p| p.target)
            .unwrap_or(MentionTarget::CommentEditor);
        match target {
            MentionTarget::CommentEditor => match key.code {
                KeyCode::Backspace if ctrl => self.editor_delete_word_left(),
                KeyCode::Backspace => self.editor_delete_left(),
                KeyCode::Delete => self.editor_delete_right(),
                KeyCode::Left => self.editor_cursor_left(),
                KeyCode::Right => self.editor_cursor_right(),
                KeyCode::Home => self.editor_cursor_home(),
                KeyCode::End => self.editor_cursor_end(),
                KeyCode::Char(c) if !ctrl => self.editor_insert_char(c),
                _ => {}
            },
            MentionTarget::ComposeBody => {
                if let Some(state) = self.compose.as_mut() {
                    if !matches!(state.focused, ComposeField::Body) {
                        state.focused = ComposeField::Body;
                    }
                    match key.code {
                        KeyCode::Backspace if ctrl => compose_field_delete_word_left(state),
                        KeyCode::Backspace => compose_field_delete_left(state),
                        KeyCode::Char('h') if ctrl => compose_field_delete_word_left(state),
                        KeyCode::Char('w') if ctrl => compose_field_delete_word_left(state),
                        KeyCode::Left => compose_field_cursor_left(state),
                        KeyCode::Right => compose_field_cursor_right(state),
                        KeyCode::Home => compose_field_cursor_home(state),
                        KeyCode::End => compose_field_cursor_end(state),
                        KeyCode::Char(c) if !ctrl => compose_field_insert_char(state, c),
                        _ => {}
                    }
                }
                self.compose_body_anchor_to_cursor();
            }
        }
        self.update_mention_query_from_editor();
    }

    fn mention_popup_scroll(&mut self, delta: i32) {
        let Some(p) = self.mention_popup.as_mut() else {
            return;
        };
        let vis = p.last_visible.max(1) as usize;
        let max_scroll = p.filtered.len().saturating_sub(vis);
        let new = (p.scroll as i32 + delta).max(0) as usize;
        p.scroll = new.min(max_scroll);
    }

    /// Resolver-style lookup used during conversation render.
    /// Returns `Some((colour, is_pr))` if the number is known to
    /// match a PR or an issue in this repo.
    fn resolve_hash_ref(&self, n: u64) -> Option<(Color, bool)> {
        if self.items.iter().any(|p| p.number == n) {
            return Some((self.ctx.color_theme.list_hash_fg, true));
        }
        if self.mention_issue_numbers.contains(&n) {
            return Some((self.ctx.color_theme.status_success_fg, false));
        }
        None
    }

    /// Find the first captured `#N` reference whose line falls
    /// inside the currently-selected comment's span and follow it.
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

    /// Navigate to a `#N` reference — PRs pivot in-place via
    /// `open_by_number`, issues hop to the Issues view via
    /// cross-view dispatch.
    fn follow_reference(&mut self, number: u64, is_pr: bool) {
        if !is_pr {
            self.comment_editor = None;
            self.tx.send(AppEvent::OpenIssueDetail { number });
            return;
        }
        self.open_by_number(number);
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
        // Compose owns its own footer hints — `↑↓` nav + submit +
        // cancel. The branch picker, when open, narrows it further
        // to just nav + select + cancel.
        if matches!(self.mode, Mode::Compose) {
            let submitting = self
                .compose
                .as_ref()
                .map_or(false, |c| c.submitting);
            if submitting {
                return format!("⌘ {}", ["Submitting…", "Esc:cancel"].join("▕▏"));
            }
            if self.branch_picker.is_some() {
                return format!(
                    "⌘ {}",
                    ["↑↓:select", "Enter:pick", "Esc:close"].join("▕▏")
                );
            }
            return format!(
                "⌘ {}",
                ["↑↓:field", "Ctrl+S:create", "Esc:cancel"].join("▕▏")
            );
        }
        let parts: Vec<&str> = match self.mode {
            Mode::List => vec!["n:new PR", "r:reload"],
            Mode::Compose => vec![],
            Mode::Detail => {
                // Inside a drill-down the footer collapses to just
                // Esc — every other shortcut belongs to the list view.
                if self.files_drilldown.is_some()
                    || self.commits_drilldown.is_some()
                {
                    return format!("⌘ {}", "Esc:back");
                }
                // Hold the footer until the PR detail has fully
                // loaded — surfacing `c:comment`, `a:approve`, etc.
                // before the data lands would let the user fire
                // actions on a half-populated PR. Only `r:reload`
                // and `Esc` are safe before the fetch completes.
                if self.opened_detail().is_none() {
                    return format!("⌘ {}", ["r:reload", "Esc:back"].join("▕▏"));
                }
                // Footer = REVIEW actions (daily verbs). State /
                // meta shortcuts (Ctrl+X close, draft toggle, labels,
                // reviewers, open-in-web) sit right-aligned on the
                // tab-bar row instead — keeps this row scannable.
                let mut p = vec!["c:comment"];
                if matches!(self.active_tab, Tab::Commits | Tab::Files) {
                    p.push("Enter:view diff");
                }
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
        // The compose form has real text inputs (title, body) and a
        // picker that swallows arrow keys — both need raw key events
        // instead of UserEvent shortcut translations.
        self.comment_editor.is_some() || matches!(self.mode, Mode::Compose)
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
        self.spawn_mention_issue_numbers_fetch();
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

        // Mention popup overlays the editor — it intercepts keys
        // (↑↓/Enter/Esc) and forwards chars/backspaces through so
        // the query stays in sync with what the user types.
        if self.mention_popup.is_some() {
            match event_with_count.event {
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

        // While the inline editor is open, EVERY key must route to it —
        // no view-level shortcuts (`r` reload, `c` comment, etc.) may
        // hijack the keystroke. Otherwise the user can't type `r`/`c`
        // in their message.
        if self.comment_editor.is_some() {
            match self.mode {
                Mode::List => self.handle_event_list(event_with_count, key),
                Mode::Detail => self.handle_event_detail(event_with_count, key),
                Mode::Compose => self.handle_event_compose(event_with_count, key),
            }
            return;
        }

        // Compose mode owns every key — the form has its own text
        // inputs, including `r`, so the view-level reload shortcut
        // must not steal it.
        if matches!(self.mode, Mode::Compose) {
            self.handle_event_compose(event_with_count, key);
            return;
        }

        // `r` reloads — works in list / detail modes when no input is active.
        if key.code == KeyCode::Char('r') && key.modifiers == KeyModifiers::NONE {
            self.reload();
            return;
        }

        match self.mode {
            Mode::List => self.handle_event_list(event_with_count, key),
            Mode::Detail => self.handle_event_detail(event_with_count, key),
            Mode::Compose => self.handle_event_compose(event_with_count, key),
        }
        let _ = KeyModifiers::NONE;
    }

    fn handle_event_list(
        &mut self,
        event_with_count: UserEventWithCount,
        key: KeyEvent,
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        // `n` opens the compose-new-PR view — keep it before the
        // UserEvent dispatch since `n` doesn't map to any standard
        // `UserEvent`.
        if key.code == KeyCode::Char('n') && key.modifiers == KeyModifiers::NONE {
            self.start_compose_pr();
            return;
        }
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
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
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
            KeyCode::Char('l') if plain => {
                self.start_labels_picker();
                return;
            }
            KeyCode::Char('v') if plain => {
                self.start_reviewers_picker();
                return;
            }
            KeyCode::Char('x') if ctrl => {
                self.confirm_close_pr();
                return;
            }
            KeyCode::Char('o') if ctrl => {
                self.confirm_reopen_pr();
                return;
            }
            KeyCode::Char('d') if ctrl => {
                self.confirm_toggle_draft();
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
        // ── Reaction picker takes over every key while open ────────
        if self.reaction_picker.is_some() {
            self.handle_event_reaction_picker(key);
            return;
        }

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
                KeyCode::Char('+') => {
                    self.start_react();
                    return;
                }
                _ => {}
            }
        }

        match event_with_count.event {
            UserEvent::Cancel | UserEvent::Close => {
                // Esc inside a drill-down goes back to the file /
                // commit list first. Only a second Esc exits to the
                // PR list.
                if self.files_drilldown.is_some() {
                    self.files_drilldown = None;
                    self.files_drilldown_scroll = 0;
                } else if self.commits_drilldown.is_some() {
                    self.commits_drilldown = None;
                    self.commits_drilldown_scroll = 0;
                } else {
                    self.back_to_list();
                }
            }
            // Enter on a file or commit row drills into its diff.
            UserEvent::Confirm => self.enter_drilldown(),
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

    fn enter_drilldown(&mut self) {
        // Conversation tab: Enter on the selected comment follows
        // the first `#N` reference inside it, when there is one.
        // Other tabs keep their existing drilldown semantics.
        if matches!(self.active_tab, Tab::Conversation) {
            self.follow_first_ref_in_selected_card();
            return;
        }
        match self.active_tab {
            Tab::Files => {
                let Some(detail) = self.opened_detail() else {
                    return;
                };
                let Some(file) = detail.files.get(self.files_hovered) else {
                    return;
                };
                // Transition to the existing single-file DiffView so
                // the user sees the same renderer the rest of the app
                // uses (enhanced / raw / side-by-side modes etc.).
                self.tx.send(AppEvent::OpenPrFileDiff {
                    pr_number: detail.number,
                    sha: detail.head_sha.clone(),
                    file_path: file.filename.clone(),
                });
            }
            Tab::Commits => {
                let Some(detail) = self.opened_detail() else {
                    return;
                };
                let Some(commit) = detail.commit_list.get(self.commits_hovered) else {
                    return;
                };
                // Transition to the existing CommitDetail view —
                // the app's `OpenPrCommitDetail` handler does the
                // background `git fetch` if the commit isn't local
                // yet, then opens `View::Detail`.
                self.tx.send(AppEvent::OpenPrCommitDetail {
                    pr_number: detail.number,
                    sha: commit.sha.clone(),
                });
            }
            _ => {}
        }
    }

    /// Move the per-tab hovered cursor / scroll. Used by ↑↓ + PgUp/Dn.
    fn tab_nav(&mut self, delta: i32) {
        // While drilled in on a file or commit, ↑↓/PgUp/Dn scroll the
        // diff content directly — there's no row cursor to track.
        if self.active_tab == Tab::Files && self.files_drilldown.is_some() {
            self.files_drilldown_scroll =
                adjust_scroll(self.files_drilldown_scroll, delta);
            return;
        }
        if self.active_tab == Tab::Commits && self.commits_drilldown.is_some() {
            self.commits_drilldown_scroll =
                adjust_scroll(self.commits_drilldown_scroll, delta);
            return;
        }
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
        if self.active_tab == Tab::Files && self.files_drilldown.is_some() {
            self.files_drilldown_scroll =
                adjust_scroll(self.files_drilldown_scroll, delta);
            return;
        }
        if self.active_tab == Tab::Commits && self.commits_drilldown.is_some() {
            self.commits_drilldown_scroll =
                adjust_scroll(self.commits_drilldown_scroll, delta);
            return;
        }
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

    fn confirm_close_pr(&mut self) {
        let Some(detail) = self.opened_detail() else {
            return;
        };
        // Only meaningful when the PR is currently open — closing a
        // merged or already-closed PR isn't a thing.
        if !matches!(detail.state, PullState::Open) {
            return;
        }
        self.tx
            .send(AppEvent::OpenDialog(crate::event::DialogKind::ConfirmPullRequestStateChange {
                pr_number: detail.number,
                pr_title: detail.title.clone(),
                closing: true,
            }));
    }

    fn confirm_reopen_pr(&mut self) {
        let Some(detail) = self.opened_detail() else {
            return;
        };
        if !matches!(detail.state, PullState::Closed) {
            return;
        }
        self.tx
            .send(AppEvent::OpenDialog(crate::event::DialogKind::ConfirmPullRequestStateChange {
                pr_number: detail.number,
                pr_title: detail.title.clone(),
                closing: false,
            }));
    }

    fn confirm_toggle_draft(&mut self) {
        let Some(detail) = self.opened_detail() else {
            return;
        };
        if !matches!(detail.state, PullState::Open) {
            return;
        }
        let to_draft = !detail.draft;
        self.tx
            .send(AppEvent::OpenDialog(crate::event::DialogKind::ConfirmPullRequestDraftToggle {
                pr_number: detail.number,
                pr_title: detail.title.clone(),
                node_id: detail.node_id.clone(),
                to_draft,
            }));
    }

    fn start_labels_picker(&mut self) {
        let Some(detail) = self.opened_detail() else {
            return;
        };
        let pr_number = detail.number;
        let pr_title = detail.title.clone();
        let currently_on_pr: Vec<String> =
            detail.labels.iter().map(|l| l.name.clone()).collect();
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        // Fetch the repo's full label set in the background — when it
        // returns we re-enter the main loop with the picker open.
        std::thread::spawn(move || {
            match crate::github::pr::list_repo_labels(&token, &coords) {
                Ok(labels) => {
                    tx.send(AppEvent::OpenPrLabelsPicker {
                        pr_number,
                        pr_title,
                        all_labels: labels,
                        currently_on_pr,
                    });
                }
                Err(e) => tx.send(AppEvent::NotifyError(format!("Labels: {}", e))),
            }
        });
    }

    fn start_reviewers_picker(&mut self) {
        let Some(detail) = self.opened_detail() else {
            return;
        };
        let pr_number = detail.number;
        let pr_title = detail.title.clone();
        let currently_requested = detail.reviewers.clone();
        // The PR author can never be a reviewer — bake that into the
        // pool we send to the picker so the user can't tick themselves
        // and get a 422 on confirm.
        let pr_author = detail.author.clone();
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            match crate::github::pr::list_repo_assignees(&token, &coords) {
                Ok(users) => {
                    let mut filtered: Vec<String> =
                        users.into_iter().filter(|u| *u != pr_author).collect();
                    // Make sure currently-requested reviewers show up
                    // even if `/assignees` didn't list them (they may
                    // not be repo collaborators yet).
                    for r in &currently_requested {
                        if !filtered.contains(r) {
                            filtered.push(r.clone());
                        }
                    }
                    filtered.sort();
                    tx.send(AppEvent::OpenPrReviewersPicker {
                        pr_number,
                        pr_title,
                        all_users: filtered,
                        currently_requested,
                    });
                }
                Err(e) => tx.send(AppEvent::NotifyError(format!("Reviewers: {}", e))),
            }
        });
    }

    /// Drill from the Commits tab into a single commit's detail view.
    /// Cache hit → instant; miss → background fetch with a loading
    /// placeholder until `on_commit_detail_fetched` swaps it in.
    fn open_commit_drilldown(&mut self) {
        let Some(detail) = self.opened_detail() else {
            return;
        };
        let Some(commit) = detail.commit_list.get(self.commits_hovered) else {
            return;
        };
        let sha = commit.sha.clone();
        self.commits_drilldown = Some(sha.clone());
        self.commits_drilldown_scroll = 0;
        if self.commit_detail_cache.contains_key(&sha)
            || self.commit_detail_loading.contains(&sha)
        {
            return;
        }
        self.commit_detail_loading.insert(sha.clone());
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        let fetch_sha = sha.clone();
        std::thread::spawn(move || {
            let result = crate::github::pr::fetch_commit_detail(&token, &coords, &fetch_sha);
            tx.send(AppEvent::PrCommitDetailFetched {
                sha: fetch_sha,
                result,
            });
        });
    }

    pub fn on_commit_detail_fetched(
        &mut self,
        sha: String,
        result: Result<crate::github::pr::CommitDetail, String>,
    ) {
        self.commit_detail_loading.remove(&sha);
        match result {
            Ok(detail) => {
                self.commit_detail_cache.insert(sha, detail);
            }
            Err(e) => {
                self.tx
                    .send(AppEvent::NotifyError(format!("Commit detail: {}", e)));
                // Drop the drill-down so the user goes back to the list.
                if self.commits_drilldown.as_deref() == Some(&sha) {
                    self.commits_drilldown = None;
                }
            }
        }
    }

    /// Enter the compose-PR mode. Pre-populates `head` from the
    /// current local HEAD branch, `base` from the most common GitHub
    /// default (`main`), and `title` from the latest commit subject
    /// on the head branch. Body starts empty; the preview pane
    /// renders lazily once `head` and `base` differ.
    fn start_compose_pr(&mut self) {
        let repo_path = self.coords_repo_path();
        let head = current_head_branch(&repo_path).unwrap_or_default();
        let title = latest_commit_subject(&repo_path, &head).unwrap_or_default();
        // Seed the body from a repo-local PR template if one exists.
        // Mirror GitHub web's lookup order — `.github/`, `docs/`, and
        // the repo root — covering the three locations the platform
        // recognises.
        let body =
            crate::github::pr::load_pr_template(&repo_path).unwrap_or_default();
        let cursor = body.len();
        self.compose = Some(ComposeState {
            head,
            base: "main".to_string(),
            title,
            body,
            draft: false,
            focused: ComposeField::Title,
            cursor,
            body_scroll: 0,
            body_last_height: 0,
            labels: Vec::new(),
            reviewers: Vec::new(),
            preview: None,
            preview_loading: false,
            submitting: false,
        });
        self.mode = Mode::Compose;
        self.refresh_compose_preview();
    }

    fn coords_repo_path(&self) -> std::path::PathBuf {
        // The view doesn't carry the local repo path directly — but
        // every action that touches local git already passes it
        // through `ctx`. We grab it via the context's accessor.
        self.ctx.repo_path.clone()
    }

    /// Re-run `git log` + `git diff` between the compose form's
    /// current `base..head` and feed the result into the preview
    /// pane. Cheap enough to run synchronously on each field edit
    /// for typical PR sizes; spawn it off if it ever feels slow.
    fn handle_event_compose(
        &mut self,
        event_with_count: UserEventWithCount,
        key: KeyEvent,
    ) {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // ── Mouse wheel: dedicated channel so the picker can pan
        // its viewport without dragging the selected row along with
        // it. Both compose-form scroll and picker scroll route here.
        if matches!(event_with_count.event, UserEvent::ScrollUp) {
            self.compose_handle_scroll(-3);
            return;
        }
        if matches!(event_with_count.event, UserEvent::ScrollDown) {
            self.compose_handle_scroll(3);
            return;
        }

        // ── Branch picker takes over every key while open ──────────
        if self.branch_picker.is_some() {
            self.handle_event_branch_picker(key);
            return;
        }

        // Submission spinner: only Esc cancels, everything else
        // is swallowed.
        let submitting = self
            .compose
            .as_ref()
            .map_or(false, |c| c.submitting);
        if submitting {
            if matches!(key.code, KeyCode::Esc) {
                self.compose = None;
                self.mode = Mode::List;
            }
            return;
        }

        // ── Global compose keys ────────────────────────────────────
        match key.code {
            KeyCode::Esc => {
                self.compose = None;
                self.mode = Mode::List;
                return;
            }
            KeyCode::Char('s') if ctrl => {
                self.submit_compose_pr();
                return;
            }
            _ => {}
        }

        // ── Vertical navigation between fields ─────────────────────
        // Up/Down walk the form. Body is multi-line: only switch
        // when the caret is at the first / last line of its buffer,
        // otherwise stay in body and move the cursor within.
        if matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let move_to_next_field = {
                let Some(state) = self.compose.as_ref() else {
                    return;
                };
                if matches!(state.focused, ComposeField::Body) {
                    let going_up = matches!(key.code, KeyCode::Up);
                    let at_first =
                        state.body[..state.cursor.min(state.body.len())].find('\n').is_none();
                    let at_last = state.body[state.cursor.min(state.body.len())..]
                        .find('\n')
                        .is_none();
                    (going_up && at_first) || (!going_up && at_last)
                } else {
                    true
                }
            };
            if move_to_next_field {
                if let Some(c) = self.compose.as_mut() {
                    c.focused = if matches!(key.code, KeyCode::Up) {
                        c.focused.prev()
                    } else {
                        c.focused.next()
                    };
                    c.cursor = compose_field_text(c).map_or(0, |t| t.len());
                }
                return;
            }
            // In body, navigate cursor vertically within text.
            if let Some(s) = self.compose.as_mut() {
                compose_body_cursor_vertical(s, key.code);
            }
            self.compose_body_anchor_to_cursor();
            return;
        }

        // ── Field-specific editing ─────────────────────────────────
        let focused = self
            .compose
            .as_ref()
            .map(|c| c.focused)
            .unwrap_or(ComposeField::Title);
        match focused {
            // Head / Base aren't text inputs — they open the picker
            // overlay. Click + Enter both trigger.
            ComposeField::Head | ComposeField::Base => {
                if matches!(key.code, KeyCode::Enter) {
                    self.open_branch_picker(focused);
                }
            }
            ComposeField::Labels => {
                if matches!(key.code, KeyCode::Enter) {
                    self.open_compose_labels_picker();
                }
            }
            ComposeField::Draft => {
                if matches!(key.code, KeyCode::Char(' ') | KeyCode::Enter) {
                    if let Some(s) = self.compose.as_mut() {
                        s.draft = !s.draft;
                    }
                }
            }
            ComposeField::Title => {
                let Some(state) = self.compose.as_mut() else {
                    return;
                };
                match key.code {
                    KeyCode::Char(c) if !ctrl => compose_field_insert_char(state, c),
                    KeyCode::Backspace if ctrl => compose_field_delete_word_left(state),
                    KeyCode::Char('h') if ctrl => compose_field_delete_word_left(state),
                    KeyCode::Char('w') if ctrl => compose_field_delete_word_left(state),
                    KeyCode::Backspace => compose_field_delete_left(state),
                    KeyCode::Left if ctrl => {
                        let new = word_left_boundary(&state.title, state.cursor);
                        state.cursor = new;
                    }
                    KeyCode::Right if ctrl => {
                        let new = word_right_boundary(&state.title, state.cursor);
                        state.cursor = new;
                    }
                    KeyCode::Left => compose_field_cursor_left(state),
                    KeyCode::Right => compose_field_cursor_right(state),
                    KeyCode::Home => compose_field_cursor_home(state),
                    KeyCode::End => compose_field_cursor_end(state),
                    _ => {}
                }
            }
            ComposeField::Body => {
                // Capture the `#` position BEFORE insertion so the
                // popup anchor points at the freshly-written hash.
                let mention_anchor: Option<usize> = match key.code {
                    KeyCode::Char('#') if !ctrl => {
                        self.compose.as_ref().map(|c| c.cursor)
                    }
                    _ => None,
                };
                {
                    let Some(state) = self.compose.as_mut() else {
                        return;
                    };
                    match key.code {
                        KeyCode::Char(c) if !ctrl => compose_field_insert_char(state, c),
                        KeyCode::Enter => compose_field_insert_char(state, '\n'),
                        KeyCode::Tab => {
                            // 4-space indent inside the body — same
                            // convention as the comment editor.
                            for _ in 0..4 {
                                compose_field_insert_char(state, ' ');
                            }
                        }
                        KeyCode::Backspace if ctrl => compose_field_delete_word_left(state),
                        KeyCode::Char('h') if ctrl => compose_field_delete_word_left(state),
                        KeyCode::Char('w') if ctrl => compose_field_delete_word_left(state),
                        KeyCode::Backspace => compose_field_delete_left(state),
                        KeyCode::Left if ctrl => {
                            let new = word_left_boundary(&state.body, state.cursor);
                            state.cursor = new;
                        }
                        KeyCode::Right if ctrl => {
                            let new = word_right_boundary(&state.body, state.cursor);
                            state.cursor = new;
                        }
                        KeyCode::Left => compose_field_cursor_left(state),
                        KeyCode::Right => compose_field_cursor_right(state),
                        KeyCode::Home => compose_field_cursor_home(state),
                        KeyCode::End => compose_field_cursor_end(state),
                        _ => {}
                    }
                }
                // Anchor the viewport on the cursor AFTER mutation
                // so typing past the last visible row scrolls into
                // view. Mouse wheel doesn't reach this branch — it's
                // intercepted earlier — so wheel scroll stays sticky.
                self.compose_body_anchor_to_cursor();
                if let Some(a) = mention_anchor {
                    self.open_mention_popup(
                        a,
                        crate::view::issue::MentionTarget::ComposeBody,
                    );
                }
            }
        }
    }

    /// Re-anchor `body_scroll` so the body cursor sits inside the
    /// last-rendered viewport. Called from cursor-mutating actions
    /// (typing, arrow keys, vertical nav, click) — NOT from the
    /// render path or from mouse-wheel scroll, so wheel scrolling
    /// stays where the user puts it.
    fn compose_body_anchor_to_cursor(&mut self) {
        let Some(c) = self.compose.as_mut() else {
            return;
        };
        if !matches!(c.focused, ComposeField::Body) {
            return;
        }
        let view = c.body_last_height;
        if view == 0 {
            return;
        }
        let (_, cursor_row) = cursor_screen_pos(&c.body, c.cursor);
        if cursor_row < c.body_scroll {
            c.body_scroll = cursor_row;
        } else if cursor_row >= c.body_scroll + view {
            c.body_scroll = cursor_row + 1 - view;
        }
    }

    fn handle_event_branch_picker(&mut self, key: ratatui::crossterm::event::KeyEvent) {
        use ratatui::crossterm::event::KeyCode;
        // Mouse wheel events arrive as raw KeyCode::Null on most
        // terminals — handle them in `handle_scroll` instead.
        let Some(picker) = self.branch_picker.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => {
                self.branch_picker = None;
            }
            KeyCode::Up => {
                if picker.hovered > 0 {
                    picker.hovered -= 1;
                }
                picker_anchor_scroll_to_hovered(picker);
            }
            KeyCode::Down => {
                if picker.hovered + 1 < picker.branches.len() {
                    picker.hovered += 1;
                }
                picker_anchor_scroll_to_hovered(picker);
            }
            KeyCode::PageUp => {
                picker.hovered = picker.hovered.saturating_sub(10);
                picker_anchor_scroll_to_hovered(picker);
            }
            KeyCode::PageDown => {
                picker.hovered = (picker.hovered + 10).min(picker.branches.len().saturating_sub(1));
                picker_anchor_scroll_to_hovered(picker);
            }
            KeyCode::Home => {
                picker.hovered = 0;
                picker_anchor_scroll_to_hovered(picker);
            }
            KeyCode::End => {
                picker.hovered = picker.branches.len().saturating_sub(1);
                picker_anchor_scroll_to_hovered(picker);
            }
            KeyCode::Enter => {
                let (target, name) = {
                    let p = picker; // freshen borrow
                    let entry = p.branches.get(p.hovered).cloned();
                    let target = p.target_field;
                    (target, entry.map(|e| e.name))
                };
                if let (Some(name), Some(state)) = (name, self.compose.as_mut()) {
                    match target {
                        ComposeField::Head => state.head = name,
                        ComposeField::Base => state.base = name,
                        _ => {}
                    }
                }
                self.branch_picker = None;
                self.refresh_compose_preview();
            }
            _ => {}
        }
    }

    /// Mouse-wheel scroll in compose mode. When the picker is open
    /// we pan its viewport without moving the highlighted row — the
    /// user can still arrow up/down to change the selection.
    fn compose_handle_scroll(&mut self, delta: i32) {
        // Branch picker scroll wins when open — its viewport
        // shouldn't be hijacked by the body underneath.
        if let Some(picker) = self.branch_picker.as_mut() {
            let max = picker.branches.len().saturating_sub(1);
            let new = (picker.scroll as i32 + delta).clamp(0, max as i32);
            picker.scroll = new as usize;
            return;
        }
        // Otherwise scroll the Body viewport. Clamp against the
        // body's row count and last-rendered height so we can't
        // scroll past the end.
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

    /// Snap the compose Body cursor to the byte position under a
    /// click at `(col, row)`. Mirrors the issue-view helper of the
    /// same name; only the field accessors differ.
    fn compose_body_move_cursor_to_click(&mut self, click_col: u16, click_row: u16) {
        let body_rect = match self
            .compose_field_rects
            .iter()
            .find(|(f, _)| matches!(f, ComposeField::Body))
            .map(|(_, r)| *r)
        {
            Some(r) => r,
            None => return,
        };
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
            c.cursor = buf.len();
            c.focused = ComposeField::Body;
            return;
        }
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
        c.cursor = byte_pos.min(buf.len());
        c.focused = ComposeField::Body;
        drop(c);
        self.compose_body_anchor_to_cursor();
    }

    /// Open a multi-select overlay for the compose-form's Labels
    /// field. Fetches the repo's labels in the background then
    /// pipes them back through `ComposeLabelsPicked` for the form
    /// to render.
    fn open_compose_labels_picker(&mut self) {
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        let initial: Vec<String> = self
            .compose
            .as_ref()
            .map(|c| c.labels.iter().map(|l| l.name.clone()).collect())
            .unwrap_or_default();
        std::thread::spawn(move || {
            match crate::github::pr::list_repo_labels(&token, &coords) {
                Ok(labels) => tx.send(AppEvent::OpenComposeLabelsPicker {
                    all_labels: labels,
                    currently_selected: initial,
                }),
                Err(e) => tx.send(AppEvent::NotifyError(format!("Labels: {}", e))),
            }
        });
    }

    /// Handler called from the app when the user confirms the
    /// compose-labels picker — stores the chosen subset on the
    /// compose state so the next render shows them, and so the
    /// submit step can apply them after PR creation.
    pub fn on_compose_labels_picked(
        &mut self,
        labels: Vec<crate::github::pr::Label>,
    ) {
        if let Some(s) = self.compose.as_mut() {
            s.labels = labels;
        }
    }

    fn open_branch_picker(&mut self, target: ComposeField) {
        if !matches!(target, ComposeField::Head | ComposeField::Base) {
            return;
        }
        let repo_path = self.coords_repo_path();
        let branches = load_local_and_remote_branches(&repo_path);
        let current = self.compose.as_ref().map(|c| match target {
            ComposeField::Head => c.head.clone(),
            ComposeField::Base => c.base.clone(),
            _ => String::new(),
        });
        let hovered = current
            .and_then(|name| branches.iter().position(|b| b.name == name))
            .unwrap_or(0);
        self.branch_picker = Some(BranchPicker {
            target_field: target,
            branches,
            hovered,
            scroll: 0,
            overlay_rect: None,
            row_rects: Vec::new(),
            visible_height: 0,
        });
    }

    /// Push the head branch (if needed) then `POST /pulls`. Result
    /// is funnelled through `PullRequestActionDone` so the PR list
    /// re-fetches and we transition into the new PR's detail.
    fn submit_compose_pr(&mut self) {
        let Some(state) = self.compose.as_ref() else {
            return;
        };
        if state.head.trim().is_empty() || state.base.trim().is_empty() {
            self.tx.send(AppEvent::NotifyWarn(
                "Head and base branches are required.".into(),
            ));
            return;
        }
        if state.title.trim().is_empty() {
            self.tx.send(AppEvent::NotifyWarn(
                "Title cannot be empty.".into(),
            ));
            return;
        }
        let head = state.head.clone();
        let base = state.base.clone();
        let title = state.title.clone();
        let body = state.body.clone();
        let draft = state.draft;
        let labels: Vec<String> = state.labels.iter().map(|l| l.name.clone()).collect();
        if let Some(c) = self.compose.as_mut() {
            c.submitting = true;
        }
        let token = self.token.clone();
        let coords = self.coords.clone();
        let repo_path = self.coords_repo_path();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            // Push the branch first so GitHub knows what `head` ref
            // points to. We try `git push -u origin <head>` and
            // tolerate a failure (the branch may already be
            // tracking + up to date).
            let _ = std::process::Command::new("git")
                .args(["push", "-u", "origin", &head])
                .current_dir(&repo_path)
                .output();
            let result = crate::github::pr::create_pull_request(
                &token, &coords, &head, &base, &title, &body, draft,
            );
            match result {
                Ok(number) => {
                    // Apply labels best-effort — failure is non-fatal
                    // (the PR was created successfully, the user can
                    // re-pick labels via the regular flow).
                    if !labels.is_empty() {
                        let _ = crate::github::pr::set_pull_request_labels(
                            &token, &coords, number, &labels,
                        );
                    }
                    tx.send(AppEvent::PrCreated { number });
                }
                Err(e) => tx.send(AppEvent::PullRequestActionDone {
                    number: 0,
                    action: "Create PR".into(),
                    result: Err(e),
                }),
            }
        });
    }

    /// Hook called from the app when `PrCreated` fires — clears the
    /// compose draft, reloads the PR list, and opens the new PR.
    pub fn on_pr_created(&mut self, number: u64) {
        self.compose = None;
        self.mode = Mode::List;
        self.reload();
        self.opened_pr_number = Some(number);
        self.mode = Mode::Detail;
        if !self.detail_cache.contains_key(&number) {
            self.spawn_detail_fetch(number);
        }
        self.tx.send(AppEvent::NotifySuccess(format!(
            "Pull request #{} created.",
            number
        )));
    }

    fn refresh_compose_preview(&mut self) {
        let Some(state) = self.compose.as_ref() else {
            return;
        };
        if state.head.is_empty() || state.base.is_empty() || state.head == state.base {
            if let Some(ref mut s) = self.compose {
                s.preview = None;
            }
            return;
        }
        let repo_path = self.coords_repo_path();
        let commits = compose_preview_commits(&repo_path, &state.base, &state.head);
        let files = compose_preview_files(&repo_path, &state.base, &state.head);
        if let Some(ref mut s) = self.compose {
            s.preview = Some(ComposePreview { commits, files });
        }
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

    /// Open the inline reaction-emoji picker on the currently-
    /// selected comment. Bails when the selected entry isn't a
    /// reactable comment (reviews don't carry reactions).
    fn start_react(&mut self) {
        let target_idx = self.conversation_selected;
        let (comment_id, is_review) = {
            let Some(entry) = self.selected_conversation_entry() else {
                return;
            };
            if entry.id.is_none() {
                return;
            }
            (
                entry.id.unwrap(),
                matches!(entry.kind, ConversationKind::ReviewComment { .. }),
            )
        };
        self.reaction_picker = Some(ReactionPicker {
            target_idx,
            hovered: 0,
            overlay_rect: None,
            row_rects: Vec::new(),
        });
        self.spawn_viewer_reactions_fetch(target_idx, comment_id, is_review);
    }

    fn spawn_viewer_reactions_fetch(
        &self,
        target_idx: usize,
        comment_id: u64,
        is_review: bool,
    ) {
        let Some(number) = self.opened_number() else {
            return;
        };
        let Some(me) = self.me_login.clone() else {
            return;
        };
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let reactions = if is_review {
                crate::github::pr::list_my_review_comment_reactions(
                    &token, &coords, comment_id, &me,
                )
            } else {
                crate::github::pr::list_my_issue_comment_reactions(
                    &token, &coords, comment_id, &me,
                )
            }
            .unwrap_or_default();
            tx.send(AppEvent::PrViewerReactionsFetched {
                pr_number: number,
                target_idx,
                reactions,
            });
        });
    }

    pub fn on_viewer_reactions_fetched(
        &mut self,
        pr_number: u64,
        target_idx: usize,
        reactions: Vec<(crate::github::pr::ReactionKind, u64)>,
    ) {
        self.viewer_reactions
            .insert((pr_number, target_idx), reactions);
    }

    fn handle_event_reaction_picker(
        &mut self,
        key: ratatui::crossterm::event::KeyEvent,
    ) {
        use ratatui::crossterm::event::KeyCode;
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

    /// Fire the reaction POST in a background thread. The PR view's
    /// detail cache is invalidated on success so the next render
    /// re-fetches and surfaces the updated chip row.
    fn submit_reaction(
        &mut self,
        target_idx: usize,
        kind: crate::github::pr::ReactionKind,
    ) {
        let prev_selected = self.conversation_selected;
        self.conversation_selected = target_idx;
        let entry_data = self.selected_conversation_entry().map(|e| {
            (
                e.id,
                matches!(e.kind, ConversationKind::ReviewComment { .. }),
            )
        });
        self.conversation_selected = prev_selected;
        let Some((Some(comment_id), is_review)) = entry_data else {
            return;
        };
        let Some(number) = self.opened_number() else {
            return;
        };
        let token = self.token.clone();
        let coords = self.coords.clone();
        let tx = self.tx.clone();

        // Toggle: if the viewer already reacted with this kind on
        // this entry, DELETE the recorded reaction instead of POSTing
        // a duplicate (which GitHub rejects with 422 anyway).
        let existing_id: Option<u64> = self
            .viewer_reactions
            .get(&(number, target_idx))
            .and_then(|v| v.iter().find(|(k, _)| *k == kind).map(|(_, id)| *id));
        if let Some(rid) = existing_id {
            std::thread::spawn(move || {
                let result = if is_review {
                    crate::github::pr::delete_review_comment_reaction(
                        &token, &coords, comment_id, rid,
                    )
                } else {
                    crate::github::pr::delete_issue_comment_reaction(
                        &token, &coords, comment_id, rid,
                    )
                };
                if result.is_ok() {
                    tx.send(AppEvent::PrReactionRemoved {
                        pr_number: number,
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
            let result = if is_review {
                crate::github::pr::add_review_comment_reaction(
                    &token, &coords, comment_id, kind,
                )
            } else {
                crate::github::pr::add_issue_comment_reaction(
                    &token, &coords, comment_id, kind,
                )
            };
            match result {
                Ok(reaction_id) => {
                    tx.send(AppEvent::PrReactionApplied {
                        pr_number: number,
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
        pr_number: u64,
        target_idx: usize,
        kind: crate::github::pr::ReactionKind,
        reaction_id: u64,
    ) {
        self.viewer_reactions
            .entry((pr_number, target_idx))
            .or_default()
            .push((kind, reaction_id));
        self.spawn_detail_fetch(pr_number);
    }

    pub fn on_reaction_removed(
        &mut self,
        pr_number: u64,
        target_idx: usize,
        kind: crate::github::pr::ReactionKind,
    ) {
        if let Some(v) = self.viewer_reactions.get_mut(&(pr_number, target_idx)) {
            v.retain(|(k, _)| *k != kind);
        }
        self.spawn_detail_fetch(pr_number);
    }

    fn viewer_reactions_for(
        &self,
        target_idx: usize,
    ) -> Vec<crate::github::pr::ReactionKind> {
        let Some(number) = self.opened_number() else {
            return Vec::new();
        };
        self.viewer_reactions
            .get(&(number, target_idx))
            .map(|v| v.iter().map(|(k, _)| *k).collect())
            .unwrap_or_default()
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
        // Capture the `#` position BEFORE we insert so the popup
        // anchor points at the freshly-written hash, ready for the
        // splice on pick.
        let anchor: Option<usize> = if c == '#' {
            self.comment_editor.as_ref().map(|ed| ed.cursor)
        } else {
            None
        };
        if let Some(ed) = self.comment_editor.as_mut() {
            let cursor = ed.cursor;
            ed.buffer.insert(cursor, c);
            ed.cursor += c.len_utf8();
        }
        if let Some(a) = anchor {
            self.open_mention_popup(a, crate::view::issue::MentionTarget::CommentEditor);
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
        // Reaction picker is modal: click on a cell fires that
        // reaction (or toggles it off); click outside closes.
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
            } else if !rect_contains(
                self.reaction_picker.as_ref().and_then(|p| p.overlay_rect),
                col,
                row,
            ) {
                self.reaction_picker = None;
            }
            return;
        }
        // Mention popup is modal: clicking a row picks it; clicking
        // outside closes it (without bubbling to the editor below).
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
                // Conversation refs: hit-test `#N` link rects FIRST so
                // a click on a coloured mention navigates instead of
                // just bumping the selected comment.
                if matches!(self.active_tab, Tab::Conversation) {
                    if let Some(area) = self.tab_content_area {
                        if rect_contains(Some(area), col, row) {
                            let logical_line =
                                (row.saturating_sub(area.y) as usize)
                                    + self.conversation_scroll;
                            if let Some(link) = self
                                .conversation_ref_links
                                .iter()
                                .find(|l| {
                                    l.line == logical_line
                                        && (area.x + l.col_start) <= col
                                        && col < (area.x + l.col_end)
                                })
                                .copied()
                            {
                                self.follow_reference(link.number, link.is_pr);
                                return;
                            }
                        }
                    }
                }
                // Click inside a tab's row-based content → set hovered
                // for that tab. Files / Commits / Checks all use the
                // same "rows of items" model. On the Commits + Files
                // tabs a click also drills into the clicked row (same
                // outcome as Enter) — matches what users expect from
                // GitHub web's PR sub-pages.
                if let Some(area) = self.tab_content_area {
                    if rect_contains(Some(area), col, row) {
                        if let Some(idx) = self.row_at_tab(row, area) {
                            self.set_tab_hovered(idx);
                            if matches!(self.active_tab, Tab::Commits | Tab::Files) {
                                self.enter_drilldown();
                            }
                        }
                    }
                }
            }
            Mode::Compose => {
                // Picker overlay swallows clicks while open.
                if let Some(picker) = self.branch_picker.as_mut() {
                    if let Some(idx) = picker
                        .row_rects
                        .iter()
                        .position(|r| rect_contains(Some(*r), col, row))
                    {
                        picker.hovered = picker.scroll + idx;
                        let (target, name) = {
                            let entry = picker.branches.get(picker.hovered).cloned();
                            (picker.target_field, entry.map(|e| e.name))
                        };
                        if let (Some(name), Some(s)) = (name, self.compose.as_mut()) {
                            match target {
                                ComposeField::Head => s.head = name,
                                ComposeField::Base => s.base = name,
                                _ => {}
                            }
                        }
                        self.branch_picker = None;
                        self.refresh_compose_preview();
                    } else if !rect_contains(
                        self.branch_picker.as_ref().and_then(|p| p.overlay_rect),
                        col,
                        row,
                    ) {
                        // Click outside the overlay cancels.
                        self.branch_picker = None;
                    }
                    return;
                }
                // Click on a compose-form field row. Draft toggles;
                // Head/Base open the picker; Title/Body focus +
                // position cursor at end (no per-column placement
                // yet — would need to remember each field's start x).
                let hit = self
                    .compose_field_rects
                    .iter()
                    .find(|(_, rect)| rect_contains(Some(*rect), col, row))
                    .map(|(f, _)| *f);
                let Some(field) = hit else {
                    return;
                };
                if let Some(state) = self.compose.as_mut() {
                    state.focused = field;
                    state.cursor = match field {
                        ComposeField::Title => state.title.len(),
                        // Body cursor lands precisely under the
                        // click, not at the end — handled below.
                        ComposeField::Body => state.cursor,
                        _ => 0,
                    };
                }
                match field {
                    ComposeField::Head | ComposeField::Base => {
                        self.open_branch_picker(field);
                    }
                    ComposeField::Labels => {
                        self.open_compose_labels_picker();
                    }
                    ComposeField::Draft => {
                        if let Some(s) = self.compose.as_mut() {
                            s.draft = !s.draft;
                        }
                    }
                    ComposeField::Body => {
                        self.compose_body_move_cursor_to_click(col, row);
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        // Mention popup is modal: hover inside it highlights the
        // row under the cursor, hover outside is ignored.
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
        // highlighted emoji.
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
            Mode::Compose => {
                // Picker overlay: hover highlights a row.
                if let Some(picker) = self.branch_picker.as_mut() {
                    if let Some(idx) = picker
                        .row_rects
                        .iter()
                        .position(|r| rect_contains(Some(*r), col, row))
                    {
                        picker.hovered = picker.scroll + idx;
                    }
                    return;
                }
                // Hover on a form-field row focuses it — same model
                // as the comment-card hover in Detail mode.
                let hit = self
                    .compose_field_rects
                    .iter()
                    .find(|(_, rect)| rect_contains(Some(*rect), col, row))
                    .map(|(f, _)| *f);
                if let (Some(field), Some(state)) = (hit, self.compose.as_mut()) {
                    if state.focused != field {
                        state.focused = field;
                        state.cursor = match field {
                            ComposeField::Title => state.title.len(),
                            ComposeField::Body => state.body.len(),
                            _ => 0,
                        };
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
        // from a different layout never matches a click. Also clear
        // the terminal cursor position — left over from compose / a
        // comment editor it would otherwise stick on the screen after
        // the editor closes (the terminal keeps the cursor wherever
        // we last positioned it).
        self.list_area = None;
        self.tab_content_area = None;
        self.editor_body_area = None;
        self.tab_bar_rects.clear();
        self.comment_editor_cursor_pos = None;
        // Reset the per-frame avatar accumulator — render paths
        // push their intents here, then the trailing diff pass at
        // the end of this fn evicts last-frame's stale avatars
        // (e.g. when the previous frame rendered the PR list and
        // this frame renders detail mode) before painting fresh.
        self.pending_avatar_paints.clear();

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
            Mode::Compose => self.render_compose_mode(f, body_area),
        }

        // `#` mention popup floats above the comment editor when
        // open — drawn last so it sits on top of all other content.
        if self.mention_popup.is_some() {
            let editor_rect = self.editor_body_area.unwrap_or(body_area);
            let theme = self.ctx.color_theme.clone();
            if let Some(p) = self.mention_popup.as_mut() {
                crate::view::issue::paint_mention_popup(f, body_area, editor_rect, p, &theme);
            }
        }

        // Single avatar diff pass for the whole frame — handles
        // cross-section transitions cleanly: any avatar in `prev`
        // but missing from `pending` (filter switch, scrolled
        // out of view, tab switch, list → detail, …) gets a
        // `clear_cell` emitted at its old position so the
        // persistent image placement is evicted. We additionally
        // drop pending avatars whose screen position falls inside
        // a currently-open overlay (mention popup, reaction picker)
        // so those overlays don't get image bytes punched through
        // them — terminal images stack on top of cell content, so
        // without this filter the popup body would show the cards'
        // avatars bleeding through.
        let pending = std::mem::take(&mut self.pending_avatar_paints);
        let prev = std::mem::take(&mut self.prev_painted_avatars);
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
        if let Some(p) = self.branch_picker.as_ref() {
            if let Some(r) = p.overlay_rect {
                occluders.push(r);
            }
        }
        let pending: Vec<(PaintedAvatar, Color)> = pending
            .into_iter()
            .filter(|(pa, _)| {
                !occluders
                    .iter()
                    .any(|r| rect_contains(Some(*r), pa.screen_x, pa.screen_y))
            })
            .collect();
        self.prev_painted_avatars =
            paint_avatars_with_diff(f, &self.ctx, &prev, pending);

        // Place the terminal cursor on the inline comment editor when
        // it's the active input surface — same convention as the rebase
        // reword editor.
        if let Some((cx, cy)) = self.comment_editor_cursor_pos {
            // Suppress while the mention popup overlaps the cursor
            // position — a blinking cursor on top of the popup body
            // reads as a glitch. Popup intercepts every keystroke so
            // the cursor isn't actionable until the popup closes.
            let occluded = self
                .mention_popup
                .as_ref()
                .and_then(|p| p.overlay_rect)
                .is_some_and(|rect| {
                    cx >= rect.x
                        && cx < rect.x + rect.width
                        && cy >= rect.y
                        && cy < rect.y + rect.height
                });
            if !occluded {
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
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let title = Line::from(vec![
            Span::raw("  "),
            // `⋔` (pitchfork) = fork-and-merge shape, the visual
            // signature of a pull request. Purple matches GitHub's
            // PR brand colour.
            Span::styled(
                "⋔ ",
                Style::default().fg(MERGED_PURPLE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Pull Requests ",
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            // Repo coords get their own dedicated teal token (REPO_TEAL),
            // not shared with hash/branch/label palettes — so the name
            // reads as its own kind of identifier.
            Span::styled(
                format!("{}/{}", self.coords.owner, self.coords.repo),
                Style::default().fg(REPO_TEAL).add_modifier(Modifier::BOLD),
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
        let row_width = body_area.width as usize;
        self.list_avatar_slots.clear();
        let avatars_on = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        let mut local_slots: Vec<AvatarSlot> = Vec::new();
        let items: Vec<ListItem<'static>> = filtered
            .iter()
            .enumerate()
            .map(|(i, pr)| {
                let is_marked = self.hovered == i;
                let (line, avatar_col) =
                    self.format_pr_row(pr, is_marked, &cols, row_width, avatars_on);
                // Track only rows currently inside the visible window —
                // the List widget itself clips, so painting an avatar
                // off-screen would waste a buffer write.
                if let Some(col) = avatar_col {
                    if i >= self.list_scroll_offset
                        && visible > 0
                        && i < self.list_scroll_offset + visible
                    {
                        local_slots.push(AvatarSlot {
                            login: pr.author.clone(),
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
        // No row-level bg — the label chips paint their own and a
        // List highlight_style would patch over them. We paint the
        // selection bg manually per-span in `format_pr_row` instead,
        // so chips keep their colours intact.
        let list = List::new(items).highlight_style(
            Style::default().add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, body_area, &mut state);

        // Push slot intents into the per-frame accumulator. The
        // single diff pass at the end of `render()` decides what
        // to clear vs. skip vs. paint — using one `prev` for the
        // entire view so cross-section transitions automatically
        // evict stale image placements.
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
                PaintedAvatar {
                    login: slot.login.clone(),
                    screen_x,
                    screen_y,
                    is_selected: slot.is_selected,
                },
                bg,
            ));
        }
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

    /// Returns the formatted row + the column offset (from the start
    /// of the row's enclosing area) where the avatar should land —
    /// just to the left of the author column. `Some(col)` when the
    /// 3-cell pad was reserved (avatars enabled); `None` when avatars
    /// are off so the row layout stays tight.
    fn format_pr_row(
        &self,
        pr: &PullRequest,
        is_opened: bool,
        cols: &PrListColumns,
        row_width: usize,
        avatars_on: bool,
    ) -> (Line<'static>, Option<u16>) {
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
        // Short label chips — up to MAX_INLINE_LABELS labels, each
        // rendered as ` XX ` (2-letter abbreviation) with the label's
        // own colour as background. Empty cells pad to the column
        // width so neighbouring rows stay aligned.
        let mut row: Vec<Span<'static>> = vec![
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
        ];
        if cols.labels > 0 {
            row.push(Span::raw(" "));
            let mut chips_width = 0usize;
            for lab in pr.labels.iter().take(MAX_INLINE_LABELS) {
                row.extend(short_label_chip_spans(lab));
                chips_width += LABEL_CHIP_WIDTH;
            }
            // Pad the remaining slots with plain spaces so author
            // stays at the same column across rows.
            if chips_width < cols.labels {
                row.push(Span::raw(" ".repeat(cols.labels - chips_width)));
            }
        }
        // 1-cell gap between label chips and the avatar so they
        // never touch — image 25 showed `BU` glued to the avatar
        // without this padding.
        if avatars_on {
            row.push(Span::raw(" "));
        }
        // Capture the row-relative column where the avatar will be
        // painted. `row` so far ends just after the spacing we just
        // pushed; the avatar lands in the next two cells, then a
        // trailing space keeps the author legible.
        let avatar_col: Option<u16> = if avatars_on {
            Some(
                row.iter()
                    .map(|s| console::measure_text_width(s.content.as_ref()) as u16)
                    .sum(),
            )
        } else {
            None
        };
        // Reserve 3 cells (avatar 2 + breathing space 1) only when
        // avatars are enabled; otherwise the row keeps a tight 2-cell
        // gap.
        let pre_author = if avatars_on { "   " } else { "  " };
        row.extend([
            Span::raw(pre_author),
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
        ]);

        // Selection bg painted manually per-span — chips already have
        // their own bg and we leave those alone so GitHub colours
        // stay visible on the hovered row. Trailing fill extends the
        // selection across the row's remaining width.
        if is_opened {
            let sel_bg = theme.list_selected_bg;
            for span in row.iter_mut() {
                if span.style.bg.is_none() {
                    span.style = span.style.bg(sel_bg);
                }
            }
            let content_width: usize = row
                .iter()
                .map(|s| console::measure_text_width(s.content.as_ref()))
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


    fn render_detail_mode(&mut self, f: &mut Frame, area: Rect) {
        let current_number = self.opened_number();
        let cached_detail =
            current_number.and_then(|n| self.detail_cache.get(&n)).cloned();
        let theme_label_fg = self.ctx.color_theme.detail_label_fg;

        // ── Layout: info ─ [labels chips] ─ divider ─ tabs ─ tab
        // content. The labels row is conditional (collapses to 0 rows
        // when the PR has none), and the divider always sits BELOW
        // the labels so the chip row reads as part of the PR header.
        let has_labels = cached_detail
            .as_ref()
            .map(|d| !d.labels.is_empty())
            .unwrap_or(false);
        let labels_height: u16 = if has_labels { 1 } else { 0 };
        let [
            sub_header_area,
            labels_area,
            header_divider_area,
            tab_bar_area,
            tab_content_area,
        ] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(labels_height),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .areas(area);
        self.tab_content_area = Some(tab_content_area);

        self.render_pr_sub_header(f, sub_header_area, cached_detail.as_ref(), current_number);
        if has_labels {
            self.render_pr_labels_row(f, labels_area, cached_detail.as_ref());
        }
        // Header divider — single `─` row spanning the area width.
        {
            let theme = &self.ctx.color_theme;
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "─".repeat(header_divider_area.width as usize),
                    Style::default().fg(theme.divider_fg),
                ))),
                header_divider_area,
            );
        }
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

        // Reaction picker is the last thing drawn so it sits on top
        // of the conversation cards (and any inline editor below).
        if self.reaction_picker.is_some() {
            self.render_reaction_picker_overlay(f, area);
        }
    }

    /// Full-screen compose-new-PR view — borrows the Detail layout:
    /// a one-row sub-header, a divider, then a side-by-side form /
    /// preview split. Designed to feel like a sibling of the Detail
    /// page so the user never wonders where they are.
    fn render_compose_mode(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        self.compose_field_rects.clear();
        // Sub-header (1 row) + divider (1) + body (rest).
        let [sub_header_area, divider_area, body_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(area);

        // ── Sub-header: `Create New Pull Request · branch → main` ──
        let (head, base, draft) = self
            .compose
            .as_ref()
            .map(|c| (c.head.clone(), c.base.clone(), c.draft))
            .unwrap_or_default();
        let mut header_spans: Vec<Span<'static>> = vec![
            Span::raw("  "),
            Span::styled(
                if draft { "DRAFT" } else { "NEW" },
                Style::default()
                    .fg(if draft {
                        theme.detail_label_fg
                    } else {
                        theme.status_success_fg
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "Create new Pull Request".to_string(),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
        ];
        if !head.is_empty() && !base.is_empty() {
            header_spans.push(Span::styled(
                " · ",
                Style::default().fg(theme.detail_label_fg),
            ));
            header_spans.push(Span::styled(
                head,
                Style::default()
                    .fg(theme.list_ref_branch_fg)
                    .add_modifier(Modifier::BOLD),
            ));
            header_spans.push(Span::styled(
                " → ",
                Style::default().fg(theme.detail_label_fg),
            ));
            header_spans.push(Span::styled(
                base,
                Style::default()
                    .fg(theme.list_ref_remote_branch_fg)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        f.render_widget(
            Paragraph::new(Line::from(header_spans)),
            sub_header_area,
        );
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "─".repeat(divider_area.width as usize),
                Style::default().fg(theme.divider_fg),
            ))),
            divider_area,
        );

        // ── Body: left = form (slightly wider), right = preview ──
        // Form gets ~60% so the multi-line Body editor has room to
        // breathe without crowding labels/draft rows. Preview pane
        // still has enough width for commit subjects + file paths.
        let [form_area, preview_area] = Layout::horizontal([
            Constraint::Percentage(60),
            Constraint::Percentage(40),
        ])
        .areas(body_area);
        self.render_compose_form(f, form_area);
        self.render_compose_preview(f, preview_area);

        // Branch picker overlay on top — drawn last so it covers
        // the form when open.
        if self.branch_picker.is_some() {
            self.render_branch_picker_overlay(f, area);
        }
    }

    /// Branch-picker overlay — drawn on top of the compose layout
    /// when `self.branch_picker.is_some()`. Centered list with
    /// per-row colour by `BranchKind`.
    /// Centered emoji picker overlay for the eight GitHub reactions
    /// — drawn on top of the conversation when `+` was pressed on a
    /// reactable comment. Keyboard arrows move the highlight, Enter
    /// fires the POST, Esc closes.
    fn render_reaction_picker_overlay(&mut self, f: &mut Frame, area: Rect) {
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
        // Single-Paragraph rendering so ratatui's double-width
        // continuation markers stay consistent across spans — fixes
        // 🚀 disappearing into ❤️'s VS16 residue.
        let cell_width: u16 = 5;
        let Some(picker) = self.reaction_picker.as_mut() else {
            return;
        };
        let width: u16 = (kinds.len() as u16 * cell_width).saturating_add(2);
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
        // Manual buffer-level rendering: force the continuation
        // skip on every emoji's second visual cell so ❤️'s
        // unicode-width=1 vs terminal=2 mismatch doesn't blank out
        // the 🚀 next to it.
        let buf = f.buffer_mut();
        for (i, kind) in kinds.iter().enumerate() {
            let is_selected = i == picker.hovered;
            let is_mine = mine.contains(kind);
            let cell_rect = Rect::new(x_cursor, inner.y, cell_width, 1);
            picker.row_rects.push(cell_rect);
            // Hover on a chip the user already owns stays red — just
            // a deeper shade so the focus indicator reads without
            // erasing the "mine" signal.
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
            for col in 0..cell_width {
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
            if cell_width >= 3 && x_cursor + 1 < buf.area.x + buf.area.width {
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
            x_cursor = x_cursor.saturating_add(cell_width);
        }
    }

    fn render_branch_picker_overlay(&mut self, f: &mut Frame, area: Rect) {
        let Some(picker) = self.branch_picker.as_mut() else {
            return;
        };
        let theme = &self.ctx.color_theme;
        let width = area.width.saturating_mul(2) / 5;
        let width = width.max(40).min(area.width.saturating_sub(4));
        let height = (picker.branches.len() as u16 + 4).min(area.height.saturating_sub(4));
        let height = height.max(8);
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let rect = Rect::new(x, y, width, height);
        picker.overlay_rect = Some(rect);

        // Wipe the area underneath so the form text doesn't bleed.
        f.render_widget(ratatui::widgets::Clear, rect);

        let title_text = match picker.target_field {
            ComposeField::Head => " Choose head branch ",
            ComposeField::Base => " Choose base branch ",
            _ => " Pick branch ",
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(Span::styled(
                title_text.to_string(),
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(rect);
        f.render_widget(block, rect);

        // Render clamps scroll within bounds but doesn't force the
        // hovered row back into view — manual wheel scrolling can
        // legitimately move the viewport away from the selection.
        let visible = inner.height as usize;
        picker.visible_height = visible;
        let max_scroll = picker.branches.len().saturating_sub(visible);
        if picker.scroll > max_scroll {
            picker.scroll = max_scroll;
        }
        picker.row_rects.clear();
        for (i, entry) in picker
            .branches
            .iter()
            .enumerate()
            .skip(picker.scroll)
            .take(visible)
        {
            let row_y = inner.y + (i - picker.scroll) as u16;
            let row_rect = Rect::new(inner.x, row_y, inner.width, 1);
            picker.row_rects.push(row_rect);
            let is_selected = i == picker.hovered;
            let marker = if is_selected {
                Span::styled(
                    "● ".to_string(),
                    Style::default()
                        .fg(theme.status_info_fg)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled("○ ".to_string(), Style::default().fg(theme.divider_fg))
            };
            let name_color = match entry.kind {
                BranchKind::Local => branch_color_from_graph_palette(
                    theme,
                    &self.ctx.graph_color_set,
                    &entry.name,
                ),
                BranchKind::Remote => theme.list_ref_remote_branch_fg,
            };
            let name_style = Style::default().fg(name_color).add_modifier(Modifier::BOLD);
            // Selected row is signalled by the filled `●` marker +
            // bold name (no row-level bg fill — keeps the chip-like
            // branch colours intact).
            let line = Line::from(vec![
                Span::raw(" "),
                marker,
                Span::styled(entry.name.clone(), name_style),
            ]);
            f.render_widget(Paragraph::new(line), row_rect);
        }
    }

    fn render_compose_form(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
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
        let inner = block.inner(area);
        f.render_widget(block, area);

        let Some(state) = self.compose.as_ref().cloned() else {
            return;
        };

        // Label column width + leading indent constants.
        const INDENT: u16 = 2;
        const LABEL_WIDTH: u16 = 8; // "Title:  " etc.

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

        // ── Branch field row (Head or Base) — looks like a button
        //    rather than a text input. Shows the current branch in
        //    its branch colour. Enter or click opens the picker.
        let render_branch_row = |f: &mut Frame,
                                 y_pos: u16,
                                 field: ComposeField,
                                 label: &'static str,
                                 value: &str,
                                 color: ratatui::style::Color|
         -> Rect {
            let focused = state.focused == field;
            let rect = Rect::new(inner.x, y_pos, inner.width.saturating_sub(2), 1);
            let placeholder_style = Style::default().fg(theme.detail_label_fg);
            let value_span = if value.is_empty() {
                Span::styled("(select…)".to_string(), placeholder_style)
            } else {
                Span::styled(
                    value.to_string(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )
            };
            let line = Line::from(vec![
                Span::raw(" ".repeat(INDENT as usize)),
                focus_indicator(focused),
                Span::styled(format!("{:<w$}", label, w = LABEL_WIDTH as usize), label_style),
                Span::raw(" "),
                value_span,
                Span::styled(
                    "  (Enter to choose)".to_string(),
                    if focused {
                        Style::default().fg(theme.detail_label_fg)
                    } else {
                        Style::default().fg(theme.bg)
                    },
                ),
            ]);
            f.render_widget(Paragraph::new(line), rect);
            rect
        };

        // ── Text-input row (Title) — flat text, no bg fill. Visual
        //    cue is the focus arrow + a thin underline below the
        //    typed value, like a vintage form field.
        let render_text_input_row = |f: &mut Frame,
                                     y_pos: u16,
                                     field: ComposeField,
                                     label: &'static str,
                                     value: &str|
         -> (Rect, u16) {
            let focused = state.focused == field;
            let row = Rect::new(inner.x, y_pos, inner.width, 1);
            let value_start_x = inner.x + INDENT + 2 + LABEL_WIDTH + 1;
            let value_width = inner
                .width
                .saturating_sub(INDENT + 2 + LABEL_WIDTH + 1 + 2);
            let mut spans = vec![
                Span::raw(" ".repeat(INDENT as usize)),
                focus_indicator(focused),
                Span::styled(format!("{:<w$}", label, w = LABEL_WIDTH as usize), label_style),
                Span::raw(" "),
            ];
            if value.is_empty() {
                spans.push(Span::styled(
                    "(type a title)".to_string(),
                    Style::default().fg(theme.detail_label_fg),
                ));
            } else {
                spans.push(Span::styled(
                    value.to_string(),
                    Style::default().fg(theme.fg),
                ));
            }
            f.render_widget(Paragraph::new(Line::from(spans)), row);
            // Underline below the row when focused (1-row strip).
            if focused {
                let underline_y = y_pos.saturating_add(1).min(inner.y + inner.height - 1);
                let underline = Line::from(Span::styled(
                    "─".repeat(value_width as usize),
                    Style::default().fg(theme.list_head_fg),
                ));
                f.render_widget(
                    Paragraph::new(underline),
                    Rect::new(value_start_x, underline_y, value_width, 1),
                );
            }
            (row, value_start_x)
        };

        // Layout walker.
        let mut y = inner.y;

        // Head row — colour from the graph palette so each branch
        // gets the same hue as in the commit graph view.
        let head_color = branch_color_from_graph_palette(
            theme,
            &self.ctx.graph_color_set,
            &state.head,
        );
        let head_rect = render_branch_row(
            f,
            y,
            ComposeField::Head,
            "Head:",
            &state.head,
            head_color,
        );
        self.compose_field_rects.push((ComposeField::Head, head_rect));
        y += 2;

        // Base row
        let base_color = branch_color_from_graph_palette(
            theme,
            &self.ctx.graph_color_set,
            &state.base,
        );
        let base_rect = render_branch_row(
            f,
            y,
            ComposeField::Base,
            "Base:",
            &state.base,
            base_color,
        );
        self.compose_field_rects.push((ComposeField::Base, base_rect));
        y += 2;

        // Title row
        let (title_rect, title_value_x) =
            render_text_input_row(f, y, ComposeField::Title, "Title:", &state.title);
        self.compose_field_rects.push((ComposeField::Title, title_rect));
        y += 2;

        // Body block — uses a Block with a tinted bg so it reads as
        // a real multi-line text area.
        let body_focused = state.focused == ComposeField::Body;
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
        // Body block fills remaining vertical space, leaving room
        // below for the Labels row, a 1-row gap, and the Draft row.
        // Without this reservation the Draft line bleeds onto the
        // Compose Block's bottom border.
        let body_block_height = inner
            .y
            .saturating_add(inner.height)
            .saturating_sub(y)
            .saturating_sub(3)
            .max(3);
        let body_block_rect = Rect::new(
            inner.x + INDENT + 2,
            y,
            inner.width.saturating_sub(INDENT + 2 + 2),
            body_block_height,
        );
        // Body: bordered Block — accent border when focused, no bg
        // fill (which read as gross yellow on some themes).
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
        // Render-time scroll: ONLY clamp against the current max.
        // We deliberately do NOT re-anchor on the cursor here —
        // that lives in `compose_body_anchor_to_cursor` and is
        // called from cursor-mutating actions only. Re-anchoring at
        // render time would snap the viewport back to the cursor on
        // every frame, undoing mouse-wheel scrolls and making
        // every hover feel like it jumped to the bottom.
        let total_rows = if state.body.is_empty() {
            1
        } else {
            state.body.matches('\n').count() as u16 + 1
        };
        let mut scroll = state.body_scroll;
        let max_scroll = total_rows.saturating_sub(body_inner.height);
        if scroll > max_scroll {
            scroll = max_scroll;
        }
        if let Some(s) = self.compose.as_mut() {
            s.body_scroll = scroll;
            s.body_last_height = body_inner.height;
        }
        let base_style = Style::default().fg(theme.fg);
        let mention_fg = theme.list_hash_fg;
        let body_lines: Vec<Line<'static>> = if state.body.is_empty() {
            vec![Line::from(Span::styled(
                "(type a description — supports markdown)".to_string(),
                Style::default().fg(theme.detail_label_fg),
            ))]
        } else {
            state
                .body
                .split('\n')
                .skip(scroll as usize)
                .take(body_inner.height as usize)
                .map(|l| {
                    crate::view::issue::editor_line_with_mentions(l, base_style, mention_fg)
                })
                .collect()
        };
        f.render_widget(Paragraph::new(body_lines), body_inner);
        // Expose the body's inner rect so the mention popup can
        // anchor on it (popup positioning reads `editor_body_area`).
        // Done only when Body is focused — clicks/keys elsewhere
        // shouldn't be hit-tested against the body.
        if body_focused {
            self.editor_body_area = Some(body_inner);
        }
        y = body_block_rect.y + body_block_rect.height;

        // Labels row — shows the currently-picked chips inline. The
        // overlay opens on Enter / click.
        let labels_focused = state.focused == ComposeField::Labels;
        let labels_rect = Rect::new(inner.x, y, inner.width, 1);
        let mut labels_line_spans = vec![
            Span::raw(" ".repeat(INDENT as usize)),
            focus_indicator(labels_focused),
            Span::styled(format!("{:<w$}", "Labels:", w = LABEL_WIDTH as usize), label_style),
            Span::raw(" "),
        ];
        if state.labels.is_empty() {
            labels_line_spans.push(Span::styled(
                "(none — Enter to pick)".to_string(),
                Style::default().fg(theme.detail_label_fg),
            ));
        } else {
            for (i, lab) in state.labels.iter().enumerate() {
                if i > 0 {
                    labels_line_spans.push(Span::raw(" "));
                }
                labels_line_spans.extend(label_chip_spans_local(lab));
            }
        }
        f.render_widget(Paragraph::new(Line::from(labels_line_spans)), labels_rect);
        self.compose_field_rects
            .push((ComposeField::Labels, labels_rect));
        y += 2;

        // Draft toggle row — aligned with the other fields so the
        // form keeps a consistent `Label:  value` rhythm. The check
        // glyph sits in the value column where the picker chips /
        // input text would, and the human-readable suffix follows.
        let draft_focused = state.focused == ComposeField::Draft;
        let draft_rect = Rect::new(inner.x, y, inner.width, 1);
        let check_glyph = if state.draft { "●" } else { "○" };
        let draft_line = Line::from(vec![
            Span::raw(" ".repeat(INDENT as usize)),
            focus_indicator(draft_focused),
            Span::styled(
                format!("{:<w$}", "Draft:", w = LABEL_WIDTH as usize),
                label_style,
            ),
            Span::raw(" "),
            Span::styled(
                format!("{} ", check_glyph),
                Style::default()
                    .fg(if state.draft {
                        theme.status_info_fg
                    } else {
                        theme.divider_fg
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if state.draft { "yes (draft)" } else { "no" }.to_string(),
                if draft_focused {
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.fg)
                },
            ),
        ]);
        f.render_widget(Paragraph::new(draft_line), draft_rect);
        self.compose_field_rects
            .push((ComposeField::Draft, draft_rect));

        // ── Cursor on the active text field ────────────────────────
        match state.focused {
            ComposeField::Title => {
                let cx = title_value_x + state.cursor as u16;
                if cx < inner.x + inner.width {
                    self.comment_editor_cursor_pos = Some((cx, title_rect.y));
                }
            }
            ComposeField::Body => {
                // Only place the cursor when it sits inside the visible
                // viewport `[scroll, scroll + body_inner.height)`. Without
                // this guard, scrolling away from the cursor would clamp
                // its visual position to the top of the body (via
                // `saturating_sub`-style arithmetic), making it look like
                // the scroll moved the cursor.
                let (col, row) = cursor_screen_pos(&state.body, state.cursor);
                if row >= scroll && row < scroll + body_inner.height {
                    let cx = body_inner.x + col;
                    let cy = body_inner.y + (row - scroll);
                    if cx < body_inner.x + body_inner.width {
                        self.comment_editor_cursor_pos = Some((cx, cy));
                    }
                }
            }
            _ => {}
        }
    }

    fn render_compose_preview(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    "Preview".to_string(),
                    Style::default()
                        .fg(theme.list_head_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ]));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let Some(state) = self.compose.as_ref() else {
            return;
        };
        let Some(preview) = state.preview.as_ref() else {
            let p = Paragraph::new(Span::styled(
                "  (set head + base to see the preview)".to_string(),
                Style::default().fg(theme.detail_label_fg),
            ));
            f.render_widget(p, inner);
            return;
        };
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(Span::styled(
            format!(
                "{} commit{} · {} file{}",
                preview.commits.len(),
                if preview.commits.len() == 1 { "" } else { "s" },
                preview.files.len(),
                if preview.files.len() == 1 { "" } else { "s" },
            ),
            Style::default()
                .fg(theme.fg)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        if preview.commits.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No commits between base and head.".to_string(),
                Style::default().fg(theme.detail_label_fg),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "Commits".to_string(),
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            )));
            for c in &preview.commits {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        c.short_sha.clone(),
                        Style::default()
                            .fg(theme.list_hash_fg)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        truncate(&c.subject, 60),
                        Style::default().fg(theme.list_commit_message_fg),
                    ),
                ]));
            }
        }
        lines.push(Line::from(""));
        if !preview.files.is_empty() {
            lines.push(Line::from(Span::styled(
                "Files".to_string(),
                Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD),
            )));
            let cols = FileColumns::compute(&preview.files, inner.width as usize);
            for file in &preview.files {
                lines.push(file_line(theme, file, &cols));
            }
        }
        f.render_widget(Paragraph::new(lines), inner);
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
        let mention_fg = theme.list_hash_fg;
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
                .map(|l| {
                    crate::view::issue::editor_line_with_mentions(l, value, mention_fg)
                })
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
        &mut self,
        f: &mut Frame,
        area: Rect,
        detail: Option<&PullRequestDetail>,
        number: Option<u64>,
    ) {
        let theme = &self.ctx.color_theme;
        let avatars_on = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::raw("  "));
        let mut badge_spans: Vec<Span<'static>> = Vec::new();
        // Captured during span construction so we can paint the
        // avatar after `f.render_widget` without re-measuring.
        let mut avatar_paint: Option<(String, u16)> = None;
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
            // +3 cells reserved between `by ` and the author for the
            // GitHub avatar (2 cells image + 1 cell breathing space) —
            // only when avatars are enabled, otherwise the line stays
            // tight and no empty gap appears.
            let avatar_pad: usize = if avatars_on { 3 } else { 0 };
            let fixed_overhead = 2
                + measure(&state_span)
                + 2
                + measure(&num_span)
                + 2
                + 3
                + measure(&by_span)
                + avatar_pad
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
            if avatars_on {
                // Capture the col offset where the 2-cell avatar will
                // be painted — i.e., right after `by `, before the
                // author name. Reserve 3 cells (avatar + breathing).
                let avatar_col: u16 = spans
                    .iter()
                    .map(|s| console::measure_text_width(s.content.as_ref()) as u16)
                    .sum();
                spans.push(Span::raw("   "));
                avatar_paint = Some((detail.author.clone(), avatar_col));
            }
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

        // Divider intentionally omitted — `render_detail_mode` owns
        // a dedicated row for it that sits below the optional labels
        // chip row, so the order on screen is always:
        //   info → [labels] → divider → tabs → divider → content.
        f.render_widget(
            Paragraph::new(Line::from(spans)),
            area,
        );
        if let Some((login, col)) = avatar_paint {
            self.pending_avatar_paints.push((
                PaintedAvatar {
                    login,
                    screen_x: area.x + col,
                    screen_y: area.y,
                    is_selected: false,
                },
                self.ctx.color_theme.bg,
            ));
        }
    }

    /// One-row strip showing the PR's currently-attached labels as
    /// GitHub-style coloured chips. Sits between the sub-header info
    /// row and the tab bar — only rendered when the PR actually has
    /// labels (otherwise the row is collapsed by the layout).
    fn render_pr_labels_row(
        &self,
        f: &mut Frame,
        area: Rect,
        detail: Option<&PullRequestDetail>,
    ) {
        let Some(detail) = detail else { return };
        let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
        for (i, lab) in detail.labels.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" "));
            }
            spans.extend(label_chip_spans_local(lab));
        }
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// Tab bar: clickable list of [Conversation, Commits, Checks, Files]
    /// with the active one highlighted (HEAD-accent fg + underlined).
    fn render_tab_bar(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let detail = self.opened_detail().cloned();
        // Per-tab counts only show once the PR detail has loaded —
        // displaying `Conversation 0  Commits 0  ...` during the
        // fetch would falsely advertise empty tabs.
        let counts: Option<[usize; 4]> = detail.as_ref().map(|d| {
            [
                d.conversation.len(),
                d.commit_list.len(),
                d.check_runs.len(),
                d.files.len(),
            ]
        });

        // Render each tab as `  Label N  ` so click hit-testing can map
        // raw column → tab. We accumulate the start column as we go.
        let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
        let mut cursor_x: u16 = area.x + 2; // skip the leading 2-space indent
        let tabs = Tab::all();
        for (i, tab) in tabs.iter().enumerate() {
            let is_active = *tab == self.active_tab;
            let is_hovered = self.hovered_tab == Some(*tab) && !is_active;
            let label = match counts {
                Some(c) => format!("{} {}", tab.label(), c[i]),
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
        // Right-aligned "Manage" shortcuts — state + meta operations
        // on the PR sit here so the bottom footer stays focused on
        // the daily review verbs. Hidden inside a drill-down (the
        // user only needs `Esc:back` there) AND until the PR detail
        // has finished loading — surfacing close / draft / labels /
        // reviewers before we know the PR's state would let the
        // user fire actions against missing data.
        let manage_spans = if detail.is_some()
            && self.files_drilldown.is_none()
            && self.commits_drilldown.is_none()
        {
            self.manage_shortcut_spans(detail.as_ref())
        } else {
            Vec::new()
        };
        let manage_width: usize = manage_spans
            .iter()
            .map(|s| console::measure_text_width(s.content.as_ref()))
            .sum();
        let trailing_pad = 2;
        let gap = (area.width as usize)
            .saturating_sub(cursor_x.saturating_sub(area.x) as usize)
            .saturating_sub(manage_width)
            .saturating_sub(trailing_pad);
        if gap > 0 && manage_width > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
            spans.extend(manage_spans);
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

    /// Build the right-side "Manage" shortcut spans for the tab bar.
    /// Picks state-changing keys that depend on the PR's current
    /// flags (Ctrl+X close ↔ Ctrl+O reopen, Ctrl+D to draft ↔
    /// mark ready) so we never advertise no-op shortcuts.
    fn manage_shortcut_spans(
        &self,
        detail: Option<&PullRequestDetail>,
    ) -> Vec<Span<'static>> {
        let theme = &self.ctx.color_theme;
        let is_open = detail
            .map(|d| matches!(d.state, PullState::Open))
            .unwrap_or(false);
        let is_closed = detail
            .map(|d| matches!(d.state, PullState::Closed))
            .unwrap_or(false);
        let is_draft = detail.map(|d| d.draft).unwrap_or(false);

        let mut parts: Vec<String> = Vec::new();
        if is_open {
            parts.push("Ctrl+X:close".into());
            parts.push(
                if is_draft {
                    "Ctrl+D:mark ready"
                } else {
                    "Ctrl+D:to draft"
                }
                .into(),
            );
        } else if is_closed {
            parts.push("Ctrl+O:reopen".into());
        }
        parts.push("l:labels".into());
        parts.push("v:reviewers".into());

        // ▕▏ matches the footer hint separator used elsewhere in the
        // app for visual consistency. `⌘` mirrors the footer prefix
        // so both rows read as a unified shortcut layer. Separator
        // shares the same colour as the labels — the bottom footer
        // does the same (it renders the whole row as one Span).
        let style = Style::default().fg(theme.detail_label_fg);
        let mut spans: Vec<Span<'static>> = vec![Span::styled("⌘ ".to_string(), style)];
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("▕▏".to_string(), style));
            }
            spans.push(Span::styled(part.clone(), style));
        }
        spans
    }

    fn render_tab_conversation(
        &mut self,
        f: &mut Frame,
        area: Rect,
        detail: &PullRequestDetail,
    ) {
        // Snapshot once so every card uses the same decision — and
        // we don't churn the mutex per push.
        let avatars_on = self.ctx.avatar_manager.lock().unwrap().is_enabled();
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
        self.conversation_avatar_slots.clear();

        let pr_idx = 0usize;
        let pr_first = lines.len();
        let pr_is_me = self
            .me_login
            .as_deref()
            .map_or(false, |me| me == detail.author);
        // 1. PR description rendered as the first "card" (top-level, no
        //    parent → empty ancestor gutters, and no descending line
        //    since the conversation feed is not its "child" tree).
        let pr_card_layout = push_comment_card(
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
                // PR body itself doesn't surface its reactions here
                // — they'd need a separate `GET /reactions` call
                // (the PR endpoint doesn't inline them on the body).
                reactions: crate::github::pr::ReactionCounts::default(),
                avatar_login: if avatars_on {
                    Some(&detail.author)
                } else {
                    None
                },
            },
        );
        if let Some(col) = pr_card_layout.avatar_slot {
            self.conversation_avatar_slots.push(AvatarSlot {
                login: detail.author.clone(),
                line: pr_card_layout.top_line,
                col,
                is_selected: self.conversation_selected == pr_idx,
            });
        }
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
                    avatars_on,
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

        // Post-process every line to recolour resolvable `#N`
        // mentions (issue → status_success_fg, PR → list_hash_fg)
        // with UNDERLINED+BOLD, while capturing click hit-boxes for
        // the mouse handler. Lines without refs pass through.
        self.conversation_ref_links.clear();
        let pr_set: rustc_hash::FxHashSet<u64> =
            self.items.iter().map(|p| p.number).collect();
        let issue_set = self.mention_issue_numbers.clone();
        let issue_fg: Color = self.ctx.color_theme.status_success_fg;
        let pr_fg: Color = self.ctx.color_theme.list_hash_fg;
        let mut links: Vec<crate::view::issue::RefLink> = Vec::new();
        let lines: Vec<Line<'static>> = lines
            .into_iter()
            .enumerate()
            .map(|(line_idx, line)| {
                crate::view::issue::restyle_and_track_hash_refs(
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
        // The single diff pass at the end of `render()` handles
        // clearing stale slots (scroll, comment removed, …) and
        // skip-rendering unchanged ones.
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
                PaintedAvatar {
                    login: slot.login.clone(),
                    screen_x,
                    screen_y,
                    is_selected: false,
                },
                theme_bg,
            ));
        }
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
        avatars_on: bool,
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
        // Reactions are available on every comment that has an id —
        // i.e. proper issue / review comments, not the synthesized
        // review summary rows (which never carry one).
        if entry.id.is_some() {
            shortcuts.push("+:react");
        }
        // `↵:open` chip whenever the body contains a `#N` we can
        // resolve to an issue or a PR in this repo — clicking on
        // the chip / pressing Enter follows the first reference.
        let has_resolvable_ref = crate::view::issue::extract_hash_refs(&entry.body)
            .into_iter()
            .any(|n| self.resolve_hash_ref(n).is_some());
        if has_resolvable_ref {
            shortcuts.push("↵:open");
        }
        let card_layout = push_comment_card(
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
                reactions: entry.reactions.clone(),
                avatar_login: if avatars_on {
                    Some(&entry.author)
                } else {
                    None
                },
            },
        );
        if let Some(col) = card_layout.avatar_slot {
            self.conversation_avatar_slots.push(AvatarSlot {
                login: entry.author.clone(),
                line: card_layout.top_line,
                col,
                is_selected: self.conversation_selected == my_idx,
            });
        }
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
                    avatars_on,
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
        // Drill-down: render the commit's full message + per-file diff
        // instead of the list. Esc clears `commits_drilldown` and
        // brings the list back.
        if let Some(sha) = self.commits_drilldown.clone() {
            self.render_commit_drilldown(f, area, &sha);
            return;
        }
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
        // Build rows + collect avatar paints for visible commits.
        let avatars_on = self.ctx.avatar_manager.lock().unwrap().is_enabled();
        let mut paints: Vec<(String, u16, u16)> = Vec::new();
        let items: Vec<ListItem<'static>> = detail
            .commit_list
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (line, avatar_col) = commit_row(theme, c, &cols, avatars_on);
                if let Some(col) = avatar_col {
                    if i >= self.commits_scroll
                        && visible > 0
                        && i < self.commits_scroll + visible
                    {
                        paints.push((
                            c.author_login.clone(),
                            col,
                            (i - self.commits_scroll) as u16,
                        ));
                    }
                }
                ListItem::new(line)
            })
            .collect();
        let mut state = ListState::default();
        state.select(Some(self.commits_hovered));
        *state.offset_mut() = self.commits_scroll;
        let list = List::new(items).highlight_style(
            Style::default()
                .bg(theme.list_selected_bg)
                .add_modifier(Modifier::BOLD),
        );
        f.render_stateful_widget(list, area, &mut state);

        // Push commits-tab avatars into the per-frame accumulator.
        let selected = self.commits_hovered;
        let theme_bg = self.ctx.color_theme.bg;
        let sel_bg = self.ctx.color_theme.list_selected_bg;
        for (login, col, row) in paints {
            if row as usize >= area.height as usize {
                continue;
            }
            let abs_row = self.commits_scroll + row as usize;
            let is_sel = abs_row == selected;
            let bg = if is_sel { sel_bg } else { theme_bg };
            self.pending_avatar_paints.push((
                PaintedAvatar {
                    login,
                    screen_x: area.x + col,
                    screen_y: area.y + row,
                    is_selected: is_sel,
                },
                bg,
            ));
        }
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
        // Drill-down — replace the list with the file's patch.
        if let Some(idx) = self.files_drilldown {
            self.render_file_drilldown(f, area, detail, idx);
            return;
        }
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

    /// Full-screen commit detail view, modelled on the local
    /// commit-detail layout: a `┐ ┘` framed header with sha / author
    /// / date / stats / message body, then the file diffs below.
    fn render_commit_drilldown(&mut self, f: &mut Frame, area: Rect, sha: &str) {
        let theme = &self.ctx.color_theme;
        let Some(commit) = self.commit_detail_cache.get(sha).cloned() else {
            let p = Paragraph::new(Span::styled(
                "  Loading commit detail…".to_string(),
                Style::default().fg(theme.detail_label_fg),
            ));
            f.render_widget(p, area);
            return;
        };

        // Layout = header card (fixed height) + body (scrollable diff).
        let header_height = self.commit_header_height(&commit, area.width);
        let [header_area, body_area] = Layout::vertical([
            Constraint::Length(header_height),
            Constraint::Min(0),
        ])
        .areas(area);
        self.render_commit_header_card(f, header_area, &commit);

        // Body = files + patches, rendered through the active mode.
        let diff_mode = self.ctx.ui_config.common.diff_mode;
        let mut body_lines: Vec<Line<'static>> = Vec::new();
        if commit.files.is_empty() {
            body_lines.push(Line::from(Span::styled(
                "  (no files in this commit)".to_string(),
                Style::default().fg(theme.detail_label_fg),
            )));
        } else {
            for (i, file) in commit.files.iter().enumerate() {
                if i > 0 {
                    body_lines.push(Line::from(""));
                }
                body_lines.push(Line::from(diff_file_header_spans(theme, file)));
                body_lines.push(Line::from(Span::styled(
                    "─".repeat(body_area.width as usize),
                    Style::default().fg(theme.divider_fg),
                )));
                if let Some(patch) = &file.patch {
                    let hunks = parse_patch(patch);
                    let rendered = render_patch_lines(
                        &hunks,
                        diff_mode,
                        theme,
                        body_area.width as usize,
                    );
                    body_lines.extend(rendered);
                } else {
                    body_lines.push(Line::from(Span::styled(
                        "    (no patch — binary or too large)".to_string(),
                        Style::default().fg(theme.detail_label_fg),
                    )));
                }
            }
        }
        let max_scroll = body_lines.len().saturating_sub(body_area.height as usize);
        if self.commits_drilldown_scroll > max_scroll {
            self.commits_drilldown_scroll = max_scroll;
        }
        f.render_widget(
            Paragraph::new(body_lines)
                .scroll((self.commits_drilldown_scroll as u16, 0)),
            body_area,
        );
    }

    /// Pre-compute the header card height — 2 rows of border + body
    /// rows (sha/author/date/stats line, then the commit message
    /// split on `\n`). Capped to area.height / 2 so a giant commit
    /// message never eats the whole screen.
    fn commit_header_height(
        &self,
        commit: &crate::github::pr::CommitDetail,
        _width: u16,
    ) -> u16 {
        let msg_rows = commit.message.lines().count().max(1) as u16;
        // 1 meta row + 1 blank separator + msg + 2 border rows.
        (1 + 1 + msg_rows + 2).min(12)
    }

    fn render_commit_header_card(
        &self,
        f: &mut Frame,
        area: Rect,
        commit: &crate::github::pr::CommitDetail,
    ) {
        let theme = &self.ctx.color_theme;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    "Commit".to_string(),
                    Style::default()
                        .fg(theme.list_head_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ]));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Meta row: short SHA + author + date + stats.
        let short_sha: String = commit.sha.chars().take(7).collect();
        let meta = Line::from(vec![
            Span::styled(
                short_sha,
                Style::default()
                    .fg(theme.list_hash_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(theme.detail_label_fg)),
            Span::styled("by ", Style::default().fg(theme.detail_label_fg)),
            Span::styled(commit.author.clone(), Style::default().fg(theme.list_name_fg)),
            Span::styled(" · ", Style::default().fg(theme.detail_label_fg)),
            Span::styled(commit.date.clone(), Style::default().fg(theme.list_date_fg)),
            Span::styled(" · ", Style::default().fg(theme.detail_label_fg)),
            Span::styled(
                format!(
                    "{} file{} · ",
                    commit.files.len(),
                    if commit.files.len() == 1 { "" } else { "s" }
                ),
                Style::default().fg(theme.detail_label_fg),
            ),
            Span::styled(
                format!("+{}", commit.additions),
                Style::default().fg(theme.detail_file_change_add_fg),
            ),
            Span::raw(" "),
            Span::styled(
                format!("-{}", commit.deletions),
                Style::default().fg(theme.detail_file_change_delete_fg),
            ),
        ]);

        let mut lines: Vec<Line<'static>> = vec![meta, Line::from("")];
        for (i, msg_line) in commit.message.lines().enumerate() {
            // First line of the commit message is the subject — render
            // it bold so it pops over the body.
            let style = if i == 0 {
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg)
            };
            lines.push(Line::from(Span::styled(msg_line.to_string(), style)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    /// Full-screen file diff view — framed header card (status, path,
    /// stats, "i / total" counter) on top, scrollable diff body
    /// below. Render mode follows the global `diff_mode` setting so
    /// it matches the local diff view.
    fn render_file_drilldown(
        &mut self,
        f: &mut Frame,
        area: Rect,
        detail: &PullRequestDetail,
        idx: usize,
    ) {
        let theme = &self.ctx.color_theme;
        let Some(file) = detail.files.get(idx) else {
            self.files_drilldown = None;
            return;
        };

        let [header_area, body_area] =
            Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);
        self.render_file_header_card(f, header_area, file, idx, detail.files.len());

        let mut body_lines: Vec<Line<'static>> = Vec::new();
        if let Some(patch) = &file.patch {
            let diff_mode = self.ctx.ui_config.common.diff_mode;
            let hunks = parse_patch(patch);
            body_lines.extend(render_patch_lines(
                &hunks,
                diff_mode,
                theme,
                body_area.width as usize,
            ));
        } else {
            body_lines.push(Line::from(Span::styled(
                "    (no patch — binary or too large)".to_string(),
                Style::default().fg(theme.detail_label_fg),
            )));
        }
        let max_scroll = body_lines.len().saturating_sub(body_area.height as usize);
        if self.files_drilldown_scroll > max_scroll {
            self.files_drilldown_scroll = max_scroll;
        }
        f.render_widget(
            Paragraph::new(body_lines)
                .scroll((self.files_drilldown_scroll as u16, 0)),
            body_area,
        );
    }

    fn render_file_header_card(
        &self,
        f: &mut Frame,
        area: Rect,
        file: &crate::github::pr::PullFile,
        idx: usize,
        total: usize,
    ) {
        let theme = &self.ctx.color_theme;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    "File".to_string(),
                    Style::default()
                        .fg(theme.list_head_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ]));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Split the path into directory / basename — basename bold,
        // directory muted — mirrors how local file views display it.
        let (dir, name) = match file.filename.rfind('/') {
            Some(slash) => (&file.filename[..=slash], &file.filename[slash + 1..]),
            None => ("", file.filename.as_str()),
        };
        let (tag, tag_color) = match file.status {
            FileStatus::Added => ("Added", theme.detail_file_change_add_fg),
            FileStatus::Modified => ("Modified", theme.detail_file_change_modify_fg),
            FileStatus::Removed => ("Deleted", theme.detail_file_change_delete_fg),
            FileStatus::Renamed => ("Renamed", theme.detail_file_change_move_fg),
            FileStatus::Other => ("Changed", theme.detail_label_fg),
        };
        let spans = vec![
            Span::styled(
                tag.to_string(),
                Style::default().fg(tag_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(theme.detail_label_fg)),
            Span::styled(dir.to_string(), Style::default().fg(theme.detail_label_fg)),
            Span::styled(
                name.to_string(),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", Style::default().fg(theme.detail_label_fg)),
            Span::styled(
                format!("+{}", file.additions),
                Style::default().fg(theme.detail_file_change_add_fg),
            ),
            Span::raw(" "),
            Span::styled(
                format!("-{}", file.deletions),
                Style::default().fg(theme.detail_file_change_delete_fg),
            ),
            Span::styled(" · ", Style::default().fg(theme.detail_label_fg)),
            Span::styled(
                format!("{} of {}", idx + 1, total),
                Style::default().fg(theme.detail_label_fg),
            ),
        ];
        f.render_widget(Paragraph::new(Line::from(spans)), inner);
    }
}

/// One comment "card" in the Conversation tab: a full 4-sided box
/// (`┌─┐ │ └─┘`) with the author / action / timestamp baked into the
/// top border. Replies sit indented to the right of their parent; the
/// connector is a real T-shaped dashed line (`┊╌╌╌`) that descends from
/// the parent's bottom and bends right to dock at each reply's top-left.
/// A card with no replies emits no descending line at all.
pub(crate) struct CommentCardInput<'a> {
    pub(crate) author: &'a str,
    pub(crate) action: CommentAction,
    pub(crate) when: &'a str,
    pub(crate) body: &'a str,
    /// When `Some`, push_comment_card reserves a 2-cell wide slot
    /// just after `┌─ ` in the top border. The caller is expected
    /// to paint the avatar there itself (after the Paragraph
    /// containing this card has been rendered) using the
    /// `CommentCardLayout::avatar_slot` returned from this call.
    /// Empty / `None` keeps the original PR layout intact.
    pub(crate) avatar_login: Option<&'a str>,
    /// `ancestor_gutters[i] == true` means the gutter at depth i still
    /// has children coming below; we draw `│` there. `false` means the
    /// gutter is closed (we draw spaces). The LAST entry is the parent's
    /// gutter and dictates the bend on the junction row.
    pub(crate) ancestor_gutters: &'a [bool],
    /// True when at least one reply will be rendered directly under this
    /// card. Drives the bottom-left corner of the box: `├` (attaches the
    /// descending line) vs the standard `└` (closes the box cleanly).
    pub(crate) has_children: bool,
    /// When `true`, this card is the currently-selected comment in the
    /// Conversation tab — the box border switches to the head accent so
    /// the user can see which card has focus.
    pub(crate) is_selected: bool,
    /// `true` when `author` matches the authenticated GitHub login —
    /// we append a small `(me)` chip after the name.
    pub(crate) is_me: bool,
    /// Action shortcuts to surface inline in the top border when this
    /// card is selected (e.g. ["R:reply", "e:edit", "d:delete"]).
    /// Empty for cards that have no card-scoped actions (PR description,
    /// reviews) or when not selected.
    pub(crate) inline_shortcuts: Vec<&'static str>,
    /// Reaction totals to render as an `emoji N` chip row below the
    /// comment body. Empty / zero-count = no row drawn.
    pub(crate) reactions: crate::github::pr::ReactionCounts,
}

pub(crate) enum CommentAction {
    Opened,
    /// Issue-context variant of `Opened` — renders "opened this issue"
    /// in the top border instead of "opened this PR". Lets the Issues
    /// view reuse `push_comment_card` without dragging in PR semantics.
    OpenedIssue,
    Commented,
    #[allow(dead_code)]
    Review(ReviewState),
    #[allow(dead_code)]
    ReviewComment { file: String, line: Option<u64> },
}

/// Column width of one tree level (`┊   `): dashed gutter + 3-space pad.
pub(crate) const TREE_LEVEL_WIDTH: u16 = 4;

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

pub(crate) fn build_body_prefix(
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

pub(crate) fn build_junction_prefix(
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

/// One pending avatar paint. Carries the `login` directly (instead
/// of a borrow) so the slot can outlive the iteration that produced
/// it — important because the rendering loop borrows `self.detail`
/// while pushing slots, and the paint pass borrows `self` mutably.
#[derive(Debug, Clone)]
pub(crate) struct AvatarSlot {
    pub(crate) login: String,
    /// Logical line index inside the Paragraph buffer (pre-scroll).
    pub(crate) line: usize,
    /// Column offset inside that line where the avatar's first
    /// cell lands.
    pub(crate) col: u16,
    /// Selection state — affects the background blending of the
    /// rendered avatar (selected cards have a different bg).
    pub(crate) is_selected: bool,
}

/// Materialised avatar position — what was painted on screen during
/// a particular frame. The diff helper compares the previous frame's
/// list against the current one to decide which cells to clear vs.
/// skip vs. paint anew. Equality includes the selected state so a
/// row swapping selected/unselected forces a repaint (different bg).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PaintedAvatar {
    pub(crate) login: String,
    pub(crate) screen_x: u16,
    pub(crate) screen_y: u16,
    pub(crate) is_selected: bool,
}

/// Diff-aware avatar painter. Given the previous frame's painted set
/// and the current frame's intended set (with the bg colour for each
/// new paint), it:
///
///   * For slots in `prev` but not in `current` → writes the image
///     protocol's `clear_cell` over the old position so the persistent
///     Kitty / iTerm2 placement is evicted (fixes the "ghost trail"
///     left by hover/scroll/filter changes).
///   * For slots present in both → marks the cells as `set_skip(true)`
///     so ratatui's diff renderer doesn't re-emit the protocol bytes
///     this frame (kills the hover flicker — image bytes weighing
///     thousands of bytes were being shipped on every cursor move).
///   * For slots in `current` but not in `prev` (or moved/changed) →
///     paints via `paint_login_avatar` as before.
///
/// Returns the new `prev` for the caller to store on the view —
/// typically as `self.prev_*_painted_avatars`.
pub(crate) fn paint_avatars_with_diff(
    f: &mut Frame,
    ctx: &AppContext,
    prev: &[PaintedAvatar],
    current: Vec<(PaintedAvatar, Color)>,
) -> Vec<PaintedAvatar> {
    use crate::protocol::ImageProtocol;
    let is_kitty = matches!(
        ctx.image_protocol,
        ImageProtocol::Kitty | ImageProtocol::KittyUnicode { .. }
    );
    // 1. Clear residue at every position that was painted last frame
    //    but isn't part of the current set.
    //
    // For Kitty: emit the cursor-position + delete-at-cursor escape
    // sequence DIRECTLY to stdout — we can't stuff this into the
    // ratatui buffer cell because the resulting symbol's
    // `unicode-width` ends up at ~11 (`_Ga=d,d=C;` + payload char),
    // and ratatui then marks the next 10 cells as continuation and
    // skips their emit → the underlying text disappears. By writing
    // the delete escape outside the buffer pipeline, the terminal
    // evicts the placement BEFORE ratatui flushes its cells; the
    // Paragraph's chars at those positions then render normally
    // on top.
    //
    // For iTerm2 / Sixel: images flow inline with terminal content;
    // ratatui's normal re-paint at those positions already replaces
    // the image bytes, no protocol escape needed.
    if is_kitty {
        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        let area = f.buffer_mut().area();
        let right = area.right();
        let bottom = area.bottom();
        for p in prev {
            if current.iter().any(|(c, _)| c == p) {
                continue;
            }
            for dx in 0..2u16 {
                let x = p.screen_x + dx;
                if x >= right || p.screen_y >= bottom {
                    break;
                }
                // CSI cursor-position is 1-based: row Y, col X.
                let _ = write!(
                    stdout,
                    "\x1b[{};{}H\x1b_Ga=d,d=C;\x1b\\",
                    p.screen_y + 1,
                    x + 1
                );
            }
        }
        let _ = stdout.flush();
    }
    // 2. For each current slot, decide whether to skip-paint (no
    //    re-emit, preserve terminal pixels) or paint fresh.
    for (pa, bg) in &current {
        if prev.contains(pa) {
            let buf = f.buffer_mut();
            let area = buf.area();
            let right = area.right();
            let bottom = area.bottom();
            for dx in 0..2u16 {
                let x = pa.screen_x + dx;
                if x >= right || pa.screen_y >= bottom {
                    break;
                }
                buf[(x, pa.screen_y)].set_skip(true);
            }
        } else {
            paint_login_avatar(
                f,
                ctx,
                &pa.login,
                pa.screen_x,
                pa.screen_y,
                pa.is_selected,
                *bg,
            );
        }
    }
    current.into_iter().map(|(p, _)| p).collect()
}

/// Paint a single login's avatar at `(screen_x, screen_y)`. No-op
/// when avatars are disabled, the login is empty, or the avatar
/// isn't yet on disk (a background prefetch may have been fired but
/// hasn't completed). Triggers a prefetch on miss so the next render
/// has it.
///
/// `bg` is the rounded-edge blend colour — must match the cell
/// background visible BEHIND the slot, otherwise the alpha-blended
/// circle reads as a misaligned chip. Callers in a list with a row
/// highlight pass `list_selected_bg` for selected rows and
/// `theme.bg` otherwise; callers with no row bg change always pass
/// `theme.bg`. The `is_selected` flag keys a separate cached variant
/// of the prepared image so a row toggling selected/unselected
/// doesn't churn the cache.
pub(crate) fn paint_login_avatar(
    f: &mut Frame,
    ctx: &AppContext,
    login: &str,
    screen_x: u16,
    screen_y: u16,
    is_selected: bool,
    bg: Color,
) {
    if login.is_empty() {
        return;
    }
    let mut manager = ctx.avatar_manager.lock().unwrap();
    if !manager.is_enabled() {
        return;
    }
    // Try to bring the avatar online — `ensure_uploaded_login` is
    // idempotent and cheap when the image is already prepared.
    let on_disk = manager.cached_avatar_exists_login(login);
    if on_disk {
        manager.ensure_uploaded_login(login, 1, is_selected, bg);
    } else {
        // Fire-and-forget background fetch; the AvatarsUpdated event
        // triggers a redraw once the image lands.
        manager.prefetch_login(login);
        return;
    }
    let Some(prepared) = manager.prepared_image_login(login, 1, is_selected) else {
        return;
    };
    let buf = f.buffer_mut();
    for (dx, cell) in prepared.cells().iter().take(2).enumerate() {
        let x = screen_x + dx as u16;
        if x >= buf.area().right() || screen_y >= buf.area().bottom() {
            break;
        }
        let bc = &mut buf[(x, screen_y)];
        bc.set_symbol(cell.symbol());
        bc.set_style(cell.style().bg(bg));
        bc.set_skip(cell.skip());
    }
}

/// Layout coordinates a caller needs after `push_comment_card` to
/// paint overlays (currently: the GitHub avatar). All positions are
/// relative to the start of the card's enclosing Paragraph area; the
/// caller adds `area.x` and `area.y - scroll_offset` to get screen
/// coordinates.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CommentCardLayout {
    /// Logical line index of the top-border row (the row where the
    /// author name appears).
    pub(crate) top_line: usize,
    /// `Some((x_offset, login_token_width))` when an avatar slot was
    /// reserved — `x_offset` is the column offset inside the line
    /// where the avatar's first cell lands; `width` is always 2.
    pub(crate) avatar_slot: Option<u16>,
}

pub(crate) fn push_comment_card(
    lines: &mut Vec<Line<'static>>,
    theme: &crate::color::ColorTheme,
    available_width: u16,
    input: CommentCardInput<'_>,
) -> CommentCardLayout {
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
        CommentAction::OpenedIssue => Span::styled(" opened this issue", label),
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
    // The top border lands on the line at `lines.len()` right now.
    // Capture it BEFORE the push so the caller can paint overlays
    // at the same row.
    let top_line = lines.len();
    let mut top: Vec<Span<'static>> = build_body_prefix(theme, &top_gutters);
    // Track where the avatar slot lives so the caller can paint it
    // after this Paragraph renders. The slot sits between the
    // leading `┌─` and the author name, replacing the single space
    // that already separated them — so layout shifts by exactly 2
    // cells (avatar width) plus an extra space we add for breathing
    // room.
    let prefix_cells: u16 = top
        .iter()
        .map(|s| console::measure_text_width(s.content.as_ref()) as u16)
        .sum();
    let want_avatar = input
        .avatar_login
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let (border_open_str, avatar_slot_x): (&str, Option<u16>) = if want_avatar {
        // `"┌─ "` + 2 reserved cells + extra space → `┌─ AV `
        // where AV is the painted avatar. Total prefix width = 6.
        ("┌─ ", Some(prefix_cells + 3))
    } else {
        ("┌─ ", None)
    };
    top.push(Span::styled(border_open_str.to_string(), border_style));
    let mut consumed = 3; // "┌─ "
    if avatar_slot_x.is_some() {
        // Reserve 2 cells worth of placeholder + 1 trailing space.
        // Painted with the border style so an empty slot (avatar
        // not yet on disk) reads as continuation of the border.
        top.push(Span::styled("   ".to_string(), border_style));
        consumed += 3;
    }
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
    // Reaction chip row — sits between the body and the bottom
    // border, drawn only when at least one of the eight reactions
    // has a count. Each chip = `emoji N` in a muted style, with
    // 1-col gaps between chips.
    if input.reactions.total() > 0 {
        let mut chip_spans: Vec<Span<'static>> = Vec::new();
        let count_style = Style::default()
            .fg(theme.detail_label_fg)
            .add_modifier(Modifier::BOLD);
        for (kind, count) in input
            .reactions
            .iter()
            .filter(|(_, n)| *n > 0)
            .collect::<Vec<_>>()
        {
            if !chip_spans.is_empty() {
                chip_spans.push(Span::raw("  ".to_string()));
            }
            // No bg, no brackets — just `emoji count` rendered
            // plain. The bold muted count keeps the pair readable
            // without dressing.
            chip_spans.push(Span::styled(
                format!("{} ", kind.emoji()),
                Style::default().fg(theme.fg),
            ));
            chip_spans.push(Span::styled(format!("{}", count), count_style));
        }
        // Measure to compute the trailing pad to the right border.
        let used: usize = chip_spans
            .iter()
            .map(|s| console::measure_text_width(s.content.as_ref()))
            .sum();
        let pad = inner_width.saturating_sub(used.min(inner_width));
        let mut row = build_body_prefix(theme, input.ancestor_gutters);
        row.push(Span::styled("│ ".to_string(), border_style));
        row.extend(chip_spans);
        row.push(Span::styled(format!("{} │", " ".repeat(pad)), border_style));
        lines.push(Line::from(row));
    }
    let _ = value;
    let _ = input.has_children;
    let bottom_dashes = "─".repeat(card_width.saturating_sub(2));
    let mut bot = build_body_prefix(theme, input.ancestor_gutters);
    bot.push(Span::styled(format!("└{}┘", bottom_dashes), border_style));
    lines.push(Line::from(bot));

    CommentCardLayout {
        top_line,
        avatar_slot: avatar_slot_x,
    }
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

/// Returns `(line, avatar_col)` — `Some(col)` when the 3-cell pad
/// was reserved for an avatar (avatars enabled + login known),
/// `None` when the row layout stays tight without a pad.
fn commit_row(
    theme: &crate::color::ColorTheme,
    c: &PullCommit,
    cols: &CommitColumns,
    avatars_on: bool,
) -> (Line<'static>, Option<u16>) {
    let mut spans: Vec<Span<'static>> = vec![
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
    ];
    // 1-cell gap between the subject and the avatar so dense
    // commit titles don't crash into the avatar. Only when we'll
    // actually paint an avatar — otherwise the regular 2-cell
    // gap below covers the column rhythm.
    let want_avatar = avatars_on && !c.author_login.is_empty();
    if want_avatar {
        spans.push(Span::raw(" "));
    }
    // Only reserve the 3-cell pad when avatars are enabled AND the
    // commit has a resolved GitHub login (otherwise paint would
    // skip and the pad would just be a gap).
    let avatar_col = if want_avatar {
        let col: u16 = spans.iter().map(|s| s.content.chars().count() as u16).sum();
        spans.push(Span::raw("   "));
        Some(col)
    } else {
        spans.push(Span::raw("  "));
        None
    };
    spans.push(Span::styled(
        fit_cell(&c.author, cols.author),
        Style::default().fg(theme.list_name_fg),
    ));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(
        fit_cell(&c.date, cols.date),
        Style::default().fg(theme.list_date_fg),
    ));
    (Line::from(spans), avatar_col)
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

/// Render a single label as a GitHub-style coloured chip ` name `,
/// flipping foreground between black and white based on the label
/// colour's perceived luminance. Falls back to plain dim text when
/// the API didn't return a hex colour.
pub(crate) fn label_chip_spans_local(
    label: &crate::github::pr::Label,
) -> Vec<Span<'static>> {
    if let Some((r, g, b)) = label
        .color
        .as_deref()
        .and_then(parse_hex_color)
    {
        let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        let fg = if luminance > 140.0 {
            Color::Rgb(0, 0, 0)
        } else {
            Color::Rgb(255, 255, 255)
        };
        vec![Span::styled(
            format!(" {} ", label.name),
            Style::default()
                .fg(fg)
                .bg(Color::Rgb(r, g, b))
                .add_modifier(Modifier::BOLD),
        )]
    } else {
        vec![Span::raw(label.name.clone())]
    }
}

/// Compact 4-col label chip for the PR list — `LABEL_CHIP_WIDTH` cols
/// total: leading space + 2-letter abbreviation + trailing space, all
/// painted in the label's background colour. Two letters are picked
/// from the start of the label name, uppercased so it reads as a
/// badge rather than a word fragment.
pub(crate) fn short_label_chip_spans(
    label: &crate::github::pr::Label,
) -> Vec<Span<'static>> {
    let abbr: String = label
        .name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_uppercase();
    // Pad short names (single char or empty) so the chip stays the
    // canonical width.
    let abbr_padded = if abbr.chars().count() >= 2 {
        abbr
    } else {
        format!("{:<2}", abbr)
    };
    if let Some((r, g, b)) = label.color.as_deref().and_then(parse_hex_color) {
        let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        let fg = if luminance > 140.0 {
            Color::Rgb(0, 0, 0)
        } else {
            Color::Rgb(255, 255, 255)
        };
        vec![Span::styled(
            format!(" {} ", abbr_padded),
            Style::default()
                .fg(fg)
                .bg(Color::Rgb(r, g, b))
                .add_modifier(Modifier::BOLD),
        )]
    } else {
        vec![Span::raw(format!(" {} ", abbr_padded))]
    }
}

pub(crate) fn parse_hex_color(hex: &str) -> Option<(u8, u8, u8)> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Inline file header for the drill-down view — tag + filename +
/// `+N -M`, sharing the colour palette of the list rows so they read
/// the same.
fn diff_file_header_spans(
    theme: &crate::color::ColorTheme,
    f: &crate::github::pr::PullFile,
) -> Vec<Span<'static>> {
    let (tag, tag_color) = match f.status {
        FileStatus::Added => ("A", theme.detail_file_change_add_fg),
        FileStatus::Modified => ("M", theme.detail_file_change_modify_fg),
        FileStatus::Removed => ("D", theme.detail_file_change_delete_fg),
        FileStatus::Renamed => ("R", theme.detail_file_change_move_fg),
        FileStatus::Other => ("?", theme.detail_label_fg),
    };
    vec![
        Span::raw("  "),
        Span::styled(
            tag.to_string(),
            Style::default().fg(tag_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            f.filename.clone(),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
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
    ]
}

/// One line inside a parsed unified-diff hunk — kind drives the
/// per-mode rendering decisions (colour, gutter line numbers,
/// side-by-side bucketing).
#[derive(Debug, Clone)]
enum PatchLineKind {
    Context,
    Add,
    Delete,
}

#[derive(Debug, Clone)]
struct PatchLine {
    kind: PatchLineKind,
    /// Line content without the leading `+`, `-`, or ` ` marker.
    text: String,
}

#[derive(Debug, Clone)]
struct PatchHunk {
    old_start: u32,
    new_start: u32,
    /// Trailing context text after the second `@@` marker (e.g.,
    /// function name) — surfaced in the enhanced gutter header.
    header_extra: String,
    lines: Vec<PatchLine>,
}

/// Parse a GitHub-style unified diff patch into hunks. Robust to
/// malformed input: lines that don't fit any pattern get dropped
/// quietly so the renderer never panics on a weird payload.
fn parse_patch(patch: &str) -> Vec<PatchHunk> {
    let mut hunks: Vec<PatchHunk> = Vec::new();
    for raw_line in patch.lines() {
        if let Some(rest) = raw_line.strip_prefix("@@") {
            // Header: `@@ -old_start[,old_count] +new_start[,new_count] @@ extra`
            let (range_part, extra) = rest
                .find("@@")
                .map(|i| (&rest[..i], rest[i + 2..].trim().to_string()))
                .unwrap_or((rest, String::new()));
            let mut old_start = 0u32;
            let mut new_start = 0u32;
            for token in range_part.split_whitespace() {
                let (sign, body) = token.split_at(1);
                let start: u32 = body
                    .split(',')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                match sign {
                    "-" => old_start = start,
                    "+" => new_start = start,
                    _ => {}
                }
            }
            hunks.push(PatchHunk {
                old_start,
                new_start,
                header_extra: extra,
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            // Lines before the first `@@` (file headers from the
            // raw patch, "No newline at end of file" markers, etc.)
            // are not meaningful for the diff body — skip them.
            continue;
        };
        let (kind, text) = match raw_line.chars().next() {
            Some('+') => (PatchLineKind::Add, raw_line[1..].to_string()),
            Some('-') => (PatchLineKind::Delete, raw_line[1..].to_string()),
            Some(' ') => (PatchLineKind::Context, raw_line[1..].to_string()),
            Some('\\') => continue, // `\ No newline at end of file`
            _ => continue,
        };
        hunk.lines.push(PatchLine { kind, text });
    }
    hunks
}

/// Background colours for the enhanced diff modes — picked to match
/// the local commit-detail view so PR diffs read the same. Dark and
/// light themes get distinct values to keep the contrast tasteful.
struct EnhancedDiffPalette {
    add_bg: Color,
    del_bg: Color,
}

impl EnhancedDiffPalette {
    fn for_theme(theme: &crate::color::ColorTheme) -> Self {
        let bg_is_light = match theme.bg {
            Color::Rgb(r, g, b) => {
                (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) > 128.0
            }
            _ => false,
        };
        if bg_is_light {
            Self {
                add_bg: Color::Rgb(172, 242, 189),
                del_bg: Color::Rgb(255, 186, 181),
            }
        } else {
            Self {
                add_bg: Color::Rgb(32, 68, 45),
                del_bg: Color::Rgb(68, 35, 40),
            }
        }
    }
}

/// Render a parsed patch into ratatui lines using the user's active
/// diff mode. The width is needed to know how much room each side
/// gets in the side-by-side modes; it's ignored by raw + enhanced.
fn render_patch_lines(
    hunks: &[PatchHunk],
    mode: crate::config::DiffMode,
    theme: &crate::color::ColorTheme,
    width: usize,
) -> Vec<Line<'static>> {
    match mode {
        crate::config::DiffMode::Raw => render_patch_raw(hunks, theme),
        crate::config::DiffMode::Enhanced => render_patch_enhanced(hunks, theme),
        crate::config::DiffMode::SideBySide => {
            render_patch_split(hunks, theme, width, false)
        }
        crate::config::DiffMode::SideBySideEnhanced => {
            render_patch_split(hunks, theme, width, true)
        }
    }
}

/// Raw mode — closest to the unified-diff text on the wire. Each
/// line keeps its leading marker and is fg-coloured by kind.
fn render_patch_raw(
    hunks: &[PatchHunk],
    theme: &crate::color::ColorTheme,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    for hunk in hunks {
        out.push(Line::from(Span::styled(
            format!(
                "@@ -{} +{} @@ {}",
                hunk.old_start, hunk.new_start, hunk.header_extra
            ),
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD),
        )));
        for line in &hunk.lines {
            let (marker, style) = match line.kind {
                PatchLineKind::Add => (
                    "+",
                    Style::default().fg(theme.detail_file_change_add_fg),
                ),
                PatchLineKind::Delete => (
                    "-",
                    Style::default().fg(theme.detail_file_change_delete_fg),
                ),
                PatchLineKind::Context => (" ", Style::default().fg(theme.fg)),
            };
            out.push(Line::from(Span::styled(
                format!("{}{}", marker, line.text),
                style,
            )));
        }
    }
    out
}

/// Enhanced mode — per-side line-number gutter, full-row background
/// colour for add / del, vertical bar at the left of each diff line
/// to make the change region pop. Matches the local diff view.
fn render_patch_enhanced(
    hunks: &[PatchHunk],
    theme: &crate::color::ColorTheme,
) -> Vec<Line<'static>> {
    let pal = EnhancedDiffPalette::for_theme(theme);
    let max_lineno = hunks
        .iter()
        .map(|h| {
            let last_old = h.old_start
                + h.lines
                    .iter()
                    .filter(|l| !matches!(l.kind, PatchLineKind::Add))
                    .count() as u32;
            let last_new = h.new_start
                + h.lines
                    .iter()
                    .filter(|l| !matches!(l.kind, PatchLineKind::Delete))
                    .count() as u32;
            last_old.max(last_new)
        })
        .max()
        .unwrap_or(0);
    let gutter_width = (max_lineno.to_string().len()).max(2);

    let mut out: Vec<Line<'static>> = Vec::new();
    for hunk in hunks {
        // Hunk header — single accent row, no bg fill.
        out.push(Line::from(Span::styled(
            format!(
                " {} @@ -{} +{} @@ {}",
                " ".repeat(2 * gutter_width + 1),
                hunk.old_start,
                hunk.new_start,
                hunk.header_extra,
            ),
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD),
        )));
        let mut old_no = hunk.old_start;
        let mut new_no = hunk.new_start;
        for line in &hunk.lines {
            let (old_label, new_label, bar_color, bg) = match line.kind {
                PatchLineKind::Add => {
                    let label = format!("{:>w$}", new_no, w = gutter_width);
                    new_no += 1;
                    (
                        " ".repeat(gutter_width),
                        label,
                        theme.detail_file_change_add_fg,
                        Some(pal.add_bg),
                    )
                }
                PatchLineKind::Delete => {
                    let label = format!("{:>w$}", old_no, w = gutter_width);
                    old_no += 1;
                    (
                        label,
                        " ".repeat(gutter_width),
                        theme.detail_file_change_delete_fg,
                        Some(pal.del_bg),
                    )
                }
                PatchLineKind::Context => {
                    let o = format!("{:>w$}", old_no, w = gutter_width);
                    let n = format!("{:>w$}", new_no, w = gutter_width);
                    old_no += 1;
                    new_no += 1;
                    (o, n, theme.divider_fg, None)
                }
            };
            let base = bg
                .map(|b| Style::default().bg(b))
                .unwrap_or_else(Style::default);
            let gutter_style = base.fg(theme.detail_label_fg);
            let bar_style = if matches!(line.kind, PatchLineKind::Context) {
                Style::default().fg(theme.divider_fg)
            } else {
                base.fg(bar_color)
            };
            let text_style = match line.kind {
                PatchLineKind::Add => base.fg(theme.fg),
                PatchLineKind::Delete => base.fg(theme.fg),
                PatchLineKind::Context => Style::default().fg(theme.fg),
            };
            out.push(Line::from(vec![
                Span::styled(format!(" {} {} ", old_label, new_label), gutter_style),
                Span::styled("▍".to_string(), bar_style),
                Span::styled(format!(" {}", line.text), text_style),
            ]));
        }
    }
    out
}

/// Side-by-side mode — old version on the left, new on the right,
/// `│` separator in the middle. Modifications (a Delete followed by
/// an Add inside the same hunk) zip row-by-row so the changed text
/// aligns horizontally. `enhanced` adds the per-side bg fill +
/// line-number gutters to match `SideBySideEnhanced` from the local
/// diff view.
fn render_patch_split(
    hunks: &[PatchHunk],
    theme: &crate::color::ColorTheme,
    width: usize,
    enhanced: bool,
) -> Vec<Line<'static>> {
    let pal = EnhancedDiffPalette::for_theme(theme);
    let max_lineno = hunks
        .iter()
        .map(|h| {
            let last_old = h.old_start
                + h.lines
                    .iter()
                    .filter(|l| !matches!(l.kind, PatchLineKind::Add))
                    .count() as u32;
            let last_new = h.new_start
                + h.lines
                    .iter()
                    .filter(|l| !matches!(l.kind, PatchLineKind::Delete))
                    .count() as u32;
            last_old.max(last_new)
        })
        .max()
        .unwrap_or(0);
    let gutter_width = if enhanced {
        max_lineno.to_string().len().max(2)
    } else {
        0
    };
    // Total width minus `│` and two spaces around it.
    let side_width = width.saturating_sub(3) / 2;
    let text_side_width = side_width.saturating_sub(gutter_width + 2).max(1);

    let mut out: Vec<Line<'static>> = Vec::new();
    for hunk in hunks {
        out.push(Line::from(Span::styled(
            format!("@@ -{} +{} @@ {}", hunk.old_start, hunk.new_start, hunk.header_extra),
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD),
        )));
        // Group consecutive Del/Add runs so we can zip them.
        let mut i = 0usize;
        let mut old_no = hunk.old_start;
        let mut new_no = hunk.new_start;
        while i < hunk.lines.len() {
            match hunk.lines[i].kind {
                PatchLineKind::Context => {
                    let text = hunk.lines[i].text.clone();
                    out.push(split_row(
                        Some((old_no, &text)),
                        Some((new_no, &text)),
                        gutter_width,
                        text_side_width,
                        enhanced,
                        None,
                        None,
                        theme,
                    ));
                    old_no += 1;
                    new_no += 1;
                    i += 1;
                }
                _ => {
                    // Collect a run of deletes followed by adds.
                    let mut dels: Vec<&str> = Vec::new();
                    let mut adds: Vec<&str> = Vec::new();
                    while i < hunk.lines.len()
                        && matches!(hunk.lines[i].kind, PatchLineKind::Delete)
                    {
                        dels.push(&hunk.lines[i].text);
                        i += 1;
                    }
                    while i < hunk.lines.len()
                        && matches!(hunk.lines[i].kind, PatchLineKind::Add)
                    {
                        adds.push(&hunk.lines[i].text);
                        i += 1;
                    }
                    let pair_count = dels.len().max(adds.len());
                    for k in 0..pair_count {
                        let left = dels.get(k).map(|t| (old_no + k as u32, *t));
                        let right = adds.get(k).map(|t| (new_no + k as u32, *t));
                        out.push(split_row(
                            left,
                            right,
                            gutter_width,
                            text_side_width,
                            enhanced,
                            if enhanced { Some(pal.del_bg) } else { None },
                            if enhanced { Some(pal.add_bg) } else { None },
                            theme,
                        ));
                    }
                    old_no += dels.len() as u32;
                    new_no += adds.len() as u32;
                }
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn split_row(
    left: Option<(u32, &str)>,
    right: Option<(u32, &str)>,
    gutter_width: usize,
    text_side_width: usize,
    enhanced: bool,
    left_bg: Option<Color>,
    right_bg: Option<Color>,
    theme: &crate::color::ColorTheme,
) -> Line<'static> {
    let format_side = |side: Option<(u32, &str)>, bg: Option<Color>| -> Vec<Span<'static>> {
        let base = bg
            .map(|b| Style::default().bg(b))
            .unwrap_or_else(Style::default);
        let (lineno, text) = side
            .map(|(n, t)| (format!("{:>w$}", n, w = gutter_width), t.to_string()))
            .unwrap_or_else(|| (" ".repeat(gutter_width), String::new()));
        let truncated: String = if text.chars().count() > text_side_width {
            let cut: String = text.chars().take(text_side_width.saturating_sub(1)).collect();
            format!("{}…", cut)
        } else {
            format!("{:<w$}", text, w = text_side_width)
        };
        if enhanced {
            vec![
                Span::styled(
                    format!("{} ", lineno),
                    base.fg(theme.detail_label_fg),
                ),
                Span::styled(truncated, base.fg(theme.fg)),
            ]
        } else {
            vec![Span::styled(truncated, base.fg(theme.fg))]
        }
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.extend(format_side(left, left_bg));
    spans.push(Span::styled(
        " │ ".to_string(),
        Style::default().fg(theme.divider_fg),
    ));
    spans.extend(format_side(right, right_bg));
    Line::from(spans)
}

/// Compute (column, row) within the editor body for a buffer cursor at
/// the given byte offset. Lines are split on `\n`; `col` is the
/// visible-width count of the prefix in the current logical line.
pub(crate) fn cursor_screen_pos(buf: &str, byte_cursor: usize) -> (u16, u16) {
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
pub(crate) fn word_left_boundary(buf: &str, mut cursor: usize) -> usize {
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

pub(crate) fn word_right_boundary(buf: &str, mut cursor: usize) -> usize {
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

// ─── Compose-form editor helpers ──────────────────────────────────

/// Return a mutable reference to the text buffer for the focused
/// compose field — None when the focused field isn't text (Draft).
fn compose_field_text_mut(state: &mut ComposeState) -> Option<&mut String> {
    match state.focused {
        ComposeField::Head => Some(&mut state.head),
        ComposeField::Base => Some(&mut state.base),
        ComposeField::Title => Some(&mut state.title),
        ComposeField::Body => Some(&mut state.body),
        ComposeField::Labels | ComposeField::Draft => None,
    }
}

fn compose_field_text(state: &ComposeState) -> Option<&str> {
    Some(match state.focused {
        ComposeField::Head => state.head.as_str(),
        ComposeField::Base => state.base.as_str(),
        ComposeField::Title => state.title.as_str(),
        ComposeField::Body => state.body.as_str(),
        ComposeField::Labels | ComposeField::Draft => return None,
    })
}

fn compose_field_insert_char(state: &mut ComposeState, c: char) {
    let cursor = state.cursor;
    let Some(buf) = compose_field_text_mut(state) else {
        return;
    };
    let safe = cursor.min(buf.len());
    buf.insert(safe, c);
    state.cursor = safe + c.len_utf8();
}

fn compose_field_delete_left(state: &mut ComposeState) {
    let cursor = state.cursor;
    let Some(buf) = compose_field_text_mut(state) else {
        return;
    };
    if cursor == 0 {
        return;
    }
    let mut new_cursor = cursor - 1;
    while new_cursor > 0 && !buf.is_char_boundary(new_cursor) {
        new_cursor -= 1;
    }
    buf.replace_range(new_cursor..cursor, "");
    state.cursor = new_cursor;
}

fn compose_field_delete_word_left(state: &mut ComposeState) {
    let cursor = state.cursor;
    let Some(buf) = compose_field_text_mut(state) else {
        return;
    };
    let new_cursor = word_left_boundary(buf, cursor);
    buf.replace_range(new_cursor..cursor, "");
    state.cursor = new_cursor;
}

fn compose_field_cursor_left(state: &mut ComposeState) {
    let cursor = state.cursor;
    let Some(buf) = compose_field_text(state) else {
        return;
    };
    if cursor == 0 {
        return;
    }
    let mut c = cursor - 1;
    while c > 0 && !buf.is_char_boundary(c) {
        c -= 1;
    }
    state.cursor = c;
}

fn compose_field_cursor_right(state: &mut ComposeState) {
    let cursor = state.cursor;
    let Some(buf) = compose_field_text(state) else {
        return;
    };
    if cursor >= buf.len() {
        return;
    }
    let mut c = cursor + 1;
    while c < buf.len() && !buf.is_char_boundary(c) {
        c += 1;
    }
    state.cursor = c;
}

fn compose_field_cursor_home(state: &mut ComposeState) {
    let cursor = state.cursor;
    let Some(buf) = compose_field_text(state) else {
        return;
    };
    // For multi-line body: jump to start of current line.
    let prefix = &buf[..cursor.min(buf.len())];
    state.cursor = prefix.rfind('\n').map(|i| i + 1).unwrap_or(0);
}

fn compose_field_cursor_end(state: &mut ComposeState) {
    let cursor = state.cursor;
    let Some(buf) = compose_field_text(state) else {
        return;
    };
    state.cursor = buf[cursor.min(buf.len())..]
        .find('\n')
        .map(|i| cursor + i)
        .unwrap_or(buf.len());
}

fn compose_body_cursor_vertical(
    state: &mut ComposeState,
    key: ratatui::crossterm::event::KeyCode,
) {
    use ratatui::crossterm::event::KeyCode;
    if !matches!(state.focused, ComposeField::Body) {
        return;
    }
    let cursor = state.cursor;
    let buf = state.body.clone();
    let line_start = buf[..cursor.min(buf.len())]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let col = cursor - line_start;
    match key {
        KeyCode::Up => {
            if line_start == 0 {
                state.cursor = 0;
                return;
            }
            let prev_end = line_start - 1;
            let prev_start = buf[..prev_end]
                .rfind('\n')
                .map(|i| i + 1)
                .unwrap_or(0);
            let prev_line_len = prev_end - prev_start;
            state.cursor = prev_start + col.min(prev_line_len);
        }
        KeyCode::Down => {
            let line_end = buf[line_start..]
                .find('\n')
                .map(|i| line_start + i)
                .unwrap_or(buf.len());
            if line_end >= buf.len() {
                state.cursor = buf.len();
                return;
            }
            let next_start = line_end + 1;
            let next_end = buf[next_start..]
                .find('\n')
                .map(|i| next_start + i)
                .unwrap_or(buf.len());
            let next_line_len = next_end - next_start;
            state.cursor = next_start + col.min(next_line_len);
        }
        _ => {}
    }
}

/// Re-anchor the picker's scroll so its currently-hovered row sits
/// inside the visible window. Called after keyboard nav; mouse-wheel
/// scrolls deliberately don't touch hovered, so they bypass this.
fn picker_anchor_scroll_to_hovered(picker: &mut BranchPicker) {
    if picker.visible_height == 0 {
        return;
    }
    if picker.hovered < picker.scroll {
        picker.scroll = picker.hovered;
    } else if picker.hovered >= picker.scroll + picker.visible_height {
        picker.scroll = picker.hovered + 1 - picker.visible_height;
    }
}

/// Pick a stable colour for a branch name out of the user's graph
/// palette so the picker rows feel cohesive with the commit-graph
/// view. Falls back to the theme's `list_ref_branch_fg` when the
/// palette is empty.
fn branch_color_from_graph_palette(
    theme: &crate::color::ColorTheme,
    graph_color_set: &crate::color::GraphColorSet,
    name: &str,
) -> Color {
    if graph_color_set.colors.is_empty() {
        return theme.list_ref_branch_fg;
    }
    let mut hash: u32 = 5381;
    for b in name.as_bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(u32::from(*b));
    }
    let idx = (hash as usize) % graph_color_set.colors.len();
    graph_color_set.colors[idx].to_ratatui_color()
}

/// Enumerate every local + remote branch in the repo. Used to
/// populate the compose-PR branch picker — local branches come
/// first, then remotes, each tagged with a `BranchKind` so the UI
/// can colour them accordingly.
fn load_local_and_remote_branches(repo_path: &std::path::Path) -> Vec<BranchPickerEntry> {
    let mut out: Vec<BranchPickerEntry> = Vec::new();
    let _ = std::process::Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads",
        ])
        .current_dir(repo_path)
        .output()
        .map(|o| {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                let name = line.trim();
                if !name.is_empty() {
                    out.push(BranchPickerEntry {
                        name: name.to_string(),
                        kind: BranchKind::Local,
                    });
                }
            }
        });
    let _ = std::process::Command::new("git")
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/remotes",
        ])
        .current_dir(repo_path)
        .output()
        .map(|o| {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                let name = line.trim();
                if name.is_empty() || name.ends_with("/HEAD") {
                    continue;
                }
                out.push(BranchPickerEntry {
                    name: name.to_string(),
                    kind: BranchKind::Remote,
                });
            }
        });
    out
}

/// Resolve the local repo's current HEAD branch name. Returns
/// `None` when HEAD is detached (or the call to `git symbolic-ref`
/// otherwise fails) — the compose form just falls back to an empty
/// `head` field in that case.
fn current_head_branch(repo_path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Pull the subject (`%s`) of the most recent commit on `branch`.
/// Used to pre-fill the compose form's title field — the same
/// convention the GitHub web UI follows.
fn latest_commit_subject(repo_path: &std::path::Path, branch: &str) -> Option<String> {
    if branch.is_empty() {
        return None;
    }
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--pretty=%s", branch])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Commits unique to `head` vs `base` (the set the PR would
/// introduce). Returns an empty list when the call fails, since the
/// preview pane just shows "(no commits)" in that case.
fn compose_preview_commits(
    repo_path: &std::path::Path,
    base: &str,
    head: &str,
) -> Vec<crate::github::pr::PullCommit> {
    let range = format!("{}..{}", base, head);
    let output = match std::process::Command::new("git")
        .args(["log", "--reverse", "--pretty=%H%x1f%h%x1f%an%x1f%aI%x1f%s", &range])
        .current_dir(repo_path)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\x1f').collect();
            if parts.len() < 5 {
                return None;
            }
            Some(crate::github::pr::PullCommit {
                sha: parts[0].to_string(),
                short_sha: parts[1].to_string(),
                author: parts[2].to_string(),
                // Local git log fallback — no GitHub login resolution
                // here; avatar overlay will simply skip these rows.
                author_login: String::new(),
                date: short_relative_iso(parts[3]),
                subject: parts[4].to_string(),
            })
        })
        .collect()
}

/// Files changed between `base` and `head` (the set the PR would
/// touch). Parsed from `git diff --numstat` so we can fill the
/// preview pane's file list with the same shape as the GitHub
/// `/files` endpoint.
fn compose_preview_files(
    repo_path: &std::path::Path,
    base: &str,
    head: &str,
) -> Vec<crate::github::pr::PullFile> {
    let range = format!("{}..{}", base, head);
    let output = match std::process::Command::new("git")
        .args(["diff", "--numstat", &range])
        .current_dir(repo_path)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let status_map = compose_preview_file_statuses(repo_path, &range);
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(3, '\t').collect();
            if parts.len() < 3 {
                return None;
            }
            let additions: u64 = parts[0].parse().unwrap_or(0);
            let deletions: u64 = parts[1].parse().unwrap_or(0);
            let filename = parts[2].to_string();
            let status = status_map
                .get(&filename)
                .copied()
                .unwrap_or(crate::github::pr::FileStatus::Other);
            Some(crate::github::pr::PullFile {
                filename,
                status,
                additions,
                deletions,
                patch: None,
            })
        })
        .collect()
}

fn compose_preview_file_statuses(
    repo_path: &std::path::Path,
    range: &str,
) -> rustc_hash::FxHashMap<String, crate::github::pr::FileStatus> {
    let mut map = rustc_hash::FxHashMap::default();
    let Ok(output) = std::process::Command::new("git")
        .args(["diff", "--name-status", range])
        .current_dir(repo_path)
        .output()
    else {
        return map;
    };
    if !output.status.success() {
        return map;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.splitn(2, '\t');
        let Some(tag) = parts.next() else { continue };
        let Some(name) = parts.next() else { continue };
        let status = match tag.chars().next() {
            Some('A') => crate::github::pr::FileStatus::Added,
            Some('M') => crate::github::pr::FileStatus::Modified,
            Some('D') => crate::github::pr::FileStatus::Removed,
            Some('R') => crate::github::pr::FileStatus::Renamed,
            _ => crate::github::pr::FileStatus::Other,
        };
        // For renames, the format is `R100\told\tnew` — strip to the
        // final path.
        let name = name.split('\t').last().unwrap_or(name);
        map.insert(name.to_string(), status);
    }
    map
}

fn short_relative_iso(iso: &str) -> String {
    use chrono::DateTime;
    let Ok(dt) = DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let now = chrono::Utc::now();
    let secs = (now.timestamp() - dt.timestamp()).max(0);
    if secs < 60 {
        return format!("{}s ago", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m ago", mins);
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h ago", hours);
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{}d ago", days);
    }
    let months = days / 30;
    if months < 12 {
        return format!("{}mo ago", months);
    }
    format!("{}y ago", months / 12)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", cut)
    }
}
