use std::rc::Rc;

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
use rustc_hash::{FxHashMap, FxHashSet};
use tui_input::{backend::crossterm::EventHandler, Input};

use crate::{
    app::AppContext,
    color::ColorTheme,
    config::UserListColumnType,
    git::{Commit, CommitHash, Head, Ref},
    graph::GraphImageManager,
    protocol::PreparedImage,
};

static FUZZY_MATCHER: Lazy<SkimMatcherV2> = Lazy::new(|| SkimMatcherV2::default().respect_case());

const ELLIPSIS: &str = "...";

pub fn uncommitted_commit_hash() -> CommitHash {
    CommitHash::from("0000000000000000000000000000000000000000")
}

#[derive(Debug)]
pub struct CommitInfo<'a> {
    commit: Option<&'a Commit>,
    refs: Vec<&'a Ref>,
    graph_color: Color,
    pub is_uncommitted: bool,
    pub uncommitted_staged: usize,
    pub uncommitted_unstaged: usize,
    pub uncommitted_untracked: usize,
    /// Date de dernière modification des fichiers uncommitted (Option<DateTime<FixedOffset>>)
    pub uncommitted_last_modified: Option<chrono::DateTime<chrono::FixedOffset>>,
}

impl<'a> CommitInfo<'a> {
    pub fn new(commit: &'a Commit, refs: Vec<&'a Ref>, graph_color: Color) -> Self {
        Self {
            commit: Some(commit),
            refs,
            graph_color,
            is_uncommitted: false,
            uncommitted_staged: 0,
            uncommitted_unstaged: 0,
            uncommitted_untracked: 0,
            uncommitted_last_modified: None,
        }
    }

    pub fn new_uncommitted(
        graph_color: Color,
        staged: usize,
        unstaged: usize,
        untracked: usize,
        last_modified: Option<chrono::DateTime<chrono::FixedOffset>>,
    ) -> Self {
        Self {
            commit: None,
            refs: vec![],
            graph_color,
            is_uncommitted: true,
            uncommitted_staged: staged,
            uncommitted_unstaged: unstaged,
            uncommitted_untracked: untracked,
            uncommitted_last_modified: last_modified,
        }
    }

    pub fn commit(&self) -> Option<&'a Commit> {
        self.commit
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
        transient_message: TransientMessage,
    },
    Applied {
        match_index: usize,
        total_match: usize,
        ignore_case: bool,
        fuzzy: bool,
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
}

#[derive(Debug, Default, Clone)]
struct SearchMatch {
    refs: FxHashMap<String, SearchMatchPosition>,
    subject: Option<SearchMatchPosition>,
    author_name: Option<SearchMatchPosition>,
    commit_hash: Option<SearchMatchPosition>,
    match_index: usize, // 1-based
}

impl SearchMatch {
    fn set(&mut self, c: &Commit, refs: &[&Ref], matcher: &SearchMatcher) {
        self.refs = refs
            .iter()
            .filter(|r| !matches!(*r, Ref::Stash { .. }))
            .filter_map(|r| {
                matcher
                    .matched_position(r.name())
                    .map(|pos| (r.name().into(), pos))
            })
            .collect();
        self.subject = matcher.matched_position(&c.subject);
        self.author_name = matcher.matched_position(&c.author_name);
        self.commit_hash = matcher.matched_position(c.commit_hash.as_short_hash());
        self.match_index = 0;
    }

    fn matched(&self) -> bool {
        !self.refs.is_empty()
            || self.subject.is_some()
            || self.author_name.is_some()
            || self.commit_hash.is_some()
    }

    fn clear(&mut self) {
        self.refs.clear();
        self.subject = None;
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
    query: String,
    ignore_case: bool,
    fuzzy: bool,
}

impl SearchMatcher {
    fn new(query: &str, ignore_case: bool, fuzzy: bool) -> Self {
        let query = if ignore_case {
            query.to_lowercase()
        } else {
            query.into()
        };
        Self {
            query,
            ignore_case,
            fuzzy,
        }
    }

