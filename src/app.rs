use std::{
    io::{self, Write},
    rc::Rc,
};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph},
    DefaultTerminal, Frame,
};
use rustc_hash::FxHashMap;

use crate::{
    color::{ColorTheme, GraphColorSet},
    config::{save, CoreConfig, CursorType, UiConfig, UserCommand, UserCommandType},
    event::{AppEvent, DialogKind, EventController, GitAction, Sender, UserEvent, UserEventWithCount},
    external::{
        copy_to_clipboard, exec_user_command, exec_user_command_suspend, ExternalCommandParameters,
    },
    git::{actions, diff::DiffEntry, status::UncommittedChanges, Commit, CommitHash, FileChange, Head, Ref, Repository},
    graph::{CellWidthType, Graph, GraphImageManager},
    keybind::KeyBind,
    protocol::ImageProtocol,
    view::{RefreshViewContext, View},
    widget::{branch_detail::BranchMetadata, tag_detail::TagMetadata},
    widget::commit_list::{CommitInfo, CommitListState},
};
use ratatui::style::Color;

#[derive(Debug, Default)]
enum StatusLine {
    #[default]
    None,
    Input(String, Option<u16>, Option<String>),
    NotificationInfo(String),
    NotificationSuccess(String),
    NotificationWarn(String),
    NotificationError(String),
}

#[derive(Clone, Copy)]
pub enum InitialSelection {
    Latest,
    Head,
}

pub enum Ret {
    Quit,
    Refresh(RefreshRequest),
}

pub struct RefreshRequest {
    pub context: RefreshViewContext,
}

#[derive(Debug, Clone)]
pub struct AppContext {
    pub keybind: KeyBind,
    pub core_config: CoreConfig,
    pub ui_config: UiConfig,
    pub color_theme: ColorTheme,
    pub image_protocol: ImageProtocol,
    pub git_user_name: String,
    pub git_user_email: String,
    pub branch_color_map: FxHashMap<String, Color>,
}

impl Default for AppContext {
    fn default() -> Self {
        Self {
            keybind: KeyBind::default(),
            core_config: CoreConfig::default(),
            ui_config: UiConfig::default(),
            color_theme: ColorTheme::default(),
            image_protocol: ImageProtocol::Iterm2,
            git_user_name: String::new(),
            git_user_email: String::new(),
            branch_color_map: FxHashMap::default(),
        }
    }
}

#[derive(Debug, Default)]
struct AppStatus {
    status_line: StatusLine,
    numeric_prefix: String,
    view_area: Rect,
}

#[derive(Debug)]
pub struct App<'a> {
    repository: &'a Repository,
    view: View<'a>,
    app_status: AppStatus,
    ctx: Rc<AppContext>,
    ec: &'a EventController,
}

impl<'a> App<'a> {
    pub fn new(
        repository: &'a Repository,
        graph_image_manager: GraphImageManager<'a>,
        graph: &'a Graph,
        graph_color_set: &'a GraphColorSet,
        cell_width_type: CellWidthType,
        initial_selection: InitialSelection,
        ctx: Rc<AppContext>,
        ec: &'a EventController,
        refresh_view_context: Option<RefreshViewContext>,
    ) -> Self {
        let mut ref_name_to_commit_index_map = FxHashMap::default();
        let commits = graph
            .commits
            .iter()
            .enumerate()
            .map(|(i, commit)| {
                let refs = repository.refs(&commit.commit_hash);
                for r in &refs {
                    ref_name_to_commit_index_map.insert(r.name(), i);
                }
                let (pos_x, _) = graph.commit_pos_map[&commit.commit_hash];
                let graph_color = graph_color_set.get(pos_x).to_ratatui_color();
                CommitInfo::new(commit, refs, graph_color)
            })
            .collect();
        let mut commits: Vec<CommitInfo> = commits;

        if let Some(changes) = repository.uncommitted_changes() {
            if changes.is_dirty() {
                let last_modified = changes.last_modified.map(|dt| dt.fixed_offset());
                let uncommitted_info = CommitInfo::new_uncommitted(
                    Color::Gray,
                    changes.staged.len(),
                    changes.unstaged.len(),
                    changes.untracked.len(),
                    last_modified,
                );
                commits.insert(0, uncommitted_info);
                let mut shifted_map = FxHashMap::default();
                for (name, idx) in &ref_name_to_commit_index_map {
                    shifted_map.insert(*name, *idx + 1);
                }
                ref_name_to_commit_index_map = shifted_map;
            }
        }
        let graph_cell_width = match cell_width_type {
            CellWidthType::Double => (graph.max_pos_x + 1) as u16 * 2,
            CellWidthType::Single => (graph.max_pos_x + 1) as u16,
        };
        let head = repository.head();
        let mut branch_color_map = FxHashMap::default();
        for r in repository.all_refs() {
            match r {
                Ref::Branch { name, target } => {
                    if let Some(&(pos_x, _)) = graph.commit_pos_map.get(target) {
                        let color = graph_color_set.get(pos_x).to_ratatui_color();
                        branch_color_map.insert(name.clone(), color);
                    }
                }
                Ref::RemoteBranch { name, target } => {
                    if let Some(&(pos_x, _)) = graph.commit_pos_map.get(target) {
                        let color = graph_color_set.get(pos_x).to_ratatui_color();
                        branch_color_map.insert(name.clone(), color);
                        if let Some((_, base)) = name.split_once('/') {
                            branch_color_map.insert(base.to_string(), color);
                        }
                    }
                }
                _ => {}
            }
        }
        // Update ctx with branch_color_map for use in footer and other widgets
        let ctx = {
            let mut ctx_mut = (*ctx).clone();
            ctx_mut.branch_color_map = branch_color_map.clone();
            Rc::new(ctx_mut)
        };

        let mut commit_list_state = CommitListState::new(
            commits,
            graph_image_manager,
            graph_cell_width,
            head,
            ref_name_to_commit_index_map,
            branch_color_map,
            ctx.core_config.search.ignore_case,
            ctx.core_config.search.fuzzy,
        );
        if let InitialSelection::Head = initial_selection {
            match repository.head() {
                Head::Branch { name } => commit_list_state.select_ref(name),
                Head::Detached { target } => commit_list_state.select_commit_hash(target),
                Head::None => {}
            }
        }
        let view = View::of_list(commit_list_state, ctx.clone(), ec.sender());

        let mut app = Self {
            repository,
            view,
            app_status: AppStatus::default(),
            ctx,
            ec,
        };

        if let Some(context) = refresh_view_context {
            app.init_with_context(context);
        }

        app
    }
}

