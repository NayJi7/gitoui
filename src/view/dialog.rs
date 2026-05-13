use crate::{
    app::AppContext,
    config::CursorType,
    event::{AppEvent, DialogKind, GitAction, Sender, UserEvent, UserEventWithCount},
    view::View,
};
use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
    Frame,
};
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DialogElement {
    Input,
    SecondInput,
    BodyExpand,
    Checkbox(usize),
    Radio(usize),
    Validate,
    Cancel,
}

#[derive(Debug)]
pub struct DialogView<'a> {
    before: View<'a>,
    kind: DialogKind,
    input_value: String,
    input_cursor: usize,
    second_input_value: String,
    second_input_cursor: usize,
    dropdown_selected: usize,
    checkboxes: Vec<bool>,
    focused: DialogElement,
    hovered: Option<DialogElement>,
    dialog_area: Rect,
    inner_area: Rect,
    input_row: Option<usize>,
    second_input_row: Option<usize>,
    /// Row index (within inner_area) of the "[ ↵ add body ]" expand button.
    /// Only set for CommitWithMessage / AmendMessage when no body exists yet.
    body_expand_row: Option<usize>,
    checkbox_rows: Vec<usize>,
    radio_rows: Vec<usize>,
    button_row: usize,
    validate_col: (u16, u16),
    cancel_col: (u16, u16),
    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> DialogView<'a> {
    pub fn new(before: View<'a>, kind: DialogKind, ctx: Rc<AppContext>, tx: Sender) -> Self {
        let (checkboxes, dropdown_selected) = match &kind {
            DialogKind::AddTag { .. } => (vec![false], 0),
            DialogKind::CreateBranch { .. } => (vec![false], 0),
            DialogKind::CherryPick { .. } => (vec![false, false], 0),
            DialogKind::Merge { .. } => (vec![true, false, false], 0),
            // Both checkboxes default to false — matches git's native
            // behaviour (no -i, preserves author dates).
            DialogKind::Rebase { .. } => (vec![false, false], 0),
            DialogKind::Reset { .. } => (vec![], 1),
            DialogKind::Squash { .. } => (vec![], 0),
            // Default to "squash" — by far the most common choice when
            // merging a PR in a team workflow.
            DialogKind::MergePullRequest { .. } => (vec![], 1),
            DialogKind::PushBranch { .. } => (vec![false], 0),
            DialogKind::Checkout { .. } => (vec![false], 0),
            DialogKind::RenameBranch { .. } => (vec![], 0),
            DialogKind::DeleteBranch { .. } => (vec![false], 0),
            DialogKind::PullBranch { .. } => (vec![false], 0),
            DialogKind::CreateBranchFromStash { .. } => (vec![false], 0),
            DialogKind::StashWithMessage => (vec![false], 0),
            DialogKind::CommitWithMessage => (vec![false], 0),
            DialogKind::CleanUntracked => (vec![], 0),
            DialogKind::ConfirmPopStash { .. } => (vec![], 0),
            DialogKind::ConfirmDropStash { .. } => (vec![], 0),
            DialogKind::ConfirmDeleteComment { .. } => (vec![], 0),
            DialogKind::ConfirmPullRequestStateChange { .. } => (vec![], 0),
            DialogKind::ConfirmPullRequestDraftToggle { .. } => (vec![], 0),
            // Multi-select pickers — seed `checkboxes` from the
            // caller's selection so the dialog opens reflecting the
            // PR's current state.
            DialogKind::PullRequestLabels { selected, .. } => (selected.clone(), 0),
            DialogKind::PullRequestReviewers { selected, .. } => (selected.clone(), 0),
            DialogKind::AddWorktree => (vec![false], 0),
            // Two radio options: 0=Stash, 1=Discard. Default to Stash (safer).
            DialogKind::CheckoutHasLocalChanges { .. } => (vec![], 0),
            _ => (vec![], 0),
        };

        // Pre-fill input_value for dialogs that need pre-populated text
        let input_value = if let DialogKind::AmendMessage { current_message } = &kind {
            current_message.clone()
        } else if matches!(kind, DialogKind::AddWorktree) {
            "feat/".to_string()
        } else {
            String::new()
        };
        let input_cursor = input_value.len();

        let focused = if Self::has_input_for(&kind) {
            DialogElement::Input
        } else if matches!(
            kind,
            DialogKind::Reset { .. } | DialogKind::CheckoutHasLocalChanges { .. }
        ) {
            DialogElement::Radio(dropdown_selected)
        } else if !checkboxes.is_empty() {
            DialogElement::Checkbox(0)
        } else {
            DialogElement::Validate
        };

        Self {
            before,
            kind,
            input_value,
            input_cursor,
            second_input_value: String::new(),
            second_input_cursor: 0,
            dropdown_selected,
            checkboxes,
            focused,
            hovered: None,
            dialog_area: Rect::default(),
            inner_area: Rect::default(),
            input_row: None,
            second_input_row: None,
            body_expand_row: None,
            checkbox_rows: Vec::new(),
            radio_rows: Vec::new(),
            button_row: 0,
            validate_col: (0, 0),
            cancel_col: (0, 0),
            ctx,
            tx,
        }
    }

    pub fn is_input_focused(&self) -> bool {
        matches!(self.focused, DialogElement::Input | DialogElement::SecondInput)
    }

    fn has_input_for(kind: &DialogKind) -> bool {
        matches!(
            kind,
            DialogKind::AddTag { .. }
                | DialogKind::CreateBranch { .. }
                | DialogKind::RenameBranch { .. }
                | DialogKind::CreateBranchFromStash { .. }
                | DialogKind::StashWithMessage
                | DialogKind::CommitWithMessage
                | DialogKind::AddRemote
                | DialogKind::AmendMessage { .. }
                | DialogKind::AddWorktree
        )
    }

    fn has_input_field(&self) -> bool {
        Self::has_input_for(&self.kind)
    }

    fn has_second_input_field(&self) -> bool {
        matches!(self.kind, DialogKind::AddRemote)
            || (matches!(self.kind, DialogKind::AddTag { .. })
                && self.checkboxes.first().copied().unwrap_or(false))
    }

    fn radio_count(&self) -> usize {
        match &self.kind {
            DialogKind::Reset { .. } => 3,
            DialogKind::MergePullRequest { .. } => 3,
            DialogKind::ChooseRemote { remotes, .. } => remotes.len(),
            DialogKind::SetUpstream { remotes, .. } => remotes.len(),
            DialogKind::CheckoutHasLocalChanges { .. } => 2,
            _ => 0,
        }
    }

    fn elements(&self) -> Vec<DialogElement> {
        let mut els = vec![];
        if self.has_input_field() {
            els.push(DialogElement::Input);
        }
        // AddRemote: SecondInput (URL) comes right after the name input.
        if matches!(self.kind, DialogKind::AddRemote) {
            els.push(DialogElement::SecondInput);
        }
        // Body expand button is focusable only when no body exists yet.
        if matches!(
            self.kind,
            DialogKind::CommitWithMessage | DialogKind::AmendMessage { .. }
        ) && !self.input_value.contains('\n')
        {
            els.push(DialogElement::BodyExpand);
        }
        let n = self.radio_count();
        if n > 0 {
            for i in 0..n {
                els.push(DialogElement::Radio(i));
            }
        } else {
            for i in 0..self.checkboxes.len() {
                els.push(DialogElement::Checkbox(i));
            }
        }
        // AddTag: when annotated is checked, the Message: input lives below
        // the checkbox, so it appears after Checkbox(0) in tab order.
        if matches!(self.kind, DialogKind::AddTag { .. })
            && self.checkboxes.first().copied().unwrap_or(false)
        {
            els.push(DialogElement::SecondInput);
        }
        els.push(DialogElement::Validate);
        els.push(DialogElement::Cancel);
        els
    }

    fn focus_next(&mut self) {
        let els = self.elements();
        if let Some(idx) = els.iter().position(|e| *e == self.focused) {
            self.focused = els[(idx + 1) % els.len()];
        }
    }

    fn focus_prev(&mut self) {
        let els = self.elements();
        if let Some(idx) = els.iter().position(|e| *e == self.focused) {
            self.focused = els[if idx == 0 { els.len() - 1 } else { idx - 1 }];
        }
    }

    fn is_highlighted(&self, element: DialogElement) -> bool {
        self.focused == element || self.hovered == Some(element)
    }

    /// Returns true when the `▸` cursor indicator should point at this
    /// element. The cursor follows the mouse hover when one exists,
    /// otherwise it follows the keyboard focus — never both at once.
    fn is_pointed_at(&self, element: DialogElement) -> bool {
        match self.hovered {
            Some(h) => h == element,
            None => self.focused == element,
        }
    }

    /// Returns `(line_index, col_in_line)` for `byte_offset` within `s`.
    fn cursor_line_col(s: &str, byte_offset: usize) -> (usize, usize) {
        let clamped = byte_offset.min(s.len());
        let before = &s[..clamped];
        let line = before.chars().filter(|&c| c == '\n').count();
        let col = before.rfind('\n').map_or(before.len(), |p| before.len() - p - 1);
        (line, col)
    }

    /// Move the cursor to the same column on the previous line (for multi-line commit dialogs).
    fn move_cursor_up(&mut self) {
        let (line_idx, col) = Self::cursor_line_col(&self.input_value, self.input_cursor);
        if line_idx == 0 {
            // Already on first line; move to start.
            self.input_cursor = 0;
            return;
        }
        // Find the start of the previous line.
        let lines: Vec<&str> = self.input_value.split('\n').collect();
        let prev_line = lines[line_idx - 1];
        let target_col = col.min(prev_line.len());
        // Byte offset of the start of the previous line.
        let prev_line_start: usize = lines[..line_idx - 1]
            .iter()
            .map(|l| l.len() + 1) // +1 for '\n'
            .sum();
        self.input_cursor = prev_line_start + target_col;
    }

    /// Move the cursor to the same column on the next line (for multi-line commit dialogs).
    fn move_cursor_down(&mut self) {
        let (line_idx, col) = Self::cursor_line_col(&self.input_value, self.input_cursor);
        let lines: Vec<&str> = self.input_value.split('\n').collect();
        if line_idx + 1 >= lines.len() {
            // Already on last line; move to end.
            self.input_cursor = self.input_value.len();
            return;
        }
        let next_line = lines[line_idx + 1];
        let target_col = col.min(next_line.len());
        let next_line_start: usize = lines[..line_idx + 1]
            .iter()
            .map(|l| l.len() + 1)
            .sum();
        self.input_cursor = next_line_start + target_col;
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        if matches!(self.focused, DialogElement::Input) {
            use ratatui::crossterm::event::{KeyCode, KeyModifiers};

            // Multi-line shortcuts for commit/amend dialogs only.
            if matches!(
                self.kind,
                DialogKind::CommitWithMessage | DialogKind::AmendMessage { .. }
            ) {
                // Ctrl+Enter OR Alt+Enter → insert newline into message body.
                // Alt+Enter is the reliable cross-terminal alias (Ctrl+Enter
                // can be swallowed or re-encoded differently per terminal).
                let is_newline_key = key.code == KeyCode::Enter
                    && (key.modifiers.contains(KeyModifiers::CONTROL)
                        || key.modifiers.contains(KeyModifiers::ALT));
                if is_newline_key {
                    self.insert_body_newline();
                    return;
                }
                if key.code == KeyCode::Up {
                    self.move_cursor_up();
                    return;
                }
                if key.code == KeyCode::Down {
                    if !self.input_value.contains('\n') {
                        // No body yet — arrow down moves focus to expand button.
                        self.focused = DialogElement::BodyExpand;
                    } else {
                        let (line_idx, _) =
                            Self::cursor_line_col(&self.input_value, self.input_cursor);
                        let total_lines = self.input_value.split('\n').count();
                        if line_idx + 1 >= total_lines {
                            // Already on last line — leave input and go to next element.
                            self.focus_next();
                        } else {
                            self.move_cursor_down();
                        }
                    }
                    return;
                }
            }

            match key.code {
                // Ctrl+H is the terminal alias for Ctrl+Backspace (ASCII ^H).
                // Must be checked before the generic Char(c) arm.
                ratatui::crossterm::event::KeyCode::Char('h')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    let new_pos = word_left(&self.input_value, self.input_cursor);
                    self.input_value.drain(new_pos..self.input_cursor);
                    self.input_cursor = new_pos;
                    return;
                }
                // Ctrl+W: Unix word-delete-left alias (same as Ctrl+Backspace).
                ratatui::crossterm::event::KeyCode::Char('w')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    let new_pos = word_left(&self.input_value, self.input_cursor);
                    self.input_value.drain(new_pos..self.input_cursor);
                    self.input_cursor = new_pos;
                    return;
                }
                ratatui::crossterm::event::KeyCode::Char(c) => {
                    self.input_value.insert(self.input_cursor, c);
                    self.input_cursor += 1;
                    return;
                }
                ratatui::crossterm::event::KeyCode::Backspace
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    let new_pos = word_left(&self.input_value, self.input_cursor);
                    self.input_value.drain(new_pos..self.input_cursor);
                    self.input_cursor = new_pos;
                    return;
                }
                ratatui::crossterm::event::KeyCode::Backspace => {
                    if self.input_cursor > 0 {
                        self.input_cursor -= 1;
                        self.input_value.remove(self.input_cursor);
                    }
                    return;
                }
                ratatui::crossterm::event::KeyCode::Delete
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    let new_pos = word_right(&self.input_value, self.input_cursor);
                    self.input_value.drain(self.input_cursor..new_pos);
                    return;
                }
                ratatui::crossterm::event::KeyCode::Delete => {
                    if self.input_cursor < self.input_value.len() {
                        self.input_value.remove(self.input_cursor);
                    }
                    return;
                }
                ratatui::crossterm::event::KeyCode::Left
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.input_cursor = word_left(&self.input_value, self.input_cursor);
                    return;
                }
                ratatui::crossterm::event::KeyCode::Left => {
                    if self.input_cursor > 0 {
                        self.input_cursor -= 1;
                    }
                    return;
                }
                ratatui::crossterm::event::KeyCode::Right
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.input_cursor = word_right(&self.input_value, self.input_cursor);
                    return;
                }
                ratatui::crossterm::event::KeyCode::Right => {
                    if self.input_cursor < self.input_value.len() {
                        self.input_cursor += 1;
                    }
                    return;
                }
                ratatui::crossterm::event::KeyCode::Home => {
                    self.input_cursor = 0;
                    return;
                }
                ratatui::crossterm::event::KeyCode::End => {
                    self.input_cursor = self.input_value.len();
                    return;
                }
                _ => {}
            }
        }

        if matches!(self.focused, DialogElement::SecondInput) {
            use ratatui::crossterm::event::KeyModifiers;
            match key.code {
                ratatui::crossterm::event::KeyCode::Char('h')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    let new_pos = word_left(&self.second_input_value, self.second_input_cursor);
                    self.second_input_value.drain(new_pos..self.second_input_cursor);
                    self.second_input_cursor = new_pos;
                    return;
                }
                ratatui::crossterm::event::KeyCode::Char('w')
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    let new_pos = word_left(&self.second_input_value, self.second_input_cursor);
                    self.second_input_value.drain(new_pos..self.second_input_cursor);
                    self.second_input_cursor = new_pos;
                    return;
                }
                ratatui::crossterm::event::KeyCode::Char(c) => {
                    self.second_input_value.insert(self.second_input_cursor, c);
                    self.second_input_cursor += 1;
                    return;
                }
                ratatui::crossterm::event::KeyCode::Backspace
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    let new_pos = word_left(&self.second_input_value, self.second_input_cursor);
                    self.second_input_value.drain(new_pos..self.second_input_cursor);
                    self.second_input_cursor = new_pos;
                    return;
                }
                ratatui::crossterm::event::KeyCode::Backspace => {
                    if self.second_input_cursor > 0 {
                        self.second_input_cursor -= 1;
                        self.second_input_value.remove(self.second_input_cursor);
                    }
                    return;
                }
                ratatui::crossterm::event::KeyCode::Delete
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    let new_pos = word_right(&self.second_input_value, self.second_input_cursor);
                    self.second_input_value.drain(self.second_input_cursor..new_pos);
                    return;
                }
                ratatui::crossterm::event::KeyCode::Delete => {
                    if self.second_input_cursor < self.second_input_value.len() {
                        self.second_input_value.remove(self.second_input_cursor);
                    }
                    return;
                }
                ratatui::crossterm::event::KeyCode::Left
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.second_input_cursor =
                        word_left(&self.second_input_value, self.second_input_cursor);
                    return;
                }
                ratatui::crossterm::event::KeyCode::Left => {
                    if self.second_input_cursor > 0 {
                        self.second_input_cursor -= 1;
                    }
                    return;
                }
                ratatui::crossterm::event::KeyCode::Right
                    if key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    self.second_input_cursor =
                        word_right(&self.second_input_value, self.second_input_cursor);
                    return;
                }
                ratatui::crossterm::event::KeyCode::Right => {
                    if self.second_input_cursor < self.second_input_value.len() {
                        self.second_input_cursor += 1;
                    }
                    return;
                }
                ratatui::crossterm::event::KeyCode::Home => {
                    self.second_input_cursor = 0;
                    return;
                }
                ratatui::crossterm::event::KeyCode::End => {
                    self.second_input_cursor = self.second_input_value.len();
                    return;
                }
                _ => {}
            }
        }

        let event = event_with_count.event;
        match event {
            UserEvent::Confirm => match self.focused {
                DialogElement::Validate => self.confirm(),
                DialogElement::Cancel => self.tx.send(AppEvent::DialogCancel),
                DialogElement::Checkbox(i) => self.toggle_checkbox_at(i),
                DialogElement::Radio(i) => {
                    self.dropdown_selected = i;
                }
                DialogElement::BodyExpand => self.insert_body_newline(),
                DialogElement::Input => {
                    self.focus_next();
                }
                DialogElement::SecondInput => {
                    self.focus_next();
                }
            },
            UserEvent::Cancel => self.tx.send(AppEvent::DialogCancel),
            UserEvent::Close => self.tx.send(AppEvent::DialogCancel),
            UserEvent::NavigateDown => match self.focused {
                DialogElement::Radio(i) if i + 1 < self.radio_count() => {
                    self.dropdown_selected = i + 1;
                    self.focused = DialogElement::Radio(i + 1);
                }
                _ => self.focus_next(),
            },
            UserEvent::NavigateUp => match self.focused {
                DialogElement::Radio(i) if i > 0 => {
                    self.dropdown_selected = i - 1;
                    self.focused = DialogElement::Radio(i - 1);
                }
                _ => self.focus_prev(),
            },
            UserEvent::NavigateLeft => {
                if matches!(self.focused, DialogElement::Input) {
                    if self.input_cursor > 0 {
                        self.input_cursor -= 1;
                    }
                } else if matches!(self.focused, DialogElement::SecondInput) {
                    if self.second_input_cursor > 0 {
                        self.second_input_cursor -= 1;
                    }
                } else {
                    match self.focused {
                        DialogElement::Validate => self.focused = DialogElement::Cancel,
                        DialogElement::Cancel => self.focused = DialogElement::Validate,
                        DialogElement::Checkbox(i) => self.toggle_checkbox_at(i),
                        _ => self.focus_prev(),
                    }
                }
            }
            UserEvent::NavigateRight => {
                if matches!(self.focused, DialogElement::Input) {
                    if self.input_cursor < self.input_value.len() {
                        self.input_cursor += 1;
                    }
                } else if matches!(self.focused, DialogElement::SecondInput) {
                    if self.second_input_cursor < self.second_input_value.len() {
                        self.second_input_cursor += 1;
                    }
                } else {
                    match self.focused {
                        DialogElement::Validate => self.focused = DialogElement::Cancel,
                        DialogElement::Cancel => self.focused = DialogElement::Validate,
                        DialogElement::Checkbox(i) => self.toggle_checkbox_at(i),
                        _ => self.focus_next(),
                    }
                }
            }
            UserEvent::RefList => {
                self.focus_next();
            }
            _ => {}
        }
    }

    fn toggle_checkbox_at(&mut self, index: usize) {
        if let Some(cb) = self.checkboxes.get_mut(index) {
            *cb = !*cb;
        }
    }

    /// Insert a newline at the end of the subject line and move the cursor
    /// into the body area. Used by the `[ ↵ add body ]` button and Alt+Enter.
    fn insert_body_newline(&mut self) {
        // Always append to the end of the first line (subject), regardless
        // of cursor position — the body button always starts a new body.
        let subject_end = self.input_value.find('\n').unwrap_or(self.input_value.len());
        self.input_value.insert(subject_end, '\n');
        self.input_cursor = subject_end + 1;
        self.focused = DialogElement::Input;
        self.body_expand_row = None;
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        if let Some(element) = self.find_element_at(col, row) {
            self.focused = element;
            match element {
                DialogElement::Validate => self.confirm(),
                DialogElement::Cancel => self.tx.send(AppEvent::DialogCancel),
                DialogElement::Checkbox(i) => self.toggle_checkbox_at(i),
                DialogElement::Radio(i) => {
                    self.dropdown_selected = i;
                }
                DialogElement::BodyExpand => self.insert_body_newline(),
                DialogElement::Input | DialogElement::SecondInput => {}
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        if col >= self.inner_area.x
            && col < self.inner_area.x + self.inner_area.width
            && row >= self.inner_area.y
            && row < self.inner_area.y + self.inner_area.height
        {
            let inner_col = col - self.inner_area.x;
            let inner_row = row - self.inner_area.y;
            self.hovered = self.find_element_at_inner(inner_col, inner_row);
        } else {
            self.hovered = None;
        }
    }

    fn find_element_at(&self, col: u16, row: u16) -> Option<DialogElement> {
        if col < self.inner_area.x || row < self.inner_area.y {
            return None;
        }
        let inner_col = col - self.inner_area.x;
        let inner_row = row - self.inner_area.y;
        if inner_col >= self.inner_area.width || inner_row >= self.inner_area.height {
            return None;
        }
        self.find_element_at_inner(inner_col, inner_row)
    }

    fn find_element_at_inner(&self, col: u16, row: u16) -> Option<DialogElement> {
        let row = row as usize;

        // Body expand button (takes priority over the generic input-row check below).
        if let Some(expand_row) = self.body_expand_row {
            if row == expand_row {
                return Some(DialogElement::BodyExpand);
            }
        }

        if row == self.button_row {
            let (vs, ve) = self.validate_col;
            let (cs, ce) = self.cancel_col;
            if col >= vs && col < ve {
                return Some(DialogElement::Validate);
            }
            if col >= cs && col < ce {
                return Some(DialogElement::Cancel);
            }
            return None;
        }

        if let Some(input_row) = self.input_row {
            // For multi-line commit dialogs the input spans several rows.
            let input_height = if matches!(
                self.kind,
                DialogKind::CommitWithMessage | DialogKind::AmendMessage { .. }
            ) {
                let line_count = self.input_value.lines().count().max(1);
                // +2 for separator row and placeholder / extra blank row
                (line_count + 2).min(8)
            } else {
                1
            };
            if row >= input_row && row < input_row + input_height {
                return Some(DialogElement::Input);
            }
        }

        if let Some(second_input_row) = self.second_input_row {
            if row == second_input_row {
                return Some(DialogElement::SecondInput);
            }
        }

        for (i, &cb_row) in self.checkbox_rows.iter().enumerate() {
            if row == cb_row {
                return Some(DialogElement::Checkbox(i));
            }
        }

        for (i, &radio_row) in self.radio_rows.iter().enumerate() {
            if row == radio_row {
                return Some(DialogElement::Radio(i));
            }
        }

        None
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.before.render(f, area);

        let width = (area.width as u32 * 50 / 100).clamp(42, 65) as u16;
        let inner_width = width.saturating_sub(2);

        let lines = self.build_lines(inner_width);

        let height = (lines.len() as u16 + 2).min(area.height);
        let dialog_area = centered_rect_exact(width, height, area);
        self.dialog_area = dialog_area;

        f.render_widget(Clear, dialog_area);

        let theme = &self.ctx.color_theme;
        let border_color = theme.divider_fg;
        let title_color = theme.fg;

        let title = self.title();
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color))
            .title(
                Line::from(Span::styled(
                    title,
                    Style::default()
                        .fg(title_color)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
            );

        let inner = block.inner(dialog_area);
        self.inner_area = inner;

        f.render_widget(block, dialog_area);
        f.render_widget(Paragraph::new(lines), inner);

        if self.is_highlighted(DialogElement::Input) {
            if let Some(input_row) = self.input_row {
                let (cursor_x, cursor_y) = if matches!(
                    self.kind,
                    DialogKind::CommitWithMessage | DialogKind::AmendMessage { .. }
                ) {
                    let (line_idx, col) =
                        Self::cursor_line_col(&self.input_value, self.input_cursor);
                    // Row layout: subject=+0, separator=+1, body_line_k=+(2+k)
                    let row_offset = if line_idx == 0 {
                        0u16
                    } else {
                        // +1 for separator row, +1 for "Body:" label, then body line index
                        2 + line_idx as u16
                    };
                    let cx = self.inner_area.x + 1 + col as u16;
                    let cy = self.inner_area.y + input_row as u16 + row_offset;
                    (cx, cy)
                } else {
                    let cx = self.inner_area.x + 1 + self.input_cursor as u16;
                    let cy = self.inner_area.y + input_row as u16;
                    (cx, cy)
                };
                match &self.ctx.ui_config.common.cursor_type {
                    CursorType::Native => {
                        f.set_cursor_position((cursor_x, cursor_y));
                    }
                    CursorType::Virtual(cursor) => {
                        let style = Style::default().fg(self.ctx.color_theme.virtual_cursor_fg);
                        f.buffer_mut().set_string(cursor_x, cursor_y, cursor, style);
                    }
                }
            }
        }

        if self.is_highlighted(DialogElement::SecondInput) {
            if let Some(second_input_row) = self.second_input_row {
                let cursor_x = self.inner_area.x + 1 + self.second_input_cursor as u16;
                let cursor_y = self.inner_area.y + second_input_row as u16;
                match &self.ctx.ui_config.common.cursor_type {
                    CursorType::Native => {
                        f.set_cursor_position((cursor_x, cursor_y));
                    }
                    CursorType::Virtual(cursor) => {
                        let style = Style::default().fg(self.ctx.color_theme.virtual_cursor_fg);
                        f.buffer_mut().set_string(cursor_x, cursor_y, cursor, style);
                    }
                }
            }
        }
    }

    fn build_lines(&mut self, inner_width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        self.input_row = None;
        self.second_input_row = None;
        self.checkbox_rows.clear();
        self.radio_rows.clear();
        self.body_expand_row = None;

        let theme = &self.ctx.color_theme;
        let dim_fg = theme.list_commit_message_fg;
        let fg = theme.fg;
        let yellow = theme.list_hash_fg;
        let warn_fg = theme.status_warn_fg;
        let divider_fg = theme.divider_fg;

        lines.push(separator_line(inner_width, divider_fg));

        match &self.kind {
            DialogKind::AddTag { target } => {
                lines.push(info_line("Target:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(label_line("Tag name:", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg, inner_width));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Annotated tag"));
                if self.checkboxes.first().copied().unwrap_or(false) {
                    lines.push(Line::from(""));
                    lines.push(label_line("Message:", dim_fg));
                    self.second_input_row = Some(lines.len());
                    lines.push(self.second_input_line(fg, inner_width));
                }
            }
            DialogKind::CreateBranch { target } => {
                lines.push(info_line("At:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(label_line("Branch name:", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg, inner_width));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Checkout after creation"));
            }
            DialogKind::Checkout { target, is_branch } => {
                let kind_str = if *is_branch { "branch" } else { "commit" };
                lines.push(info_line(
                    &format!("Checkout {}:", kind_str),
                    target,
                    dim_fg,
                    yellow,
                ));
            }
            DialogKind::CherryPick { target } => {
                lines.push(info_line("Commit:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Record origin (-x)"));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(1, "No commit (-n)"));
            }
            DialogKind::Revert { target } => {
                lines.push(info_line("Commit:", target, dim_fg, yellow));
            }
            DialogKind::Drop { target } => {
                lines.push(info_line("Commit:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(warning_line(
                    "This will permanently remove the commit.",
                    warn_fg,
                ));
            }
            DialogKind::Squash { target } => {
                // Centered layout — the squash dialog has no fields/options,
                // so visually anchoring the text in the middle reads cleaner
                // than left-aligned with the standard 2-space gutter.
                lines.push(
                    Line::from(vec![
                        Span::styled("Commit: ", Style::default().fg(dim_fg)),
                        Span::styled(target.clone(), Style::default().fg(yellow)),
                    ])
                    .alignment(ratatui::layout::Alignment::Center),
                );
                lines.push(Line::from(""));
                lines.push(
                    Line::from(Span::styled(
                        "This will fuse the commit with its parent and rewrite history.",
                        Style::default().fg(warn_fg),
                    ))
                    .alignment(ratatui::layout::Alignment::Center),
                );
            }
            DialogKind::Merge { target, is_branch } => {
                let kind_str = if *is_branch { "branch" } else { "commit" };
                lines.push(info_line(
                    &format!("Merge {}:", kind_str),
                    target,
                    dim_fg,
                    yellow,
                ));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "No fast-forward (--no-ff)"));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(1, "Squash (--squash)"));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(2, "No commit (--no-commit)"));
            }
            DialogKind::Rebase { target } => {
                lines.push(info_line("Rebase onto:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Interactive (-i)"));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(1, "Ignore date (--ignore-date)"));
            }
            DialogKind::Reset { target } => {
                lines.push(info_line("Reset to:", target, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(label_line("Mode:", dim_fg));
                self.radio_rows.push(lines.len());
                lines.push(self.radio_line(0, "Soft  — Keep all changes, reset head"));
                self.radio_rows.push(lines.len());
                lines.push(self.radio_line(1, "Mixed — Keep working tree, reset index"));
                self.radio_rows.push(lines.len());
                lines.push(self.radio_line(2, "Hard  — Discard all changes"));
            }
            DialogKind::MergePullRequest { number, pr_title, .. } => {
                lines.push(info_line(
                    "Merge PR:",
                    &format!("#{} {}", number, pr_title),
                    dim_fg,
                    yellow,
                ));
                lines.push(Line::from(""));
                lines.push(label_line("Method:", dim_fg));
                self.radio_rows.push(lines.len());
                lines.push(
                    self.radio_line(0, "Merge commit — keep history of branch"),
                );
                self.radio_rows.push(lines.len());
                lines.push(self.radio_line(1, "Squash — collapse into a single commit"));
                self.radio_rows.push(lines.len());
                lines.push(
                    self.radio_line(2, "Rebase — replay each commit onto base"),
                );
            }
            DialogKind::RenameBranch { branch } => {
                lines.push(info_line("Current:", branch, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(label_line("New name:", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg, inner_width));
            }
            DialogKind::DeleteBranch { branch, is_remote } => {
                let kind_str = if *is_remote {
                    "Remote branch"
                } else {
                    "Branch"
                };
                lines.push(info_line(kind_str, branch, dim_fg, yellow));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Force delete"));
            }
            DialogKind::DeleteTag { tag } => {
                lines.push(info_line("Tag:", tag, dim_fg, yellow));
            }
            DialogKind::PushTag { tag } => {
                lines.push(info_line("Tag:", tag, dim_fg, yellow));
            }
            DialogKind::PushBranch { branch } => {
                lines.push(info_line("Branch:", branch, dim_fg, yellow));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Force with lease (--force-with-lease)"));
            }
            DialogKind::PullBranch { branch } => {
                lines.push(info_line("Branch:", branch, dim_fg, yellow));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Rebase (--rebase)"));
            }
            DialogKind::CreateBranchFromStash {
                target: _,
                stash_ref,
            } => {
                lines.push(info_line("Stash:", stash_ref, dim_fg, yellow));
                lines.push(Line::from(""));
                lines.push(label_line("Branch name:", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg, inner_width));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Checkout after creation"));
            }
            DialogKind::StashWithMessage => {
                lines.push(label_line("Message (optional):", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg, inner_width));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Include untracked files"));
            }
            DialogKind::CommitWithMessage => {
                lines.push(label_line("Message:", dim_fg));
                self.input_row = Some(lines.len());
                for ml in self.multiline_input_lines(fg, dim_fg, divider_fg, inner_width) {
                    lines.push(ml);
                }
                if !self.input_value.contains('\n') {
                    self.body_expand_row = self.input_row.map(|r| r + 2);
                }
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Amend previous commit"));
            }
            DialogKind::ConfirmDiscardFile { file } => {
                lines.push(Line::from(Span::styled(
                    format!("Discard changes to '{}?", file),
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(warning_line("This action cannot be undone.", warn_fg));
            }
            DialogKind::ConfirmDiscardAll => {
                lines.push(Line::from(Span::styled(
                    "Discard all unstaged changes?",
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(warning_line("This action cannot be undone.", warn_fg));
            }
            DialogKind::ConfirmStageAll => {
                lines.push(Line::from(Span::styled(
                    "Stage all changes?",
                    Style::default().fg(fg),
                )));
            }
            DialogKind::ConfirmUnstageAll => {
                lines.push(Line::from(Span::styled(
                    "Unstage all changes?",
                    Style::default().fg(fg),
                )));
            }
            DialogKind::CleanUntracked => {
                lines.push(Line::from(Span::styled(
                    "Remove all untracked files and directories?",
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(warning_line("This action cannot be undone.", warn_fg));
            }
            DialogKind::ConfirmPopStash { stash_ref } => {
                lines.push(Line::from(Span::styled(
                    format!("Apply and remove stash '{}'?", stash_ref),
                    Style::default().fg(fg),
                )));
            }
            DialogKind::ConfirmDropStash { stash_ref } => {
                lines.push(Line::from(Span::styled(
                    format!("Permanently remove stash '{}'?", stash_ref),
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(warning_line("This action cannot be undone.", warn_fg));
            }
            DialogKind::ConfirmDeleteComment {
                author,
                body_preview,
                ..
            } => {
                lines.push(Line::from(vec![
                    Span::styled("Author: ", Style::default().fg(dim_fg)),
                    Span::styled(
                        format!("@{}", author),
                        Style::default().fg(self.ctx.color_theme.list_name_fg),
                    ),
                ]));
                lines.push(Line::from(""));
                // Body preview — full markdown rendering (bold, italic,
                // code, lists, blockquotes, links, GFM task lists) so
                // what the user sees matches the actual comment they're
                // deleting. Capped so the popup stays compact.
                let avail = (inner_width as usize).saturating_sub(2);
                const MAX_PREVIEW_LINES: usize = 6;
                let theme = &self.ctx.color_theme;
                let rendered = crate::view::pr::render_markdown_body(
                    body_preview,
                    theme,
                    avail.max(1),
                );
                let overflowed = rendered.len() > MAX_PREVIEW_LINES;
                for line_spans in rendered.into_iter().take(MAX_PREVIEW_LINES) {
                    let mut row: Vec<Span<'static>> =
                        vec![Span::raw("  ".to_string())];
                    row.extend(line_spans);
                    lines.push(Line::from(row));
                }
                if overflowed {
                    lines.push(Line::from(Span::styled(
                        "  …",
                        Style::default().fg(dim_fg),
                    )));
                }
                lines.push(Line::from(""));
                lines.push(warning_line(
                    "Delete this comment? This cannot be undone.",
                    warn_fg,
                ));
            }
            DialogKind::ConfirmPullRequestStateChange {
                pr_number,
                pr_title,
                closing,
            } => {
                lines.push(pr_header_line(
                    *pr_number,
                    pr_title,
                    &self.ctx.color_theme,
                ));
                lines.push(Line::from(""));
                let msg = if *closing {
                    "Close this PR without merging?"
                } else {
                    "Reopen this PR?"
                };
                lines.push(Line::from(Span::styled(
                    msg.to_string(),
                    Style::default().fg(fg),
                )));
            }
            DialogKind::ConfirmPullRequestDraftToggle {
                pr_number,
                pr_title,
                to_draft,
                ..
            } => {
                lines.push(pr_header_line(
                    *pr_number,
                    pr_title,
                    &self.ctx.color_theme,
                ));
                lines.push(Line::from(""));
                let msg = if *to_draft {
                    "Convert this PR back to a draft? Reviewers will be notified."
                } else {
                    "Mark this draft as ready for review?"
                };
                lines.push(Line::from(Span::styled(
                    msg.to_string(),
                    Style::default().fg(fg),
                )));
            }
            DialogKind::PullRequestLabels {
                pr_number,
                pr_title,
                all_labels,
                ..
            } => {
                lines.push(pr_header_line(
                    *pr_number,
                    pr_title,
                    &self.ctx.color_theme,
                ));
                lines.push(Line::from(""));
                if all_labels.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No labels defined on this repo.".to_string(),
                        Style::default().fg(dim_fg),
                    )));
                } else {
                    // Currently-attached chip row (GitHub-style colored
                    // pills) — gives instant context for what's on the
                    // PR before scanning the togglable list.
                    let attached: Vec<&crate::github::pr::Label> = all_labels
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| self.checkboxes.get(*i).copied().unwrap_or(false))
                        .map(|(_, l)| l)
                        .collect();
                    let mut chip_row: Vec<Span<'static>> =
                        vec![Span::raw("  ".to_string())];
                    if attached.is_empty() {
                        chip_row.push(Span::styled(
                            "(no labels)".to_string(),
                            Style::default().fg(dim_fg),
                        ));
                    } else {
                        for (i, lab) in attached.iter().enumerate() {
                            if i > 0 {
                                chip_row.push(Span::raw(" ".to_string()));
                            }
                            chip_row.extend(label_chip_spans(lab, dim_fg));
                        }
                    }
                    lines.push(Line::from(chip_row));
                    lines.push(Line::from(""));
                    for (i, lab) in all_labels.iter().enumerate() {
                        // The clickable row is the first line returned
                        // — continuation rows (long descriptions) sit
                        // below but aren't independently selectable.
                        let rendered =
                            self.checkbox_line_labelled(i, lab, inner_width);
                        self.checkbox_rows.push(lines.len());
                        for ln in rendered {
                            lines.push(ln);
                        }
                    }
                }
            }
            DialogKind::PullRequestReviewers {
                pr_number,
                pr_title,
                all_users,
                ..
            } => {
                lines.push(pr_header_line(
                    *pr_number,
                    pr_title,
                    &self.ctx.color_theme,
                ));
                lines.push(Line::from(""));
                if all_users.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "No assignable users found.".to_string(),
                        Style::default().fg(dim_fg),
                    )));
                } else {
                    // Currently-requested reviewers as a @user list
                    // above the picker, same idea as the labels chip
                    // row — quick visual summary of the PR's state.
                    let requested: Vec<&String> = all_users
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| self.checkboxes.get(*i).copied().unwrap_or(false))
                        .map(|(_, u)| u)
                        .collect();
                    let mut chip_row: Vec<Span<'static>> =
                        vec![Span::raw("  ".to_string())];
                    if requested.is_empty() {
                        chip_row.push(Span::styled(
                            "(no reviewers requested)".to_string(),
                            Style::default().fg(dim_fg),
                        ));
                    } else {
                        for (i, name) in requested.iter().enumerate() {
                            if i > 0 {
                                chip_row.push(Span::styled(
                                    " ".to_string(),
                                    Style::default().fg(dim_fg),
                                ));
                            }
                            chip_row.push(Span::styled(
                                format!("@{}", name),
                                Style::default()
                                    .fg(self.ctx.color_theme.list_name_fg),
                            ));
                        }
                    }
                    lines.push(Line::from(chip_row));
                    lines.push(Line::from(""));
                    for (i, name) in all_users.iter().enumerate() {
                        self.checkbox_rows.push(lines.len());
                        lines.push(self.checkbox_line(i, name));
                    }
                }
            }
            DialogKind::AddRemote => {
                lines.push(label_line("Name:", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg, inner_width));
                lines.push(Line::from(""));
                lines.push(label_line("URL:", dim_fg));
                self.second_input_row = Some(lines.len());
                lines.push(self.second_input_line(fg, inner_width));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "  The name groups branches: \"origin\" \u{2192} origin/main\u{2026}",
                    Style::default().fg(dim_fg),
                )));
            }
            DialogKind::ConfirmDeleteRemote { name } => {
                lines.push(Line::from(Span::styled(
                    format!("Remove remote '{}'?", name),
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(warning_line(
                    "This will only remove the remote configuration.",
                    warn_fg,
                ));
            }
            DialogKind::ChooseRemote { remotes, branch } => {
                lines.push(Line::from(Span::styled(
                    format!("No upstream for branch '{}'.", branch),
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(Span::styled(
                    "Choose a remote to push and set as upstream:",
                    Style::default().fg(dim_fg),
                )));
                lines.push(Line::from(""));
                for (i, remote) in remotes.iter().enumerate() {
                    self.radio_rows.push(lines.len());
                    lines.push(self.radio_line(i, remote));
                }
            }
            DialogKind::SetUpstream { remotes, branch } => {
                lines.push(Line::from(Span::styled(
                    format!("Set upstream for branch '{}':", branch),
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(Span::styled(
                    "Choose a remote to track:",
                    Style::default().fg(dim_fg),
                )));
                lines.push(Line::from(""));
                for (i, remote) in remotes.iter().enumerate() {
                    self.radio_rows.push(lines.len());
                    lines.push(self.radio_line(i, remote));
                }
            }
            DialogKind::ConfirmAbortOperation { op_name } => {
                lines.push(Line::from(Span::styled(
                    format!("Abort {} in progress?", op_name),
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(warning_line(
                    "All changes will be preserved in working tree.",
                    warn_fg,
                ));
            }
            DialogKind::AmendMessage { .. } => {
                lines.push(label_line("New commit message:", dim_fg));
                self.input_row = Some(lines.len());
                for ml in self.multiline_input_lines(fg, dim_fg, divider_fg, inner_width) {
                    lines.push(ml);
                }
                if !self.input_value.contains('\n') {
                    self.body_expand_row = self.input_row.map(|r| r + 2);
                }
            }
            DialogKind::AddWorktree => {
                lines.push(label_line("Name:", dim_fg));
                self.input_row = Some(lines.len());
                lines.push(self.input_line(fg, inner_width));
                lines.push(Line::from(Span::styled(
                    "  Used as branch name and to derive the worktree path.",
                    Style::default().fg(dim_fg),
                )));
                lines.push(Line::from(""));
                self.checkbox_rows.push(lines.len());
                lines.push(self.checkbox_line(0, "Switch to new worktree"));
            }
            DialogKind::ConfirmDeleteWorktree {
                path,
                display_name,
                is_dirty,
            } => {
                lines.push(Line::from(Span::styled(
                    format!("Remove worktree '{}'?", display_name),
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("Path: {}", path),
                    Style::default().fg(dim_fg),
                )));
                lines.push(Line::from(""));
                if *is_dirty {
                    lines.push(warning_line(
                        "Worktree has uncommitted changes — will use --force.",
                        warn_fg,
                    ));
                } else {
                    lines.push(warning_line(
                        "The working directory will be removed.",
                        warn_fg,
                    ));
                }
            }
            DialogKind::CheckoutHasLocalChanges { target, is_branch } => {
                let kind_str = if *is_branch { "branch" } else { "commit" };
                lines.push(info_line(
                    &format!("Checkout {}:", kind_str),
                    target,
                    dim_fg,
                    yellow,
                ));
                lines.push(Line::from(""));
                lines.push(warning_line(
                    "Local changes would be overwritten by checkout.",
                    warn_fg,
                ));
                lines.push(Line::from(""));
                lines.push(label_line("Choose how to proceed:", dim_fg));
                self.radio_rows.push(lines.len());
                lines.push(self.radio_line(0, "Stash changes, then checkout"));
                self.radio_rows.push(lines.len());
                lines.push(self.radio_line(1, "Discard changes, then checkout"));
            }
            DialogKind::ConfirmSwitchWorktree {
                path,
                display_name,
            } => {
                lines.push(Line::from(Span::styled(
                    format!("Switch to worktree '{}'?", display_name),
                    Style::default().fg(fg),
                )));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("Path: {}", path),
                    Style::default().fg(dim_fg),
                )));
                lines.push(Line::from(""));
                lines.push(warning_line(
                    "The entire app context will reload from the new path.",
                    warn_fg,
                ));
            }
        }

        lines.push(Line::from(""));
        lines.push(separator_line(inner_width, self.ctx.color_theme.divider_fg));

        self.button_row = lines.len();
        let (btn_line, v_col, c_col) = self.button_line(inner_width);
        self.validate_col = v_col;
        self.cancel_col = c_col;
        lines.push(btn_line);

        lines
    }

    fn title(&self) -> String {
        match &self.kind {
            DialogKind::AddTag { .. } => " Add Tag ",
            DialogKind::CreateBranch { .. } => " Create Branch ",
            DialogKind::Checkout { .. } => " Checkout ",
            DialogKind::CherryPick { .. } => " Cherry Pick ",
            DialogKind::Revert { .. } => " Revert ",
            DialogKind::Drop { .. } => " Drop Commit ",
            DialogKind::Squash { .. } => " Squash with Parent ",
            DialogKind::Merge { .. } => " Merge ",
            DialogKind::Rebase { .. } => " Rebase ",
            DialogKind::Reset { .. } => " Reset ",
            DialogKind::MergePullRequest { .. } => " Merge Pull Request ",
            DialogKind::RenameBranch { .. } => " Rename Branch ",
            DialogKind::DeleteBranch { .. } => " Delete Branch ",
            DialogKind::DeleteTag { .. } => " Delete Tag ",
            DialogKind::PushTag { .. } => " Push Tag ",
            DialogKind::PushBranch { .. } => " Push Branch ",
            DialogKind::PullBranch { .. } => " Pull Branch ",
            DialogKind::CreateBranchFromStash { .. } => " Branch from Stash ",
            DialogKind::StashWithMessage => " Stash Changes ",
            DialogKind::CommitWithMessage => " Commit ",
            DialogKind::CleanUntracked => " Clean Untracked ",
            DialogKind::ConfirmDiscardFile { .. } => " Confirm Discard ",
            DialogKind::ConfirmDiscardAll => " Discard All ",
            DialogKind::ConfirmStageAll => " Stage All ",
            DialogKind::ConfirmUnstageAll => " Unstage All ",
            DialogKind::ConfirmPopStash { .. } => " Pop Stash ",
            DialogKind::ConfirmDropStash { .. } => " Drop Stash ",
            DialogKind::ConfirmDeleteComment { .. } => " Delete Comment ",
            DialogKind::ConfirmPullRequestStateChange { closing: true, .. } => {
                " Close Pull Request "
            }
            DialogKind::ConfirmPullRequestStateChange { closing: false, .. } => {
                " Reopen Pull Request "
            }
            DialogKind::ConfirmPullRequestDraftToggle { to_draft: true, .. } => {
                " Convert to Draft "
            }
            DialogKind::ConfirmPullRequestDraftToggle { to_draft: false, .. } => {
                " Mark Ready for Review "
            }
            DialogKind::PullRequestLabels { .. } => " Labels ",
            DialogKind::PullRequestReviewers { .. } => " Reviewers ",
            DialogKind::AddRemote => " Add Remote ",
            DialogKind::ConfirmDeleteRemote { .. } => " Remove Remote ",
            DialogKind::ChooseRemote { .. } => " Push — Set Upstream ",
            DialogKind::SetUpstream { .. } => " Set Upstream ",
            DialogKind::ConfirmAbortOperation { .. } => " Abort Operation ",
            DialogKind::AmendMessage { .. } => " Amend Commit ",
            DialogKind::ConfirmSwitchWorktree { .. } => " Switch Worktree ",
            DialogKind::ConfirmDeleteWorktree { .. } => " Remove Worktree ",
            DialogKind::AddWorktree => " Add Worktree ",
            DialogKind::CheckoutHasLocalChanges { .. } => " Checkout — Local Changes ",
        }
        .to_string()
    }

    fn input_line(&self, fg: Color, inner_width: u16) -> Line<'static> {
        let focused = self.is_highlighted(DialogElement::Input);
        let theme = &self.ctx.color_theme;
        let input_bg = theme.list_selected_bg;
        let text_fg = if focused { fg } else { theme.detail_label_fg };
        let content_width = inner_width.saturating_sub(2) as usize;
        let padded = format!("{:<width$}", self.input_value, width = content_width);
        Line::from(vec![
            Span::raw(" "),
            Span::styled(padded, Style::default().fg(text_fg).bg(input_bg)),
            Span::raw(" "),
        ])
    }

    fn second_input_line(&self, fg: Color, inner_width: u16) -> Line<'static> {
        let focused = self.is_highlighted(DialogElement::SecondInput);
        let theme = &self.ctx.color_theme;
        let input_bg = theme.list_selected_bg;
        let text_fg = if focused { fg } else { theme.detail_label_fg };
        let content_width = inner_width.saturating_sub(2) as usize;
        let padded = format!("{:<width$}", self.second_input_value, width = content_width);
        Line::from(vec![
            Span::raw(" "),
            Span::styled(padded, Style::default().fg(text_fg).bg(input_bg)),
            Span::raw(" "),
        ])
    }

    /// Render the multi-line input area for commit/amend dialogs.
    ///
    /// Layout per call:
    ///  - Row 0:   subject line + char-count indicator `(N/72)` on right
    ///  - Row 1:   visual separator `─────────`
    ///  - Rows 2+: body lines (or placeholder when body is empty)
    ///
    /// The total number of rendered rows is capped at 8 (1 subject + 1 sep + 6 body).
    fn multiline_input_lines(
        &self,
        fg: Color,
        dim_fg: Color,
        divider_fg: Color,
        inner_width: u16,
    ) -> Vec<Line<'static>> {
        let focused = self.is_highlighted(DialogElement::Input);
        let theme = &self.ctx.color_theme;
        let input_bg = theme.list_selected_bg;
        let text_fg = if focused { fg } else { theme.detail_label_fg };
        let content_width = inner_width.saturating_sub(2) as usize; // minus 2 side spaces

        // Split on '\n'; always have at least one element.
        let value_lines: Vec<&str> = if self.input_value.is_empty() {
            vec![""]
        } else {
            // split('\n') gives correct empty-string parts for trailing newlines.
            self.input_value.split('\n').collect()
        };

        let mut result: Vec<Line<'static>> = Vec::new();

        // --- Subject line (index 0) ---
        let subject = value_lines[0];
        let subject_len = subject.chars().count();
        let counter_text = format!("({}/72)", subject_len);
        let counter_color = if subject_len <= 50 {
            theme.status_success_fg
        } else if subject_len <= 72 {
            theme.list_hash_fg // yellow
        } else {
            theme.status_error_fg
        };

        // Available width for the subject text itself (leave room for " " + counter + " ").
        let counter_width = counter_text.chars().count();
        // We need: 1 (left space) + subject_display + padding + counter + 1 (right space) == inner_width
        // subject_display width = content_width - counter_width
        let subject_display_width = content_width.saturating_sub(counter_width);
        let subject_padded = format!("{:<width$}", subject, width = subject_display_width);

        result.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(subject_padded, Style::default().fg(text_fg).bg(input_bg)),
            Span::styled(counter_text, Style::default().fg(counter_color).bg(input_bg)),
            Span::raw(" "),
        ]));

        // --- Visual separator after subject ---
        let sep = "─".repeat(inner_width as usize);
        result.push(Line::from(Span::styled(
            sep,
            Style::default().fg(divider_fg),
        )));

        // --- Body area (rows 2+): "Body:" label + content, or expand button ---
        // Total cap: 8 rows (1 subject + 1 sep + 1 label + 5 body, or 1+1+1 button).
        if value_lines.len() > 1 {
            // Body exists: label row then content lines (max 5 body rows = 8 total).
            // Skip leading blank lines that come from a "\n\n" git-convention separator
            // so the body area looks clean whether input_value has "\n" or "\n\n".
            let body_content: Vec<&str> = value_lines[1..]
                .iter()
                .skip_while(|l| l.is_empty())
                .copied()
                .collect();
            result.push(Line::from(Span::styled(
                "  Body:",
                Style::default().fg(dim_fg),
            )));
            let max_body_rows = 5usize;
            for body_line in body_content.iter().take(max_body_rows) {
                let padded = format!("{:<width$}", body_line, width = content_width);
                result.push(Line::from(vec![
                    Span::raw(" "),
                    Span::styled(padded, Style::default().fg(text_fg).bg(input_bg)),
                    Span::raw(" "),
                ]));
            }
        } else {
            // No body yet: focusable chevron button (no input_bg so it looks
            // clearly distinct from the subject field). Tab or click to focus,
            // then Enter/click to expand the body area.
            let highlighted = self.is_highlighted(DialogElement::BodyExpand);
            let btn_text = "[ ▼  add body ]";
            let btn_padded = format!("{:^width$}", btn_text, width = inner_width as usize);
            let btn_style = if highlighted {
                Style::default().fg(fg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(divider_fg)
            };
            result.push(Line::from(Span::styled(btn_padded, btn_style)));
        }

        result
    }

    fn checkbox_line(&self, index: usize, label: &str) -> Line<'static> {
        let theme = &self.ctx.color_theme;
        let checked = self.checkboxes.get(index).copied().unwrap_or(false);
        let pointed = self.is_pointed_at(DialogElement::Checkbox(index));

        // Cursor arrow follows hover-or-focus, not both.
        let indicator = if pointed { "▸ " } else { "  " };
        // Render the toggle as a filled/unfilled circle (same look as radios).
        let check_span = if checked {
            Span::styled("● ", Style::default().fg(theme.status_info_fg))
        } else {
            Span::styled("○ ", Style::default().fg(theme.divider_fg))
        };
        let label_style = if pointed {
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.fg)
        };

        Line::from(vec![
            Span::raw(indicator),
            check_span,
            Span::styled(label.to_string(), label_style),
        ])
    }

    /// Same as `checkbox_line` but renders a GitHub-style coloured
    /// chip for the label instead of plain text — used by the
    /// `PullRequestLabels` picker so the row reads like the chip
    /// row above it.
    fn checkbox_line_labelled(
        &self,
        index: usize,
        label: &crate::github::pr::Label,
        inner_width: u16,
    ) -> Vec<Line<'static>> {
        let theme = &self.ctx.color_theme;
        let checked = self.checkboxes.get(index).copied().unwrap_or(false);
        let pointed = self.is_pointed_at(DialogElement::Checkbox(index));

        let indicator = if pointed { "▸ " } else { "  " };
        let check_span = if checked {
            Span::styled("● ", Style::default().fg(theme.status_info_fg))
        } else {
            Span::styled("○ ", Style::default().fg(theme.divider_fg))
        };
        // Width of the indicator + checkmark + chip + the 2-space
        // gap before the description — descriptions wrap onto
        // continuation lines indented to this offset so they line
        // up under the description instead of the row's left edge.
        let chip_width = label.name.chars().count() + 2; // ` name `
        let indent: usize = 2 /* indicator */
            + 2 /* checkmark */
            + chip_width
            + 2 /* gap before description */;
        let avail = (inner_width as usize).saturating_sub(indent).max(1);

        let mut first_row: Vec<Span<'static>> =
            vec![Span::raw(indicator.to_string()), check_span];
        first_row.extend(label_chip_spans(label, theme.fg));

        let desc = label.description.as_deref().unwrap_or("").trim();
        if desc.is_empty() {
            return vec![Line::from(first_row)];
        }
        let wrapped = wrap_description(desc, avail);
        let mut out: Vec<Line<'static>> = Vec::with_capacity(wrapped.len());
        let muted = Style::default().fg(theme.detail_label_fg);
        for (i, chunk) in wrapped.iter().enumerate() {
            if i == 0 {
                let mut row = first_row.clone();
                row.push(Span::styled(format!("  {}", chunk), muted));
                out.push(Line::from(row));
            } else {
                // Continuation row: indent to where the description
                // started on the first row, then the wrapped chunk.
                out.push(Line::from(vec![Span::styled(
                    format!("{}{}", " ".repeat(indent), chunk),
                    muted,
                )]));
            }
        }
        out
    }

    fn radio_line(&self, index: usize, label: &str) -> Line<'static> {
        let theme = &self.ctx.color_theme;
        let selected = self.dropdown_selected == index;
        let pointed = self.is_pointed_at(DialogElement::Radio(index));

        let indicator = if pointed { "▸ " } else { "  " };
        let bullet_span = if selected {
            Span::styled("● ", Style::default().fg(theme.status_info_fg))
        } else {
            Span::styled("○ ", Style::default().fg(theme.divider_fg))
        };
        let label_style = if pointed {
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default().fg(theme.fg)
        } else {
            Style::default().fg(theme.list_commit_message_fg)
        };

        Line::from(vec![
            Span::raw(indicator),
            bullet_span,
            Span::styled(label.to_string(), label_style),
        ])
    }

    fn button_line(&self, inner_width: u16) -> (Line<'static>, (u16, u16), (u16, u16)) {
        let theme = &self.ctx.color_theme;
        let validate_text = "[ Validate ]";
        let cancel_text = "[ Cancel ]";
        let spacing = 4;
        let total = validate_text.len() + cancel_text.len() + spacing;
        let start = (inner_width as usize).saturating_sub(total) / 2;

        let v_start = start as u16;
        let v_end = (start + validate_text.len()) as u16;
        let c_start = (start + validate_text.len() + spacing) as u16;
        let c_end = (start + validate_text.len() + spacing + cancel_text.len()) as u16;

        // Buttons are mutually exclusive — only one can ever be the
        // active target. Use `is_pointed_at` (mouse hover overrides
        // keyboard focus) so we never paint both Validate and Cancel
        // as selected at once.
        let validate_style = if self.is_pointed_at(DialogElement::Validate) {
            Style::default()
                .fg(theme.bg)
                .bg(theme.status_success_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.status_success_fg)
        };

        let cancel_style = if self.is_pointed_at(DialogElement::Cancel) {
            Style::default()
                .fg(theme.bg)
                .bg(theme.status_error_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.status_error_fg)
        };

        let padding = " ".repeat(start);
        let gap = " ".repeat(spacing);

        let line = Line::from(vec![
            Span::raw(padding),
            Span::styled(validate_text, validate_style),
            Span::raw(gap),
            Span::styled(cancel_text, cancel_style),
        ]);

        (line, (v_start, v_end), (c_start, c_end))
    }

    fn confirm(&mut self) {
        if let DialogKind::ConfirmDeleteWorktree { path, is_dirty, .. } = &self.kind {
            let force = *is_dirty;
            let path = path.clone();
            self.tx.send(AppEvent::ExecuteGitAction {
                target: path,
                action: GitAction::DeleteWorktree { force },
            });
            return;
        }
        if let DialogKind::ConfirmSwitchWorktree { path, .. } = &self.kind {
            let path = path.clone();
            self.tx.send(AppEvent::CloseDialog);
            self.tx.send(AppEvent::SwitchWorktree { path });
            return;
        }
        let (target, action) = match &self.kind {
            DialogKind::AddTag { target } => {
                if self.input_value.trim().is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("Tag name cannot be empty".into()));
                    return;
                }
                let annotated = self.checkboxes.first().copied().unwrap_or(false);
                let msg = if annotated && !self.second_input_value.trim().is_empty() {
                    Some(self.second_input_value.clone())
                } else {
                    None
                };
                (
                    target.clone(),
                    GitAction::AddTag {
                        name: self.input_value.clone(),
                        annotated,
                        message: msg,
                    },
                )
            }
            DialogKind::CreateBranch { target }
            | DialogKind::CreateBranchFromStash { target, .. } => {
                if self.input_value.trim().is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("Branch name cannot be empty".into()));
                    return;
                }
                let checkout = self.checkboxes.get(0).copied().unwrap_or(false);
                (
                    target.clone(),
                    GitAction::CreateBranch {
                        name: self.input_value.clone(),
                        checkout,
                    },
                )
            }
            DialogKind::Checkout { target, .. } => (target.clone(), GitAction::Checkout),
            DialogKind::CherryPick { target } => {
                let record_origin = self.checkboxes.get(0).copied().unwrap_or(false);
                let no_commit = self.checkboxes.get(1).copied().unwrap_or(false);
                (
                    target.clone(),
                    GitAction::CherryPick {
                        no_commit,
                        record_origin,
                    },
                )
            }
            DialogKind::Revert { target } => (target.clone(), GitAction::Revert),
            DialogKind::Drop { target } => (target.clone(), GitAction::Drop),
            DialogKind::Squash { target } => (target.clone(), GitAction::SquashWithParent),
            DialogKind::Merge { target, .. } => {
                let no_ff = self.checkboxes.get(0).copied().unwrap_or(true);
                let squash = self.checkboxes.get(1).copied().unwrap_or(false);
                let no_commit = self.checkboxes.get(2).copied().unwrap_or(false);
                (
                    target.clone(),
                    GitAction::Merge {
                        no_ff,
                        squash,
                        no_commit,
                    },
                )
            }
            DialogKind::Rebase { target } => {
                let interactive = self.checkboxes.get(0).copied().unwrap_or(false);
                let ignore_date = self.checkboxes.get(1).copied().unwrap_or(false);
                (
                    target.clone(),
                    GitAction::Rebase {
                        ignore_date,
                        interactive,
                    },
                )
            }
            DialogKind::Reset { target } => {
                let mode = match self.dropdown_selected {
                    0 => "--soft",
                    1 => "--mixed",
                    2 => "--hard",
                    _ => "--mixed",
                };
                (
                    target.clone(),
                    GitAction::Reset {
                        mode: mode.to_string(),
                    },
                )
            }
            DialogKind::MergePullRequest { number, .. } => {
                let method = match self.dropdown_selected {
                    0 => "merge",
                    1 => "squash",
                    2 => "rebase",
                    _ => "squash",
                };
                (
                    number.to_string(),
                    GitAction::MergePullRequest {
                        method: method.to_string(),
                    },
                )
            }
            DialogKind::ConfirmPullRequestStateChange {
                pr_number,
                closing,
                ..
            } => {
                let state = if *closing { "closed" } else { "open" };
                (
                    pr_number.to_string(),
                    GitAction::SetPullRequestState {
                        pr_number: *pr_number,
                        state: state.to_string(),
                    },
                )
            }
            DialogKind::ConfirmPullRequestDraftToggle {
                pr_number,
                node_id,
                to_draft,
                ..
            } => (
                pr_number.to_string(),
                GitAction::SetPullRequestDraft {
                    pr_number: *pr_number,
                    node_id: node_id.clone(),
                    draft: *to_draft,
                },
            ),
            DialogKind::PullRequestLabels {
                pr_number,
                all_labels,
                for_compose,
                ..
            } => {
                // Compose-PR uses the same dialog as the existing-PR
                // labels picker, but on confirm it shouldn't dispatch
                // a GitAction — the PR doesn't exist yet. Send the
                // picked labels to the view to store on the compose
                // state instead.
                if *for_compose {
                    let picked: Vec<crate::github::pr::Label> = all_labels
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| self.checkboxes.get(*i).copied().unwrap_or(false))
                        .map(|(_, l)| l.clone())
                        .collect();
                    self.tx
                        .send(AppEvent::ComposeLabelsPicked { labels: picked });
                    self.tx.send(AppEvent::CloseDialog);
                    return;
                }
                let labels: Vec<String> = all_labels
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| self.checkboxes.get(*i).copied().unwrap_or(false))
                    .map(|(_, l)| l.name.clone())
                    .collect();
                (
                    pr_number.to_string(),
                    GitAction::SetPullRequestLabels {
                        pr_number: *pr_number,
                        labels,
                    },
                )
            }
            DialogKind::PullRequestReviewers {
                pr_number,
                all_users,
                initial,
                ..
            } => {
                // Diff the picker's final state against the initial
                // state so we only POST additions and DELETE removals.
                let mut to_add = Vec::new();
                let mut to_remove = Vec::new();
                for (i, name) in all_users.iter().enumerate() {
                    let was = initial.get(i).copied().unwrap_or(false);
                    let now = self.checkboxes.get(i).copied().unwrap_or(false);
                    if !was && now {
                        to_add.push(name.clone());
                    } else if was && !now {
                        to_remove.push(name.clone());
                    }
                }
                (
                    pr_number.to_string(),
                    GitAction::SetPullRequestReviewers {
                        pr_number: *pr_number,
                        to_add,
                        to_remove,
                    },
                )
            }
            DialogKind::RenameBranch { branch } => {
                if self.input_value.trim().is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("Branch name cannot be empty".into()));
                    return;
                }
                (
                    branch.clone(),
                    GitAction::RenameBranch {
                        new_name: self.input_value.clone(),
                    },
                )
            }
            DialogKind::DeleteBranch { branch, .. } => {
                let force = self.checkboxes.get(0).copied().unwrap_or(false);
                (branch.clone(), GitAction::DeleteBranch { force })
            }
            DialogKind::DeleteTag { tag } => (tag.clone(), GitAction::DeleteTag),
            DialogKind::PushTag { tag } => (tag.clone(), GitAction::PushTag),
            DialogKind::PullBranch { branch } => {
                let rebase = self.checkboxes.get(0).copied().unwrap_or(false);
                (branch.clone(), GitAction::PullBranch { rebase })
            }
            DialogKind::PushBranch { branch } => {
                let force = self.checkboxes.get(0).copied().unwrap_or(false);
                (branch.clone(), GitAction::PushBranch { force })
            }
            DialogKind::StashWithMessage => {
                let message = if self.input_value.is_empty() {
                    None
                } else {
                    Some(self.input_value.clone())
                };
                let include_untracked = self.checkboxes.get(0).copied().unwrap_or(false);
                (
                    String::new(),
                    GitAction::Stash {
                        message,
                        include_untracked,
                    },
                )
            }
            DialogKind::CommitWithMessage => {
                if self.input_value.trim().is_empty() {
                    self.tx.send(AppEvent::NotifyError(
                        "Commit message cannot be empty".into(),
                    ));
                    return;
                }
                let amend = self.checkboxes.get(0).copied().unwrap_or(false);
                (
                    String::new(),
                    GitAction::Commit {
                        message: Self::to_git_message(&self.input_value),
                        amend,
                    },
                )
            }
            DialogKind::ConfirmDiscardFile { file } => {
                (file.clone(), GitAction::DiscardFile { file: file.clone() })
            }
            DialogKind::ConfirmDiscardAll => (String::new(), GitAction::DiscardAll),
            DialogKind::ConfirmStageAll => (String::new(), GitAction::StageAll),
            DialogKind::ConfirmUnstageAll => (String::new(), GitAction::UnstageAll),
            DialogKind::CleanUntracked => (String::new(), GitAction::CleanUntracked),
            DialogKind::ConfirmPopStash { stash_ref } => (stash_ref.clone(), GitAction::PopStash),
            DialogKind::ConfirmDropStash { stash_ref } => (stash_ref.clone(), GitAction::DropStash),
            DialogKind::ConfirmDeleteComment {
                pr_number,
                comment_id,
                is_review,
                ..
            } => (
                comment_id.to_string(),
                GitAction::DeletePrComment {
                    pr_number: *pr_number,
                    is_review: *is_review,
                },
            ),
            DialogKind::AddRemote => {
                if self.input_value.trim().is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("Remote name cannot be empty".into()));
                    return;
                }
                if self.second_input_value.trim().is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("Remote URL cannot be empty".into()));
                    return;
                }
                (
                    self.input_value.trim().to_string(),
                    GitAction::AddRemote {
                        url: self.second_input_value.trim().to_string(),
                    },
                )
            }
            DialogKind::ConfirmDeleteRemote { name } => (name.clone(), GitAction::RemoveRemote),
            DialogKind::ChooseRemote { remotes, branch } => {
                if remotes.is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("No remotes available".into()));
                    return;
                }
                let remote = remotes
                    .get(self.dropdown_selected)
                    .cloned()
                    .unwrap_or_else(|| remotes[0].clone());
                (remote, GitAction::PushSetUpstream { branch: branch.clone() })
            }
            DialogKind::SetUpstream { remotes, branch } => {
                if remotes.is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("No remotes available".into()));
                    return;
                }
                let remote = remotes
                    .get(self.dropdown_selected)
                    .cloned()
                    .unwrap_or_else(|| remotes[0].clone());
                (remote, GitAction::SetUpstream { branch: branch.clone() })
            }
            DialogKind::ConfirmAbortOperation { op_name } => {
                let action = match op_name.as_str() {
                    "rebase" => GitAction::AbortRebase,
                    "merge" => GitAction::AbortMerge,
                    "cherry-pick" => GitAction::AbortCherryPick,
                    _ => GitAction::AbortRebase,
                };
                (String::new(), action)
            }
            DialogKind::AmendMessage { .. } => {
                if self.input_value.trim().is_empty() {
                    self.tx.send(AppEvent::NotifyError(
                        "Commit message cannot be empty".into(),
                    ));
                    return;
                }
                (
                    String::new(),
                    GitAction::Commit {
                        message: Self::to_git_message(&self.input_value),
                        amend: true,
                    },
                )
            }
            DialogKind::AddWorktree => {
                let name = self.input_value.trim().to_string();
                if name.is_empty() {
                    self.tx
                        .send(AppEvent::NotifyError("Name cannot be empty".into()));
                    return;
                }
                let checkout = self.checkboxes.get(0).copied().unwrap_or(false);
                (
                    String::new(),
                    GitAction::AddWorktree { name, checkout },
                )
            }
            DialogKind::CheckoutHasLocalChanges { target, .. } => {
                let action = match self.dropdown_selected {
                    1 => GitAction::CheckoutDiscard,
                    _ => GitAction::CheckoutStash,
                };
                (target.clone(), action)
            }
            // Handled by early-return above; these arms are unreachable at runtime.
            DialogKind::ConfirmSwitchWorktree { .. } => unreachable!(),
            DialogKind::ConfirmDeleteWorktree { .. } => unreachable!(),
        };
        self.tx.send(AppEvent::ExecuteGitAction { target, action });
    }

    /// Normalise a commit message to follow git convention:
    /// subject and body are separated by a blank line (`\n\n`).
    /// If the dialog inserted only a single `\n`, this upgrades it.
    fn to_git_message(input: &str) -> String {
        if let Some(nl) = input.find('\n') {
            let subject = &input[..nl];
            let rest = input[nl + 1..].trim_start_matches('\n');
            if rest.is_empty() {
                subject.to_string()
            } else {
                format!("{}\n\n{}", subject, rest)
            }
        } else {
            input.to_string()
        }
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        std::rc::Rc::make_mut(&mut self.ctx).color_theme = theme.clone();
        self.before.update_color_theme(theme);
    }

    pub fn take_before_view(&mut self) -> View<'a> {
        std::mem::take(&mut self.before)
    }

    pub fn dialog_area(&self) -> Rect {
        self.dialog_area
    }
}