    fn matched_position(&self, s: &str) -> Option<SearchMatchPosition> {
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
    pub name: String,
    pub is_tag: bool,
}

#[derive(Debug)]
pub struct CommitListState<'a> {
    commits: Vec<CommitInfo<'a>>,
    commit_hash_set: FxHashSet<&'a CommitHash>,
    graph_image_manager: GraphImageManager<'a>,
    graph_cell_width: u16,
    head: &'a Head,
    head_commit_hash: Option<CommitHash>,
    uncommitted_hash: CommitHash,

    ref_name_to_commit_index_map: FxHashMap<&'a str, usize>,
    branch_color_map: FxHashMap<String, Color>,

    search_state: SearchState,
    search_input: Input,
    search_matches: Vec<SearchMatch>,

    selected: usize,
    offset: usize,
    total: usize,
    height: usize,

    default_ignore_case: bool,
    default_fuzzy: bool,

    pub ref_hit_areas: Vec<RefHitArea>,
    pub hovered_branch: Option<String>,
    pub hovered_tag: Option<String>,
    pub hovered_row: Option<usize>,
}

impl<'a> CommitListState<'a> {
    pub fn new(
        commits: Vec<CommitInfo<'a>>,
        graph_image_manager: GraphImageManager<'a>,
        graph_cell_width: u16,
        head: &'a Head,
        ref_name_to_commit_index_map: FxHashMap<&'a str, usize>,
        branch_color_map: FxHashMap<String, Color>,
        default_ignore_case: bool,
        default_fuzzy: bool,
    ) -> CommitListState<'a> {
        let total = commits.len();
        let has_uncommitted = commits.first().map_or(false, |c| c.is_uncommitted);
        let commit_hash_set = commits
            .iter()
            .filter_map(|c| c.commit.map(|commit| &commit.commit_hash))
            .collect();
        let uncommitted_hash = if has_uncommitted {
            uncommitted_commit_hash()
        } else {
            CommitHash::default()
        };
        let head_commit_hash = match head {
            Head::Detached { target } => Some(target.clone()),
            Head::Branch { name } => ref_name_to_commit_index_map
                .get(name.as_str())
                .and_then(|&index| commits.get(index))
                .and_then(|info| info.commit.map(|c| c.commit_hash.clone())),
            Head::None => None,
        };
        CommitListState {
            commits,
            commit_hash_set,
            graph_image_manager,
            graph_cell_width,
            head,
            head_commit_hash,
            uncommitted_hash,
            ref_name_to_commit_index_map,
            branch_color_map,
            search_state: SearchState::Inactive,
            search_input: Input::default(),
            search_matches: vec![SearchMatch::default(); total],
            selected: 0,
            offset: 0,
            total,
            height: 0,
            default_ignore_case,
            default_fuzzy,
            ref_hit_areas: Vec::new(),
            hovered_branch: None,
            hovered_tag: None,
            hovered_row: None,
        }
    }

    pub fn graph_area_cell_width(&self) -> u16 {
        self.graph_cell_width + 1 // right pad
    }

    pub fn update_height(&mut self, height: usize) {
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
        let has_uncommitted = self.commits.first().map_or(false, |c| c.is_uncommitted);
        self.graph_image_manager.set_has_uncommitted(has_uncommitted);

        let current_head_hash = self.head_commit_hash.clone();
        let manager_head = self.graph_image_manager.head_commit_hash().cloned();
        if manager_head.as_ref() != current_head_hash.as_ref() {
            if let Some(old) = manager_head {
                if old == self.uncommitted_hash {
                    self.graph_image_manager.invalidate_uncommitted();
                } else {
                    self.graph_image_manager.invalidate(&old);
                }
            }
            if let Some(new) = &current_head_hash {
                if *new == self.uncommitted_hash {
                    self.graph_image_manager.invalidate_uncommitted();
                } else {
                    self.graph_image_manager.invalidate(new);
                }
            }
            self.graph_image_manager
                .set_head_commit_hash(current_head_hash.as_ref());
        }

        self.commits
            .iter()
            .skip(self.offset)
            .take(self.height)
            .for_each(|commit_info| {
                if let Some(commit) = commit_info.commit {
                    self.graph_image_manager
                        .ensure_uploaded(&commit.commit_hash);
                } else if commit_info.is_uncommitted {
                    self.graph_image_manager
                        .ensure_uploaded_uncommitted(commit_info.graph_color);
                }
            });
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        self.graph_image_manager.drain_pending_uploads()
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
        if self.selected < (self.total - 1).min(self.height - 1) {
            self.selected += 1;
        } else if self.selected + self.offset < self.total - 1 {
            self.offset += 1;
        }
    }