impl App<'_> {
    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<Ret, std::io::Error> {
        // Clearing the screen here, as it should be cleared upon refresh
        self.clear_image(None)?;
        terminal.clear()?;

        loop {
            self.prepare_render(terminal)?;
            self.flush_pending_graph_uploads()?;
            terminal.draw(|f| self.render(f))?;
            match self.ec.recv() {
                AppEvent::Key(key) => {
                    match self.app_status.status_line {
                        StatusLine::None | StatusLine::Input(_, _, _) => {
                            // do nothing
                        }
                        StatusLine::NotificationInfo(_)
                        | StatusLine::NotificationSuccess(_)
                        | StatusLine::NotificationWarn(_) => {
                            // Clear message and pass key input as is,
                            // but preserve search match messages
                            if !self.view.is_search_active() {
                                self.clear_status_line();
                            }
                        }
                        StatusLine::NotificationError(_) => {
                            // Clear message and cancel key input
                            self.clear_status_line();
                            continue;
                        }
                    }

                    let user_event = self.ctx.keybind.get(&key);

                    if let Some(UserEvent::Cancel) = user_event {
                        if !self.app_status.numeric_prefix.is_empty() {
                            // Clear numeric prefix and cancel the event
                            self.app_status.numeric_prefix.clear();
                            continue;
                        }
                    }

                    match user_event {
                        Some(UserEvent::ForceQuit) => {
                            self.ec.send(AppEvent::Quit);
                        }
                        Some(ue) => {
                            let event_with_count =
                                process_numeric_prefix(&self.app_status.numeric_prefix, *ue, key);
                            self.view.handle_event(event_with_count, key);
                            self.app_status.numeric_prefix.clear();
                        }
                        None => {
                            if let StatusLine::Input(_, _, _) = self.app_status.status_line {
                                // In input mode, pass all key events to the view
                                // fixme: currently, the only thing that processes key_event is searching the list,
                                //        so this probably works, but it's not the right process...
                                self.app_status.numeric_prefix.clear();
                                self.view.handle_event(
                                    UserEventWithCount::from_event(UserEvent::Unknown),
                                    key,
                                );
                            } else if self.view.is_input_active() {
                                // Config text edit mode: pass all key events
                                self.app_status.numeric_prefix.clear();
                                self.view.handle_event(
                                    UserEventWithCount::from_event(UserEvent::Unknown),
                                    key,
                                );
                            } else if let KeyCode::Char(c) = key.code {
                                // Accumulate numeric prefix
                                if c.is_ascii_digit()
                                    && (c != '0' || !self.app_status.numeric_prefix.is_empty())
                                {
                                    self.app_status.numeric_prefix.push(c);
                                }
                            }
                        }
                    }
                }
                AppEvent::Resize(w, h) => {
                    let _ = (w, h);
                }
                AppEvent::Mouse(mouse) => {
                    self.handle_mouse_event(mouse);
                }
                AppEvent::Quit => {
                    self.cleanup_graph_images()?;
                    return Ok(Ret::Quit);
                }
                AppEvent::OpenDetail => {
                    self.clear_image(Some(terminal))?;
                    self.open_detail();
                }
                AppEvent::OpenUncommitted => {
                    self.info_notification("Uncommitted Changes view coming soon...".into());
                }
                AppEvent::CloseDetail => {
                    terminal.clear()?;
                    self.close_detail();
                }
                AppEvent::OpenUserCommand(n) => {
                    self.clear_image(Some(terminal))?;
                    self.open_user_command(n, Some(terminal));
                }
                AppEvent::CloseUserCommand => {
                    terminal.clear()?;
                    self.close_user_command();
                }
                AppEvent::OpenRefs => {
                    self.open_refs();
                }
                AppEvent::CloseRefs => {
                    self.close_refs();
                }
                AppEvent::OpenHelp => {
                    self.clear_image(None)?;
                    self.open_help();
                }
                AppEvent::CloseHelp => {
                    terminal.clear()?;
                    self.close_help();
                }
                AppEvent::OpenConfig => {
                    self.clear_image(None)?;
                    self.open_config();
                }
                AppEvent::CloseConfig => {
                    terminal.clear()?;
                    self.close_config();
                }
                AppEvent::OpenFileDiff { hash, file_path } => {
                    self.clear_image(Some(terminal))?;
                    self.open_file_diff(hash, file_path);
                }
                AppEvent::CloseDiff => {
                    terminal.clear()?;
                    self.close_diff();
                }
                AppEvent::CloseDiffToDetail => {
                    terminal.clear()?;
                    self.close_diff_to_detail();
                }
                AppEvent::SelectOlderCommit => {
                    self.select_older_commit();
                }
                AppEvent::SelectNewerCommit => {
                    self.select_newer_commit();
                }
                AppEvent::SelectParentCommit => {
                    self.select_parent_commit();
                }
                AppEvent::CopyToClipboard { name, value } => {
                    self.copy_to_clipboard(name, value);
                }
                AppEvent::Refresh(context) => {
                    self.cleanup_graph_images()?;
                    let request = RefreshRequest { context };
                    return Ok(Ret::Refresh(request));
                }
                AppEvent::ClearStatusLine => {
                    self.clear_status_line();
                }
                AppEvent::UpdateStatusInput(msg, cursor_pos, msg_r) => {
                    self.update_status_input(msg, cursor_pos, msg_r);
                }
                AppEvent::NotifyInfo(msg) => {
                    self.info_notification(msg);
                }
                AppEvent::NotifySuccess(msg) => {
                    self.success_notification(msg);
                }
                AppEvent::NotifyWarn(msg) => {
                    self.warn_notification(msg);
                }
                AppEvent::NotifyError(msg) => {
                    self.error_notification(msg);
                }
                // Phase 2 - Git Actions
                AppEvent::OpenDialog(kind) => self.open_dialog(kind),
                AppEvent::CloseDialog => self.close_dialog(),
                AppEvent::DialogConfirm => self.dialog_confirm(),
                AppEvent::DialogCancel => self.close_dialog(),
                AppEvent::DialogInput(_input) => {}
                AppEvent::ExecuteGitAction { target, action } => {
                    self.execute_git_action(target, action);
                }
                AppEvent::OpenBranchDetail { branch_name } => {
                    self.open_branch_detail(branch_name);
                }
                AppEvent::OpenTagDetail { tag_name } => {
                    self.open_tag_detail(tag_name);
                }
                AppEvent::OpenUncommitted => {
                    self.open_uncommitted();
                }
                AppEvent::StageFile { file } => self.stage_file(file),
                AppEvent::UnstageFile { file } => self.unstage_file(file),
                AppEvent::DiscardFile { file } => self.discard_file(file),
            }
        }
    }

    fn prepare_render(&mut self, terminal: &mut DefaultTerminal) -> Result<(), std::io::Error> {
        let area: Rect = terminal.size()?.into();
        let [view_area, _, _] = split_app_areas(area);
        self.update_state(view_area);
        self.view.update_layout(view_area);
        self.view.prepare_graph_uploads();
        Ok(())
    }

    fn flush_pending_graph_uploads(&mut self) -> Result<(), std::io::Error> {
        let uploads = self.view.drain_pending_graph_uploads();
        if uploads.is_empty() {
            return Ok(());
        }

        let mut stdout = io::stdout().lock();
        for upload in uploads {
            stdout.write_all(upload.as_bytes())?;
        }
        stdout.flush()
    }

    fn cleanup_graph_images(&self) -> Result<(), std::io::Error> {
        let image_ids = self.view.graph_image_ids_sorted();
        self.ctx.image_protocol.delete_images(&image_ids)
    }

    fn render(&mut self, f: &mut Frame) {
        let base = Block::default()
            .fg(self.ctx.color_theme.fg);
        f.render_widget(base, f.area());

        let [view_area, _gap, status_line_area] = split_app_areas(f.area());

        self.update_state(view_area);

        self.view.render(f, view_area);
        self.render_status_line(f, status_line_area);
    }
}

