use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};

/// Global de-dup for the lazy-load auto-trigger. Stores the cursor
/// position at which we last fired `LoadMore`. Lives at module scope
/// so it survives the `CommitListState` rebuild that follows each
/// reload, without that, every keystroke at the bottom would re-fire
/// the loader and the user would see a hard blink per nav event.
///
/// `usize::MAX` is the sentinel for "never fired" so the very first
/// near-bottom move always passes the gate.
static LAST_LAZY_TRIGGER_POS: AtomicUsize = AtomicUsize::new(usize::MAX);

use fuzzy_matcher::{skim::SkimMatcherV2, FuzzyMatcher};
use laurier::highlight::highlight_matched_text;
use once_cell::sync::Lazy;
use ratatui::{
    buffer::Buffer,
    crossterm::event::{Event, KeyEvent},
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph, StatefulWidget, Widget},
};
use regex::RegexBuilder;
use rustc_hash::{FxHashMap, FxHashSet};
use tui_input::{backend::crossterm::EventHandler, Input};

use crate::{
    app::AppContext,
    color::ColorTheme,
    config::UserListColumnType,
    git::{Commit, CommitHash, Head, Ref},
    graph::GraphImageManager,
    protocol::{kitty_encode_cropped, ImageProtocol, PreparedImage},
};

static FUZZY_MATCHER: Lazy<SkimMatcherV2> = Lazy::new(|| SkimMatcherV2::default().respect_case());

const ELLIPSIS: &str = "...";

#[derive(Debug)]
pub struct CommitInfo {
    pub commit: std::sync::Arc<Commit>,
    refs: Vec<Ref>,
    pub graph_color: Color,
    pub is_uncommitted: bool,
    pub uncommitted_staged: usize,
    pub uncommitted_unstaged: usize,
    pub uncommitted_untracked: usize,
    /// Files with status Unmerged (active merge conflict). Drives the
    /// "⚠ N conflict(s)" badge on the Uncommitted Changes row.
    pub uncommitted_unmerged: usize,
    pub uncommitted_last_modified: Option<chrono::DateTime<chrono::FixedOffset>>,
}

impl CommitInfo {
    pub fn new(commit: std::sync::Arc<Commit>, refs: Vec<Ref>, graph_color: Color) -> Self {
        Self {
            commit,
            refs,
            graph_color,
            is_uncommitted: false,
            uncommitted_staged: 0,
            uncommitted_unstaged: 0,
            uncommitted_untracked: 0,
            uncommitted_unmerged: 0,
            uncommitted_last_modified: None,
        }
    }

