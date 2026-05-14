//! Interactive rebase editor — GitKraken-inspired UI in three layouts.
//!
//! Opens from the existing Rebase dialog when the user checks
//! `Interactive (-i)`. The dialog hands us the base commit; we load the
//! commits in `base..HEAD` and let the user assign each one an action
//! (pick / reword / edit / squash / fixup / drop), reorder them, then
//! apply with Enter — which runs `git rebase -i base` driven by our
//! prepared todo via GIT_SEQUENCE_EDITOR.
//!
//! Three layouts selectable via `[ui.common] rebase_view`:
//! - `compact` : single dense list (CLI-style)
//! - `inline`  : list + per-row italic description + inline reword editor
//! - `split`   : list on top + Result preview pane below
//!
//! Keyboard:
//! - `p` / `r` / `e` / `s` / `f` / `d`  set the current row's action
//! - `↑` / `↓`                          move selection
//! - `Shift+↑` / `Shift+↓`              reorder the selected commit
//! - `←` / `→`                          cycle action prev / next
//! - `Enter`                            apply the rebase
//! - `Esc`                              cancel
//!
//! Mouse:
//! - Click an action label → cycle to the next action
//! - Click a row           → select it (and start reword if action == Reword)
//! - Hover                 → row highlight

use std::{path::PathBuf, rc::Rc};

use ratatui::{
    crossterm::event::{KeyEvent, KeyModifiers},
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::{
    app::AppContext,
    config::RebaseViewMode,
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    git::rebase::{RebaseAction, RebaseItem},
    widget::commit_list::CommitListState,
};

/// Per-row rect captured each frame for mouse hit-testing.
#[derive(Debug, Clone)]
struct RowRect {
    /// Index of the corresponding `items` entry.
    item_idx: usize,
    /// Inner area row this entry occupies on screen.
    row: u16,
    /// Whether the click range here cycles the action (the `[Pick]` tag)
    /// or just selects the row.
    action_col_start: u16,
    action_col_end: u16,
    full_x: u16,
    full_x_end: u16,
}

#[derive(Debug)]
pub struct InteractiveRebaseView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    repo_path: PathBuf,
    base_hash: String,
    /// Commits to rebase, in chronological order (oldest first).
    /// `len() == 0` means there's nothing to rebase — handled with a
    /// gentle empty-state instead of an error.
    items: Vec<RebaseItem>,
    /// Index of the focused row. Both keyboard arrows and mouse hover
    /// drive it directly — single-cursor model so the user never sees two
    /// rows highlighted at once.
    selected: usize,
    /// Effective layout mode — read once from config at view creation.
    mode: RebaseViewMode,
    /// Vertical scroll inside the list. Auto-anchors on `selected`.
    scroll_delta: i32,
    /// When `Some`, the user is editing the reword message of items[idx].
    /// The String buffer holds the current text + cursor at its end.
    reword_editing: Option<RewordEdit>,
    /// Screen position (col, row) where the reword input starts — captured
    /// during render so we can place the terminal cursor after drawing.
    /// `None` when the editor isn't visible.
    reword_screen_pos: Option<(u16, u16)>,
    /// `true` when the user has "grabbed" the selected row — arrows then
    /// move the commit instead of moving the cursor. Toggled with Space.
    grabbed: bool,
    /// Row rects captured during the previous render for mouse hit-testing.
    row_rects: Vec<RowRect>,
    /// Last apply error, persistently displayed at the top of the view.
    /// Stays until the user dismisses it (any key) or runs a successful
    /// apply. Outlives the transient toast notification so the user can
    /// actually read it.
    last_error: Option<String>,
    /// When `Some`, we're not editing a plan — we're showing the recovery
    /// UI for an already-running rebase (`.git/rebase-merge/*` exists).
    /// `items` is empty in this mode; rendering and key handling switch
    /// to the resume panel.
    resume: Option<crate::git::rebase::ResumeState>,
    ctx: Rc<AppContext>,
    tx: Sender,
}

#[derive(Debug)]
struct RewordEdit {
    item_idx: usize,
    buffer: String,
    cursor: usize,
}