impl App<'_> {
    fn render_status_line(&self, f: &mut Frame, area: Rect) {
        let mut spans = match &self.app_status.status_line {
            StatusLine::None if self.app_status.numeric_prefix.is_empty() => vec![],
            StatusLine::None => {
                vec![Span::styled(
                    self.app_status.numeric_prefix.as_str(),
                    Style::default().fg(self.ctx.color_theme.status_input_transient_fg),
                )]
            }
            StatusLine::Input(msg, _, transient_msg) => {
                let msg_w = console::measure_text_width(msg.as_str());
                if let Some(t_msg) = transient_msg {
                    let t_msg_w = console::measure_text_width(t_msg.as_str());
                    let pad_w = area.width as usize - msg_w - t_msg_w - 2;
                    vec![
                        Span::styled(msg.as_str(), Style::default().fg(self.ctx.color_theme.status_input_fg)),
                        Span::raw(" ".repeat(pad_w)),
                        Span::styled(t_msg.as_str(), Style::default().fg(self.ctx.color_theme.status_input_transient_fg)),
                    ]
                } else {
                    vec![Span::styled(
                        msg.as_str(),
                        Style::default().fg(self.ctx.color_theme.status_input_fg),
                    )]
                }
            }
            StatusLine::NotificationInfo(msg) => {
                vec![Span::styled(msg.as_str(), Style::default().fg(self.ctx.color_theme.status_info_fg))]
            }
            StatusLine::NotificationSuccess(msg) => {
                vec![Span::styled(
                    msg.as_str(),
                    Style::default().fg(self.ctx.color_theme.status_success_fg).add_modifier(Modifier::BOLD),
                )]
            }
            StatusLine::NotificationWarn(msg) => {
                vec![Span::styled(
                    msg.as_str(),
                    Style::default().fg(self.ctx.color_theme.status_warn_fg).add_modifier(Modifier::BOLD),
                )]
            }
            StatusLine::NotificationError(msg) => {
                vec![Span::styled(
                    format!("ERROR: {msg}"),
                    Style::default().fg(self.ctx.color_theme.status_error_fg).add_modifier(Modifier::BOLD),
                )]
            }
        };

        let dim_separator = Style::default().fg(Color::Rgb(59, 66, 97));
        let dim_text = Style::default().fg(Color::Rgb(86, 95, 137));
        let is_search_active = self.view.is_search_active();
        let is_search_querying = self.view.is_search_querying();
        let is_config_active = self.view.is_config_active();
        let show_enhanced = matches!(
            &self.app_status.status_line,
            StatusLine::None | StatusLine::NotificationInfo(_)
        ) && !is_search_active && !is_config_active;
        let show_shortcuts = matches!(&self.app_status.status_line, StatusLine::None) || is_search_active || is_config_active;
        let is_diff = matches!(&self.view, View::Diff(_));

        let status_area = if show_shortcuts {
            let shortcut_text: String = if is_search_querying {
                "Esc:cancel".into()
            } else if is_search_active {
                let (ignore_case, fuzzy) = self.view.search_case_fuzzy().unwrap_or((false, false));
                // ON = case-sensitive (ignore_case=false), OFF = case-insensitive (ignore_case=true)
                let case_str = if ignore_case { "[OFF]" } else { "[ON]" };
                let fuzzy_str = if fuzzy { "[ON]" } else { "[OFF]" };
                format!("s:case{case_str} z:fuzzy{fuzzy_str} n:next N:prev Esc:clear")
            } else if is_config_active {
                "Enter/←→:cycle Esc:close".into()
            } else {
                match &self.view {
                    View::List(_) => "f:search c:hash C:subject r:refresh ?:help q:quit".into(),
                    View::Diff(_) => self.view.diff_footer_hint().unwrap_or_else(|| "c:copy-path Esc:close".into()),
                    View::Detail(_) => "c:hash C:subject Esc:close".into(),
                    View::Refs(_) => "Esc:close".into(),
                    View::Help(_) => "Esc:close".into(),
                    View::UserCommand(_) => "Esc:close".into(),
                    _ => "f:search ?:help q:quit r:refresh".into(),
                }
            };

            let right_constraint = if is_search_querying {
                Constraint::Length(16)
            } else if is_search_active {
                Constraint::Length(58)
            } else if is_config_active {
                Constraint::Length(32)
            } else if is_diff {
                Constraint::Length(55)
            } else {
                Constraint::Length(shortcut_text.len() as u16 + 4)
            };
            let [left_area, right_area] = Layout::horizontal([
                Constraint::Min(0),
                right_constraint,
            ]).areas(area);
            let shortcut_spans = vec![Span::styled(shortcut_text, dim_text)];
            let shortcut_line = Line::from(shortcut_spans);
            let shortcut_paragraph = Paragraph::new(shortcut_line)
                .style(Style::default().bg(Color::Rgb(36, 40, 59)))
                .alignment(Alignment::Right)
                .block(Block::default().padding(Padding::horizontal(1)));

            f.render_widget(shortcut_paragraph, right_area);

            if is_config_active {
                let config_hint = Line::from(vec![Span::styled(
                    "Changes are applied to your config.toml",
                    dim_text,
                )]);
                let config_hint_paragraph = Paragraph::new(config_hint)
                    .style(Style::default().bg(Color::Rgb(36, 40, 59)))
                    .block(Block::default().padding(Padding::horizontal(1)));
                f.render_widget(config_hint_paragraph, left_area);
                // We still need a valid area for the rest, use a zero-height area
                Rect::new(left_area.x, left_area.y, 0, left_area.height)
            } else {
                left_area
            }
        } else {
            area
        };

        if show_enhanced {
            match self.repository.head() {
                Head::Branch { name } => {
                    let branch_color = self.ctx.branch_color_map
                        .get(name)
                        .copied()
                        .unwrap_or(Color::Rgb(122, 162, 247));
                    spans.push(Span::styled(
                        "HEAD → ",
                        Style::default().fg(Color::Rgb(125, 207, 255)).add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(
                        "⎇ ",
                        Style::default().fg(branch_color),
                    ));
                    spans.push(Span::styled(
                        name.clone(),
                        Style::default()
                            .fg(branch_color)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                Head::Detached { .. } => {
                    spans.push(Span::styled(
                        "HEAD → ",
                        Style::default().fg(Color::Rgb(125, 207, 255)).add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(
                        "● detached",
                        Style::default().fg(Color::Rgb(255, 158, 100)).add_modifier(Modifier::BOLD),
                    ));
                }
                Head::None => {}
            }

            if let Some(changes) = self.repository.uncommitted_changes() {
                let staged = changes.staged.len();
                let unstaged = changes.unstaged.len();
                let untracked = changes.untracked.len();

                if changes.is_dirty() {
                    spans.push(Span::styled(" │ ", dim_separator));
                    if staged > 0 {
                        spans.push(Span::styled(
                            format!(" ✓{}", staged),
                            Style::default().fg(Color::Rgb(158, 206, 106)),
                        ));
                    }
                    if unstaged > 0 {
                        spans.push(Span::styled(
                            format!(" ⚡{}", unstaged),
                            Style::default().fg(Color::Rgb(224, 175, 104)),
                        ));
                    }
                    if untracked > 0 {
                        spans.push(Span::styled(
                            format!(" ?{}", untracked),
                            Style::default().fg(Color::Rgb(187, 154, 247)),
                        ));
                    }
                }
            }
        }

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line)
            .style(Style::default().bg(Color::Rgb(36, 40, 59)))
            .block(Block::default().padding(Padding::horizontal(1)));
        f.render_widget(paragraph, status_area);

        if let StatusLine::Input(_, Some(cursor_pos), _) = &self.app_status.status_line {
            let (x, y) = (area.x + cursor_pos + 1, area.y);
            match &self.ctx.ui_config.common.cursor_type {
                CursorType::Native => {
                    f.set_cursor_position((x, y));
                }
                CursorType::Virtual(cursor) => {
                    let style = Style::default().fg(self.ctx.color_theme.virtual_cursor_fg);
                    f.buffer_mut().set_string(x, y, cursor, style);
                }
            }
        }
    }
}

fn split_app_areas(area: Rect) -> [Rect; 3] {
    Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1), // gap between commit list and status bar
        Constraint::Length(1), // status bar
    ])
    .areas(area)
}