    pub fn new_uncommitted(
        commit: std::sync::Arc<Commit>,
        graph_color: Color,
        staged: usize,
        unstaged: usize,
        untracked: usize,
        unmerged: usize,
        last_modified: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> Self {
        Self {
            commit,
            refs: vec![],
            graph_color,
            is_uncommitted: true,
            uncommitted_staged: staged,
            uncommitted_unstaged: unstaged,
            uncommitted_untracked: untracked,
            uncommitted_unmerged: unmerged,
            uncommitted_last_modified: last_modified,
        }
    }

    pub fn commit(&self) -> &Commit {
        &self.commit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchState {
    Inactive,
    Searching {
        start_index: usize,
        match_index: usize,
        ignore_case: bool,
        fuzzy: bool,
        regex: bool,
        transient_message: TransientMessage,
    },
    Applied {
        match_index: usize,
        total_match: usize,
        ignore_case: bool,
        fuzzy: bool,
        regex: bool,
    },
}

impl SearchState {
    pub fn is_active(&self) -> bool {
        !matches!(self, SearchState::Inactive)
    }

    pub fn is_applied(&self) -> bool {
        matches!(self, SearchState::Applied { .. })
    }

    pub fn is_querying(&self) -> bool {
        matches!(self, SearchState::Searching { .. })
    }
}

impl SearchState {
    fn update_match_index(&mut self, index: usize) {
        match self {
            SearchState::Searching { match_index, .. } => *match_index = index,
            SearchState::Applied { match_index, .. } => *match_index = index,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransientMessage {
    None,
    IgnoreCaseOff,
    IgnoreCaseOn,
    FuzzyOff,
    FuzzyOn,
    RegexOff,
    RegexOn,
}

#[derive(Debug, Default, Clone)]
struct SearchMatch {
    refs: FxHashMap<String, SearchMatchPosition>,
    commit_message: Option<SearchMatchPosition>,
    author_name: Option<SearchMatchPosition>,
    commit_hash: Option<SearchMatchPosition>,
    match_index: usize, // 1-based
}

impl SearchMatch {
    fn set(&mut self, c: &Commit, refs: &[Ref], matcher: &SearchMatcher) {
        self.refs = refs
            .iter()
            .filter(|r| !matches!(r, Ref::Stash { .. }))
            .filter_map(|r| {
                matcher
                    .matched_position(r.name())
                    .map(|pos| (r.name().into(), pos))
            })
            .collect();
        self.commit_message = matcher.matched_position(&c.commit_message);
        self.author_name = matcher.matched_position(&c.author_name);
        self.commit_hash = matcher.matched_position(c.commit_hash.as_short_hash());
        self.match_index = 0;
    }

    fn matched(&self) -> bool {
        !self.refs.is_empty()
            || self.commit_message.is_some()
            || self.author_name.is_some()
            || self.commit_hash.is_some()
    }

    fn clear(&mut self) {
        self.refs.clear();
        self.commit_message = None;
        self.author_name = None;
        self.commit_hash = None;
    }
}

#[derive(Debug, Default, Clone)]
struct SearchMatchPosition {
    matched_indices: Vec<usize>,
}

impl SearchMatchPosition {
    fn new(matched_indices: Vec<usize>) -> Self {
        Self { matched_indices }
    }
}

struct SearchMatcher {
    /// The search query; lowercased when `ignore_case` is on AND we're not in
    /// regex mode (regex case-insensitivity is handled by RegexBuilder instead).
    query: String,
    ignore_case: bool,
    fuzzy: bool,
    regex: bool,
    /// Pre-compiled regex when `regex` is on. `None` if regex is off OR the
    /// pattern failed to compile, in the latter case the matcher returns no
    /// matches at all (silent fallback; the empty result list signals the user
    /// that their pattern is invalid).
    regex_compiled: Option<regex::Regex>,
}

impl SearchMatcher {
    fn new(query: &str, ignore_case: bool, fuzzy: bool, regex: bool) -> Self {
        // Regex takes precedence over fuzzy when both happen to be enabled
        // fuzzy is a substring-style matcher, regex is a strict pattern match.
        let regex_compiled = if regex && !query.is_empty() {
            RegexBuilder::new(query)
                .case_insensitive(ignore_case)
                .build()
                .ok()
        } else {
            None
        };
        // Lowercasing the stored query only matters for the literal / fuzzy
        // paths; regex uses RegexBuilder's case_insensitive flag instead.
        let stored_query = if ignore_case && !regex {
            query.to_lowercase()
        } else {
            query.into()
        };
        Self {
            query: stored_query,
            ignore_case,
            fuzzy,
            regex,
            regex_compiled,
        }
    }

    fn matched_position(&self, s: &str) -> Option<SearchMatchPosition> {
        if self.regex {
            // regex_compiled is None if compilation failed → return no match.
            let re = self.regex_compiled.as_ref()?;
            let m = re.find(s)?;
            // Highlight indices cover every byte of the match so the renderer
            // can tint the whole span (works for ASCII; multi-byte chars get
            // their bytes individually marked, which is fine since the
            // highlighter operates per-byte too).
            let indices: Vec<usize> = (m.start()..m.end()).collect();
            return Some(SearchMatchPosition::new(indices));
        }
        if self.fuzzy {
            let result = if self.ignore_case {
                FUZZY_MATCHER.fuzzy_indices(&s.to_lowercase(), &self.query)
            } else {
                FUZZY_MATCHER.fuzzy_indices(s, &self.query)
            };
            result
                .map(|(_, indices)| indices)
                .map(SearchMatchPosition::new)
        } else {
            let result = if self.ignore_case {
                s.to_lowercase().find(&self.query)
            } else {
                s.find(&self.query)
            };
            result
                .map(|p| (p..(p + self.query.len())).collect())
                .map(SearchMatchPosition::new)
        }
    }
}

#[derive(Debug, Clone)]
pub struct RefHitArea {
    pub row: u16,
    pub col_start: u16,
    pub col_end: u16,
    pub names: Vec<String>,
    pub is_tag: bool,
}

#[derive(Debug)]
pub struct CommitListState<'a> {
    commits: Vec<CommitInfo>,
    /// Maps each commit hash to its index in `commits`. Replaces the
    /// previous `FxHashSet<CommitHash>` — `.contains_key(h)` covers the
    /// old membership-check use, and the index value gives `select_*`
    /// O(1) lookups instead of the O(N) linear scan they used to do
    /// (catastrophic on 332k-commit repos).
    commit_hash_to_index: FxHashMap<CommitHash, usize>,
    graph_image_manager: GraphImageManager<'a>,
    graph_cell_width: u16,
    head: &'a Head,
    head_commit_hash: Option<CommitHash>,

    ref_name_to_commit_index_map: FxHashMap<String, usize>,
    branch_color_map: FxHashMap<String, Color>,

    search_state: SearchState,
    search_input: Input,
    search_matches: Vec<SearchMatch>,

    /// First endpoint of a 2-commit comparison. `Some(hash)` puts the list
    /// view in compare-pending mode: the marked row is highlighted, and the
    /// next Space / Ctrl+click on a different commit opens the
    /// cumulative diff between the two. Cleared on Esc, on toggle, or after
    /// a successful comparison view is opened.
    marked_compare_commit: Option<CommitHash>,

    selected: usize,
    offset: usize,
    total: usize,
    height: usize,
    /// Cached max widths so the per-frame `content_column_widths`
    /// call doesn't have to iterate the entire commit list
    /// (catastrophic on 332k-commit repos: ~664k string-measure
    /// calls per frame × 20 frames per second froze the UI for
    /// the whole bg-streaming window). Recomputed only when
    /// commits are appended (`extend_commits`) - which is
    /// itself a one-shot per-batch O(batch_size) pass, not a
    /// per-frame cost.
    cached_author_name_width: u16,
    // Lazy-load dedup lives in a module-level static (see
    // `LAST_LAZY_TRIGGER_POS` below) so it survives the state rebuild
    // that happens on every refresh. Storing it here would be reset
    // by every `LoadMore` reload, causing the loader to re-fire on
    // every keypress while the user is near the bottom.
    default_ignore_case: bool,
    default_fuzzy: bool,
    default_regex: bool,

    pub ref_hit_areas: Vec<RefHitArea>,
    pub hovered_branch: Option<String>,
    pub hovered_tag: Option<String>,
    pub hovered_row: Option<usize>,

    // Tracks the (offset, height, area, graph_scroll_x) of the last graph
    // render so we can skip re-rendering graph image cells when nothing
    // visible has changed.
    graph_render_state: Option<(usize, usize, Rect, u16)>,
    // Horizontal scroll offset (in graph cells) for the lane viewport.
    // Only matters when the native graph is wider than the column cap
    // (40 % of panel) — the visible portion of the lanes slides via
    // Kitty's source-rect crop. Clamped to `0..=graph.max_pos_x` and
    // reset to 0 on filter change / data reload.
    graph_scroll_x: u16,
    // Width of the graph column the last frame allocated (in cells, minus
    // the 1-cell pad). Used to clamp `graph_scroll_x` from the input
    // handler so the user can't scroll past the last visible lane.
    last_graph_area_cells: u16,
    // Stable hash of (offset, height, area, visible_emails+prepared flags), does NOT
    // include `selected`, so hover-driven selection changes don't trigger a full re-render.
    avatar_stable_key: Option<u64>,
    // Which visual row was selected in the last avatar render.  Tracked separately from
    // the stable key so we can do a targeted 2-row re-render on selection change.
    avatar_prev_selected: Option<usize>,
    // Set to true when all visible commits have their avatars prepared; cleared on
    // scroll/resize so `ensure_visible_avatars_uploaded` can skip its iteration.
    avatars_fully_prepared: bool,
}

impl<'a> CommitListState<'a> {
    pub fn new(
        commits: Vec<CommitInfo>,
        graph_image_manager: GraphImageManager<'a>,
        graph_cell_width: u16,
        head: &'a Head,
        ref_name_to_commit_index_map: FxHashMap<String, usize>,
        branch_color_map: FxHashMap<String, Color>,
        default_ignore_case: bool,
        default_fuzzy: bool,
        default_regex: bool,
    ) -> CommitListState<'a> {
        let total = commits.len();
        let _has_uncommitted = commits.first().is_some_and(|c| c.is_uncommitted);
        let commit_hash_to_index: FxHashMap<CommitHash, usize> = commits
            .iter()
            .enumerate()
            .map(|(i, c)| (c.commit.commit_hash.clone(), i))
            .collect();
        // Seed the column-width cache from the initial commit set
        // (the cheap path: 500 commits at launch, walks once). The
        // bg streaming loader keeps it up to date via
        // `extend_commits`.
        let cached_author_name_width: u16 = commits
            .iter()
            .filter(|c| !c.is_uncommitted)
            .map(|c| console::measure_text_width(&c.commit.author_name) as u16)
            .max()
            .unwrap_or(0);
        let head_commit_hash = match head {
            Head::Detached { target } => Some(target.clone()),
            Head::Branch { name } => ref_name_to_commit_index_map
                .get(name.as_str())
                .and_then(|&index| commits.get(index))
                .map(|info| info.commit.commit_hash.clone()),
            Head::None => None,
        };
        CommitListState {
            commits,
            commit_hash_to_index,
            graph_image_manager,
            graph_cell_width,
            head,
            head_commit_hash,
            ref_name_to_commit_index_map,
            branch_color_map,
            search_state: SearchState::Inactive,
            search_input: Input::default(),
            search_matches: vec![SearchMatch::default(); total],
            marked_compare_commit: None,
            selected: 0,
            offset: 0,
            total,
            height: 0,
            cached_author_name_width,
            default_ignore_case,
            default_fuzzy,
            default_regex,
            ref_hit_areas: Vec::new(),
            hovered_branch: None,
            hovered_tag: None,
            hovered_row: None,
            graph_render_state: None,
            graph_scroll_x: 0,
            last_graph_area_cells: 0,
            avatar_stable_key: None,
            avatar_prev_selected: None,
            avatars_fully_prepared: false,
        }
    }

    pub fn graph_area_cell_width(&self) -> u16 {
        self.graph_cell_width + 1 // right pad
    }

    /// Native lane count of the graph (in cells). Used to clamp
    /// horizontal scroll so the user can't slide past the last lane.
    pub fn graph_native_cell_width(&self) -> u16 {
        self.graph_cell_width
    }

    pub fn graph_scroll_x(&self) -> u16 {
        self.graph_scroll_x
    }

    /// Slide the horizontal viewport over the graph lanes. Caller must
    /// clamp to a valid range before calling (see
    /// `widget::commit_list::CommitList::scroll_graph_horizontal`).
    /// Setting always invalidates the render cache so the next frame
    /// re-emits the placement commands with the new crop offset.
    pub fn set_graph_scroll_x(&mut self, x: u16) {
        if self.graph_scroll_x == x {
            return;
        }
        self.graph_scroll_x = x;
        self.graph_render_state = None;
    }

    /// Maximum valid `graph_scroll_x` so the rightmost lane stays at least
    /// partially visible. Returns 0 when the native graph already fits the
    /// allocated column (no overflow → scroll is a no-op).
    pub fn max_graph_scroll_x(&self) -> u16 {
        self.graph_cell_width
            .saturating_sub(self.last_graph_area_cells)
    }

    /// Slide the horizontal viewport left by `cells`, clamped to 0.
    pub fn scroll_graph_left(&mut self, cells: u16) {
        let new = self.graph_scroll_x.saturating_sub(cells);
        self.set_graph_scroll_x(new);
    }

    /// Slide the horizontal viewport right by `cells`, clamped to
    /// `max_graph_scroll_x()` so the rightmost lane stays visible.
    pub fn scroll_graph_right(&mut self, cells: u16) {
        let max = self.max_graph_scroll_x();
        let new = self.graph_scroll_x.saturating_add(cells).min(max);
        self.set_graph_scroll_x(new);
    }

    pub fn update_height(&mut self, height: usize) {
        if height != self.height {
            self.avatars_fully_prepared = false; // visible set may change
        }
        self.height = height;

        if self.total > self.height && self.total - self.height < self.offset {
            let diff = self.offset - (self.total - self.height);
            self.selected += diff;
            self.offset -= diff;
        }
        if self.selected >= self.height {
            let diff = self.selected - self.height + 1;
            self.selected -= diff;
            self.offset += diff;
        }
    }

    pub fn ensure_visible_graph_uploaded(&mut self) {
        let current_head_hash = self.head_commit_hash.clone();
        let manager_head = self.graph_image_manager.head_commit_hash().cloned();
        if manager_head.as_ref() != current_head_hash.as_ref() {
            if let Some(old) = manager_head {
                self.graph_image_manager.invalidate(&old);
            }
            if let Some(new) = &current_head_hash {
                self.graph_image_manager.invalidate(new);
            }
            self.graph_image_manager
                .set_head_commit_hash(current_head_hash.as_ref());
            self.graph_render_state = None; // HEAD appearance changed
        }

        self.commits
            .iter()
            .skip(self.offset)
            .take(self.height)
            .for_each(|commit_info| {
                self.graph_image_manager
                    .ensure_uploaded(&commit_info.commit.commit_hash);
            });
    }

    pub fn ensure_visible_avatars_uploaded(
        &mut self,
        avatar_manager: &mut crate::avatar::AvatarManager,
        bg: Color,
        selected_bg: Color,
    ) {
        if !avatar_manager.is_enabled() {
            return;
        }
        // Fast path: if we already confirmed all visible avatars are ready, skip
        if self.avatars_fully_prepared {
            return;
        }
        let selected_idx = self.selected;
        let mut all_prepared = true;
        for (i, commit_info) in self
            .commits
            .iter()
            .skip(self.offset)
            .take(self.height)
            .enumerate()
            .filter(|(_, ci)| !ci.is_uncommitted)
        {
            let email = &commit_info.commit.author_email;
            // Transparent version for non-selected rows
            if avatar_manager.prepared_image(email, 1, false).is_none() {
                all_prepared = false;
                if avatar_manager.cached_avatar_exists(email) {
                    avatar_manager.ensure_uploaded(email, 1, false, bg);
                } else {
                    // Collect multiple commit hashes for this email so the GitHub API
                    // has several chances to find a commit where `author` is not null.
                    // (A single hash often returns `author: null` when the commit email
                    // is not linked to a GitHub account, even if other commits by the
                    // same person are properly linked.)
                    let hashes: Vec<String> = self
                        .commits
                        .iter()
                        .filter(|ci| !ci.is_uncommitted && ci.commit.author_email == *email)
                        .map(|ci| ci.commit.commit_hash.as_str().to_string())
                        .take(8)
                        .collect();
                    avatar_manager.prefetch(hashes, email);
                }
            }
            // Pre-baked selected variant for the selected row
            if i == selected_idx && avatar_manager.prepared_image(email, 1, true).is_none() {
                all_prepared = false;
                if avatar_manager.cached_avatar_exists(email) {
                    avatar_manager.ensure_uploaded(email, 1, true, selected_bg);
                }
            }
        }
        self.avatars_fully_prepared = all_prepared;
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        self.graph_image_manager.drain_pending_uploads()
    }

    pub fn clear_graph_images(&mut self) {
        self.graph_image_manager.clear_prepared_images();
        self.graph_render_state = None; // images cleared, must re-render
        self.avatar_stable_key = None;
        self.avatar_prev_selected = None;
        self.avatars_fully_prepared = false;
    }

    pub fn invalidate_image_caches(&mut self, bg: ratatui::style::Color) {
        if let ratatui::style::Color::Rgb(r, g, b) = bg {
            self.graph_image_manager.update_background_color(r, g, b);
        } else {
            // Non-RGB color: just clear, can't update bg
            self.graph_image_manager.clear_prepared_images();
        }
        self.graph_render_state = None;
        self.avatar_stable_key = None;
        self.avatars_fully_prepared = false;
    }

    /// Like `invalidate_image_caches` but also swaps the entire graph
    /// palette (branch colours + circle edge + bg), used by the live
    /// theme-cycle path so the next render rebakes images with the new
    /// theme's branch colours, not just its background.
    pub fn invalidate_image_caches_with_palette(
        &mut self,
        graph_color_set: &crate::color::GraphColorSet,
    ) {
        self.graph_image_manager.update_palette(graph_color_set);
        self.graph_render_state = None;
        self.avatar_stable_key = None;
        self.avatars_fully_prepared = false;
    }

    /// Live-update the graph corner style (Rounded / Angular / Smooth).
    /// Clears the image cache so the next render rebakes with the new
    /// style. Lets the Config view apply `graph_style` changes without
    /// a full app refresh.
    pub fn update_graph_style(&mut self, style: crate::graph::GraphStyle) {
        self.graph_image_manager.update_graph_style(style);
        self.graph_render_state = None;
        self.avatar_stable_key = None;
        self.avatars_fully_prepared = false;
    }

    /// Live-update the graph cell width (Single / Double). Recomputes
    /// the image params + drawing pixels + clears the cache so the
    /// next render rebakes with the new cell sizing. Used by the
    /// Config view's smooth-exit path for `graph_width` changes.
    pub fn update_cell_width_type(
        &mut self,
        cell_width_type: crate::graph::CellWidthType,
        graph_color_set: &crate::color::GraphColorSet,
    ) {
        self.graph_image_manager
            .update_cell_width_type(cell_width_type, graph_color_set);
        self.graph_render_state = None;
        self.avatar_stable_key = None;
        self.avatars_fully_prepared = false;
    }

    /// Read-only access to the underlying graph topology, used by the
    /// live `graph_width` exit path so the caller can resolve the
    /// `Option<GraphWidthType>` (Auto / Single / Double) into a
    /// concrete `CellWidthType` via `check::decide_cell_width_type`.
    /// Append more commits to the in-memory list. Used by the
    /// background streaming loader: bg walks `git log` (or reads
    /// the disk cache) on its own OS thread and ships batched
    /// `CommitInfo` entries to the main thread via
    /// `AppEvent::AppendCommits`. The handler in `app.rs` calls
    /// this to extend the list WITHOUT rebuilding the App - the
    /// cursor / scroll position / search state survive
    /// untouched.
    pub fn extend_commits(&mut self, new_commits: Vec<CommitInfo>) {
        if new_commits.is_empty() {
            return;
        }
        let grow = new_commits.len();
        let base_index = self.commits.len();
        // Roll the cached author column width over the new batch
        // (one O(batch_size) pass here, vs. the previous O(N) per
        // RENDER frame on the full list). Date width is a function
        // of the format string only, not the individual values, so
        // it stays put.
        //
        // Also extend `ref_name_to_commit_index_map` with refs
        // present on the streamed commits: without this, the Refs
        // tab (Tab key) sees the new refs in `repository.all_refs()`
        // but `select_ref` returns None when the user hovers or
        // clicks one of them (the map only had refs found on the
        // fg-loaded initial 500). Symptom: refs view hover and
        // click silently no-op on bg-streamed branches.
        for (offset, info) in new_commits.iter().enumerate() {
            let i = base_index + offset;
            self.commit_hash_to_index
                .insert(info.commit.commit_hash.clone(), i);
            if !info.is_uncommitted {
                let w = console::measure_text_width(&info.commit.author_name) as u16;
                if w > self.cached_author_name_width {
                    self.cached_author_name_width = w;
                }
            }
            for r in &info.refs {
                self.ref_name_to_commit_index_map
                    .insert(r.name().to_string(), i);
            }
        }
        self.commits.extend(new_commits);
        // Keep the parallel `search_matches` vec the same length as
        // `commits` so per-row lookups don't panic.
        self.search_matches
            .extend(std::iter::repeat_with(SearchMatch::default).take(grow));
        // CRITICAL: `total` is the scroll/navigation cap (used by
        // select_next, select_last, render bounds, etc.). Without
        // bumping it the appended commits sit in the Vec but are
        // unreachable - the cursor would refuse to leave the
        // initial 500-commit window.
        self.total += grow;
        // Force a recompute on the next render so visible newly-loaded
        // rows pick up their avatar / graph cells.
        self.avatars_fully_prepared = false;
    }

    /// Cached longest author name across the entire commit list.
    /// Computed at `new()` and rolled forward on `extend_commits`,
    /// so per-frame access is O(1) instead of the old O(N).
    pub fn cached_author_name_width(&self) -> u16 {
        self.cached_author_name_width
    }

    /// Mark the visible-avatars cache as needing a refresh on the
    /// next render. Used by the Refs view close path: hover does
    /// NOT clear this flag (so hovering through refs stays snappy),
    /// but once the user actually settles on a ref and closes the
    /// view back to the commit list, we want the freshly-shown
    /// window's avatars to materialise.
    pub fn invalidate_visible_avatars(&mut self) {
        self.avatars_fully_prepared = false;
    }

    pub fn graph(&self) -> &crate::graph::Graph<'a> {
        self.graph_image_manager.graph()
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        let mut image_ids: Vec<u32> = self
            .graph_image_manager
            .image_ids()
            .iter()
            .copied()
            .collect();
        image_ids.sort_unstable();
        image_ids
    }

    pub fn select_next(&mut self) {
        // Saturating subs everywhere: a numeric-prefix batch scroll
        // (e.g. user types `99999` then `j`) runs this in a tight
        // loop before any render gets a chance to refresh `height`.
        // If `total == 0` or `height == 0` the old `self.total - 1`
        // / `self.height - 1` underflow panicked in debug; if the
        // count was wildly larger than the commit list, offset
        // could grow without an upper bound and `current_selected_index`
        // would index past `commits.len()`.
        let total = self.total;
        let height = self.height;
        if total == 0 || height == 0 {
            return;
        }
        let max_selected = (total - 1).min(height - 1);
        let max_offset = total.saturating_sub(height);
        if self.selected < max_selected {
            self.selected += 1;
            self.avatars_fully_prepared = false;
        } else if self.offset < max_offset {
            self.offset += 1;
            self.avatars_fully_prepared = false;
        }
    }

    pub fn select_parent(&mut self) {
        if let Some(target_commit) = self.selected_commit_parent_hash().cloned() {
            // O(1) jump via the hash->index map instead of the
            // previous loop-and-select_next dance that was O(N)
            // on huge repos.
            self.select_commit_hash(&target_commit);
        }
    }

    pub fn selected_commit_parent_hash(&self) -> Option<&CommitHash> {
        self.commits[self.current_selected_index()]
            .commit
            .parent_commit_hashes
            .first()
    }

    /// Returns true when the cursor is within `THRESHOLD` rows of the
    /// last loaded commit AND we have moved at least `MIN_PROGRESS`
    /// rows past the spot where we last fired a load. The progress
    /// gate is the dedup: without it, every keystroke while the user
    /// is parked at the bottom would re-fire `LoadMore`, and each
    /// reload takes a frame or two, perceived as a hard blink.
    ///
    /// The dedup lives in a module-level `AtomicUsize` so it survives
    /// the `CommitListState` rebuild that happens after every reload.
    /// Field storage would reset back to 0 on rebuild and defeat the
    /// guard.
    pub fn should_trigger_lazy_load(&self) -> bool {
        const THRESHOLD: usize = 300;
        const MIN_PROGRESS: usize = 200;
        if self.total == 0 {
            return false;
        }
        let pos = self.current_selected_index();
        if pos + THRESHOLD < self.total {
            return false;
        }
        let last = LAST_LAZY_TRIGGER_POS.load(Ordering::Relaxed);
        // First trigger of the session OR meaningful forward progress
        // past the last trigger position, both unlock another load.
        last == usize::MAX || pos >= last.saturating_add(MIN_PROGRESS)
    }

    pub fn mark_lazy_attempt(&mut self) {
        LAST_LAZY_TRIGGER_POS.store(self.current_selected_index(), Ordering::Relaxed);
    }

    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.avatars_fully_prepared = false;
        } else if self.offset > 0 {
            self.offset -= 1;
            self.avatars_fully_prepared = false;
        }
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
        self.offset = 0;
        self.avatars_fully_prepared = false;
    }

    pub fn select_last(&mut self) {
        if self.total == 0 || self.height == 0 {
            return;
        }
        self.selected = (self.height - 1).min(self.total - 1);
        if self.height < self.total {
            self.offset = self.total - self.height;
            self.avatars_fully_prepared = false;
        }
    }

    /// Scroll to and select the commit that HEAD points at, vertically
    /// centered in the viewport. No-op when the list has no commits,
    /// when HEAD is detached without a matching row (e.g. orphan), or
    /// when the HEAD commit isn't in the currently loaded slice
    /// (lazy-load: user can `]` more then retry).
    pub fn select_head_commit(&mut self) {
        let Some(target) = self.head_commit_hash.as_ref() else {
            return;
        };
        let Some(index) = self
            .commits
            .iter()
            .position(|c| &c.commit.commit_hash == target)
        else {
            return;
        };
        if self.height == 0 {
            return;
        }
        if self.total <= self.height {
            // Everything fits on screen, no scroll, just move the cursor.
            self.selected = index;
            return;
        }
        // Aim for the middle row; clamp so we don't scroll past either end.
        let max_offset = self.total - self.height;
        let half = self.height / 2;
        let ideal_offset = index.saturating_sub(half);
        let offset = ideal_offset.min(max_offset);
        self.selected = index - offset;
        self.offset = offset;
        self.avatars_fully_prepared = false;
    }

    pub fn scroll_down(&mut self) {
        let max_offset = self.total.saturating_sub(self.height);
        if self.offset < max_offset {
            self.offset += 1;
            self.avatars_fully_prepared = false;
        }
    }

    pub fn scroll_up(&mut self) {
        if self.offset > 0 {
            self.offset -= 1;
            self.avatars_fully_prepared = false;
        }
    }

    pub fn restore_visual_selection(&mut self, visual_row: usize) {
        let current_index = self.current_selected_index();
        let max_offset = self.total.saturating_sub(self.height);
        self.offset = current_index.saturating_sub(visual_row).min(max_offset);
        self.selected = current_index.saturating_sub(self.offset);
        self.avatars_fully_prepared = false;
    }

    pub fn scroll_down_page(&mut self) {
        self.scroll_down_height(self.height);
    }

    pub fn scroll_up_page(&mut self) {
        self.scroll_up_height(self.height);
    }

    pub fn scroll_down_half(&mut self) {
        self.scroll_down_height(self.height / 2);
    }

    pub fn scroll_up_half(&mut self) {
        self.scroll_up_height(self.height / 2);
    }

    fn scroll_down_height(&mut self, scroll_height: usize) {
        if self.offset + self.height + scroll_height < self.total {
            self.offset += scroll_height;
        } else {
            let old_offset = self.offset;
            let size = self.height.min(self.total);
            self.offset = self.total - size;
            self.selected += scroll_height - (self.offset - old_offset);
            if self.selected >= size {
                self.selected = size - 1;
            }
        }
        self.avatars_fully_prepared = false;
    }

    fn scroll_up_height(&mut self, scroll_height: usize) {
        if self.offset > scroll_height {
            self.offset -= scroll_height;
        } else {
            let old_offset = self.offset;
            self.offset = 0;
            self.selected = self
                .selected
                .saturating_sub(scroll_height - (old_offset - self.offset));
        }
        self.avatars_fully_prepared = false;
    }

    pub fn select_high(&mut self) {
        self.selected = 0;
    }

    pub fn select_middle(&mut self) {
        if self.total > self.height {
            self.selected = self.height / 2;
        } else {
            self.selected = self.total / 2;
        }
    }

    pub fn select_low(&mut self) {
        if self.total > self.height {
            self.selected = self.height - 1;
        } else {
            self.selected = self.total - 1;
        }
    }

    fn select_index(&mut self, index: usize) {
        if index < self.total {
            if self.total > self.height {
                self.selected = 0;
                self.offset = index;
                self.avatars_fully_prepared = false;
            } else {
                self.selected = index;
            }
        }
    }

    pub fn select(&mut self, index: usize) {
        if index < self.total {
            self.selected = index.saturating_sub(self.offset);
            if self.selected >= self.height {
                self.selected = self.height - 1;
                self.offset = index - self.height + 1;
                self.avatars_fully_prepared = false;
            }
        }
    }

    pub fn select_next_match(&mut self) {
        self.select_next_match_index(self.current_selected_index());
    }

    pub fn select_prev_match(&mut self) {
        self.select_prev_match_index(self.current_selected_index());
    }

    /// Refresh `match_index` from the currently-selected row so the
    /// "Match X of Y" footer follows arrow-key / mouse navigation
    /// instead of staying frozen on the last cycle target. No-op when
    /// the selected row isn't a match (keeps the last visited index
    /// visible, vim-like "anchor" behaviour) or when search isn't
    /// applied at all.
    pub fn sync_match_index_to_selected(&mut self) {
        if !matches!(self.search_state, SearchState::Applied { .. }) {
            return;
        }
        let idx = self.current_selected_index();
        if idx >= self.search_matches.len() {
            return;
        }
        let m = &self.search_matches[idx];
        if m.matched() {
            let new_index = m.match_index;
            self.search_state.update_match_index(new_index);
        }
    }

    pub fn selected_commit_hash(&self) -> &CommitHash {
        let info = &self.commits[self.current_selected_index()];
        &info.commit.commit_hash
    }

    pub fn selected_commit_message(&self) -> Option<&str> {
        let info = &self.commits[self.current_selected_index()];
        Some(info.commit.commit_message.as_str())
    }

    pub fn is_uncommitted_selected(&self) -> bool {
        let info = &self.commits[self.current_selected_index()];
        info.is_uncommitted
    }

    /// Hash of the commit currently marked as the first endpoint of a 2-commit
    /// comparison, or `None` if no mark is active.
    pub fn marked_compare_commit(&self) -> Option<&CommitHash> {
        self.marked_compare_commit.as_ref()
    }

    /// Toggle the compare mark on the currently selected commit. Does nothing
    /// (and returns `None`) when the cursor is on the Uncommitted Changes row,
    /// since a synthetic uncommitted hash has no real ancestry to diff
    /// against. Returns the new mark state for caller-side notifications.
    pub fn toggle_compare_mark(&mut self) -> Option<CommitHash> {
        if self.is_uncommitted_selected() {
            return None;
        }
        let current = self.selected_commit_hash().clone();
        match &self.marked_compare_commit {
            Some(h) if *h == current => {
                // Toggle off: space on the already-marked row clears it.
                self.marked_compare_commit = None;
                None
            }
            _ => {
                self.marked_compare_commit = Some(current.clone());
                Some(current)
            }
        }
    }

    pub fn clear_compare_mark(&mut self) {
        self.marked_compare_commit = None;
    }

    fn current_selected_index(&self) -> usize {
        // Clamp to the last valid commit index. The raw
        // `offset + selected` could exceed `commits.len()` if
        // `extend_commits` was called between two render frames
        // and a stale offset survives, or if `select_*` was
        // invoked before the first render set `height`. Returning
        // a guaranteed-in-bounds index lets every call site index
        // `self.commits[...]` without panicking.
        let raw = self.offset + self.selected;
        raw.min(self.commits.len().saturating_sub(1))
    }

    pub fn current_list_status(&self) -> (usize, usize, usize) {
        (self.selected, self.offset, self.height)
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn reset_height(&mut self, height: usize) {
        self.height = height;
    }

    pub fn select_ref(&mut self, ref_name: &str) {
        if let Some(&index) = self.ref_name_to_commit_index_map.get(ref_name) {
            if self.total > self.height {
                self.selected = 0;
                self.offset = index;
                // Intentionally DON'T clear `avatars_fully_prepared` here.
                // The Refs view drives this on every hover frame; if
                // each hover transition triggered a sync sweep of the
                // newly-visible rows for avatar disk-read + image
                // decode + PNG encode, hovering through a list of
                // refs felt ultra-laggy on huge repos (~50 rows × ~30 ms
                // per new author = 1-2 s per hover). The avatars
                // behind the refs panel are partially obscured anyway;
                // the next stable render after the user closes the
                // Refs view will pick up the avatars naturally.
            } else {
                self.selected = index;
            }
        }
    }

    pub fn select_commit_hash(&mut self, commit_hash: &CommitHash) {
        // O(1) via the hash->index map; previously this scanned
        // the full `commits` vec linearly which on 332k-commit
        // repos meant a 5-10 ms hit per call (and could be
        // triggered repeatedly by ref hover, parent jump, etc.).
        let Some(&i) = self.commit_hash_to_index.get(commit_hash) else {
            return;
        };
        if self.total > self.height {
            self.selected = 0;
            self.offset = i;
            self.avatars_fully_prepared = false;
        } else {
            self.selected = i;
        }
    }

    pub fn search_state(&self) -> SearchState {
        self.search_state
    }

    /// Returns `(ignore_case, fuzzy, regex)` for the active search state, or
    /// `None` when search is inactive. Used by the footer hint to render
    /// `[ON]/[OFF]` for each modifier.
    pub fn search_case_fuzzy_regex(&self) -> Option<(bool, bool, bool)> {
        match self.search_state {
            SearchState::Searching {
                ignore_case,
                fuzzy,
                regex,
                ..
            }
            | SearchState::Applied {
                ignore_case,
                fuzzy,
                regex,
                ..
            } => Some((ignore_case, fuzzy, regex)),
            _ => None,
        }
    }

    pub fn branch_at_position(&self, col: u16, row: u16) -> Option<String> {
        self.ref_hit_areas.iter().find_map(|hit| {
            if !hit.is_tag && hit.row == row && col >= hit.col_start && col < hit.col_end {
                hit.names.first().cloned()
            } else {
                None
            }
        })
    }

    pub fn tag_at_position(&self, col: u16, row: u16) -> Option<String> {
        self.ref_hit_areas.iter().find_map(|hit| {
            if hit.is_tag && hit.row == row && col >= hit.col_start && col < hit.col_end {
                hit.names.first().cloned()
            } else {
                None
            }
        })
    }

    pub fn set_hovered_branch(&mut self, branch: Option<String>) {
        self.hovered_branch = branch;
    }

    pub fn set_hovered_tag(&mut self, tag: Option<String>) {
        self.hovered_tag = tag;
    }

    pub fn set_hovered_row(&mut self, row: Option<usize>) {
        self.hovered_row = row;
    }

    pub fn start_search(&mut self) {
        if let SearchState::Inactive | SearchState::Applied { .. } = self.search_state {
            self.search_state = SearchState::Searching {
                start_index: self.current_selected_index(),
                match_index: 0,
                ignore_case: self.default_ignore_case,
                fuzzy: self.default_fuzzy,
                regex: self.default_regex,
                transient_message: TransientMessage::None,
            };
            self.search_input.reset();
            self.clear_search_matches();
        }
    }

    pub fn handle_search_input(&mut self, key: KeyEvent) {
        if let SearchState::Searching {
            transient_message, ..
        } = &mut self.search_state
        {
            *transient_message = TransientMessage::None;
        }

        if let SearchState::Searching {
            start_index,
            ignore_case,
            fuzzy,
            regex,
            ..
        } = self.search_state
        {
            self.search_input.handle_event(&Event::Key(key));
            self.update_search_matches(ignore_case, fuzzy, regex);
            self.select_current_or_next_match_index(start_index);
        }
    }

    pub fn apply_search(&mut self) {
        if let SearchState::Searching {
            match_index,
            ignore_case,
            fuzzy,
            regex,
            ..
        } = self.search_state
        {
            if self.search_input.value().is_empty() {
                self.search_state = SearchState::Inactive;
            } else {
                let total_match = self.search_matches.iter().filter(|m| m.matched()).count();
                self.search_state = SearchState::Applied {
                    match_index,
                    total_match,
                    ignore_case,
                    fuzzy,
                    regex,
                };
            }
        }
    }

    pub fn cancel_search(&mut self) {
        if let SearchState::Searching { .. } | SearchState::Applied { .. } = self.search_state {
            self.search_state = SearchState::Inactive;
            self.search_input.reset();
            self.clear_search_matches();
        }
    }

    pub fn toggle_ignore_case(&mut self) -> Option<(bool, bool)> {
        let mut is_applied = false;
        let mut start_index = self.current_selected_index();
        if let SearchState::Searching {
            ignore_case,
            transient_message,
            ..
        } = &mut self.search_state
        {
            *ignore_case = !*ignore_case;
            // In UI: ON = case-sensitive (ignore_case=false), OFF = case-insensitive (ignore_case=true)
            *transient_message = if *ignore_case {
                TransientMessage::IgnoreCaseOff
            } else {
                TransientMessage::IgnoreCaseOn
            };
        }
        if let SearchState::Applied {
            match_index,
            ignore_case,
            ..
        } = &mut self.search_state
        {
            *ignore_case = !*ignore_case;
            is_applied = true;
            start_index = *match_index;
        }

        let (ignore_case, fuzzy, regex) = self.search_modifiers()?;
        self.update_search_matches(ignore_case, fuzzy, regex);
        self.select_current_or_next_match_index(start_index);
        if is_applied {
            let total_match = self.search_matches.iter().filter(|m| m.matched()).count();
            if let SearchState::Applied { match_index, .. } = &mut self.search_state {
                *match_index = start_index.min(total_match.saturating_sub(1));
            }
        }
        self.default_ignore_case = ignore_case;
        Some((ignore_case, fuzzy))
    }

    pub fn toggle_fuzzy(&mut self) -> Option<(bool, bool)> {
        let mut is_applied = false;
        let mut start_index = self.current_selected_index();
        if let SearchState::Searching {
            fuzzy,
            transient_message,
            ..
        } = &mut self.search_state
        {
            *fuzzy = !*fuzzy;
            *transient_message = if *fuzzy {
                TransientMessage::FuzzyOn
            } else {
                TransientMessage::FuzzyOff
            };
        }
        if let SearchState::Applied {
            match_index, fuzzy, ..
        } = &mut self.search_state
        {
            *fuzzy = !*fuzzy;
            is_applied = true;
            start_index = *match_index;
        }

        let (ignore_case, fuzzy, regex) = self.search_modifiers()?;
        self.update_search_matches(ignore_case, fuzzy, regex);
        self.select_current_or_next_match_index(start_index);
        if is_applied {
            let total_match = self.search_matches.iter().filter(|m| m.matched()).count();
            if let SearchState::Applied { match_index, .. } = &mut self.search_state {
                *match_index = start_index.min(total_match.saturating_sub(1));
            }
        }
        self.default_fuzzy = fuzzy;
        Some((ignore_case, fuzzy))
    }

    /// Toggle the regex matching mode. Mirrors `toggle_ignore_case` /
    /// `toggle_fuzzy`: only fires meaningfully in `Searching` / `Applied`
    /// states (the dispatch in `view::list` already gates on `Applied`), saves
    /// the new state to `default_regex` so the next `start_search` inherits
    /// it, and returns the resulting `(ignore_case, fuzzy, regex)` triple so
    /// the caller can persist it to the config.
    pub fn toggle_regex(&mut self) -> Option<(bool, bool, bool)> {
        let mut is_applied = false;
        let mut start_index = self.current_selected_index();
        if let SearchState::Searching {
            regex,
            transient_message,
            ..
        } = &mut self.search_state
        {
            *regex = !*regex;
            *transient_message = if *regex {
                TransientMessage::RegexOn
            } else {
                TransientMessage::RegexOff
            };
        }
        if let SearchState::Applied {
            match_index, regex, ..
        } = &mut self.search_state
        {
            *regex = !*regex;
            is_applied = true;
            start_index = *match_index;
        }

        let (ignore_case, fuzzy, regex) = self.search_modifiers()?;
        self.update_search_matches(ignore_case, fuzzy, regex);
        self.select_current_or_next_match_index(start_index);
        if is_applied {
            let total_match = self.search_matches.iter().filter(|m| m.matched()).count();
            if let SearchState::Applied { match_index, .. } = &mut self.search_state {
                *match_index = start_index.min(total_match.saturating_sub(1));
            }
        }
        self.default_regex = regex;
        Some((ignore_case, fuzzy, regex))
    }

    /// Helper used by every toggle to read the current modifier triple in one
    /// shot (returns `None` when search is inactive, callers should bail).
    fn search_modifiers(&self) -> Option<(bool, bool, bool)> {
        match self.search_state {
            SearchState::Searching {
                ignore_case,
                fuzzy,
                regex,
                ..
            }
            | SearchState::Applied {
                ignore_case,
                fuzzy,
                regex,
                ..
            } => Some((ignore_case, fuzzy, regex)),
            _ => None,
        }
    }

    pub fn search_query_string(&self) -> Option<String> {
        if let SearchState::Searching { .. } = self.search_state {
            let query = self.search_input.value();
            Some(format!("Search: {query}"))
        } else {
            None
        }
    }

    pub fn matched_query_string(&self) -> Option<(String, bool)> {
        if let SearchState::Applied {
            match_index,
            total_match,
            ..
        } = self.search_state
        {
            let query = self.search_input.value();
            if total_match == 0 {
                let msg = format!("No matches found (query: \"{query}\")");
                Some((msg, false))
            } else {
                let msg = format!("Match {match_index} of {total_match} (query: \"{query}\")");
                Some((msg, true))
            }
        } else {
            None
        }
    }

    pub fn search_query_cursor_position(&self) -> u16 {
        self.search_input.visual_cursor() as u16 + 8 // add 8 for "Search: "
    }

    pub fn transient_message_string(&self) -> Option<String> {
        if let SearchState::Searching {
            transient_message, ..
        } = self.search_state
        {
            match transient_message {
                TransientMessage::None => None,
                TransientMessage::IgnoreCaseOn => Some("Case: ON ".to_string()),
                TransientMessage::IgnoreCaseOff => Some("Case: OFF".to_string()),
                TransientMessage::FuzzyOn => Some("Fuzzy match: ON ".to_string()),
                TransientMessage::FuzzyOff => Some("Fuzzy match: OFF".to_string()),
                TransientMessage::RegexOn => Some("Regex match: ON ".to_string()),
                TransientMessage::RegexOff => Some("Regex match: OFF".to_string()),
            }
        } else {
            None
        }
    }

    fn update_search_matches(&mut self, ignore_case: bool, fuzzy: bool, regex: bool) {
        let matcher = SearchMatcher::new(self.search_input.value(), ignore_case, fuzzy, regex);
        let mut match_index = 1;
        for (i, commit_info) in self.commits.iter().enumerate() {
            let m = &mut self.search_matches[i];
            m.set(&commit_info.commit, commit_info.refs.as_slice(), &matcher);
            if m.matched() {
                m.match_index = match_index;
                match_index += 1;
            }
        }
    }

    fn clear_search_matches(&mut self) {
        self.search_matches.iter_mut().for_each(|m| m.clear());
    }

    fn select_current_or_next_match_index(&mut self, current_index: usize) {
        if self.search_matches[current_index].matched() {
            self.select_index(current_index);
            self.search_state
                .update_match_index(self.search_matches[current_index].match_index);
        } else {
            self.select_next_match_index(current_index)
        }
    }

    fn select_next_match_index(&mut self, current_index: usize) {
        if self.total == 0 {
            return;
        }
        let mut i = (current_index + 1) % self.total;
        while i != current_index {
            if self.search_matches[i].matched() {
                self.select_index(i);
                self.search_state
                    .update_match_index(self.search_matches[i].match_index);
                return;
            }
            if i == self.total - 1 {
                i = 0;
            } else {
                i += 1;
            }
        }
    }

    fn select_prev_match_index(&mut self, current_index: usize) {
        if self.total == 0 {
            return;
        }
        let mut i = (current_index + self.total - 1) % self.total;
        while i != current_index {
            if self.search_matches[i].matched() {
                self.select_index(i);
                self.search_state
                    .update_match_index(self.search_matches[i].match_index);
                return;
            }
            if i == 0 {
                i = self.total - 1;
            } else {
                i -= 1;
            }
        }
    }

    fn prepared_image(
        &self,
        commit_info: &'a CommitInfo,
        _visible_row_index: usize,
    ) -> Option<&PreparedImage> {
        self.graph_image_manager
            .prepared_image(&commit_info.commit.commit_hash)
    }
}

pub struct CommitList<'a> {
    ctx: Rc<AppContext>,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl<'a> CommitList<'a> {
    pub fn new(ctx: Rc<AppContext>) -> Self {
        Self {
            ctx,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<'a> StatefulWidget for CommitList<'a> {
    type State = CommitListState<'a>;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        if area.height == 0 {
            return;
        }

        state.ref_hit_areas.clear();

        // Compute once per render, avoids 4 separate mutex lock/unlock cycles
        let avatars_enabled = self.ctx.avatar_manager.lock().unwrap().is_enabled();

        let (header_area, rows_area) = if area.height >= 3 {
            (
                Some(Rect::new(area.x, area.y, area.width, 1)),
                Rect::new(area.x, area.y + 2, area.width, area.height - 2),
            )
        } else {
            (None, area)
        };

        // Reserve the bottom row of the commit list area for a
        // "..." loading indicator while the background streamer is
        // still appending commits. The commit-row rendering then
        // runs against a slightly smaller area (height - 1); the
        // reserved row gets painted at the end of this function.
        let bg_loading = self
            .ctx
            .bg_full_load_in_progress
            .load(std::sync::atomic::Ordering::Acquire);
        let (commits_area, loading_row_y) = if bg_loading && rows_area.height >= 2 {
            (
                Rect::new(
                    rows_area.x,
                    rows_area.y,
                    rows_area.width,
                    rows_area.height - 1,
                ),
                Some(rows_area.y + rows_area.height - 1),
            )
        } else {
            (rows_area, None)
        };

        self.update_state(commits_area, state);

        if let Some(header_area) = header_area {
            self.render_header(buf, header_area, state, avatars_enabled);
            // Subtle ▁ separator line below column headers
            let sep_y = header_area.y + 1;
            let sep_style = Style::default().fg(self.ctx.color_theme.divider_fg);
            for x in header_area.left()..header_area.right() {
                buf[(x, sep_y)].set_symbol("╌");
                buf[(x, sep_y)].set_style(sep_style);
            }
        }

        // Filter out the Graph column entirely when the user has disabled
        // it — its width is freed back to the Message column and no image
        // pipeline runs (cf. `prepare_graph_uploads` in `view/list.rs`).
        let columns: Vec<UserListColumnType> = if self.ctx.ui_config.list.graph_enabled {
            self.ctx.ui_config.list.columns.clone()
        } else {
            self.ctx
                .ui_config
                .list
                .columns
                .iter()
                .filter(|c| !matches!(c, UserListColumnType::Graph))
                .cloned()
                .collect()
        };

        let widths = self.content_column_widths(commits_area.width, state, avatars_enabled);
        let constraints = calc_cell_widths(
            commits_area.width,
            self.ctx.ui_config.list.commit_message_min_width,
            widths,
            &columns,
        );
        let chunks = Layout::horizontal(constraints).split(commits_area);

        for (i, col) in columns.iter().enumerate() {
            match col {
                UserListColumnType::Graph => {
                    self.render_graph(buf, chunks[i], state);
                }
                UserListColumnType::Marker => {
                    self.render_marker(buf, chunks[i], state);
                }
                UserListColumnType::CommitMessage => {
                    self.render_commit_message(buf, chunks[i], state, avatars_enabled);
                }
                UserListColumnType::Name => {
                    self.render_name(buf, chunks[i], state, avatars_enabled);
                }
                UserListColumnType::Hash => {
                    self.render_hash(buf, chunks[i], state);
                }
                UserListColumnType::Date => {
                    self.render_date(buf, chunks[i], state);
                }
            }
        }

        // Background-streaming indicator: paint a centered "..."
        // on the row we reserved above. Stays visible the whole
        // time the bg thread is still shipping batches; disappears
        // automatically once `bg_full_load_in_progress` flips false
        // and the reserved row reverts to a regular commit row.
        if let Some(y) = loading_row_y {
            let dots = "...";
            let dots_width = dots.chars().count() as u16;
            let x = rows_area.x
                + rows_area
                    .width
                    .saturating_sub(dots_width)
                    / 2;
            // Subtler than `divider_fg` alone: stack DIM on top so
            // the "..." reads as a low-attention loading hint
            // rather than competing with the commit rows above.
            let style = Style::default()
                .fg(self.ctx.color_theme.divider_fg)
                .add_modifier(Modifier::DIM);
            for (i, ch) in dots.chars().enumerate() {
                let cx = x + i as u16;
                if cx < rows_area.right() {
                    let cell = &mut buf[(cx, y)];
                    cell.set_symbol(&ch.to_string());
                    cell.set_style(style);
                }
            }
        }
    }
}

impl CommitList<'_> {
    fn content_column_widths(
        &self,
        area_width: u16,
        state: &CommitListState,
        avatars_enabled: bool,
    ) -> CommitListColumnWidths {
        let avatar_width = if avatars_enabled { 3 } else { 0 };
        // Use the cached longest-author width (O(1)) instead of
        // re-iterating the entire commit list on EVERY frame. The
        // cache is seeded at CommitListState::new and rolled
        // forward on extend_commits, so it's always in sync with
        // the longest visible name regardless of how many
        // background-streamed batches have arrived.
        let author_content_width = state.cached_author_name_width();
        // Date width is governed by the configured format string,
        // not the individual values - all dates of the same format
        // render to the same number of characters. Sample the
        // FIRST commit's date for representativeness; falls back
        // to a "-" placeholder if the list is empty. Stable
        // across the bg stream.
        let sample_date: String = state
            .commits
            .first()
            .map(|c| {
                if c.is_uncommitted {
                    c.uncommitted_last_modified
                        .as_ref()
                        .map(|dt| {
                            self.ctx
                                .core_config
                                .date_time_format()
                                .format(dt, self.ctx.core_config.date_time_local())
                        })
                        .unwrap_or_else(|| "-".to_string())
                } else {
                    self.ctx.core_config.date_time_format().format(
                        &c.commit.author_date,
                        self.ctx.core_config.date_time_local(),
                    )
                }
            })
            .unwrap_or_else(|| "-".to_string());
        let dates = [sample_date];

        // Cap graph column at ~40 % of the panel so a wide multi-branch
        // history (rust-lang/rust, linux kernel, …) doesn't squeeze the
        // commit-message column to nothing. The graph image still bakes
        // every branch lane; what overflows the cap gets truncated at
        // render time (cf. `take(max_graph_width)` in `render_graph`).
        // Phase 2 will add `<` / `>` to scroll the hidden lanes back
        // into view. Floor at 8 cells so the graph always shows
        // something useful even on tiny terminals.
        let graph_cap = (area_width * 40 / 100).max(8);
        CommitListColumnWidths {
            graph: state.graph_area_cell_width().min(area_width).min(graph_cap),
            author: author_column_width_from_cached(
                area_width,
                avatar_width,
                author_content_width,
            ),
            hash: 9,
            date: date_column_width(&dates),
        }
    }

    fn update_state(&self, area: Rect, state: &mut CommitListState) {
        state.update_height(area.height as usize);
    }

    fn render_header(
        &self,
        buf: &mut Buffer,
        area: Rect,
        state: &CommitListState,
        avatars_enabled: bool,
    ) {
        let columns: Vec<UserListColumnType> = if self.ctx.ui_config.list.graph_enabled {
            self.ctx.ui_config.list.columns.clone()
        } else {
            self.ctx
                .ui_config
                .list
                .columns
                .iter()
                .filter(|c| !matches!(c, UserListColumnType::Graph))
                .cloned()
                .collect()
        };
        let widths = self.content_column_widths(area.width, state, avatars_enabled);
        let constraints = calc_cell_widths(
            area.width,
            self.ctx.ui_config.list.commit_message_min_width,
            widths,
            &columns,
        );
        let chunks = Layout::horizontal(constraints).split(area);

        for (i, col_type) in columns.iter().enumerate() {
            let text = column_header_text(col_type, avatars_enabled);

            if !text.is_empty() {
                let style = Style::default()
                    .fg(self.ctx.color_theme.list_ref_paren_fg)
                    .add_modifier(Modifier::BOLD);
                let line = Line::from(Span::styled(text.to_string(), style));
                let para = Paragraph::new(line);
                let text_area = Rect::new(chunks[i].x, chunks[i].y, chunks[i].width, 1);
                para.render(text_area, buf);
            }
        }
    }

    fn render_graph(&self, buf: &mut Buffer, area: Rect, state: &mut CommitListState) {
        if area.is_empty() {
            return;
        }

        // Snapshot column width so input handlers can clamp scroll without
        // racing the renderer. Clamp scroll if the panel just shrank.
        let max_graph_width_u16 = area.width.saturating_sub(1);
        if state.last_graph_area_cells != max_graph_width_u16 {
            state.last_graph_area_cells = max_graph_width_u16;
        }
        let max_scroll = state.max_graph_scroll_x();
        if state.graph_scroll_x > max_scroll {
            state.set_graph_scroll_x(max_scroll);
        }

        let key = (state.offset, state.height, area, state.graph_scroll_x);
        if state.graph_render_state == Some(key) {
            // Visible commits and graph area unchanged: write skip cells so ratatui never
            // emits escape sequences for graph positions. Terminal retains the previous render.
            let max_graph_width = area.width.saturating_sub(1) as usize;
            let pad_x = area.left() + max_graph_width as u16;
            for i in 0..state.height.min(area.height as usize) {
                let y = area.top() + i as u16;
                let _is_selected = i == state.selected
                    && state.hovered_branch.is_none()
                    && state.hovered_tag.is_none();
                // Pad cell keeps the app background, selection starts at the │ marker
                if pad_x < area.right() {
                    let pad_cell = &mut buf[(pad_x, y)];
                    pad_cell.set_symbol(" ");
                    pad_cell
                        .set_style(ratatui::style::Style::default().bg(self.ctx.color_theme.bg));
                    pad_cell.set_skip(false);
                    // Skip the image cells
                    for x in area.left()..pad_x {
                        buf[(x, y)].set_skip(true);
                    }
                } else {
                    for x in area.left()..area.right() {
                        buf[(x, y)].set_skip(true);
                    }
                }
            }
            return;
        }

        // Render real graph image cells
        let mut unsupported_overflow_rows: Vec<u16> = Vec::new();
        {
            let state_ref: &CommitListState = state;
            let max_graph_width = area.width.saturating_sub(1) as usize;
            let bg_style = ratatui::style::Style::default().bg(self.ctx.color_theme.bg);
            let scroll_x = state_ref.graph_scroll_x() as usize;
            let supports_kitty_crop = matches!(self.ctx.image_protocol, ImageProtocol::Kitty);
            let px_per_cell = state_ref.graph_image_manager.pixel_width_per_cell();

            self.rendering_commit_info_iter(state_ref)
                .for_each(|(i, commit_info)| {
                    let Some(prepared_image) = state_ref.prepared_image(commit_info, i) else {
                        return;
                    };
                    let y = area.top() + i as u16;
                    let native_cells = prepared_image.cell_width();

                    // Two reasons to take the cropped-placement path:
                    //   1. native graph wider than the column cap (overflow)
                    //   2. user has scrolled horizontally (scroll_x > 0)
                    //
                    // Without (2), the rows whose native graph fits the cap
                    // would stay glued to scroll_x=0 while the wider rows
                    // slide under them — the lanes desync visually.
                    // Re-emitting every visible row through the crop path
                    // when scroll_x > 0 keeps the whole graph coherent.
                    let needs_crop_path = native_cells > max_graph_width || scroll_x > 0;
                    if needs_crop_path {
                        if supports_kitty_crop {
                            let hash = &commit_info.commit.commit_hash;
                            if let Some(bytes) =
                                state_ref.graph_image_manager.graph_row_bytes(hash)
                            {
                                let image_id =
                                    state_ref.graph_image_manager.image_id_for(hash);
                                let visible_cells = max_graph_width.min(
                                    native_cells.saturating_sub(scroll_x),
                                );
                                if visible_cells == 0 {
                                    // Scrolled past this row's last lane.
                                    // Evict any prior placement and leave
                                    // the graph zone blank for this row.
                                    unsupported_overflow_rows.push(y);
                                    for x in area.left()..area.right() {
                                        let cell = &mut buf[(x, y)];
                                        cell.set_symbol(" ");
                                        cell.set_style(bg_style);
                                        cell.set_skip(false);
                                    }
                                    return;
                                }
                                let scroll_px = (scroll_x as u32) * px_per_cell;
                                let crop_w_px = (visible_cells as u32) * px_per_cell;
                                let symbol = kitty_encode_cropped(
                                    bytes,
                                    image_id,
                                    scroll_px,
                                    crop_w_px,
                                    visible_cells,
                                );
                                let head_cell = &mut buf[(area.left(), y)];
                                head_cell.set_symbol(&symbol);
                                head_cell.set_style(bg_style);
                                head_cell.set_skip(false);
                                for off in 1..visible_cells {
                                    let cell = &mut buf[(area.left() + off as u16, y)];
                                    cell.set_symbol(" ");
                                    cell.set_style(bg_style);
                                    cell.set_skip(false);
                                }
                                let pad_x = area.left() + visible_cells as u16;
                                if pad_x < area.right() {
                                    let cell = &mut buf[(pad_x, y)];
                                    cell.set_symbol(" ");
                                    cell.set_style(bg_style);
                                    cell.set_skip(false);
                                }
                                return;
                            }
                        }
                        // No crop support OR bytes unavailable — clear the
                        // graph zone and let the message column take over
                        // (also catches scroll-past-end on non-Kitty
                        // protocols, which is the best we can do).
                        unsupported_overflow_rows.push(y);
                        for x in area.left()..area.right() {
                            let cell = &mut buf[(x, y)];
                            cell.set_symbol(" ");
                            cell.set_style(bg_style);
                            cell.set_skip(false);
                        }
                        return;
                    }

                    let _is_selected = i == state_ref.selected
                        && state_ref.hovered_branch.is_none()
                        && state_ref.hovered_tag.is_none();
                    for (x, image_cell) in prepared_image
                        .cells()
                        .iter()
                        .take(max_graph_width)
                        .enumerate()
                    {
                        let cell = &mut buf[(area.left() + x as u16, y)];
                        cell.set_symbol(image_cell.symbol());
                        cell.set_style(image_cell.style().bg(self.ctx.color_theme.bg));
                        cell.set_skip(image_cell.skip());
                    }
                    // Pad cell keeps the app background, selection starts at the │ marker
                    let pad_x = area.left() + max_graph_width as u16;
                    if pad_x < area.right() {
                        let cell = &mut buf[(pad_x, y)];
                        cell.set_symbol(" ");
                        cell.set_style(bg_style);
                        cell.set_skip(false);
                    }
                });
        }

        // Non-Kitty terminals can't crop persistent placements: writing
        // spaces over the cells above clears iTerm2/Sixel/KittyUnicode
        // outputs, but Kitty's persistent placement also needs an explicit
        // per-row delete for rows we couldn't repaint with a cropped image.
        for y in &unsupported_overflow_rows {
            let _ = self.ctx.image_protocol.delete_row(*y);
        }

        state.graph_render_state = Some(key);
    }

    fn render_marker(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        if area.is_empty() {
            return;
        }
        let items: Vec<ListItem> = self
            .rendering_commit_info_iter(state)
            .map(|(i, commit_info)| {
                let span = Span::raw("│").fg(commit_info.graph_color);
                if i == state.selected
                    && state.hovered_branch.is_none()
                    && state.hovered_tag.is_none()
                {
                    ListItem::new(Line::from(span).bg(self.ctx.color_theme.list_selected_bg))
                } else {
                    ListItem::new(span)
                }
            })
            .collect();
        Widget::render(List::new(items), area, buf)
    }

    fn render_commit_message(
        &self,
        buf: &mut Buffer,
        area: Rect,
        state: &mut CommitListState,
        _avatars_enabled: bool,
    ) {
        let max_width = (area.width as usize).saturating_sub(2);
        if area.is_empty() || max_width == 0 {
            return;
        }
        // Pulled out of the render loop, `stopped-sha` is a single tiny
        // file. Mirror of the `↻ REBASING` badge on the Uncommitted row,
        // but anchored on the EXACT commit git is paused on so the user
        // sees where the rebase will resume from.
        let paused_sha = if !self.ctx.repo_path.as_os_str().is_empty() {
            crate::git::rebase::read_stopped_sha(&self.ctx.repo_path)
        } else {
            None
        };
        let mut items: Vec<ListItem> = Vec::new();
        for (i, commit_info) in state
            .commits
            .iter()
            .skip(state.offset)
            .take(state.height)
            .enumerate()
        {
            if commit_info.is_uncommitted {
                items.push(self.render_uncommitted_commit_message(i, commit_info, state));
                continue;
            }
            let (mut spans, hit_areas) = refs_spans(
                commit_info,
                state.head,
                &state.search_matches[state.offset + i].refs,
                &self.ctx.color_theme,
                &state.branch_color_map,
                state.hovered_branch.as_deref(),
                state.hovered_tag.as_deref(),
            );

            // Store hit areas with absolute positions
            let base_x = area.x + 1; // +1 for leading space in to_commit_list_item
            for hit in hit_areas {
                state.ref_hit_areas.push(RefHitArea {
                    row: area.y + i as u16,
                    col_start: base_x + hit.start as u16,
                    col_end: base_x + hit.end as u16,
                    names: hit.names,
                    is_tag: hit.is_tag,
                });
            }

            let ref_spans_width: usize = spans.iter().map(|s| s.width()).sum();
            // Reserve room for the paused-rebase badge BEFORE deciding
            // where to truncate the commit message. Without this the
            // badge appended below would be the first thing ratatui
            // clips when the terminal is narrow, exactly the opposite
            // of what we want (it has to stay visible because it's the
            // call-to-action telling the user to press `e`).
            let commit = &commit_info.commit;
            let paused_here = paused_sha
                .as_deref()
                .map(|s| s == commit.commit_hash.as_str())
                .unwrap_or(false);
            const PAUSED_BADGE_TEXT: &str = "  ⏸ rebase paused";
            let paused_badge_width = if paused_here {
                console::measure_text_width(PAUSED_BADGE_TEXT)
            } else {
                0
            };
            let max_width = max_width
                .saturating_sub(ref_spans_width)
                .saturating_sub(paused_badge_width);
            if max_width > ELLIPSIS.len() {
                // Always take the first line only: %s should already be single-line
                // but defensively guard against any embedded newlines.
                let subject = commit
                    .commit_message
                    .lines()
                    .next()
                    .unwrap_or(&commit.commit_message);
                let truncate = console::measure_text_width(subject) > max_width;
                let commit_message = if truncate {
                    console::truncate_str(subject, max_width, ELLIPSIS).to_string()
                } else {
                    subject.to_string()
                };

                let sub_spans = if let Some(pos) = state.search_matches[state.offset + i]
                    .commit_message
                    .clone()
                {
                    highlighted_spans(
                        commit_message.into(),
                        pos,
                        self.ctx.color_theme.list_commit_message_fg,
                        Modifier::empty(),
                        &self.ctx.color_theme,
                        truncate,
                    )
                } else {
                    vec![commit_message.fg(self.ctx.color_theme.list_commit_message_fg)]
                };

                spans.extend(sub_spans)
            }
            // ⏸ badge on the exact commit git is paused on. Width is
            // reserved up in the truncation block so the badge always
            // fits, same call-to-action role as `⚠ N conflicts` on
            // the Uncommitted row, can't be the first thing clipped.
            if paused_here {
                spans.push(
                    Span::raw(PAUSED_BADGE_TEXT)
                        .fg(self.ctx.color_theme.status_warn_fg)
                        .add_modifier(Modifier::BOLD),
                );
            }
            items.push(self.to_commit_list_item(i, spans, state));
        }
        Widget::render(List::new(items), area, buf);
    }

    fn render_name(
        &self,
        buf: &mut Buffer,
        area: Rect,
        state: &mut CommitListState,
        avatars_enabled: bool,
    ) {
        let max_width = (area.width as usize).saturating_sub(2);
        if area.is_empty() || max_width == 0 {
            return;
        }
        let avatar_width = 3; // 2 cells image + 1 space
        let items: Vec<ListItem> = self
            .rendering_commit_info_iter(state)
            .map(|(i, commit_info)| {
                if commit_info.is_uncommitted {
                    let slash_fg = if i == state.selected {
                        self.ctx.color_theme.list_selected_fg
                    } else {
                        self.ctx.color_theme.list_name_fg
                    };
                    let mut spans = if avatars_enabled && max_width > 10 {
                        vec![Span::raw("   ")]
                    } else {
                        vec![]
                    };
                    spans.push("/".fg(slash_fg));
                    return self.to_commit_list_item(i, spans, state);
                }
                let commit = &commit_info.commit;
                let effective_max = if avatars_enabled && max_width > 10 {
                    max_width.saturating_sub(avatar_width)
                } else {
                    max_width
                };
                let truncate = console::measure_text_width(&commit.author_name) > effective_max;
                let name = if truncate {
                    console::truncate_str(&commit.author_name, effective_max, ELLIPSIS).to_string()
                } else {
                    commit.author_name.to_string()
                };
                let mut spans = if avatars_enabled && max_width > 10 {
                    vec![Span::raw("   ")]
                } else {
                    vec![]
                };
                let name_spans =
                    if let Some(pos) = state.search_matches[state.offset + i].author_name.clone() {
                        highlighted_spans(
                            (*name).into(),
                            pos,
                            self.ctx.color_theme.list_name_fg,
                            Modifier::empty(),
                            &self.ctx.color_theme,
                            truncate,
                        )
                    } else {
                        vec![name.fg(self.ctx.color_theme.list_name_fg)]
                    };
                spans.extend(name_spans);
                self.to_commit_list_item(i, spans, state)
            })
            .collect();
        Widget::render(List::new(items), area, buf);

        if !avatars_enabled || max_width <= 10 {
            state.avatar_stable_key = None;
            state.avatar_prev_selected = None;
            return;
        }

        // Single lock for the entire avatar render, avoids repeated lock/unlock cycles
        let avatar_manager = self.ctx.avatar_manager.lock().unwrap();

        // Stable key: excludes `selected` so hover-driven selection changes don't trigger a
        // full re-render of all avatar cells (which would flash every image on screen).
        let stable_key = {
            let mut h = DefaultHasher::new();
            state.offset.hash(&mut h);
            state.height.hash(&mut h);
            area.hash(&mut h);
            for (_, commit_info) in self.rendering_commit_info_iter(state) {
                if commit_info.is_uncommitted {
                    false.hash(&mut h);
                    continue;
                }
                let email = &commit_info.commit.author_email;
                email.hash(&mut h);
                avatar_manager
                    .prepared_image(email.as_str(), 1, false)
                    .is_some()
                    .hash(&mut h);
            }
            h.finish()
        };

        let stable_matches = state.avatar_stable_key == Some(stable_key);
        let selected_matches = state.avatar_prev_selected == Some(state.selected);

        // --- FAST PATH: nothing changed ---
        if stable_matches && selected_matches {
            for (i, (_, commit_info)) in self.rendering_commit_info_iter(state).enumerate() {
                if commit_info.is_uncommitted {
                    continue;
                }
                let email = &commit_info.commit.author_email;
                let is_selected = i == state.selected;
                if avatar_manager
                    .prepared_image(email.as_str(), 1, is_selected)
                    .is_some()
                    || avatar_manager
                        .prepared_image(email.as_str(), 1, false)
                        .is_some()
                {
                    let y = area.top() + i as u16;
                    for x in 0..2 {
                        buf[(area.left() + x as u16 + 1, y)].set_skip(true);
                    }
                }
            }
            return;
        }

        // --- SELECTIVE PATH: only the selected row changed, re-render just the two affected rows ---
        if stable_matches && !selected_matches {
            let old_selected = state.avatar_prev_selected;
            let _clear_cell = self.ctx.image_protocol.clear_cell();
            for (i, (_, commit_info)) in self.rendering_commit_info_iter(state).enumerate() {
                if commit_info.is_uncommitted {
                    continue;
                }
                let email = &commit_info.commit.author_email;
                let is_newly_selected = i == state.selected;
                let is_previously_selected = old_selected == Some(i);
                if is_newly_selected || is_previously_selected {
                    // Re-render this row with the correct variant
                    let is_selected = is_newly_selected;
                    let cell_bg = if is_selected {
                        self.ctx.color_theme.list_selected_bg
                    } else {
                        self.ctx.color_theme.bg
                    };
                    let prepared = avatar_manager
                        .prepared_image(email.as_str(), 1, is_selected)
                        .or_else(|| avatar_manager.prepared_image(email.as_str(), 1, false));
                    let y = area.top() + i as u16;
                    if let Some(prepared) = prepared {
                        for (x, image_cell) in prepared.cells().iter().enumerate() {
                            let cell = &mut buf[(area.left() + x as u16 + 1, y)];
                            cell.set_symbol(image_cell.symbol());
                            cell.set_style(image_cell.style().bg(self.ctx.color_theme.bg));
                            cell.set_skip(image_cell.skip());
                        }
                    } else {
                        // No avatar and stable key unchanged (no old image to delete).
                        // Write plain spaces so the List widget's selection background shows
                        // correctly, using the Kitty delete APC here corrupts the bg.
                        for x in 0..2 {
                            let cell = &mut buf[(area.left() + x as u16 + 1, y)];
                            cell.set_symbol(" ");
                            cell.set_style(Style::default().bg(cell_bg));
                            cell.set_skip(false);
                        }
                    }
                } else {
                    // Unchanged row, preserve terminal state
                    if avatar_manager
                        .prepared_image(email.as_str(), 1, false)
                        .is_some()
                    {
                        let y = area.top() + i as u16;
                        for x in 0..2 {
                            buf[(area.left() + x as u16 + 1, y)].set_skip(true);
                        }
                    }
                }
            }
            state.avatar_prev_selected = Some(state.selected);
            return;
        }

        // --- FULL RENDER PATH: offset/height/area/loading changed ---
        let clear_cell = self.ctx.image_protocol.clear_cell();
        for (i, (_, commit_info)) in self.rendering_commit_info_iter(state).enumerate() {
            let y = area.top() + i as u16;
            if commit_info.is_uncommitted {
                // Explicitly clear avatar cells: a committed row's avatar may have been
                // at this y position before scrolling and must not bleed through.
                let cell_bg = if i == state.selected {
                    self.ctx.color_theme.list_selected_bg
                } else {
                    self.ctx.color_theme.bg
                };
                for x in 0..2 {
                    let cell = &mut buf[(area.left() + x as u16 + 1, y)];
                    cell.set_symbol(clear_cell.symbol());
                    cell.set_style(clear_cell.style().bg(cell_bg));
                    cell.set_skip(false);
                }
                continue;
            }
            let email = &commit_info.commit.author_email;
            let is_selected = i == state.selected;
            let cell_bg = if is_selected {
                self.ctx.color_theme.list_selected_bg
            } else {
                self.ctx.color_theme.bg
            };
            let prepared = avatar_manager
                .prepared_image(email.as_str(), 1, is_selected)
                .or_else(|| avatar_manager.prepared_image(email.as_str(), 1, false));
            if let Some(prepared) = prepared {
                for (x, image_cell) in prepared.cells().iter().enumerate() {
                    let cell = &mut buf[(area.left() + x as u16 + 1, y)];
                    cell.set_symbol(image_cell.symbol());
                    cell.set_style(image_cell.style().bg(self.ctx.color_theme.bg));
                    cell.set_skip(image_cell.skip());
                }
            } else {
                for x in 0..2 {
                    let cell = &mut buf[(area.left() + x as u16 + 1, y)];
                    cell.set_symbol(clear_cell.symbol());
                    cell.set_style(clear_cell.style().bg(cell_bg));
                    cell.set_skip(clear_cell.skip());
                }
            }
        }
        state.avatar_stable_key = Some(stable_key);
        state.avatar_prev_selected = Some(state.selected);
    }

    fn render_hash(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        if area.is_empty() {
            return;
        }
        let items: Vec<ListItem> = self
            .rendering_commit_info_iter(state)
            .map(|(i, commit_info)| {
                if commit_info.is_uncommitted {
                    return self.to_commit_list_item(
                        i,
                        vec!["/".fg(self.ctx.color_theme.list_hash_fg)],
                        state,
                    );
                }
                let commit = &commit_info.commit;
                let hash = commit.commit_hash.as_short_hash();
                let spans =
                    if let Some(pos) = state.search_matches[state.offset + i].commit_hash.clone() {
                        highlighted_spans(
                            hash.into(),
                            pos,
                            self.ctx.color_theme.list_hash_fg,
                            Modifier::empty(),
                            &self.ctx.color_theme,
                            false,
                        )
                    } else {
                        vec![hash.fg(self.ctx.color_theme.list_hash_fg)]
                    };
                self.to_commit_list_item(i, spans, state)
            })
            .collect();
        Widget::render(List::new(items), area, buf);
    }

    fn render_date(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        if area.is_empty() {
            return;
        }
        let items: Vec<ListItem> = self
            .rendering_commit_info_iter(state)
            .map(|(i, commit_info)| {
                if commit_info.is_uncommitted {
                    let date_str = commit_info
                        .uncommitted_last_modified
                        .as_ref()
                        .map(|dt| {
                            self.ctx
                                .core_config
                                .date_time_format()
                                .format(dt, self.ctx.core_config.date_time_local())
                        })
                        .unwrap_or_else(|| "-".to_string());
                    return self.to_commit_list_item(
                        i,
                        vec![date_str.fg(self.ctx.color_theme.list_date_fg)],
                        state,
                    );
                }
                let commit = &commit_info.commit;
                let date = &commit.author_date;
                let date_str = self
                    .ctx
                    .core_config
                    .date_time_format()
                    .format(date, self.ctx.core_config.date_time_local());
                self.to_commit_list_item(
                    i,
                    vec![date_str.fg(self.ctx.color_theme.list_date_fg)],
                    state,
                )
            })
            .collect();
        Widget::render(List::new(items), area, buf);
    }

    fn rendering_commit_info_iter<'a>(
        &'a self,
        state: &'a CommitListState,
    ) -> impl Iterator<Item = (usize, &'a CommitInfo)> {
        state
            .commits
            .iter()
            .skip(state.offset)
            .take(state.height)
            .enumerate()
    }

    fn render_uncommitted_commit_message<'a>(
        &self,
        i: usize,
        commit_info: &CommitInfo,
        state: &CommitListState,
    ) -> ListItem<'a> {
        let total = commit_info.uncommitted_staged
            + commit_info.uncommitted_unstaged
            + commit_info.uncommitted_untracked;
        let unmerged = commit_info.uncommitted_unmerged;
        // Same #808080 grey as graph::image's UNCOMMITTED_COLOR and as the
        // marker `│` for this row, keeps the whole uncommitted line tonally
        // unified instead of mixing graph grey with default white text.
        let uncommitted_grey = Color::Rgb(0x80, 0x80, 0x80);
        let mut spans: Vec<Span> = vec![
            Span::raw("Uncommitted Changes")
                .fg(uncommitted_grey)
                .add_modifier(Modifier::BOLD),
            Span::raw(format!(" ({})", total))
                .fg(uncommitted_grey)
                .add_modifier(Modifier::BOLD),
        ];
        // Conflict badge, visible directly on the commit list so the user
        // knows there's a merge in progress without opening Uncommitted Details.
        if unmerged > 0 {
            let noun = if unmerged == 1 {
                "conflict"
            } else {
                "conflicts"
            };
            spans.push(
                Span::raw(format!("  ⚠ {} {}", unmerged, noun))
                    .fg(self.ctx.color_theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            );
        }
        // Rebase badge, same anchor as the conflicts marker but yellow,
        // so the user spots a paused rebase even when there are no current
        // unmerged paths (e.g. paused at an Edit step).
        if !self.ctx.repo_path.as_os_str().is_empty()
            && crate::git::rebase::rebase_in_progress(&self.ctx.repo_path)
        {
            spans.push(
                Span::raw("  ↻ REBASING")
                    .fg(self.ctx.color_theme.status_warn_fg)
                    .add_modifier(Modifier::BOLD),
            );
        }
        self.to_commit_list_item(i, spans, state)
    }