    pub fn select_parent(&mut self) {
        if let Some(target_commit) = self.selected_commit_parent_hash().cloned() {
            if self.commit_hash_set.contains(&target_commit) {
                while target_commit.as_str() != self.selected_commit_hash().as_str() {
                    self.select_next();
                }
            }
        }
    }

    pub fn selected_commit_parent_hash(&self) -> Option<&CommitHash> {
        self.commits[self.current_selected_index()]
            .commit
            .and_then(|c| c.parent_commit_hashes.first())
    }

    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        } else if self.offset > 0 {
            self.offset -= 1;
        }
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
        self.offset = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = (self.height - 1).min(self.total - 1);
        if self.height < self.total {
            self.offset = self.total - self.height;
        }
    }

    pub fn scroll_down(&mut self) {
        let max_offset = self.total.saturating_sub(self.height);
        if self.offset < max_offset {
            self.offset += 1;
        }
    }

    pub fn scroll_up(&mut self) {
        if self.offset > 0 {
            self.offset -= 1;
        }
    }

    pub fn restore_visual_selection(&mut self, visual_row: usize) {
        let current_index = self.current_selected_index();
        let max_offset = self.total.saturating_sub(self.height);
        self.offset = current_index.saturating_sub(visual_row).min(max_offset);
        self.selected = current_index.saturating_sub(self.offset);
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
            }
        }
    }

    pub fn select_next_match(&mut self) {
        self.select_next_match_index(self.current_selected_index());
    }

    pub fn select_prev_match(&mut self) {
        self.select_prev_match_index(self.current_selected_index());
    }

    pub fn selected_commit_hash(&self) -> &CommitHash {
        let info = &self.commits[self.current_selected_index()];
        info.commit
            .map(|c| &c.commit_hash)
            .unwrap_or(&self.uncommitted_hash)
    }

    pub fn selected_commit_subject(&self) -> Option<&str> {
        let info = &self.commits[self.current_selected_index()];
        info.commit.map(|c| c.subject.as_str())
    }

    pub fn is_uncommitted_selected(&self) -> bool {
        let info = &self.commits[self.current_selected_index()];
        info.is_uncommitted
    }

    fn current_selected_index(&self) -> usize {
        self.offset + self.selected
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
            } else {
                self.selected = index;
            }
        }
    }

    pub fn select_commit_hash(&mut self, commit_hash: &CommitHash) {
        if !self.commit_hash_set.contains(commit_hash) {
            return;
        }
        for (i, commit_info) in self.commits.iter().enumerate() {
            if let Some(commit) = commit_info.commit {
                if commit.commit_hash == *commit_hash {
                    if self.total > self.height {
                        self.selected = 0;
                        self.offset = i;
                    } else {
                        self.selected = i;
                    }
                    break;
                }
            }
        }
    }

    pub fn search_state(&self) -> SearchState {
        self.search_state
    }

    pub fn search_case_fuzzy(&self) -> Option<(bool, bool)> {
        match self.search_state {
            SearchState::Searching {
                ignore_case, fuzzy, ..
            }
            | SearchState::Applied {
                ignore_case, fuzzy, ..
            } => Some((ignore_case, fuzzy)),
            _ => None,
        }
    }

    pub fn branch_at_position(&self, col: u16, row: u16) -> Option<String> {
        self.ref_hit_areas.iter().find_map(|hit| {
            if !hit.is_tag && hit.row == row && col >= hit.col_start && col < hit.col_end {
                Some(hit.name.clone())
            } else {
                None
            }
        })
    }

    pub fn tag_at_position(&self, col: u16, row: u16) -> Option<String> {
        self.ref_hit_areas.iter().find_map(|hit| {
            if hit.is_tag && hit.row == row && col >= hit.col_start && col < hit.col_end {
                Some(hit.name.clone())
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
            ..
        } = self.search_state
        {
            self.search_input.handle_event(&Event::Key(key));
            self.update_search_matches(ignore_case, fuzzy);
            self.select_current_or_next_match_index(start_index);
        }
    }

    pub fn apply_search(&mut self) {
        if let SearchState::Searching {
            match_index,
            ignore_case,
            fuzzy,
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
            fuzzy,
            ..
        } = &mut self.search_state
        {
            *ignore_case = !*ignore_case;
            is_applied = true;
            start_index = *match_index;
        }

        let (ignore_case, fuzzy) = match self.search_state {
            SearchState::Searching {
                ignore_case,
                fuzzy,
                ..
            }
            | SearchState::Applied {
                ignore_case,
                fuzzy,
                ..
            } => (ignore_case, fuzzy),
            _ => return None,
        };
        self.update_search_matches(ignore_case, fuzzy);
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
            match_index,
            ignore_case,
            fuzzy,
            ..
        } = &mut self.search_state
        {
            *fuzzy = !*fuzzy;
            is_applied = true;
            start_index = *match_index;
        }

        let (ignore_case, fuzzy) = match self.search_state {
            SearchState::Searching {
                ignore_case,
                fuzzy,
                ..
            }
            | SearchState::Applied {
                ignore_case,
                fuzzy,
                ..
            } => (ignore_case, fuzzy),
            _ => return None,
        };
        self.update_search_matches(ignore_case, fuzzy);
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
            }
        } else {
            None
        }
    }

    fn update_search_matches(&mut self, ignore_case: bool, fuzzy: bool) {
        let matcher = SearchMatcher::new(self.search_input.value(), ignore_case, fuzzy);
        let mut match_index = 1;
        for (i, commit_info) in self.commits.iter().enumerate() {
            let m = &mut self.search_matches[i];
            if let Some(commit) = commit_info.commit {
                m.set(commit, commit_info.refs.as_slice(), &matcher);
            } else {
                m.clear();
            }
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

    fn prepared_image(&self, commit_info: &'a CommitInfo, _visible_row_index: usize) -> &PreparedImage {
        if let Some(commit) = commit_info.commit {
            self.graph_image_manager.prepared_image(&commit.commit_hash)
        } else {
            self.graph_image_manager.prepared_image_uncommitted()
        }
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

        let (header_area, rows_area) = if area.height >= 2 {
            (
                Some(Rect::new(area.x, area.y, area.width, 2)),
                Rect::new(area.x, area.y + 2, area.width, area.height - 2),
            )
        } else {
            (None, area)
        };

        self.update_state(rows_area, state);

        if let Some(header_area) = header_area {
            self.render_header(buf, header_area, state);
        }

        let constraints = calc_cell_widths(
            rows_area.width,
            self.ctx.ui_config.list.subject_min_width,
            state.graph_area_cell_width(),
            self.ctx.ui_config.list.name_width,
            self.ctx.ui_config.list.date_width,
            &self.ctx.ui_config.list.columns,
        );
        let chunks = Layout::horizontal(constraints).split(rows_area);

        for (i, col) in self.ctx.ui_config.list.columns.iter().enumerate() {
            match col {
                UserListColumnType::Graph => {
                    self.render_graph(buf, chunks[i], state);
                }
                UserListColumnType::Marker => {
                    self.render_marker(buf, chunks[i], state);
                }
                UserListColumnType::Subject => {
                    self.render_subject(buf, chunks[i], state);
                }
                UserListColumnType::Name => {
                    self.render_name(buf, chunks[i], state);
                }
                UserListColumnType::Hash => {
                    self.render_hash(buf, chunks[i], state);
                }
                UserListColumnType::Date => {
                    self.render_date(buf, chunks[i], state);
                }
            }
        }
    }
}

impl CommitList<'_> {
    fn update_state(&self, area: Rect, state: &mut CommitListState) {
        state.update_height(area.height as usize);
    }

    fn render_header(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        let constraints = calc_cell_widths(
            area.width,
            self.ctx.ui_config.list.subject_min_width,
            state.graph_area_cell_width(),
            self.ctx.ui_config.list.name_width,
            self.ctx.ui_config.list.date_width,
            &self.ctx.ui_config.list.columns,
        );
        let chunks = Layout::horizontal(constraints).split(area);

        for (i, col_type) in self.ctx.ui_config.list.columns.iter().enumerate() {
            let text = match col_type {
                UserListColumnType::Graph => "Graph",
                UserListColumnType::Marker => "",
                UserListColumnType::Subject => "Commit message",
                UserListColumnType::Name => "Committer",
                UserListColumnType::Hash => "SHA",
                UserListColumnType::Date => "Date",
            };

            if !text.is_empty() {
                let style = Style::default()
                    .fg(Color::Rgb(86, 95, 137))
                    .add_modifier(Modifier::BOLD);
                let line = Line::from(Span::styled(text.to_string(), style));
                let para = Paragraph::new(line);
                let text_area = Rect::new(chunks[i].x, chunks[i].y, chunks[i].width, 1);
                para.render(text_area, buf);
            }
        }

        // Draw separator line below header
        let sep_style = Style::default().fg(Color::Rgb(59, 66, 97));
        for col in area.left()..area.right() {
            buf[(col, area.top() + 1)].set_symbol("─");
            buf[(col, area.top() + 1)].set_style(sep_style);
        }
    }

    fn render_graph(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        if area.is_empty() {
            return;
        }
        self.rendering_commit_info_iter(state)
            .for_each(|(i, commit_info)| {
                let prepared_image = state.prepared_image(commit_info, i);
                let max_graph_width = area.width.saturating_sub(1) as usize;
                let y = area.top() + i as u16;
                for (x, image_cell) in prepared_image
                    .cells()
                    .iter()
                    .take(max_graph_width)
                    .enumerate()
                {
                    let cell = &mut buf[(area.left() + x as u16, y)];
                    cell.set_symbol(image_cell.symbol());
                    cell.set_style(image_cell.style());
                    cell.set_skip(image_cell.skip());
                }
            });
    }

    fn render_marker(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        if area.is_empty() {
            return;
        }
        use crate::git::CommitType;
        let items: Vec<ListItem> = self
            .rendering_commit_info_iter(state)
            .map(|(_, commit_info)| {
                let marker = if commit_info.commit().map_or(false, |c| matches!(c.commit_type, CommitType::Stash)) {
                    "◉"
                } else {
                    "│"
                };
                ListItem::new(marker.fg(commit_info.graph_color))
            })
            .collect();
        Widget::render(List::new(items), area, buf)
    }

    fn render_subject(&self, buf: &mut Buffer, area: Rect, state: &mut CommitListState) {
        let max_width = (area.width as usize).saturating_sub(2);
        if area.is_empty() || max_width == 0 {
            return;
        }
        let mut items: Vec<ListItem> = Vec::new();
        for (i, commit_info) in state.commits.iter().skip(state.offset).take(state.height).enumerate() {
            if commit_info.is_uncommitted {
                items.push(self.render_uncommitted_subject(i, commit_info, state));
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
                    name: hit.name,
                    is_tag: hit.is_tag,
                });
            }

            let ref_spans_width: usize = spans.iter().map(|s| s.width()).sum();
            let max_width = max_width.saturating_sub(ref_spans_width);
            let commit = commit_info.commit.unwrap();
            if max_width > ELLIPSIS.len() {
                let truncate = console::measure_text_width(&commit.subject) > max_width;
                let subject = if truncate {
                    console::truncate_str(&commit.subject, max_width, ELLIPSIS).to_string()
                } else {
                    commit.subject.to_string()
                };

                let sub_spans =
                    if let Some(pos) = state.search_matches[state.offset + i].subject.clone() {
                        highlighted_spans(
                            subject.into(),
                            pos,
                            self.ctx.color_theme.list_subject_fg,
                            Modifier::empty(),
                            &self.ctx.color_theme,
                            truncate,
                        )
                    } else {
                        vec![subject.fg(self.ctx.color_theme.list_subject_fg)]
                    };

                spans.extend(sub_spans)
            }
            items.push(self.to_commit_list_item(i, spans, state));
        }
        Widget::render(List::new(items), area, buf);
    }

    fn render_name(&self, buf: &mut Buffer, area: Rect, state: &CommitListState) {
        let max_width = (area.width as usize).saturating_sub(2);
        if area.is_empty() || max_width == 0 {
            return;
        }
        let items: Vec<ListItem> = self
            .rendering_commit_info_iter(state)
            .map(|(i, commit_info)| {
                if commit_info.is_uncommitted {
                    return self.to_commit_list_item(
                        i,
                        vec!["/".fg(self.ctx.color_theme.list_name_fg)],
                        state,
                    );
                }
                let commit = commit_info.commit.unwrap();
                let truncate = console::measure_text_width(&commit.author_name) > max_width;
                let name = if truncate {
                    console::truncate_str(&commit.author_name, max_width, ELLIPSIS).to_string()
                } else {
                    commit.author_name.to_string()
                };
                let spans =
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
                self.to_commit_list_item(i, spans, state)
            })
            .collect();
        Widget::render(List::new(items), area, buf);
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
                let commit = commit_info.commit.unwrap();
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
                            self.ctx.core_config.date_time_format().format(
                                dt,
                                self.ctx.core_config.date_time_local(),
                            )
                        })
                        .unwrap_or_else(|| "-".to_string());
                    return self.to_commit_list_item(
                        i,
                        vec![date_str.fg(self.ctx.color_theme.list_date_fg)],
                        state,
                    );
                }
                let commit = commit_info.commit.unwrap();
                let date = &commit.author_date;
                let date_str = self.ctx.core_config.date_time_format().format(
                    date,
                    self.ctx.core_config.date_time_local(),
                );
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
    ) -> impl Iterator<Item = (usize, &'a CommitInfo<'a>)> {
        state
            .commits
            .iter()
            .skip(state.offset)
            .take(state.height)
            .enumerate()
    }

    fn render_uncommitted_subject<'a>(
        &self,
        i: usize,
        commit_info: &CommitInfo,
        state: &CommitListState,
    ) -> ListItem<'a> {
        let total = commit_info.uncommitted_staged
            + commit_info.uncommitted_unstaged
            + commit_info.uncommitted_untracked;
        let spans: Vec<Span> = vec![
            Span::raw("Uncommitted Changes").fg(self.ctx.color_theme.fg).add_modifier(Modifier::BOLD),
            Span::raw(format!(" ({})", total)).fg(self.ctx.color_theme.fg).add_modifier(Modifier::BOLD),
        ];
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
        if i == state.selected && state.hovered_branch.is_none() && state.hovered_tag.is_none() {
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
    pub name: String,
    pub is_tag: bool,
}

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
        if let Ref::Stash { name, .. } = refs[0] {
            return (
                vec![
                    Span::raw("📦 ").fg(color_theme.list_ref_stash_fg).bold(),
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
    let mut ref_infos: Vec<(&'a str, Color, bool)> = Vec::new();
    for r in refs.iter() {
        match r {
            Ref::Branch { name, .. } => {
                let fg = branch_color_map
                    .get(name)
                    .copied()
                    .unwrap_or(color_theme.list_ref_branch_fg);
                ref_infos.push((name, fg, false));
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
                ref_infos.push((name, fg, false));
            }
            Ref::Tag { name, .. } => {
                let fg = color_theme.list_ref_tag_fg;
                ref_infos.push((name, fg, true));
            }
            Ref::Stash { .. } => {}
        }
    }

    if let Head::Detached { target } = head {
        if let Some(commit) = commit_info.commit {
            if commit.commit_hash == *target {
                spans.push(Span::raw("HEAD").fg(color_theme.list_head_fg).bold());
                current_width += 4;
                if !ref_infos.is_empty() {
                    spans.push(Span::raw(", ").fg(color_theme.list_ref_paren_fg).bold());
                    current_width += 2;
                }
            }
        }
    }

    for (i, (name, fg, is_tag)) in ref_infos.iter().enumerate() {
        if let Head::Branch { name: head_name } = head {
            if *name == head_name {
                spans.push(Span::raw("HEAD -> ").fg(color_theme.list_head_fg).bold());
                current_width += 8;
            }
        }

        let is_hovered = if *is_tag {
            hovered_tag == Some(*name)
        } else {
            hovered_branch == Some(*name)
        };

        let icon = if *is_tag {
            Span::raw("🏷 ").fg(*fg).bold()
        } else {
            Span::raw("⎇ ").fg(*fg).bold()
        };
        let icon_width = icon.width();
        spans.push(icon);
        current_width += icon_width;

        let name_start = current_width;
        let name_spans = refs_matches
            .get(*name)
            .map(|pos| {
                let modifier = if is_hovered {
                    Modifier::BOLD | Modifier::UNDERLINED | Modifier::REVERSED
                } else {
                    Modifier::BOLD
                };
                highlighted_spans(
                    (*name).into(),
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
                vec![Span::styled(*name, style)]
            });

        for span in &name_spans {
            current_width += span.width();
        }
        spans.extend(name_spans);

        hit_areas.push(RefHitAreaRel {
            start: name_start,
            end: current_width,
            name: (*name).to_string(),
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

fn calc_cell_widths(
    area_width: u16,
    subject_min_width: u16,
    graph_width: u16,
    name_width: u16,
    date_width: u16,
    columns: &[UserListColumnType],
) -> Vec<Constraint> {
    let pad = 2;
    let (
        mut graph_cell_width,
        mut marker_cell_width,
        mut name_cell_width,
        mut hash_cell_width,
        mut date_cell_width,
    ) = (0, 0, 0, 0, 0);

    for col in columns {
        match col {
            UserListColumnType::Graph => {
                graph_cell_width = graph_width.max(5);
            }
            UserListColumnType::Marker => {
                marker_cell_width = 1;
            }
            UserListColumnType::Name => {
                name_cell_width = (name_width + pad).max(9);
            }
            UserListColumnType::Hash => {
                hash_cell_width = (7 + pad).max(3);
            }
            UserListColumnType::Date => {
                date_cell_width = (date_width + pad).max(4);
            }
            UserListColumnType::Subject => {}
        }
    }

    let subject_min_width = subject_min_width.max(14);

    let mut total_width = graph_cell_width
        + marker_cell_width
        + hash_cell_width
        + name_cell_width
        + date_cell_width
        + subject_min_width;

    if total_width > area_width {
        total_width = total_width.saturating_sub(name_cell_width);
        name_cell_width = 0;
    }
    if total_width > area_width {
        total_width = total_width.saturating_sub(date_cell_width);
        date_cell_width = 0;
    }
    if total_width > area_width {
        hash_cell_width = 0;
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
            UserListColumnType::Subject => {
                constraints.push(Constraint::Min(0));
            }
            UserListColumnType::Name => {
                constraints.push(Constraint::Length(name_cell_width));
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
    fn test_calc_cell_widths_all_columns() {
        let area_width = 80;
        let subject_min_width = 20;
        let graph_width = 6;
        let name_width = 10;
        let date_width = 15;
        let columns = vec![
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::Subject,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];

        let actual = calc_cell_widths(
            area_width,
            subject_min_width,
            graph_width,
            name_width,
            date_width,
            &columns,
        );

        let expected = vec![
            Constraint::Length(6),  // Graph
            Constraint::Length(1),  // Marker
            Constraint::Min(0),     // Subject
            Constraint::Length(12), // Name (10 + 2 pad)
            Constraint::Length(9),  // Hash (7 + 2 pad)
            Constraint::Length(17), // Date (15 + 2 pad)
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_calc_cell_width_all_columns_small_area_remove_name_date_hash() {
        let area_width = 30;
        let subject_min_width = 20;
        let graph_width = 6;
        let name_width = 10;
        let date_width = 15;
        let columns = vec![
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::Subject,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];

        let actual = calc_cell_widths(
            area_width,
            subject_min_width,
            graph_width,
            name_width,
            date_width,
            &columns,
        );

        // Graph + Marker + Subject + Hash = 6 + 1 + 20 + 9 = 36 > 30
        // => Name, Date, and Hash are removed
        let expected = vec![
            Constraint::Length(6), // Graph
            Constraint::Length(1), // Marker
            Constraint::Min(0),    // Subject
            Constraint::Length(0), // Name removed
            Constraint::Length(0), // Hash removed
            Constraint::Length(0), // Date removed
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_calc_cell_width_all_columns_small_area_remove_name_date() {
        let area_width = 40;
        let subject_min_width = 20;
        let graph_width = 6;
        let name_width = 10;
        let date_width = 15;
        let columns = vec![
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::Subject,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];

        let actual = calc_cell_widths(
            area_width,
            subject_min_width,
            graph_width,
            name_width,
            date_width,
            &columns,
        );

        // Graph + Marker + Subject + Hash = 6 + 1 + 20 + 9 = 36
        // Graph + Marker + Subject + Date + Hash = 6 + 1 + 20 + 17 + 9 = 53 > 40
        // => Name and Date are removed
        let expected = vec![
            Constraint::Length(6), // Graph
            Constraint::Length(1), // Marker
            Constraint::Min(0),    // Subject
            Constraint::Length(0), // Name removed
            Constraint::Length(9), // Hash (7 + 2 pad)
            Constraint::Length(0), // Date removed
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_calc_cell_width_all_columns_small_area_remove_name() {
        let area_width = 60;
        let subject_min_width = 20;
        let graph_width = 6;
        let name_width = 10;
        let date_width = 15;
        let columns = vec![
            UserListColumnType::Graph,
            UserListColumnType::Marker,
            UserListColumnType::Subject,
            UserListColumnType::Name,
            UserListColumnType::Hash,
            UserListColumnType::Date,
        ];

        let actual = calc_cell_widths(
            area_width,
            subject_min_width,
            graph_width,
            name_width,
            date_width,
            &columns,
        );

        // Graph + Marker + Subject + Date + Hash = 6 + 1 + 20 + 17 + 9 = 53 <= 60
        // Graph + Marker + Subject + Name + Date + Hash = 6 + 1 + 20 + 12 + 17 + 9 = 65 > 60
        // => Name is removed
        let expected = vec![
            Constraint::Length(6),  // Graph
            Constraint::Length(1),  // Marker
            Constraint::Min(0),     // Subject
            Constraint::Length(0),  // Name removed
            Constraint::Length(9),  // Hash (7 + 2 pad)
            Constraint::Length(17), // Date (15 + 2 pad)
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_calc_cell_width_columns_order() {
        let area_width = 80;
        let subject_min_width = 20;
        let graph_width = 6;
        let name_width = 10;
        let date_width = 15;
        let columns = vec![
            UserListColumnType::Date,
            UserListColumnType::Subject,
            UserListColumnType::Hash,
            UserListColumnType::Graph,
        ];

        let actual = calc_cell_widths(
            area_width,
            subject_min_width,
            graph_width,
            name_width,
            date_width,
            &columns,
        );

        let expected = vec![
            Constraint::Length(17), // Date (15 + 2 pad)
            Constraint::Min(0),     // Subject
            Constraint::Length(9),  // Hash (7 + 2 pad)
            Constraint::Length(6),  // Graph
        ];
        assert_eq!(actual, expected);
    }
}