impl App<'_> {
    fn update_state(&mut self, view_area: Rect) {
        self.app_status.view_area = view_area;
    }

    fn clear_image(&self, terminal: Option<&mut DefaultTerminal>) -> Result<(), std::io::Error> {
        // Sometimes the first image fails to render after a full screen clear
        // As a workaround, the first area is preserved when a full clear is not required
        if let Some(t) = terminal {
            for y in 1..t.size()?.height {
                self.ctx.image_protocol.clear_line(y);
            }
        } else {
            self.ctx.image_protocol.clear();
        }
        Ok(())
    }

    fn open_detail(&mut self) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => view.take_list_state(),
            View::UserCommand(ref mut view) => view.take_list_state(),
            _ => return,
        };
        let (commit, changes, refs) = selected_commit_details(self.repository, &commit_list_state);
        self.view = View::of_detail(
            commit_list_state,
            commit,
            changes,
            refs,
            self.ctx.clone(),
            self.ec.sender(),
        );
    }

    fn close_detail(&mut self) {
        if let View::Detail(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
        }
    }

    fn open_file_diff(&mut self, hash: String, file_path: String) {
        let commit_list_state = match self.view {
            View::Detail(ref mut view) => view.take_list_state(),
            View::Diff(ref mut view) => view.take_list_state(),
            _ => return,
        };
        let all_files = self.repository
            .commit_detail(&CommitHash::from(hash.as_str()))
            .1
            .iter()
            .map(|c| match c {
                crate::git::FileChange::Add { path, .. } | crate::git::FileChange::Modify { path, .. } | crate::git::FileChange::Delete { path, .. } => path.clone(),
                crate::git::FileChange::Move { to, .. } => to.clone(),
            })
            .collect();
        match DiffEntry::load_for_file(self.repository.path(), &hash, &file_path) {
            Ok(diff_entry) => {
                let title = format!("Diff: {}", file_path);
                self.view = View::of_diff_with_entries(
                    commit_list_state,
                    vec![diff_entry],
                    self.ctx.clone(),
                    self.ec.sender(),
                    title,
                    hash,
                    all_files,
                );
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
                self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
            }
        }
    }

    fn close_diff(&mut self) {
        if let View::Diff(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
        }
    }

    fn close_diff_to_detail(&mut self) {
        if let View::Diff(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            let (commit, changes, refs) = selected_commit_details(self.repository, &commit_list_state);
            self.view = View::of_detail(
                commit_list_state,
                commit,
                changes,
                refs,
                self.ctx.clone(),
                self.ec.sender(),
            );
        }
    }

    fn open_user_command(
        &mut self,
        user_command_number: usize,
        terminal: Option<&mut DefaultTerminal>,
    ) {
        let clear = match extract_user_command_by_number(user_command_number, &self.ctx)
            .map(|c| &c.r#type)
        {
            Ok(UserCommandType::Inline) => {
                self.open_user_command_inline(user_command_number);
                false
            }
            Ok(UserCommandType::Silent) => {
                self.open_user_command_silent(user_command_number);
                true
            }
            Ok(UserCommandType::Suspend) => {
                self.open_user_command_suspend(user_command_number);
                true
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
                false
            }
        };
        if clear {
            if let Some(t) = terminal {
                if let Err(err) = t.clear() {
                    let msg = format!("Failed to clear terminal: {err:?}");
                    self.ec.send(AppEvent::NotifyError(msg));
                }
            }
        }
    }

    fn open_user_command_inline(&mut self, user_command_number: usize) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => view.as_list_state(),
            View::Detail(ref mut view) => view.as_list_state(),
            View::UserCommand(ref mut view) => view.as_list_state(),
            _ => return,
        };
        let (commit, _, refs) = selected_commit_details(self.repository, commit_list_state);
        let result = build_external_command_parameters_and_exec_command(
            &commit,
            &refs,
            user_command_number,
            self.app_status.view_area,
            &self.ctx,
        );
        match result {
            Ok(output) => {
                // take list state only when the command execution is successful, to avoid losing the state when the command fails
                let commit_list_state = match self.view {
                    View::List(ref mut view) => view.take_list_state(),
                    View::Detail(ref mut view) => view.take_list_state(),
                    View::UserCommand(ref mut view) => view.take_list_state(),
                    _ => return,
                };
                self.view = View::of_user_command(
                    commit_list_state,
                    output,
                    user_command_number,
                    self.ctx.clone(),
                    self.ec.sender(),
                );
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
            }
        };
    }

    fn open_user_command_silent(&mut self, user_command_number: usize) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => view.as_list_state(),
            View::Detail(ref mut view) => view.as_list_state(),
            View::UserCommand(ref mut view) => view.as_list_state(),
            _ => return,
        };
        let (commit, _, refs) = selected_commit_details(self.repository, commit_list_state);
        let result = build_external_command_parameters_and_exec_command(
            &commit,
            &refs,
            user_command_number,
            self.app_status.view_area,
            &self.ctx,
        );
        match result {
            Ok(_) => {
                if extract_user_command_refresh_by_number(user_command_number, &self.ctx) {
                    self.view.refresh();
                }
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
            }
        }
    }

    fn open_user_command_suspend(&mut self, user_command_number: usize) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => view.as_list_state(),
            View::Detail(ref mut view) => view.as_list_state(),
            View::UserCommand(ref mut view) => view.as_list_state(),
            _ => return,
        };
        let (commit, _, refs) = selected_commit_details(self.repository, commit_list_state);
        match build_external_command_parameters(
            &commit,
            &refs,
            user_command_number,
            self.app_status.view_area,
            &self.ctx,
        ) {
            Ok(params) => {
                self.ec.suspend();
                let exec_result = exec_user_command_suspend(params);
                self.ec.resume();

                if extract_user_command_refresh_by_number(user_command_number, &self.ctx) {
                    self.view.refresh();
                }

                // notify after resuming and refreshing
                if let Err(err) = exec_result {
                    self.ec.send(AppEvent::NotifyError(err));
                }
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
            }
        }
    }

    fn close_user_command(&mut self) {
        if let View::UserCommand(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
        }
    }

    fn open_refs(&mut self) {
        if let View::List(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            let refs = self.repository.all_refs().into_iter().cloned().collect();
            self.view = View::of_refs(commit_list_state, refs, self.ctx.clone(), self.ec.sender());
        }
    }

    fn close_refs(&mut self) {
        if let View::Refs(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
        }
    }

    fn open_help(&mut self) {
        let before_view = std::mem::take(&mut self.view);
        self.view = View::of_help(before_view, self.ctx.clone(), self.ec.sender());
    }

    fn close_help(&mut self) {
        if let View::Help(ref mut view) = self.view {
            self.view = view.take_before_view();
        }
    }

    fn open_config(&mut self) {
        let before = std::mem::take(&mut self.view);
        self.view = View::of_config(before, self.ctx.clone(), self.ec.sender());
    }

    fn close_config(&mut self) {
        if let View::Config(ref mut view) = self.view {
            let core = view.core_config().clone();
            let ui = view.ui_config().clone();
            let old_mouse = self.ctx.ui_config.common.mouse_enabled;
            self.view = view.take_before_view();
            let ctx = Rc::make_mut(&mut self.ctx);
            ctx.core_config = core;
            ctx.ui_config = ui.clone();
            let new_mouse = ui.common.mouse_enabled;
            if old_mouse != new_mouse {
                let _ = if new_mouse {
                    ratatui::crossterm::execute!(
                        std::io::stdout(),
                        ratatui::crossterm::event::EnableMouseCapture
                    )
                } else {
                    ratatui::crossterm::execute!(
                        std::io::stdout(),
                        ratatui::crossterm::event::DisableMouseCapture
                    )
                };
            }
            // Force full app refresh so config changes (graph style, protocol, etc.) take effect
            self.ec.send(AppEvent::Refresh(RefreshViewContext::List {
                list_context: crate::view::ListRefreshViewContext {
                    commit_hash: String::new(),
                    selected: 0,
                    height: 20,
                    scroll_to_top: false,
                },
            }));
        }
    }

    fn select_older_commit(&mut self) {
        if let View::Detail(ref mut view) = self.view {
            view.select_older_commit(self.repository);
        } else if let View::Diff(ref mut view) = self.view {
            let hash = view.as_list_state().selected_commit_hash().as_str().to_string();
            if let Err(err) = view.select_older_commit(self.repository.path(), &hash) {
                self.ec.send(AppEvent::NotifyError(err));
            }
        } else if let View::UserCommand(ref mut view) = self.view {
            view.select_older_commit(
                self.repository,
                self.app_status.view_area,
                build_external_command_parameters_and_exec_command,
            );
        }
    }

    fn select_newer_commit(&mut self) {
        if let View::Detail(ref mut view) = self.view {
            view.select_newer_commit(self.repository);
        } else if let View::Diff(ref mut view) = self.view {
            let hash = view.as_list_state().selected_commit_hash().as_str().to_string();
            if let Err(err) = view.select_newer_commit(self.repository.path(), &hash) {
                self.ec.send(AppEvent::NotifyError(err));
            }
        } else if let View::UserCommand(ref mut view) = self.view {
            view.select_newer_commit(
                self.repository,
                self.app_status.view_area,
                build_external_command_parameters_and_exec_command,
            );
        }
    }

    fn select_parent_commit(&mut self) {
        if let View::Detail(ref mut view) = self.view {
            view.select_parent_commit(self.repository);
        } else if let View::Diff(ref mut view) = self.view {
            let hash = view.as_list_state().selected_commit_hash().as_str().to_string();
            if let Err(err) = view.select_parent_commit(self.repository.path(), &hash) {
                self.ec.send(AppEvent::NotifyError(err));
            }
        } else if let View::UserCommand(ref mut view) = self.view {
            view.select_parent_commit(
                self.repository,
                self.app_status.view_area,
                build_external_command_parameters_and_exec_command,
            );
        }
    }

    fn init_with_context(&mut self, context: RefreshViewContext) {
        if let View::List(ref mut view) = self.view {
            view.reset_commit_list_with(context.list_context());
        }
        match context {
            RefreshViewContext::List { .. } => {}
            RefreshViewContext::Detail { .. } => {
                self.open_detail();
            }
            RefreshViewContext::Diff { .. } => {
                // Diff global removed; only per-file diffs are supported now
            }
            RefreshViewContext::UserCommand {
                user_command_context,
                ..
            } => {
                self.open_user_command(user_command_context.n, None);
            }
            RefreshViewContext::Refs { refs_context, .. } => {
                self.open_refs();
                if let View::Refs(ref mut view) = self.view {
                    view.reset_refs_with(refs_context);
                }
            }
        }
    }

    fn clear_status_line(&mut self) {
        self.app_status.status_line = StatusLine::None;
    }

    fn handle_mouse_event(&mut self, mouse: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                let _ = self.view.handle_event(
                    crate::event::UserEventWithCount::new(
                        crate::event::UserEvent::ScrollUp,
                        3,
                    ),
                    ratatui::crossterm::event::KeyEvent::new(
                        ratatui::crossterm::event::KeyCode::Up,
                        ratatui::crossterm::event::KeyModifiers::NONE,
                    ),
                );
            }
            MouseEventKind::ScrollDown => {
                let _ = self.view.handle_event(
                    crate::event::UserEventWithCount::new(
                        crate::event::UserEvent::ScrollDown,
                        3,
                    ),
                    ratatui::crossterm::event::KeyEvent::new(
                        ratatui::crossterm::event::KeyCode::Down,
                        ratatui::crossterm::event::KeyModifiers::NONE,
                    ),
                );
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.view.handle_click(mouse.column, mouse.row);
            }
            MouseEventKind::Moved => {
                self.view.handle_mouse_move(mouse.column, mouse.row);
            }
            _ => {}
        }
    }

    fn update_status_input(
        &mut self,
        msg: String,
        cursor_pos: Option<u16>,
        transient_msg: Option<String>,
    ) {
        self.app_status.status_line = StatusLine::Input(msg, cursor_pos, transient_msg);
    }

    fn info_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationInfo(msg);
    }

    fn success_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationSuccess(msg);
    }

    fn warn_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationWarn(msg);
    }

    fn error_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationError(msg);
    }

    fn copy_to_clipboard(&self, name: String, value: String) {
        match copy_to_clipboard(value, &self.ctx.core_config.external.clipboard) {
            Ok(_) => {
                let msg = format!("Copied {name} to clipboard successfully");
                self.ec.send(AppEvent::NotifySuccess(msg));
            }
            Err(msg) => {
                self.ec.send(AppEvent::NotifyError(msg));
            }
        }
    }

    // Phase 2 - Dialog management
    fn open_dialog(&mut self, kind: DialogKind) {
        let before = std::mem::take(&mut self.view);
        self.view = View::Dialog(Box::new(crate::view::dialog::DialogView::new(
            before,
            kind,
            self.ctx.clone(),
            self.ec.sender(),
        )));
    }

    fn close_dialog(&mut self) {
        if let View::Dialog(mut dialog) = std::mem::take(&mut self.view) {
            self.view = dialog.take_before_view();
        }
    }

    fn dialog_confirm(&mut self) {
        // DialogConfirm is handled by the DialogView itself via handle_event
        // This method is called when the dialog sends DialogConfirm event
        // In practice, DialogView sends ExecuteGitAction directly on Confirm
    }

    // Phase 2 - Git Actions execution
    fn execute_git_action(&mut self, target: String, action: GitAction) {
        let repo_path = self.repository.path();
        let result = match action {
            GitAction::Checkout => {
                if target.starts_with("refs/stash") {
                    actions::checkout_commit(repo_path, &target)
                } else {
                    actions::checkout_commit(repo_path, &target)
                }
            }
            GitAction::CreateBranch { name, checkout } => {
                actions::create_branch_at(repo_path, &name, &target, checkout)
            }
            GitAction::AddTag { name, annotated, message } => {
                let msg = if annotated { message.as_deref() } else { None };
                actions::create_tag(repo_path, &name, &target, msg)
            }
            GitAction::CherryPick { no_commit, record_origin } => {
                actions::cherry_pick(repo_path, &target, no_commit, record_origin)
            }
            GitAction::Revert => actions::revert_commit(repo_path, &target),
            GitAction::Drop => actions::drop_commit(repo_path, &target),
            GitAction::Merge { no_ff, squash, no_commit } => {
                actions::merge_commit(repo_path, &target, no_ff, squash, no_commit)
            }
            GitAction::Rebase { ignore_date, interactive } => {
                actions::rebase_onto(repo_path, &target, ignore_date, interactive)
            }
            GitAction::Reset { mode } => actions::reset(repo_path, &target, &mode),
            GitAction::DeleteBranch { force } => actions::delete_branch(repo_path, &target, force),
            GitAction::RenameBranch { new_name } => actions::rename_branch(repo_path, &target, &new_name),
            GitAction::PushBranch { force } => actions::push_branch(repo_path, &target, force),
            GitAction::PullBranch { rebase } => {
                if rebase {
                    actions::pull_branch(repo_path, &target)
                } else {
                    actions::pull_branch(repo_path, &target)
                }
            }
            GitAction::Fetch => actions::fetch(repo_path),
            GitAction::DeleteTag => actions::delete_tag(repo_path, &target),
            GitAction::PushTag => actions::push_commit(repo_path, &target),
            GitAction::ApplyStash => actions::apply_stash(repo_path, &target),
            GitAction::PopStash => actions::pop_stash(repo_path, &target),
            GitAction::DropStash => actions::drop_stash(repo_path, &target),
            GitAction::CreateBranchFromStash { branch_name } => {
                actions::create_branch_from_stash(repo_path, &branch_name, &target)
            }
            GitAction::StageFile { file } => actions::stage_file(repo_path, &file),
            GitAction::StageAll => actions::stage_all(repo_path),
            GitAction::UnstageFile { file } => actions::unstage_file(repo_path, &file),
            GitAction::UnstageAll => actions::unstage_all(repo_path),
            GitAction::DiscardFile { file } => actions::discard_file(repo_path, &file),
            GitAction::DiscardAll => actions::discard_all(repo_path),
            GitAction::Stash { message } => actions::stash(repo_path, message.as_deref()),
            GitAction::Commit { message } => actions::commit(repo_path, &message),
            GitAction::CleanUntracked => actions::clean_untracked(repo_path),
            GitAction::Push => actions::push_commit(repo_path, &target),
            GitAction::CreateArchive => {
                // git archive branch > branch.zip
                std::process::Command::new("git")
                    .current_dir(repo_path)
                    .args(["archive", "--format=zip", "-o", &format!("{}.zip", target), &target])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                    .map_err(|e| format!("Failed to create archive: {}", e))
            }
        };

        match result {
            Ok(msg) => {
                self.close_dialog();
                if !msg.is_empty() {
                    self.ec.send(AppEvent::NotifySuccess(msg));
                } else {
                    self.ec.send(AppEvent::NotifySuccess("Operation completed successfully".into()));
                }
                self.ec.send(AppEvent::Refresh(RefreshViewContext::List {
                    list_context: crate::view::ListRefreshViewContext {
                        commit_hash: String::new(),
                        selected: 0,
                        height: 20,
                        scroll_to_top: false,
                    },
                }));
            }
            Err(msg) => {
                self.ec.send(AppEvent::NotifyError(msg));
            }
        }
    }

    fn open_branch_detail(&mut self, branch_name: String) {
        let repo_path = self.repository.path();
        let is_remote = branch_name.contains('/');
        let upstream = actions::branch_upstream(repo_path, &branch_name).ok();
        let ahead = actions::branch_ahead_count(repo_path, &branch_name).unwrap_or_default();
        let behind = actions::branch_behind_count(repo_path, &branch_name).unwrap_or_default();
        let (tip_hash, tip_subject) = actions::branch_tip_info(repo_path, &branch_name)
            .map(|s| {
                let mut parts = s.splitn(2, ' ');
                (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string())
            })
            .unwrap_or_default();
        let metadata = BranchMetadata {
            branch_name: branch_name.clone(),
            is_remote,
            tip_hash,
            tip_subject,
            tip_author: String::new(),
            tip_date: String::new(),
            upstream,
            ahead,
            behind,
        };
        self.view = View::BranchDetail(Box::new(crate::view::branch_detail::BranchDetailView::new(
            branch_name,
            metadata,
            self.ctx.clone(),
            self.ec.sender(),
        )));
    }

    fn open_tag_detail(&mut self, tag_name: String) {
        let repo_path = self.repository.path();
        let metadata = TagMetadata {
            tag_name: tag_name.clone(),
            tag_type: "Tag".to_string(),
            target_hash: String::new(),
            target_subject: String::new(),
            tagger: None,
            date: None,
            message: None,
        };
        self.view = View::TagDetail(Box::new(crate::view::tag_detail::TagDetailView::new(
            tag_name,
            metadata,
            self.ctx.clone(),
            self.ec.sender(),
        )));
    }

    fn open_uncommitted(&mut self) {
        let changes = UncommittedChanges::load(self.repository.path()).unwrap_or_default();
        let convert = |f: &crate::git::status::FileStatus| crate::widget::uncommitted::UncommittedFile {
            status: f.status.clone(),
            path: f.path.clone(),
            old_path: f.old_path.clone(),
            additions: 0,
            deletions: 0,
        };
        let staged: Vec<_> = changes.staged.iter().map(convert).collect();
        let unstaged: Vec<_> = changes.unstaged.iter().map(convert).collect();
        self.view = View::Uncommitted(Box::new(crate::view::uncommitted::UncommittedView::new(
            unstaged,
            staged,
            self.ctx.clone(),
            self.ec.sender(),
        )));
    }

    fn stage_file(&mut self, file: String) {
        match actions::stage_file(self.repository.path(), &file) {
            Ok(msg) => self.ec.send(AppEvent::NotifySuccess(msg)),
            Err(msg) => self.ec.send(AppEvent::NotifyError(msg)),
        }
    }

    fn unstage_file(&mut self, file: String) {
        match actions::unstage_file(self.repository.path(), &file) {
            Ok(msg) => self.ec.send(AppEvent::NotifySuccess(msg)),
            Err(msg) => self.ec.send(AppEvent::NotifyError(msg)),
        }
    }

    fn discard_file(&mut self, file: String) {
        match actions::discard_file(self.repository.path(), &file) {
            Ok(msg) => self.ec.send(AppEvent::NotifySuccess(msg)),
            Err(msg) => self.ec.send(AppEvent::NotifyError(msg)),
        }
    }
}