    fn to_commit_list_item<'a, 'b>(
        &'b self,
        i: usize,
        spans: Vec<Span<'a>>,
        state: &'b CommitListState,
    ) -> ListItem<'a> {
        let mut spans = spans;
        spans.insert(0, Span::raw(" "));
        spans.push(Span::raw(" "));
        let mut line = Line::from(spans);

        // Check whether this visible row is the compare-mark target. The mark
        // wins over the selection highlight so the user always sees which
        // commit was first picked, even when the cursor moves onto it.
        let absolute_idx = state.offset + i;
        let is_marked = state
            .marked_compare_commit
            .as_ref()
            .zip(state.commits.get(absolute_idx))
            .map(|(marked, info)| info.commit.commit_hash == *marked)
            .unwrap_or(false);

        if is_marked {
            // Compare-mark uses its own theme tokens, each theme picks a
            // saturated, accent-colored variant of its selection palette so
            // the marked row stands out clearly without clashing.
            line = line
                .bg(self.ctx.color_theme.list_compare_marked_bg)
                .fg(self.ctx.color_theme.list_compare_marked_fg);
        } else if i == state.selected
            && state.hovered_branch.is_none()
            && state.hovered_tag.is_none()
        {
            line = line
                .bg(self.ctx.color_theme.list_selected_bg)
                .fg(self.ctx.color_theme.list_selected_fg);
        }
        ListItem::new(line)
    }
}