impl<'a> InteractiveRebaseView<'a> {
    pub fn new(
        commit_list_state: Option<CommitListState<'a>>,
        repo_path: PathBuf,
        base_hash: String,
        items: Vec<RebaseItem>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        let mode = ctx.ui_config.common.rebase_view;
        let resume = crate::git::rebase::read_resume_state(&repo_path);
        Self {
            commit_list_state,
            repo_path,
            base_hash,
            items,
            selected: 0,
            mode,
            scroll_delta: 0,
            reword_editing: None,
            reword_screen_pos: None,
            grabbed: false,
            row_rects: Vec::new(),
            last_error: None,
            resume,
            ctx,
            tx,
        }
    }

    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
    }

    pub fn footer_hint(&self) -> String {
        // Resume mode has its own minimal hint — only 4 actions matter.
        if self.resume.is_some() {
            let parts = ["C:continue", "S:skip", "A:abort", "Esc:exit"];
            return format!("⌘ {}", parts.join("▕▏"));
        }
        // Footer adapts: when a row is grabbed, arrows move it; otherwise
        // arrows navigate. The Space hint flips between Grab / Release.
        if self.grabbed {
            let parts = ["↑↓:move commit", "Space:release", "Esc:cancel"];
            format!("⌘ {}", parts.join("▕▏"))
        } else {
            let mut parts: Vec<&str> = vec![
                "p:pick",
                "r:reword",
                "e:edit",
                "s:squash",
                "f:fixup",
                "d:drop",
                "⇆:cycle",
                "Space:grab",
                "↑↓:nav",
            ];
            // Surface the abort shortcut only when there's something to abort —
            // keeps the footer quiet on the happy path.
            if crate::git::rebase::rebase_in_progress(&self.repo_path) {
                parts.push("A:abort prev");
            }
            format!("⌘ {}", parts.join("▕▏"))
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
    pub fn refresh(&mut self) {}
    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        Rc::make_mut(&mut self.ctx).color_theme = theme;
    }

    pub fn is_input_active(&self) -> bool {
        self.reword_editing.is_some()
    }

    // -------------------------- state mutation --------------------------

    fn set_action(&mut self, idx: usize, action: RebaseAction) {
        // Leaving the Reword state: save the in-flight inline-editor
        // buffer into new_message without forcing the action back to
        // Reword (commit_reword would, since that's its job). The cached
        // new_message survives the cycle so coming back to Reword later
        // restores the edit.
        if action != RebaseAction::Reword
            && matches!(&self.reword_editing, Some(r) if r.item_idx == idx)
        {
            if let Some(r) = self.reword_editing.take() {
                if let Some(it) = self.items.get_mut(r.item_idx) {
                    it.new_message = Some(r.buffer);
                }
            }
        }
        if let Some(it) = self.items.get_mut(idx) {
            it.action = action;
        }
    }

    fn cycle_action(&mut self, idx: usize, forward: bool) {
        if let Some(it) = self.items.get(idx) {
            let next = if forward {
                it.action.cycle_next()
            } else {
                it.action.cycle_prev()
            };
            // Go through set_action so it picks up the reword-editor
            // cleanup when the new action is no longer Reword.
            self.set_action(idx, next);
            // Landing on Reword opens the inline editor immediately
            // (same flow as `r` or clicking), regardless of which input
            // device started the cycle.
            if next == RebaseAction::Reword {
                self.start_reword(idx);
            }
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as isize;
        let mut next = self.selected as isize + delta;
        if next < 0 {
            next = 0;
        }
        if next >= len {
            next = len - 1;
        }
        let next = next as usize;
        if next != self.selected {
            // Leaving a row mid-reword: commit the buffer so the typed
            // text isn't silently lost when the user navigates away.
            self.commit_reword();
        }
        self.selected = next;
        self.scroll_delta = 0;
    }

    fn reorder(&mut self, delta: isize) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as isize;
        let from = self.selected as isize;
        let to = from + delta;
        if to < 0 || to >= len {
            return;
        }
        self.items.swap(from as usize, to as usize);
        self.selected = to as usize;
        self.scroll_delta = 0;
    }

    fn start_reword(&mut self, idx: usize) {
        if let Some(it) = self.items.get(idx) {
            let initial = it.new_message.clone().unwrap_or_else(|| it.subject.clone());
            // cursor is a BYTE index into `buffer` (matches dialog.rs and
            // String::insert/remove which both want byte indices). Starts
            // at the end so the user immediately appends.
            let cursor = initial.len();
            self.reword_editing = Some(RewordEdit {
                item_idx: idx,
                buffer: initial,
                cursor,
            });
        }
    }

    fn commit_reword(&mut self) {
        if let Some(r) = self.reword_editing.take() {
            if let Some(it) = self.items.get_mut(r.item_idx) {
                it.new_message = Some(r.buffer.clone());
                // Picking a reword auto-applies the Reword action so the
                // change in message actually takes effect on Apply.
                if it.action != RebaseAction::Reword {
                    it.action = RebaseAction::Reword;
                }
            }
        }
    }

    fn cancel_reword(&mut self) {
        self.reword_editing = None;
    }

    fn apply(&mut self) {
        if self.items.is_empty() {
            self.last_error = Some("No commits to rebase.".to_string());
            return;
        }
        // Flush any in-flight reword buffer before applying.
        self.commit_reword();
        // Clear any stale error from a previous attempt — the result of
        // this run will overwrite it (or stay cleared on success).
        self.last_error = None;

        match crate::git::rebase::apply_rebase(&self.repo_path, &self.base_hash, &self.items) {
            Ok(crate::git::rebase::RebaseOutcome::Clean) => {
                // Only on a fully clean rebase do we close the view — every
                // other branch keeps the user here so they can see the error
                // and act on it (abort, retry, etc.).
                self.tx
                    .send(AppEvent::NotifySuccess("Rebase complete".into()));
                self.tx.send(AppEvent::CloseInteractiveRebase);
            }
            Ok(crate::git::rebase::RebaseOutcome::Paused(msg)) => {
                // Surface only the first non-hint line of git's output. The
                // recovery advice lives in the banner's footline so we don't
                // duplicate it here.
                let summary = msg
                    .lines()
                    .find(|l| {
                        let t = l.trim();
                        !t.is_empty() && !t.starts_with("hint:")
                    })
                    .map(|l| {
                        // Strip leading "Rebasing (N/M)" if git smushed it in.
                        let trimmed = l.trim();
                        if let Some(rest) = trimmed.strip_prefix("Rebasing (") {
                            if let Some(idx) = rest.find(')') {
                                return rest[idx + 1..].trim().to_string();
                            }
                        }
                        trimmed.to_string()
                    })
                    .unwrap_or_else(|| "rebase stopped".to_string());
                self.last_error = Some(summary);
            }
            Ok(crate::git::rebase::RebaseOutcome::AlreadyInProgress) => {
                self.last_error = Some("A previous rebase is still in progress.".to_string());
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
            }
        }
    }

    /// `git rebase --abort` — clears a stale rebase state so the user can
    /// retry from a clean repo. Triggered by `A` in the editor. Silently
    /// no-ops if there's nothing to abort (the footer hint already gates
    /// this, but a stray `A` shouldn't crash anything).
    fn abort_previous(&mut self) {
        if !crate::git::rebase::rebase_in_progress(&self.repo_path) {
            return;
        }
        match crate::git::rebase::abort_rebase(&self.repo_path) {
            Ok(()) => {
                self.tx.send(AppEvent::NotifySuccess(
                    "Previous rebase aborted — you can try again now.".into(),
                ));
                self.last_error = None;
            }
            Err(e) => {
                self.last_error = Some(format!("Abort failed: {}", e));
            }
        }
    }

    /// Resume-mode handlers — these run only when the view is showing the
    /// recovery panel for an in-progress rebase.

    fn continue_rebase(&mut self) {
        match crate::git::rebase::continue_rebase(&self.repo_path) {
            Ok(crate::git::rebase::RebaseOutcome::Clean) => {
                self.tx
                    .send(AppEvent::NotifySuccess("Rebase complete".into()));
                self.tx.send(AppEvent::CloseInteractiveRebase);
            }
            Ok(crate::git::rebase::RebaseOutcome::Paused(msg)) => {
                let summary = msg
                    .lines()
                    .find(|l| {
                        let t = l.trim();
                        !t.is_empty() && !t.starts_with("hint:")
                    })
                    .map(|l| l.trim().to_string())
                    .unwrap_or_else(|| "still paused".to_string());
                self.last_error = Some(summary);
                // Re-read state so the panel reflects new done/remaining.
                self.resume = crate::git::rebase::read_resume_state(&self.repo_path);
            }
            Ok(crate::git::rebase::RebaseOutcome::AlreadyInProgress) => {
                // Shouldn't happen on continue, but be defensive.
                self.last_error = Some("Rebase state is in an unexpected place.".to_string());
            }
            Err(e) => self.last_error = Some(e),
        }
    }

    fn skip_rebase(&mut self) {
        match crate::git::rebase::skip_rebase(&self.repo_path) {
            Ok(crate::git::rebase::RebaseOutcome::Clean) => {
                self.tx.send(AppEvent::NotifySuccess(
                    "Rebase complete (skipped commit)".into(),
                ));
                self.tx.send(AppEvent::CloseInteractiveRebase);
            }
            Ok(crate::git::rebase::RebaseOutcome::Paused(msg)) => {
                let summary = msg
                    .lines()
                    .find(|l| {
                        let t = l.trim();
                        !t.is_empty() && !t.starts_with("hint:")
                    })
                    .map(|l| l.trim().to_string())
                    .unwrap_or_else(|| "still paused".to_string());
                self.last_error = Some(summary);
                self.resume = crate::git::rebase::read_resume_state(&self.repo_path);
            }
            Ok(crate::git::rebase::RebaseOutcome::AlreadyInProgress) => {
                self.last_error = Some("Rebase state is in an unexpected place.".to_string());
            }
            Err(e) => self.last_error = Some(e),
        }
    }

    fn abort_resume(&mut self) {
        match crate::git::rebase::abort_rebase(&self.repo_path) {
            Ok(()) => {
                self.tx
                    .send(AppEvent::NotifySuccess("Rebase aborted".into()));
                self.tx.send(AppEvent::CloseInteractiveRebase);
            }
            Err(e) => self.last_error = Some(format!("Abort failed: {}", e)),
        }
    }

    fn cancel(&mut self) {
        // If the user is exiting while a rebase is still half-applied, warn
        // loudly so they know recovery is on them. We don't auto-abort
        // (could destroy work they intend to resolve via the conflict
        // editor), but we make the next step obvious.
        if crate::git::rebase::rebase_in_progress(&self.repo_path) {
            self.tx.send(AppEvent::NotifyWarn(
                "Rebase still in progress — reopen the editor and press A, \
                 or run `git rebase --abort` in the shell to roll it back."
                    .into(),
            ));
        }
        self.tx.send(AppEvent::CloseInteractiveRebase);
    }

    // -------------------------- event handling --------------------------

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        use ratatui::crossterm::event::KeyCode;

        // Resume mode hijacks input — only C / S / A / Esc make sense
        // when there's an in-progress rebase to recover.
        if self.resume.is_some() {
            match key.code {
                KeyCode::Char('c') | KeyCode::Char('C') => self.continue_rebase(),
                KeyCode::Char('s') | KeyCode::Char('S') => self.skip_rebase(),
                KeyCode::Char('a') | KeyCode::Char('A') => self.abort_resume(),
                KeyCode::Esc => self.cancel(),
                _ => {}
            }
            return;
        }

        // Reword inline editor takes ALL input until Enter or Esc.
        // Mirrors the dialog-input keymap: Ctrl+H / Ctrl+W / Ctrl+Backspace
        // delete the word to the left, Ctrl+Delete the word to the right,
        // Ctrl+Left/Right jump by word.
        if self.reword_editing.is_some() {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Enter => {
                    self.commit_reword();
                    return;
                }
                KeyCode::Esc => {
                    self.cancel_reword();
                    return;
                }
                // Ctrl+H is the terminal alias for Ctrl+Backspace (ASCII ^H).
                KeyCode::Char('h') if ctrl => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        let new_pos = word_left(&r.buffer, r.cursor);
                        r.buffer.drain(new_pos..r.cursor);
                        r.cursor = new_pos;
                    }
                    return;
                }
                // Ctrl+W: Unix word-delete-left.
                KeyCode::Char('w') if ctrl => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        let new_pos = word_left(&r.buffer, r.cursor);
                        r.buffer.drain(new_pos..r.cursor);
                        r.cursor = new_pos;
                    }
                    return;
                }
                KeyCode::Backspace if ctrl => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        let new_pos = word_left(&r.buffer, r.cursor);
                        r.buffer.drain(new_pos..r.cursor);
                        r.cursor = new_pos;
                    }
                    return;
                }
                KeyCode::Backspace => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        if r.cursor > 0 {
                            // Walk back to the previous char boundary so
                            // multibyte UTF-8 chars are removed whole.
                            let mut prev = r.cursor - 1;
                            while !r.buffer.is_char_boundary(prev) {
                                prev -= 1;
                            }
                            r.buffer.drain(prev..r.cursor);
                            r.cursor = prev;
                        }
                    }
                    return;
                }
                KeyCode::Delete if ctrl => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        let new_pos = word_right(&r.buffer, r.cursor);
                        r.buffer.drain(r.cursor..new_pos);
                    }
                    return;
                }
                KeyCode::Delete => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        if r.cursor < r.buffer.len() {
                            let mut next = r.cursor + 1;
                            while next < r.buffer.len() && !r.buffer.is_char_boundary(next) {
                                next += 1;
                            }
                            r.buffer.drain(r.cursor..next);
                        }
                    }
                    return;
                }
                KeyCode::Left if ctrl => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        r.cursor = word_left(&r.buffer, r.cursor);
                    }
                    return;
                }
                KeyCode::Left => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        if r.cursor > 0 {
                            let mut prev = r.cursor - 1;
                            while !r.buffer.is_char_boundary(prev) {
                                prev -= 1;
                            }
                            r.cursor = prev;
                        }
                    }
                    return;
                }
                KeyCode::Right if ctrl => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        r.cursor = word_right(&r.buffer, r.cursor);
                    }
                    return;
                }
                KeyCode::Right => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        if r.cursor < r.buffer.len() {
                            let mut next = r.cursor + 1;
                            while next < r.buffer.len() && !r.buffer.is_char_boundary(next) {
                                next += 1;
                            }
                            r.cursor = next;
                        }
                    }
                    return;
                }
                KeyCode::Home => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        r.cursor = 0;
                    }
                    return;
                }
                KeyCode::End => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        r.cursor = r.buffer.len();
                    }
                    return;
                }
                KeyCode::Char(c) => {
                    if let Some(r) = self.reword_editing.as_mut() {
                        r.buffer.insert(r.cursor, c);
                        r.cursor += c.len_utf8();
                    }
                    return;
                }
                _ => return,
            }
        }

        // Space toggles "grab" mode on the selected row — when grabbed,
        // arrows move the commit instead of moving the cursor.
        if matches!(key.code, KeyCode::Char(' ')) {
            self.grabbed = !self.grabbed;
            return;
        }

        // Uppercase A → abort a stale previous rebase. Destructive, so
        // gated behind Shift to avoid collision with the action shortcuts.
        if matches!(key.code, KeyCode::Char('A')) {
            self.abort_previous();
            return;
        }

        // Single-letter action shortcuts are disabled while grabbed so the
        // user can't accidentally change action mid-move (would surprise).
        if !self.grabbed {
            if let KeyCode::Char(c) = key.code {
                if let Some(action) = RebaseAction::from_key(c) {
                    self.set_action(self.selected, action);
                    // Reword keypress also opens the inline editor right away.
                    if action == RebaseAction::Reword {
                        self.start_reword(self.selected);
                    }
                    return;
                }
            }
        }

        match event_with_count.event {
            UserEvent::Confirm => {
                // Enter on a grabbed row drops the grab (commit it in place).
                if self.grabbed {
                    self.grabbed = false;
                } else {
                    self.apply();
                }
            }
            UserEvent::Cancel | UserEvent::Close => {
                // Esc releases the grab first; second Esc cancels the editor.
                if self.grabbed {
                    self.grabbed = false;
                } else {
                    self.cancel();
                }
            }
            UserEvent::NavigateUp => {
                if self.grabbed {
                    self.reorder(-1);
                } else {
                    self.move_selection(-1);
                }
            }
            UserEvent::NavigateDown => {
                if self.grabbed {
                    self.reorder(1);
                } else {
                    self.move_selection(1);
                }
            }
            UserEvent::NavigateLeft => {
                if !self.grabbed {
                    self.cycle_action(self.selected, false);
                }
            }
            UserEvent::NavigateRight => {
                if !self.grabbed {
                    self.cycle_action(self.selected, true);
                }
            }
            UserEvent::ScrollUp => {
                self.scroll_delta = self.scroll_delta.saturating_sub(1);
            }
            UserEvent::ScrollDown => {
                self.scroll_delta = self.scroll_delta.saturating_add(1);
            }
            UserEvent::PageUp => self.move_selection(-10),
            UserEvent::PageDown => self.move_selection(10),
            UserEvent::GoToTop => self.selected = 0,
            UserEvent::GoToBottom => self.selected = self.items.len().saturating_sub(1),
            _ => {}
        }
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        // Click anywhere on a row cycles its action. Mirrors the single-
        // cursor UX: there's no separate "select" gesture — the mouse
        // already drives `selected` via handle_mouse_move, so clicking
        // means "act on this row". If the new action is Reword, open the
        // inline editor right away (same flow as pressing `r`).
        let hit = self
            .row_rects
            .iter()
            .find(|r| r.row == row && col >= r.full_x && col < r.full_x_end)
            .cloned();
        if let Some(hit) = hit {
            self.selected = hit.item_idx;
            // cycle_action now opens the inline editor itself when it
            // lands on Reword — no need to duplicate that here.
            self.cycle_action(hit.item_idx, true);
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        // Single-cursor model: the mouse drives `selected` directly so the
        // keyboard arrows and the pointer never end up on different rows
        // (no more "two cursors with conflicting highlights"). Re-anchor
        // scroll so the new selection stays centred just like a keyboard
        // arrow move would.
        let hit = self
            .row_rects
            .iter()
            .find(|r| r.row == row && col >= r.full_x && col < r.full_x_end)
            .map(|r| r.item_idx);
        if let Some(idx) = hit {
            if idx != self.selected {
                // Same commit-on-leave logic as move_selection so the
                // typed reword isn't lost when the mouse drifts to
                // another row.
                self.commit_reword();
                self.selected = idx;
                self.scroll_delta = 0;
            }
        }
    }

    // -------------------------- rendering --------------------------

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.row_rects.clear();
        self.reword_screen_pos = None;

        // Reserve a banner block for `last_error` when set. Height covers:
        // - 1 top border
        // - N wrapped body rows (>=1, capped to 6)
        // - 1 blank gutter row
        // - 1 hint row
        // - 1 bottom border
        // - 1 spacing row below the block (visual gutter)
        let banner_height: u16 = match &self.last_error {
            Some(msg) if area.width > 6 => {
                let inner_w = area.width.saturating_sub(4) as usize; // borders + 1 px padding each side
                let body_rows: usize = msg
                    .lines()
                    .map(|l| (l.chars().count() + inner_w.max(1) - 1) / inner_w.max(1).max(1))
                    .map(|n| n.max(1))
                    .sum();
                let body = body_rows.clamp(1, 6) as u16;
                body + 5 // 2 borders + 1 blank + 1 hint + 1 outer gutter
            }
            _ => 0,
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

        if self.resume.is_some() {
            self.render_resume(f, body_area);
        } else {
            match self.mode {
                RebaseViewMode::Compact => self.render_compact(f, body_area),
                RebaseViewMode::Inline => self.render_inline(f, body_area),
                RebaseViewMode::Split => self.render_split(f, body_area),
            }
        }

        // Place the terminal cursor on the reword editor if it's visible.
        // Honour the user's CursorType preference (Native = real terminal
        // cursor, Virtual = paint a glyph in the buffer) so the inline
        // editor behaves like every other text input in the app.
        if let (Some((buf_x, y)), Some(r)) = (self.reword_screen_pos, self.reword_editing.as_ref())
        {
            // cursor stores a BYTE index; display column = visible width
            // of the prefix slice.
            let prefix = &r.buffer.as_str()[..r.cursor.min(r.buffer.len())];
            let cx = buf_x + console::measure_text_width(prefix) as u16;
            let cy = y;
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

    /// Persistent error banner shown above the body when an apply attempt
    /// failed. Stays visible until a subsequent apply succeeds or the user
    /// presses `A` to abort the previous rebase. Body holds git's message
    /// (wrapped), footer row gives the available recovery action.
    fn render_error_banner(&self, f: &mut Frame, area: Rect) {
        if area.height < 3 {
            return;
        }
        let theme = &self.ctx.color_theme;
        let Some(msg) = self.last_error.as_ref() else {
            return;
        };
        // Drop the bottom row of the reserved area as visual gutter so the
        // banner doesn't touch the list block below it.
        let block_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: area.height.saturating_sub(1).max(3),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.status_error_fg))
            .title(Line::from(Span::styled(
                " ✗ Apply failed ",
                Style::default()
                    .fg(theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(block_area);
        f.render_widget(block, block_area);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        // Split inner area: body (top, leaving 2 rows at bottom for the
        // blank gutter + hint). If we don't have at least 3 rows we just
        // show the body and skip the hint.
        let hint_text: &str = if crate::git::rebase::rebase_in_progress(&self.repo_path) {
            "Press A to abort  •  Esc exits (rebase stays half-applied)"
        } else {
            "Adjust the plan and press Enter to retry"
        };

        if inner.height >= 3 {
            let body_height = inner.height.saturating_sub(2);
            let body_area = Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: body_height,
            };
            let body = Paragraph::new(Span::styled(
                msg.clone(),
                Style::default().fg(theme.status_error_fg),
            ))
            .wrap(ratatui::widgets::Wrap { trim: true });
            f.render_widget(body, body_area);

            // Skip y + body_height (blank gutter), render hint on the row
            // after — keeps the error and the action visually separated.
            let hint_area = Rect {
                x: inner.x,
                y: inner.y + body_height + 1,
                width: inner.width,
                height: 1,
            };
            let hint = Paragraph::new(Span::styled(
                hint_text,
                Style::default()
                    .fg(theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            ));
            f.render_widget(hint, hint_area);
        } else {
            // Cramped — body only.
            let body = Paragraph::new(Span::styled(
                msg.clone(),
                Style::default().fg(theme.status_error_fg),
            ))
            .wrap(ratatui::widgets::Wrap { trim: true });
            f.render_widget(body, inner);
        }
    }

    /// Resume panel — shown when we open the view while a previous rebase
    /// is still paused mid-flight. Lists what git has already done, what's
    /// still queued, and the recovery actions (Continue / Skip / Abort).
    fn render_resume(&self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let Some(state) = self.resume.as_ref() else {
            return;
        };
        let title = if let Some(sha) = &state.stopped_sha {
            let short = sha.chars().take(7).collect::<String>();
            format!(" Rebase paused at {} ", short)
        } else {
            " Rebase paused ".to_string()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(Span::styled(
                title,
                Style::default()
                    .fg(theme.status_warn_fg)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        f.render_widget(block, area);
        if inner.height == 0 {
            return;
        }

        // Build the body as a Text of multiple lines.
        let mut lines: Vec<Line<'static>> = Vec::new();
        if let Some(h) = &state.headline {
            lines.push(Line::from(vec![
                Span::styled(" ! ", Style::default().fg(theme.status_warn_fg)),
                Span::styled(
                    h.clone(),
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(""));
        }

        lines.push(Line::from(Span::styled(
            format!(" Done ({})", state.done.len()),
            Style::default()
                .fg(theme.detail_label_fg)
                .add_modifier(Modifier::BOLD),
        )));
        if state.done.is_empty() {
            lines.push(Line::from(Span::styled(
                "   (no steps applied yet)",
                Style::default().fg(theme.detail_label_fg),
            )));
        } else {
            for step in &state.done {
                lines.push(resume_step_line(theme, step, "  ✓"));
            }
        }
        lines.push(Line::from(""));

        lines.push(Line::from(Span::styled(
            format!(" Remaining ({})", state.remaining.len()),
            Style::default()
                .fg(theme.detail_label_fg)
                .add_modifier(Modifier::BOLD),
        )));
        if state.remaining.is_empty() {
            lines.push(Line::from(Span::styled(
                "   (no steps queued)",
                Style::default().fg(theme.detail_label_fg),
            )));
        } else {
            for (i, step) in state.remaining.iter().enumerate() {
                let marker = if i == 0 { "  ▸" } else { "  •" };
                lines.push(resume_step_line(theme, step, marker));
            }
        }
        lines.push(Line::from(""));

        lines.push(Line::from(vec![
            Span::styled(
                " C ",
                Style::default()
                    .bg(theme.status_success_fg)
                    .fg(theme.bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" continue   ", Style::default().fg(theme.fg)),
            Span::styled(
                " S ",
                Style::default()
                    .bg(theme.status_warn_fg)
                    .fg(theme.bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" skip current   ", Style::default().fg(theme.fg)),
            Span::styled(
                " A ",
                Style::default()
                    .bg(theme.status_error_fg)
                    .fg(theme.bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" abort", Style::default().fg(theme.fg)),
        ]));

        let para = Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false });
        f.render_widget(para, inner);
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let icon = Span::styled(
            "↻ ",
            Style::default()
                .fg(theme.status_warn_fg)
                .add_modifier(Modifier::BOLD),
        );
        let title = if self.resume.is_some() {
            Line::from(vec![
                Span::raw("  "),
                icon,
                Span::styled(
                    "Interactive rebase ",
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "— resume in-progress rebase",
                    Style::default().fg(theme.status_warn_fg),
                ),
            ])
        } else {
            let short_base = &self.base_hash[..self.base_hash.len().min(7)];
            Line::from(vec![
                Span::raw("  "),
                icon,
                Span::styled(
                    "Interactive rebase ",
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                ),
                Span::styled("from ", Style::default().fg(theme.detail_label_fg)),
                Span::styled(
                    short_base.to_string(),
                    Style::default().fg(theme.list_hash_fg),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{} commits", self.items.len()),
                    Style::default().fg(theme.detail_label_fg),
                ),
            ])
        };
        let divider = Line::from(Span::styled(
            "─".repeat(area.width as usize),
            Style::default().fg(theme.divider_fg),
        ));
        f.render_widget(Paragraph::new(vec![title, divider]), area);
    }

    /// Layout A — single dense list, like the CLI todo file.
    fn render_compact(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(Span::styled(
                " Todo ",
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Build all rows + an inline reword editor line for the row being
        // reworded (so the editor is visible in Compact AND Split layouts,
        // not just Inline). Then apply scroll like render_inline does.
        let mut all_lines: Vec<Line<'static>> = Vec::new();
        let mut row_screen_rows: Vec<u16> = Vec::new();
        let mut action_ranges: Vec<(u16, u16)> = Vec::new();
        let mut reword_doc_row: Option<u16> = None;
        for (i, item) in self.items.iter().enumerate() {
            let editing = matches!(&self.reword_editing, Some(r) if r.item_idx == i);
            row_screen_rows.push(all_lines.len() as u16);
            let (line, action_range) = self.render_row(i, item, inner.width, false);
            action_ranges.push(action_range);
            all_lines.push(line);
            if editing {
                reword_doc_row = Some(all_lines.len() as u16);
                all_lines.push(self.render_inline_reword_editor(inner.width));
            }
        }

        let visible_height = inner.height as usize;
        let total = all_lines.len();
        let start = if total <= visible_height {
            0
        } else {
            let sel_row = row_screen_rows.get(self.selected).copied().unwrap_or(0) as usize;
            let want = sel_row.saturating_sub(visible_height / 4);
            ((want as i32) + self.scroll_delta)
                .max(0)
                .min(total.saturating_sub(visible_height) as i32) as usize
        };
        let end = (start + visible_height).min(total);

        // Map document-row → screen-row for mouse hit-tests.
        for (i, &doc_row) in row_screen_rows.iter().enumerate() {
            let r = doc_row as usize;
            if r >= start && r < end {
                let screen = inner.y + (r - start) as u16;
                let (acs, ace) = action_ranges[i];
                self.row_rects.push(RowRect {
                    item_idx: i,
                    row: screen,
                    action_col_start: inner.x + acs,
                    action_col_end: inner.x + ace,
                    full_x: inner.x,
                    full_x_end: inner.x + inner.width,
                });
            }
        }

        let mut lines: Vec<Line<'static>> = all_lines[start..end].to_vec();
        while lines.len() < visible_height {
            lines.push(Line::raw(""));
        }
        f.render_widget(Paragraph::new(lines), inner);

        // Capture the reword input's screen position so the top-level
        // render() can place the real terminal cursor on it. Same offset
        // as render_inline: 8 spaces + "✎ " = 10 cells from inner.x.
        if let Some(doc_row) = reword_doc_row {
            let r = doc_row as usize;
            if r >= start && r < end {
                let screen_y = inner.y + (r - start) as u16;
                let buffer_x = inner.x + 10;
                self.reword_screen_pos = Some((buffer_x, screen_y));
            }
        }
    }

    /// Layout B — GitKraken style: each row + italic description + inline
    /// reword editor when the row is being reworded.
    fn render_inline(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(Span::styled(
                " Todo ",
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<Line<'static>> = Vec::new();

        // Build all lines first to know total height, then apply scroll.
        // Track the document-row of each row + the row of the active
        // reword editor (if any) so we can place the terminal cursor on
        // the right cell after rendering.
        let mut row_screen_rows: Vec<u16> = Vec::new();
        let mut action_ranges: Vec<(u16, u16)> = Vec::new();
        let mut reword_doc_row: Option<u16> = None;
        for (i, item) in self.items.iter().enumerate() {
            let editing = matches!(&self.reword_editing, Some(r) if r.item_idx == i);
            row_screen_rows.push(lines.len() as u16);
            let (line, action_range) = self.render_row(i, item, inner.width, true);
            action_ranges.push(action_range);
            lines.push(line);
            lines.push(self.render_inline_description(item));
            if editing {
                reword_doc_row = Some(lines.len() as u16);
                lines.push(self.render_inline_reword_editor(inner.width));
            }
        }

        let visible_height = inner.height as usize;
        let total = lines.len();
        let start = if total <= visible_height {
            0
        } else {
            // Anchor on the selected row.
            let sel_row = row_screen_rows.get(self.selected).copied().unwrap_or(0) as usize;
            let want = sel_row.saturating_sub(visible_height / 4);
            ((want as i32) + self.scroll_delta)
                .max(0)
                .min(total.saturating_sub(visible_height) as i32) as usize
        };
        let end = (start + visible_height).min(total);

        // Capture row rects for mouse, mapping document-row to screen-row.
        for (i, &doc_row) in row_screen_rows.iter().enumerate() {
            let r = doc_row as usize;
            if r >= start && r < end {
                let screen = inner.y + (r - start) as u16;
                let (acs, ace) = action_ranges[i];
                self.row_rects.push(RowRect {
                    item_idx: i,
                    row: screen,
                    action_col_start: inner.x + acs,
                    action_col_end: inner.x + ace,
                    full_x: inner.x,
                    full_x_end: inner.x + inner.width,
                });
            }
        }

        let mut visible: Vec<Line<'static>> = lines[start..end].to_vec();
        while visible.len() < visible_height {
            visible.push(Line::raw(""));
        }
        f.render_widget(Paragraph::new(visible), inner);

        // If the reword editor is on-screen, capture where its buffer
        // starts (8 padding spaces + "✎ " prefix = 10 cells from inner.x)
        // so the post-render pass can place the terminal cursor at the
        // user's exact insertion point.
        self.reword_screen_pos = None;
        if let Some(doc_row) = reword_doc_row {
            let r = doc_row as usize;
            if r >= start && r < end {
                let screen_y = inner.y + (r - start) as u16;
                let buffer_x = inner.x + 10;
                self.reword_screen_pos = Some((buffer_x, screen_y));
            }
        }
    }

    /// Layout C — todo list on top + Result preview on the bottom.
    fn render_split(&mut self, f: &mut Frame, area: Rect) {
        let [todo_area, result_area] =
            Layout::vertical([Constraint::Percentage(62), Constraint::Percentage(38)]).areas(area);
        self.render_compact(f, todo_area);

        let theme = &self.ctx.color_theme;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(Span::styled(
                " Result preview ",
                Style::default()
                    .fg(theme.status_success_fg)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(result_area);
        f.render_widget(block, result_area);
        let lines = self.build_result_preview(inner.width);
        let visible_height = inner.height as usize;
        let mut visible: Vec<Line<'static>> = lines.into_iter().take(visible_height).collect();
        while visible.len() < visible_height {
            visible.push(Line::raw(""));
        }
        f.render_widget(Paragraph::new(visible), inner);
    }

    fn compute_scroll_start(&self, visible_height: usize) -> usize {
        let total = self.items.len();
        if total <= visible_height {
            return 0;
        }
        let want = self.selected.saturating_sub(visible_height / 4);
        ((want as i32) + self.scroll_delta)
            .max(0)
            .min(total.saturating_sub(visible_height) as i32) as usize
    }

    /// Build a single rebase-todo row. Returns the Line and the column range
    /// of the action tag (for click hit-testing).
    fn render_row(
        &self,
        idx: usize,
        item: &RebaseItem,
        width: u16,
        verbose: bool,
    ) -> (Line<'static>, (u16, u16)) {
        let theme = &self.ctx.color_theme;
        let is_selected = idx == self.selected;

        // Single ▶ cursor on the keyboard-selected row. Mouse hover now
        // sets `self.selected` directly in handle_mouse_move (single
        // source of truth), so no separate hover glyph is needed.
        let cursor_glyph = if is_selected { "▶ " } else { "  " };
        let cursor_span = Span::styled(
            cursor_glyph.to_string(),
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD),
        );

        // [Action] tag with theme-aware outline/filled style.
        let action_text = format!("[{}]", item.action.label());
        let action_span = Span::styled(action_text.clone(), action_style(item.action, theme));

        let short_hash = &item.commit_hash[..item.commit_hash.len().min(7)];
        let hash_span = Span::styled(
            format!("  {}  ", short_hash),
            Style::default().fg(theme.list_hash_fg),
        );

        // Show the reword-edited message ONLY while the action is still
        // Reword. If the user cycled to Squash/Pick/etc. after editing,
        // the rebase won't apply that message, so displaying it would
        // mislead. The buffer stays cached in `new_message` though, so
        // cycling back to Reword later restores the previous edit.
        let subject = if item.action == RebaseAction::Reword {
            item.new_message
                .as_deref()
                .unwrap_or(&item.subject)
                .to_string()
        } else {
            item.subject.clone()
        };
        let subject_fg = if item.action == RebaseAction::Drop {
            theme.divider_fg
        } else if is_selected {
            theme.fg
        } else {
            theme.list_commit_message_fg
        };
        let subject_modifier = if item.action == RebaseAction::Drop {
            Modifier::DIM | Modifier::CROSSED_OUT
        } else if is_selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        };

        let mut spans: Vec<Span<'static>> = vec![cursor_span];
        let action_col_start = cursor_glyph.chars().count() as u16;
        spans.push(action_span);
        let action_col_end = action_col_start + action_text.chars().count() as u16;
        spans.push(hash_span);
        spans.push(Span::styled(
            subject.clone(),
            Style::default()
                .fg(subject_fg)
                .add_modifier(subject_modifier),
        ));
        if verbose {
            // Use console::measure_text_width to handle multi-cell glyphs
            // (▶ is single-cell, but the cursor / action tag can vary by
            // theme/font — be defensive). Also fixes a hardcoded "9" that
            // under-counted the "  {hash}  " span (actually 11 chars) and
            // pushed the meta column past the right border by 2 cells.
            let measured = |s: &str| console::measure_text_width(s) as u16;
            let used: u16 = measured(cursor_glyph)
                + measured(&action_text)
                + measured(&format!("  {}  ", short_hash))
                + measured(&subject);
            // Author + " · " separator + date — coloured the same way as
            // the commit list (list_name_fg / list_date_fg) so the rebase
            // editor reads like a focused slice of that view.
            let sep = "  ";
            let middot = " · ";
            let meta_w =
                measured(sep) + measured(&item.author) + measured(middot) + measured(&item.date);
            // Reserve at least 1 column on the right so the row never
            // touches the border (which clips wide glyphs).
            let avail = width
                .saturating_sub(used)
                .saturating_sub(meta_w)
                .saturating_sub(1);
            if avail > 0 {
                spans.push(Span::raw(" ".repeat(avail as usize)));
            }
            spans.push(Span::raw(sep.to_string()));
            spans.push(Span::styled(
                item.author.clone(),
                Style::default().fg(theme.list_name_fg),
            ));
            spans.push(Span::styled(
                middot.to_string(),
                Style::default().fg(theme.divider_fg),
            ));
            spans.push(Span::styled(
                item.date.clone(),
                Style::default().fg(theme.list_date_fg),
            ));
        }

        // Row background — only the cursor row gets a bg, never the hover.
        // Hover is shown via the ▷ glyph in the gutter (see cursor_glyph
        // above) so the user can never see two rows highlighted at once.
        // - Grabbed cursor → list_compare_marked_bg/fg (saturated accent).
        // - Cursor         → list_selected_bg/fg.
        // - Hovered-only / idle → no bg.
        let mut line = Line::from(spans);
        if is_selected && self.grabbed {
            line.style = Style::default()
                .bg(theme.list_compare_marked_bg)
                .fg(theme.list_compare_marked_fg)
                .add_modifier(Modifier::BOLD);
        } else if is_selected {
            line.style = Style::default()
                .bg(theme.list_selected_bg)
                .fg(theme.list_selected_fg);
        }
        (line, (action_col_start, action_col_end))
    }

    fn render_inline_description(&self, item: &RebaseItem) -> Line<'static> {
        let theme = &self.ctx.color_theme;
        let glyph = match item.action {
            RebaseAction::Pick => "└─",
            RebaseAction::Reword => "└─",
            RebaseAction::Edit => "└─",
            RebaseAction::Squash => "└─ ↑",
            RebaseAction::Fixup => "└─ ↑",
            RebaseAction::Drop => "└─ ✗",
        };
        Line::from(vec![
            Span::raw("      "),
            Span::styled(
                format!("{} {}", glyph, item.action.description()),
                Style::default()
                    .fg(theme.detail_label_fg)
                    .add_modifier(Modifier::ITALIC),
            ),
        ])
    }

    fn render_inline_reword_editor(&self, width: u16) -> Line<'static> {
        let theme = &self.ctx.color_theme;
        let editor = match &self.reword_editing {
            Some(r) => r,
            None => return Line::raw(""),
        };
        let content_width = (width as usize).saturating_sub(12);
        let buffer = &editor.buffer;
        let padded = format!("{:<width$}", buffer, width = content_width);
        Line::from(vec![
            Span::raw("        "),
            Span::styled("✎ ", Style::default().fg(theme.status_info_fg)),
            Span::styled(
                padded,
                Style::default().fg(theme.fg).bg(theme.list_selected_bg),
            ),
        ])
    }

    fn build_result_preview(&self, _width: u16) -> Vec<Line<'static>> {
        let theme = &self.ctx.color_theme;
        let mut out: Vec<Line<'static>> = Vec::new();
        let mut last_carries_into_next = false;
        for item in &self.items {
            // Only Reword applies the edited message in the final history.
            // Show the original subject for every other action so the
            // preview reflects what git will actually do.
            let display_subject = if item.action == RebaseAction::Reword {
                item.new_message
                    .as_deref()
                    .unwrap_or(&item.subject)
                    .to_string()
            } else {
                item.subject.clone()
            };
            match item.action {
                RebaseAction::Pick => {
                    out.push(result_line("● ", &display_subject, theme.fg, theme));
                    last_carries_into_next = false;
                }
                RebaseAction::Reword => {
                    out.push(result_line(
                        "● ",
                        &format!("{}   ✎ (reworded)", display_subject),
                        theme.fg,
                        theme,
                    ));
                    last_carries_into_next = false;
                }
                RebaseAction::Edit => {
                    out.push(result_line(
                        "● ",
                        &format!("{}   ✎ (will pause to amend)", display_subject),
                        theme.fg,
                        theme,
                    ));
                    last_carries_into_next = false;
                }
                RebaseAction::Squash => {
                    out.push(result_line(
                        "↑ ",
                        &format!("squashed into previous: {}", display_subject),
                        theme.status_info_fg,
                        theme,
                    ));
                    last_carries_into_next = true;
                }
                RebaseAction::Fixup => {
                    out.push(result_line(
                        "↑ ",
                        &format!("fixup into previous: {}", display_subject),
                        theme.status_info_fg,
                        theme,
                    ));
                    last_carries_into_next = true;
                }
                RebaseAction::Drop => {
                    out.push(result_line(
                        "✗ ",
                        &format!("{} (dropped)", display_subject),
                        theme.status_error_fg,
                        theme,
                    ));
                    last_carries_into_next = false;
                }
            }
        }
        if last_carries_into_next {
            out.push(Line::from(Span::styled(
                "  (some commits will be folded into previous siblings)",
                Style::default()
                    .fg(theme.detail_label_fg)
                    .add_modifier(Modifier::ITALIC),
            )));
        }
        out
    }
}

fn result_line(
    glyph: &str,
    text: &str,
    fg: Color,
    _theme: &crate::color::ColorTheme,
) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(glyph.to_string(), Style::default().fg(fg)),
        Span::styled(text.to_string(), Style::default().fg(fg)),
    ])
}

/// Style for an action tag. Themes only ship ~4 distinct hues for status
/// tokens (Tokyo Night collapses warn/hash/ref-tag onto yellow and error/
/// remote-branch onto red), so we use 4 colours + outlined/filled variants
/// to get 6 guaranteed-distinct visual styles in every theme:
///
/// Outlined (fg only) = "keeps the commit"            — Pick / Reword / Squash
/// Filled (fg on bg)  = "alters or removes"           — Edit / Fixup / Drop
///
/// - Pick   = green   outline
/// - Reword = yellow  outline   (will keep, edit message)
/// - Edit   = yellow  filled    (will pause to amend)
/// - Squash = cyan    outline   (merges up, keeps content)
/// - Fixup  = cyan    filled    (merges up, drops message)
/// - Drop   = red     filled    (destructive)
fn resume_step_line(
    theme: &crate::color::ColorTheme,
    step: &crate::git::rebase::ResumeStep,
    marker: &str,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            marker.to_string(),
            Style::default().fg(theme.status_warn_fg),
        ),
        Span::raw(" "),
        Span::styled(
            format!("[{}]", step.action.label()),
            action_style(step.action, theme),
        ),
        Span::raw(" "),
        Span::styled(
            step.short_hash.clone(),
            Style::default().fg(theme.list_hash_fg),
        ),
        Span::raw("  "),
        Span::styled(step.subject.clone(), Style::default().fg(theme.fg)),
    ])
}

fn action_style(action: RebaseAction, theme: &crate::color::ColorTheme) -> Style {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    match action {
        RebaseAction::Pick => bold.fg(theme.status_success_fg),
        RebaseAction::Reword => bold.fg(theme.status_warn_fg),
        RebaseAction::Edit => bold.fg(theme.bg).bg(theme.status_warn_fg),
        RebaseAction::Squash => bold.fg(theme.status_info_fg),
        RebaseAction::Fixup => bold.fg(theme.bg).bg(theme.status_info_fg),
        RebaseAction::Drop => bold.fg(theme.bg).bg(theme.status_error_fg),
    }
}

/// Walk byte-wise back from `cursor` past non-alphanumerics, then past
/// alphanumerics — same semantics as Ctrl+Backspace in modern editors.
/// Mirrors the helper in `view/dialog.rs`.
fn word_left(s: &str, cursor: usize) -> usize {
    let bytes = s.as_bytes();
    let mut i = cursor.min(bytes.len());
    while i > 0 && !bytes[i - 1].is_ascii_alphanumeric() {
        i -= 1;
    }
    while i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
        i -= 1;
    }
    i
}

fn word_right(s: &str, cursor: usize) -> usize {
    let bytes = s.as_bytes();
    let mut i = cursor.min(bytes.len());
    while i < bytes.len() && !bytes[i].is_ascii_alphanumeric() {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
        i += 1;
    }
    i
}