fn selected_commit_details(
    repository: &Repository,
    commit_list_state: &CommitListState,
) -> (Commit, Vec<FileChange>, Vec<Ref>) {
    let selected = commit_list_state.selected_commit_hash().clone();
    let (commit, changes) = repository.commit_detail(&selected);
    let refs: Vec<Ref> = repository.refs(&selected).into_iter().cloned().collect();
    (commit, changes, refs)
}

fn process_numeric_prefix(
    numeric_prefix: &str,
    user_event: UserEvent,
    _key_event: KeyEvent,
) -> UserEventWithCount {
    if user_event.is_countable() {
        let count = if numeric_prefix.is_empty() {
            1
        } else {
            numeric_prefix.parse::<usize>().unwrap_or(1)
        };
        UserEventWithCount::new(user_event, count)
    } else {
        UserEventWithCount::from_event(user_event)
    }
}

fn extract_user_command_by_number(
    user_command_number: usize,
    ctx: &AppContext,
) -> Result<&UserCommand, String> {
    ctx.core_config
        .user_command
        .commands
        .get(&user_command_number.to_string())
        .ok_or_else(|| format!("No user command configured for number {user_command_number}",))
}

fn extract_user_command_refresh_by_number(user_command_number: usize, ctx: &AppContext) -> bool {
    extract_user_command_by_number(user_command_number, ctx)
        .map(|c| c.refresh)
        .unwrap_or_default()
}