#[derive(Debug, Clone)]
struct RefHitAreaRel {
    pub start: usize,
    pub end: usize,
    pub names: Vec<String>,
    pub is_tag: bool,
}

#[allow(unused_assignments)]
fn refs_spans<'a>(
    commit_info: &'a CommitInfo,
    head: &'a Head,
    refs_matches: &'a FxHashMap<String, SearchMatchPosition>,
    color_theme: &'a ColorTheme,
    branch_color_map: &'a FxHashMap<String, Color>,
    hovered_branch: Option<&str>,
    hovered_tag: Option<&str>,
) -> (Vec<Span<'a>>, Vec<RefHitAreaRel>) {
    let refs = &commit_info.refs;

    if refs.len() == 1 {
        if let Ref::Stash { name, .. } = &refs[0] {
            return (
                vec![
                    Span::raw("⌧ ").fg(color_theme.list_ref_stash_fg).bold(),
                    Span::raw(name).fg(color_theme.list_ref_stash_fg).bold(),
                    Span::raw(" "),
                ],
                vec![],
            );
        }
    }

    let mut spans = Vec::new();
    let mut hit_areas = Vec::new();
    let mut current_width = 0;

    spans.push(Span::raw("(").fg(color_theme.list_ref_paren_fg).bold());
    current_width += 1;

    // Collect ref info first
    let mut ref_infos: Vec<(Vec<&'a str>, Color, bool)> = Vec::new();
    {
        let mut local_branches: Vec<(&'a str, Color)> = Vec::new();
        let mut remote_branches: Vec<(&'a str, &'a str, Color)> = Vec::new();
        let mut tags: Vec<(&'a str, Color)> = Vec::new();

        for r in refs.iter() {
            match r {
                Ref::Branch { name, .. } => {
                    let fg = branch_color_map
                        .get(name)
                        .copied()
                        .unwrap_or(color_theme.list_ref_branch_fg);
                    local_branches.push((name.as_str(), fg));
                }
                Ref::RemoteBranch { name, .. } => {
                    let fg = branch_color_map
                        .get(name)
                        .copied()
                        .or_else(|| {
                            name.split_once('/')
                                .and_then(|(_, branch)| branch_color_map.get(branch).copied())
                        })
                        .unwrap_or(color_theme.list_ref_remote_branch_fg);
                    let base = name
                        .split_once('/')
                        .map(|(_, b)| b)
                        .unwrap_or(name.as_str());
                    remote_branches.push((name.as_str(), base, fg));
                }
                Ref::Tag { name, .. } => {
                    let fg = color_theme.list_ref_tag_fg;
                    tags.push((name.as_str(), fg));
                }
                Ref::Stash { .. } => {}
            }
        }

        let mut merged_remotes: FxHashSet<usize> = FxHashSet::default();
        for (local_name, local_fg) in &local_branches {
            let matching_remote = remote_branches
                .iter()
                .enumerate()
                .find(|(ri, (_, base, _))| *base == *local_name && !merged_remotes.contains(ri));
            if let Some((ri, (remote_name, _, _))) = matching_remote {
                merged_remotes.insert(ri);
                let remote_prefix = remote_name.split_once('/').map(|(p, _)| p).unwrap_or("");
                let display = format!("{}|{}", local_name, remote_prefix);
                let fg = *local_fg;
                ref_infos.push((vec![local_name, remote_name], fg, false));
                let _ = display;
            } else {
                ref_infos.push((vec![local_name], *local_fg, false));
            }
        }
        for (ri, (remote_name, _, remote_fg)) in remote_branches.iter().enumerate() {
            if !merged_remotes.contains(&ri) {
                ref_infos.push((vec![remote_name], *remote_fg, false));
            }
        }
        for (tag_name, tag_fg) in tags {
            ref_infos.push((vec![tag_name], tag_fg, true));
        }
    }

    // Move HEAD branch to the front of ref_infos
    if let Head::Branch { name: head_name } = head {
        if let Some(head_idx) = ref_infos
            .iter()
            .position(|(names, _, _)| names.contains(&head_name.as_str()))
        {
            let head_ref = ref_infos.remove(head_idx);
            ref_infos.insert(0, head_ref);
        }
    }

    if let Head::Detached { target } = head {
        if commit_info.commit.commit_hash == *target {
            spans.push(Span::raw("౷ HEAD").fg(color_theme.list_head_fg).bold());
            current_width += 4;
            if !ref_infos.is_empty() {
                spans.push(Span::raw(", ").fg(color_theme.list_ref_paren_fg).bold());
                current_width += 2;
            }
        }
    }

    for (i, (names, fg, is_tag)) in ref_infos.iter().enumerate() {
        let display_name = if names.len() == 2 {
            let remote_prefix = names[1].split_once('/').map(|(p, _)| p).unwrap_or("");
            format!("{}|{}", names[0], remote_prefix)
        } else {
            names[0].to_string()
        };

        let is_hovered = if *is_tag {
            hovered_tag == Some(names[0])
        } else {
            hovered_branch.is_some_and(|hb| names.contains(&hb))
        };

        let is_head_branch = if let Head::Branch { name: head_name } = head {
            names.contains(&head_name.as_str())
        } else {
            false
        };

        // HEAD indicator (always cyan, non-hoverable)
        if is_head_branch {
            let head_icon =
                Span::styled("಄ ", Style::default().fg(color_theme.list_head_fg).bold());
            spans.push(head_icon);
            current_width += 1;
            let head_text = Span::styled(
                "HEAD -> ",
                Style::default().fg(color_theme.list_head_fg).bold(),
            );
            let head_text_width = head_text.width();
            spans.push(head_text);
            current_width += head_text_width;
        }

        // Tags and branches share the same style, `*is_tag` only changes
        // the leading icon glyph (set below), not the text decoration.
        let style = if is_hovered {
            Style::default()
                .fg(*fg)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(*fg).add_modifier(Modifier::BOLD)
        };

        let icon_text = if *is_tag { "🏷  " } else { "⎇ " };
        let icon = Span::styled(icon_text, style);
        let icon_width = icon.width();

        // Hit area starts at the icon (icon + name are both clickable)
        let name_start = current_width;

        spans.push(icon);
        current_width += icon_width;
        let name_spans = refs_matches
            .get(display_name.as_str())
            .or_else(|| refs_matches.get(names[0]))
            .map(|pos| {
                let modifier = if is_hovered {
                    Modifier::BOLD | Modifier::UNDERLINED | Modifier::REVERSED
                } else {
                    Modifier::BOLD
                };
                highlighted_spans(
                    Span::raw(&display_name),
                    pos.clone(),
                    *fg,
                    modifier,
                    color_theme,
                    false,
                )
            })
            .unwrap_or_else(|| {
                let style = if is_hovered {
                    Style::default()
                        .fg(*fg)
                        .add_modifier(Modifier::BOLD)
                        .add_modifier(Modifier::UNDERLINED)
                        .add_modifier(Modifier::REVERSED)
                } else {
                    Style::default().fg(*fg).add_modifier(Modifier::BOLD)
                };
                vec![Span::styled(display_name.clone(), style)]
            });

        for span in &name_spans {
            current_width += span.width();
        }
        spans.extend(name_spans);

        hit_areas.push(RefHitAreaRel {
            start: name_start,
            end: current_width,
            names: names.iter().map(|s| s.to_string()).collect(),
            is_tag: *is_tag,
        });

        if i < ref_infos.len() - 1 {
            spans.push(Span::raw(", ").fg(color_theme.list_ref_paren_fg).bold());
            current_width += 2;
        }
    }

    spans.push(Span::raw(") ").fg(color_theme.list_ref_paren_fg).bold());
    current_width += 2;

    if spans.len() == 2 {
        // Only "(" and ")"
        spans.clear();
        hit_areas.clear();
    }

    (spans, hit_areas)
}

fn highlighted_spans(
    s: Span<'_>,
    pos: SearchMatchPosition,
    base_fg: Color,
    base_modifier: Modifier,
    color_theme: &ColorTheme,
    truncate: bool,
) -> Vec<Span<'static>> {
    let mut hm = highlight_matched_text(vec![s])
        .matched_indices(pos.matched_indices)
        .not_matched_style(Style::default().fg(base_fg).add_modifier(base_modifier))
        .matched_style(
            Style::default()
                .fg(color_theme.list_match_fg)
                .bg(color_theme.list_match_bg)
                .add_modifier(base_modifier),
        );
    if truncate {
        hm = hm.ellipsis(ELLIPSIS);
    }
    hm.into_spans()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommitListColumnWidths {
    graph: u16,
    author: u16,
    hash: u16,
    date: u16,
}

const AUTHOR_MIN_WIDTH: u16 = 7;
const DATE_MIN_WIDTH: u16 = 7;
const AUTHOR_MAX_WIDTH: u16 = 24;
const DATE_MAX_WIDTH: u16 = 22;

fn author_column_width_from_cached(
    area_width: u16,
    avatar_width: u16,
    content_width: u16,
) -> u16 {
    let max_width = AUTHOR_MAX_WIDTH.min(area_width);
    (content_width + avatar_width + 2).max(9).min(max_width)
}

fn date_column_width(dates: &[String]) -> u16 {
    let content_width = dates
        .iter()
        .map(|date| console::measure_text_width(date) as u16)
        .max()
        .unwrap_or(0);
    (content_width + 2).clamp(4, DATE_MAX_WIDTH)
}

fn column_header_text(col_type: &UserListColumnType, avatars_enabled: bool) -> &'static str {
    match col_type {
        UserListColumnType::Graph => "Graph",
        UserListColumnType::Marker => "",
        UserListColumnType::CommitMessage => " Commit message",
        UserListColumnType::Name if avatars_enabled => "  Author",
        UserListColumnType::Name => " Author",
        UserListColumnType::Hash => " SHA",
        UserListColumnType::Date => " Date",
    }
}

fn calc_cell_widths(
    area_width: u16,
    commit_message_min_width: u16,
    widths: CommitListColumnWidths,
    columns: &[UserListColumnType],
) -> Vec<Constraint> {
    let mut graph_cell_width = 0;
    let mut marker_cell_width = 0;
    let mut author_cell_width = 0;
    let mut hash_cell_width = 0;
    let mut date_cell_width = 0;

    for col in columns {
        match col {
            UserListColumnType::Graph => {
                graph_cell_width = widths.graph;
            }
            UserListColumnType::Marker => {
                marker_cell_width = 1;
            }
            UserListColumnType::Name => {
                author_cell_width = widths.author;
            }
            UserListColumnType::Hash => {
                hash_cell_width = widths.hash;
            }
            UserListColumnType::Date => {
                date_cell_width = widths.date;
            }
            UserListColumnType::CommitMessage => {}
        }
    }

    let commit_message_min_width = commit_message_min_width.max(14);
    let fixed_total = |author: u16, date: u16, hash: u16| {
        graph_cell_width + marker_cell_width + author + date + hash + commit_message_min_width
    };

    if fixed_total(author_cell_width, date_cell_width, hash_cell_width) > area_width {
        let overflow = fixed_total(author_cell_width, date_cell_width, hash_cell_width)
            .saturating_sub(area_width);
        let reducible = author_cell_width.saturating_sub(AUTHOR_MIN_WIDTH);
        let reduce_by = overflow.min(reducible);
        author_cell_width = author_cell_width.saturating_sub(reduce_by);
    }
    if fixed_total(author_cell_width, date_cell_width, hash_cell_width) > area_width {
        let overflow = fixed_total(author_cell_width, date_cell_width, hash_cell_width)
            .saturating_sub(area_width);
        let reducible = date_cell_width.saturating_sub(DATE_MIN_WIDTH);
        let reduce_by = overflow.min(reducible);
        date_cell_width = date_cell_width.saturating_sub(reduce_by);
    }
    if fixed_total(author_cell_width, date_cell_width, hash_cell_width) > area_width {
        hash_cell_width = 0;
    }
    if fixed_total(author_cell_width, date_cell_width, hash_cell_width) > area_width {
        author_cell_width = 0;
    }
    if fixed_total(author_cell_width, date_cell_width, hash_cell_width) > area_width {
        date_cell_width = 0;
    }

    let mut constraints = Vec::new();
    for col in columns {
        match col {
            UserListColumnType::Graph => {
                constraints.push(Constraint::Length(graph_cell_width));
            }
            UserListColumnType::Marker => {
                constraints.push(Constraint::Length(marker_cell_width));
            }
            UserListColumnType::CommitMessage => {
                constraints.push(Constraint::Min(0));
            }
            UserListColumnType::Name => {
                constraints.push(Constraint::Length(author_cell_width));
            }
            UserListColumnType::Hash => {
                constraints.push(Constraint::Length(hash_cell_width));
            }
            UserListColumnType::Date => {
                constraints.push(Constraint::Length(date_cell_width));
            }
        }
    }
    constraints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calc_cell_widths_uses_content_widths() {
        let widths = CommitListColumnWidths {
            graph: 6,
            author: 18,
            hash: 9,
            date: 12,
        };
        let columns = vec![
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::CommitMessage,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];

        let actual = calc_cell_widths(80, 20, widths, &columns);

        assert_eq!(
            actual,
            vec![
                Constraint::Length(6),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(18),
                Constraint::Length(9),
                Constraint::Length(12),
            ]
        );
    }

    #[test]
    fn calc_cell_widths_reduces_author_before_date_and_hash() {
        let widths = CommitListColumnWidths {
            graph: 6,
            author: 30,
            hash: 9,
            date: 17,
        };
        let columns = vec![
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::CommitMessage,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];

        let actual = calc_cell_widths(60, 20, widths, &columns);

        assert_eq!(
            actual,
            vec![
                Constraint::Length(6),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(7),
                Constraint::Length(9),
                Constraint::Length(17),
            ]
        );
    }

    #[test]
    fn calc_cell_widths_hides_hash_on_very_narrow_area() {
        let widths = CommitListColumnWidths {
            graph: 6,
            author: 30,
            hash: 9,
            date: 17,
        };
        let columns = vec![
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::CommitMessage,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];

        let actual = calc_cell_widths(34, 20, widths, &columns);

        assert_eq!(
            actual,
            vec![
                Constraint::Length(6),
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(0),
                Constraint::Length(0),
                Constraint::Length(7),
            ]
        );
    }

    #[test]
    fn content_widths_clamp_long_author_names() {
        let content = ["Short", "A Very Very Very Long Author Name"]
            .iter()
            .map(|s| console::measure_text_width(s) as u16)
            .max()
            .unwrap_or(0);
        let width = author_column_width_from_cached(80, 3, content);
        assert_eq!(width, 24);
    }

    #[test]
    fn content_widths_keep_short_author_names_compact() {
        let content = ["Al", "Bob"]
            .iter()
            .map(|s| console::measure_text_width(s) as u16)
            .max()
            .unwrap_or(0);
        let width = author_column_width_from_cached(80, 0, content);
        assert_eq!(width, 9);
    }

    #[test]
    fn date_column_width_clamps_long_dates() {
        let dates = vec![
            "06/05/2026 - 11:30".to_string(),
            "2026-05-06T11:30:00+02:00".to_string(),
        ];
        let width = date_column_width(&dates);
        assert_eq!(width, 22);
    }

    #[test]
    fn name_column_header_is_author() {
        assert_eq!(
            column_header_text(&UserListColumnType::Name, true),
            "  Author"
        );
        assert_eq!(
            column_header_text(&UserListColumnType::Name, false),
            " Author"
        );
    }

    #[test]
    fn non_graph_column_headers_use_one_leading_space() {
        assert_eq!(
            column_header_text(&UserListColumnType::CommitMessage, false),
            " Commit message"
        );
        assert_eq!(column_header_text(&UserListColumnType::Hash, false), " SHA");
        assert_eq!(
            column_header_text(&UserListColumnType::Date, false),
            " Date"
        );
    }
}