fn word_left(s: &str, cursor: usize) -> usize {
    let bytes = s.as_bytes();
    let mut i = cursor;
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
    let mut i = cursor;
    while i < bytes.len() && !bytes[i].is_ascii_alphanumeric() {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
        i += 1;
    }
    i
}

fn centered_rect_exact(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

fn separator_line(width: u16, color: Color) -> Line<'static> {
    let sep = "─".repeat(width as usize);
    Line::from(Span::styled(sep, Style::default().fg(color)))
}

fn info_line(label: &str, value: &str, label_fg: Color, value_fg: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {} ", label), Style::default().fg(label_fg)),
        Span::styled(value.to_string(), Style::default().fg(value_fg)),
    ])
}

/// PR identification header for dialogs — same colour split as the
/// sub-header: `#NUM` uses the hash accent, the title uses fg BOLD,
/// `·` separator is muted. Single source of truth so close / draft /
/// labels / reviewers all read the same.
fn pr_header_line(
    pr_number: u64,
    pr_title: &str,
    theme: &crate::color::ColorTheme,
) -> Line<'static> {
    Line::from(vec![
        Span::raw("  ".to_string()),
        Span::styled(
            format!("#{}", pr_number),
            Style::default()
                .fg(theme.list_hash_fg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", Style::default().fg(theme.detail_label_fg)),
        Span::styled(
            pr_title.to_string(),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Render a single GitHub label as a coloured chip — `bg` is the
/// label's own colour, `fg` flips to black or white based on a
/// brightness heuristic so the name stays readable.
fn label_chip_spans(
    label: &crate::github::pr::Label,
    fallback_fg: Color,
) -> Vec<Span<'static>> {
    if let Some((bg, fg)) = label.color.as_deref().and_then(label_chip_colours) {
        vec![Span::styled(
            format!(" {} ", label.name),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        )]
    } else {
        vec![Span::styled(
            label.name.clone(),
            Style::default().fg(fallback_fg),
        )]
    }
}

/// Word-wrap `text` into chunks no wider than `width` (by char
/// count). Words longer than `width` are hard-split. Always returns
/// at least one chunk.
fn wrap_description(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;
    for word in text.split_whitespace() {
        let word_w = word.chars().count();
        if word_w > width {
            // Word longer than the line — flush current then
            // hard-split the word.
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
                current_w = 0;
            }
            let mut buf = String::new();
            let mut buf_w = 0;
            for c in word.chars() {
                if buf_w == width {
                    out.push(std::mem::take(&mut buf));
                    buf_w = 0;
                }
                buf.push(c);
                buf_w += 1;
            }
            current = buf;
            current_w = buf_w;
            continue;
        }
        let needed = if current_w == 0 { word_w } else { word_w + 1 };
        if current_w + needed > width {
            out.push(std::mem::take(&mut current));
            current_w = 0;
            current.push_str(word);
            current_w = word_w;
        } else {
            if current_w > 0 {
                current.push(' ');
                current_w += 1;
            }
            current.push_str(word);
            current_w += word_w;
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Parse a `rrggbb` hex string and return `(bg, contrast_fg)` —
/// black foreground on light backgrounds, white on dark.
fn label_chip_colours(hex: &str) -> Option<(Color, Color)> {
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    // Perceptual luminance — same coefficients GitHub uses in CSS
    // to pick the contrast foreground for label chips.
    let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    let fg = if luminance > 140.0 {
        Color::Rgb(0, 0, 0)
    } else {
        Color::Rgb(255, 255, 255)
    };
    Some((Color::Rgb(r, g, b), fg))
}


fn label_line(label: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {}", label),
        Style::default().fg(color),
    ))
}

fn warning_line(text: &str, color: Color) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {}", text),
        Style::default().fg(color),
    ))
}