fn build_external_command_parameters_and_exec_command(
    commit: &Commit,
    refs: &[Ref],
    user_command_number: usize,
    view_area: Rect,
    ctx: &AppContext,
) -> Result<String, String> {
    build_external_command_parameters(commit, refs, user_command_number, view_area, ctx)
        .and_then(exec_user_command)
}

fn build_external_command_parameters<'a>(
    commit: &'a Commit,
    refs: &'a [Ref],
    user_command_number: usize,
    view_area: Rect,
    ctx: &'a AppContext,
) -> Result<ExternalCommandParameters<'a>, String> {
    let command = &extract_user_command_by_number(user_command_number, ctx)?.commands;
    let target_hash = commit.commit_hash.as_str();
    let parent_hashes = commit
        .parent_commit_hashes
        .iter()
        .map(|c| c.as_str())
        .collect();

    let mut all_refs = vec![];
    let mut branches = vec![];
    let mut remote_branches = vec![];
    let mut tags = vec![];
    for r in refs {
        match r {
            Ref::Tag { .. } => tags.push(r.name()),
            Ref::Branch { .. } => branches.push(r.name()),
            Ref::RemoteBranch { .. } => remote_branches.push(r.name()),
            Ref::Stash { .. } => continue, // skip stashes
        }
        all_refs.push(r.name());
    }

    let area_width = view_area.width.saturating_sub(4); // minus the left and right padding
    let area_height = (view_area.height.saturating_sub(1))
        .min(ctx.ui_config.user_command.height)
        .saturating_sub(1); // minus the top border
    Ok(ExternalCommandParameters {
        command,
        target_hash,
        parent_hashes,
        all_refs,
        branches,
        remote_branches,
        tags,
        area_width,
        area_height,
    })
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rustfmt::skip]
    #[rstest]
    #[case("",    UserEvent::NavigateDown, UserEventWithCount::new(UserEvent::NavigateDown, 1))] // no prefix
    #[case("5",   UserEvent::NavigateUp,   UserEventWithCount::new(UserEvent::NavigateUp, 5))] // with prefix
    #[case("0",   UserEvent::PageDown,     UserEventWithCount::new(UserEvent::PageDown, 1))] // zero should be converted to 1
    #[case("42",  UserEvent::ScrollDown,   UserEventWithCount::new(UserEvent::ScrollDown, 42))] // multi-digit number
    #[case("999", UserEvent::PageDown,     UserEventWithCount::new(UserEvent::PageDown, 999))] // large number
    #[case("abc", UserEvent::ScrollUp,     UserEventWithCount::new(UserEvent::ScrollUp, 1))] // should fallback to 1
    #[case("5",   UserEvent::Quit,         UserEventWithCount::new(UserEvent::Quit, 1))] // non-countable event with prefix
    #[case("",    UserEvent::Confirm,      UserEventWithCount::new(UserEvent::Confirm, 1))] // non-countable event without prefix
    fn test_process_numeric_prefix(
        #[case] numeric_prefix: &str,
        #[case] user_event: UserEvent,
        #[case] expected: UserEventWithCount,
    ) {
        let dummy_key_event = KeyEvent::from(KeyCode::Enter); // KeyEvent is not used in the logic
        let actual = process_numeric_prefix(numeric_prefix, user_event, dummy_key_event);
        assert_eq!(actual, expected);
    }
}
