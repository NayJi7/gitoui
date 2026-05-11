use std::{
    io::{self, Write},
    rc::Rc,
    sync::Mutex,
    thread,
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
    avatar::AvatarManager,
    color::{ColorTheme, GraphColorSet},
    config::{CoreConfig, CursorType, UiConfig, UserCommand, UserCommandType},
    event::{
        AppEvent, DialogKind, EventController, GitAction, UserEvent, UserEventWithCount,
    },
    external::{
        copy_to_clipboard, exec_user_command, exec_user_command_suspend, open_url,
        ExternalCommandParameters,
    },
    git::{
        actions,
        diff::DiffEntry,
        status::{StatusType, UncommittedChanges},
        Commit, CommitHash, FileChange, Head, Ref, Repository,
    },
    github_auth::GithubAuthState,
    graph::{CellWidthType, Graph, GraphImageManager, GraphStyle},
    keybind::KeyBind,
    protocol::ImageProtocol,
    view::{RefreshViewContext, View},
    widget::commit_list::{CommitInfo, CommitListState},
    widget::{branch_detail::BranchMetadata, tag_detail::TagMetadata},
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
    Spinner(String),
}

#[derive(Clone, Copy)]
pub enum InitialSelection {
    Latest,
    Head,
}

pub enum Ret {
    Quit,
    Refresh(RefreshRequest),
    /// Like `Refresh`, but the outer `run()` loop should bump the loaded commit
    /// count by `core.option.load_more_count` before reloading the repository.
    LoadMore(RefreshRequest),
}

#[derive(Debug)]
pub struct RefreshRequest {
    pub context: RefreshViewContext,
}

impl Clone for AppContext {
    fn clone(&self) -> Self {
        Self {
            keybind: self.keybind.clone(),
            core_config: self.core_config.clone(),
            ui_config: self.ui_config.clone(),
            color_theme: self.color_theme.clone(),
            image_protocol: self.image_protocol,
            avatar_manager: Mutex::new(self.avatar_manager.lock().unwrap().clone()),
            git_user_name: self.git_user_name.clone(),
            git_user_email: self.git_user_email.clone(),
            git_default_branch: self.git_default_branch.clone(),
            github_auth_state: self.github_auth_state.clone(),
            branch_color_map: self.branch_color_map.clone(),
            graph_color_set: self.graph_color_set.clone(),
            graph_config: self.graph_config.clone(),
            current_branch_remote_state: self.current_branch_remote_state,
        }
    }
}

impl std::fmt::Debug for AppContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppContext")
            .field("keybind", &self.keybind)
            .field("core_config", &self.core_config)
            .field("ui_config", &self.ui_config)
            .field("color_theme", &self.color_theme)
            .field("image_protocol", &self.image_protocol)
            .field("avatar_manager", &"Mutex<AvatarManager>")
            .field("git_user_name", &self.git_user_name)
            .field("git_user_email", &self.git_user_email)
            .field("github_auth_state", &self.github_auth_state)
            .field("branch_color_map", &self.branch_color_map)
            .finish()
    }
}

pub struct AppContext {
    pub keybind: KeyBind,
    pub core_config: CoreConfig,
    pub ui_config: UiConfig,
    pub color_theme: ColorTheme,
    pub image_protocol: ImageProtocol,
    pub avatar_manager: Mutex<AvatarManager>,
    pub git_user_name: String,
    pub git_user_email: String,
    pub git_default_branch: String,
    pub github_auth_state: GithubAuthState,
    pub branch_color_map: FxHashMap<String, Color>,
    pub graph_color_set: GraphColorSet,
    /// User's `[graph]` TOML section. Carried alongside `graph_color_set` so
    /// the live theme-cycle path can rebuild the set against the user's
    /// branches/edge/background after a theme change — without re-reading
    /// the file. The set is derived; this is the source of truth.
    pub graph_config: crate::config::GraphConfig,
    /// (ahead, behind) commit counts of the current branch vs its upstream.
    /// `None` if HEAD is detached or has no upstream configured. Computed
    /// once per `lib.rs::run` iteration; auto-refresh recreates the context
    /// so this stays current as remote refs change.
    pub current_branch_remote_state: Option<(usize, usize)>,
}

impl Default for AppContext {
    fn default() -> Self {
        Self {
            keybind: KeyBind::default(),
            core_config: CoreConfig::default(),
            ui_config: UiConfig::default(),
            color_theme: ColorTheme::default(),
            image_protocol: ImageProtocol::Iterm2,
            avatar_manager: Mutex::new(AvatarManager::new(
                ImageProtocol::Iterm2,
                Vec::new(),
                None,
                None,
            )),
            git_user_name: String::new(),
            git_user_email: String::new(),
            git_default_branch: String::new(),
            github_auth_state: GithubAuthState::default(),
            branch_color_map: FxHashMap::default(),
            graph_color_set: GraphColorSet::new(&crate::config::GraphColorConfig::default()),
            graph_config: crate::config::GraphConfig::default(),
            current_branch_remote_state: None,
        }
    }
}

#[derive(Debug, Default)]
struct AppStatus {
    status_line: StatusLine,
    numeric_prefix: String,
    view_area: Rect,
    notification_timestamp: Option<std::time::Instant>,
    spinner_active: bool,
    spinner_frame: usize,
    /// Wall-clock of the last auto-refresh (FilesystemChanged → view.refresh()).
    /// Used to throttle bursts of refreshes — the watcher already debounces and
    /// fingerprint-compares, but two distinct legitimate changes within a
    /// couple seconds (e.g. checkout immediately followed by a fetch in
    /// another shell) still shouldn't double-flicker the screen.
    last_auto_refresh: Option<std::time::Instant>,
}

#[derive(Debug)]
pub struct App<'a> {
    repository: &'a Repository,
    view: View<'a>,
    app_status: AppStatus,
    ctx: Rc<AppContext>,
    ec: &'a EventController,
    file_stream: Vec<crate::git::diff::DiffLine>,
    brand_logo: Option<crate::protocol::PreparedImage>,
    brand_wordmark: Option<crate::protocol::PreparedImage>,
    brand_pending_uploads: Vec<String>,
    spinner_frames: Vec<crate::protocol::PreparedImage>,
    spinner_pending_uploads: Vec<String>,
    // Header image skip-optimisation: None = unrendered/dirty; Some(id) = last uploaded.
    // id = None means static G logo; Some(fidx) means animation frame fidx.
    header_logo_last: Option<Option<usize>>,
    header_wordmark_rendered: bool,
    /// Global `d`-overlay state — when active, the header pwd becomes a text
    /// input and a dropdown appears below. Hijacks key input until Esc/Enter.
    dir_input: crate::dir_input::DirInputState,
    /// Persistent list of recently-visited directories (front = most recent).
    /// Loaded on startup, updated on every successful `cd`.
    dir_recents: Vec<std::path::PathBuf>,
    /// Rectangle currently occupied by the dropdown so the run loop can
    /// delete Kitty graphics rows underneath after the buffered draw.
    /// Without this clear the commit-graph images bleed on top of the popup.
    dir_dropdown_area: Option<ratatui::layout::Rect>,
    /// `(x, y)` of the input line's left edge — recorded by `render_header`
    /// when the overlay is open, consumed by `render()` at the very end to
    /// place the terminal cursor (same pattern as the search input).
    dir_input_cursor_anchor: Option<(u16, u16)>,
    /// Set by handlers that can't return `Ret::Refresh` directly (mouse
    /// click on a dir-input suggestion, etc.). The main `run()` loop drains
    /// it after each handler returns and exits with `Ok(Ret::Refresh(req))`,
    /// the same path the keyboard Enter takes.
    pending_refresh: Option<RefreshRequest>,
    /// Transient "this dir isn't a git repo" message shown in the footer
    /// left (replacing "Changing directory…") for ~2 seconds when the user
    /// tries to commit a non-git target. Auto-clears once `Instant::elapsed`
    /// passes the 2 s threshold.
    dir_error_message: Option<(String, std::time::Instant)>,
}

impl<'a> App<'a> {
    pub fn new(
        repository: &'a Repository,
        graph_image_manager: GraphImageManager<'a>,
        graph: &'a Graph,
        graph_color_set: &'a GraphColorSet,
        cell_width_type: CellWidthType,
        graph_style: GraphStyle,
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
                let color_index = if graph_style == GraphStyle::Smooth {
                    graph
                        .commit_color_map
                        .get(&commit.commit_hash)
                        .copied()
                        .unwrap_or(pos_x)
                } else {
                    pos_x
                };
                let graph_color = graph_color_set.get(color_index).to_ratatui_color();
                if commit.commit_type == crate::git::CommitType::Uncommitted {
                    let changes = repository.uncommitted_changes().unwrap();
                    let last_modified = changes.last_modified.map(|dt| dt.fixed_offset());
                    // Match the #808080 used by graph::image for the uncommitted
                    // line — keeps the marker `│` and the message text visually
                    // consistent with the graph rendering.
                    CommitInfo::new_uncommitted(
                        commit,
                        ratatui::style::Color::Rgb(0x80, 0x80, 0x80),
                        changes.staged.len(),
                        changes.unstaged.len(),
                        changes.untracked.len(),
                        last_modified,
                    )
                } else {
                    CommitInfo::new(commit, refs, graph_color)
                }
            })
            .collect();

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
                        let color_index = if graph_style == GraphStyle::Smooth {
                            graph.commit_color_map.get(target).copied().unwrap_or(pos_x)
                        } else {
                            pos_x
                        };
                        let color = graph_color_set.get(color_index).to_ratatui_color();
                        branch_color_map.insert(name.clone(), color);
                    }
                }
                Ref::RemoteBranch { name, target } => {
                    if let Some(&(pos_x, _)) = graph.commit_pos_map.get(target) {
                        let color_index = if graph_style == GraphStyle::Smooth {
                            graph.commit_color_map.get(target).copied().unwrap_or(pos_x)
                        } else {
                            pos_x
                        };
                        let color = graph_color_set.get(color_index).to_ratatui_color();
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
            ctx.core_config.search.regex,
        );
        if let InitialSelection::Head = initial_selection {
            match repository.head() {
                Head::Branch { name } => commit_list_state.select_ref(name),
                Head::Detached { target } => commit_list_state.select_commit_hash(target),
                Head::None => {}
            }
        }
        let view = View::of_list(commit_list_state, ctx.clone(), ec.sender());

        let mut brand_pending_uploads = Vec::new();
        let mut prepare_brand = |png: Option<Vec<u8>>, cell_width: usize, image_id: u32| {
            png.map(|bytes| {
                let mut prepared = ctx.image_protocol.prepare_image(&bytes, cell_width, image_id);
                if let Some(upload) = prepared.take_upload_data() {
                    brand_pending_uploads.push(upload);
                }
                prepared
            })
        };
        let brand_logo = prepare_brand(
            crate::brand::render_logo_png(),
            crate::brand::LOGO_CELL_WIDTH,
            0x0B_2A_1D,
        );
        let brand_wordmark = prepare_brand(
            crate::brand::render_wordmark_png(),
            crate::brand::WORDMARK_CELL_WIDTH,
            0x0B_2A_1E,
        );

        // Pre-render the 28-frame G spinner animation.
        let (spinner_frames, spinner_pending_uploads) = {
            let pngs = crate::brand::render_spinner_frames();
            let mut frames = Vec::with_capacity(pngs.len());
            let mut uploads = Vec::new();
            for (i, png) in pngs.iter().enumerate() {
                let id = 0x0B_2B_00 + i as u32;
                let mut prepared = ctx
                    .image_protocol
                    .prepare_image(png, crate::brand::SPINNER_CELL_WIDTH, id);
                if let Some(upload) = prepared.take_upload_data() {
                    uploads.push(upload);
                }
                frames.push(prepared);
            }
            (frames, uploads)
        };

        let mut app = Self {
            repository,
            view,
            app_status: AppStatus::default(),
            ctx,
            ec,
            file_stream: Vec::new(),
            brand_logo,
            brand_wordmark,
            brand_pending_uploads,
            spinner_frames,
            spinner_pending_uploads,
            header_logo_last: None,
            header_wordmark_rendered: false,
            dir_input: crate::dir_input::DirInputState::default(),
            dir_dropdown_area: None,
            dir_input_cursor_anchor: None,
            pending_refresh: None,
            dir_error_message: None,
            dir_recents: {
                // On startup: load the saved list and surface the current
                // working dir at the top so reopening the same project a few
                // minutes later is one keystroke away.
                let loaded = crate::recents::load();
                if let Ok(cwd) = std::env::current_dir() {
                    crate::recents::push(&cwd, &loaded)
                } else {
                    loaded
                }
            },
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
        self.clear_image(Some(terminal))?;
        self.clear_terminal(terminal)?;

        let mut needs_draw = true;
        loop {
            // Clear notifications after 3 seconds
            if let Some(timestamp) = self.app_status.notification_timestamp {
                if timestamp.elapsed() >= std::time::Duration::from_secs(2) {
                    self.clear_status_line();
                    self.app_status.notification_timestamp = None;
                    needs_draw = true;
                }
            }

            // Debounced filesystem read for the dir-input overlay's
            // suggestion dropdown. The edit methods (insert_char,
            // backspace, etc.) only mark the input as dirty — the actual
            // `read_dir()` happens here, once the user has stopped typing
            // for `SUGGESTIONS_DEBOUNCE`. Fires regardless of `needs_draw`
            // because the Tick that wakes us up (spinner_active during
            // the overlay) drives this loop iteration.
            if self.dir_input.active {
                let recents = self.dir_recents.clone();
                if self.dir_input.flush_pending_refresh(&recents) {
                    needs_draw = true;
                }
            }

            if needs_draw {
                self.prepare_render(terminal)?;
                self.flush_pending_graph_uploads()?;
                terminal.draw(|f| self.render(f))?;
                self.flush_pending_avatar_deletes()?;
                if let Some(dialog_area) = self.view.dialog_area() {
                    if !dialog_area.is_empty() {
                        for y in dialog_area.top()..dialog_area.bottom() {
                            let _ = self.ctx.image_protocol.delete_row(y);
                        }
                    }
                }
                // Popup resize → graph re-sync. The Kitty Unicode-placeholder
                // protocol creates a placement when a placeholder cell is
                // first emitted and DOESN'T drop it when that cell is later
                // overwritten — so we manually re-issue the cleanup whenever
                // the dropdown's height changes:
                //   GROW: new rows previously held graph placeholders that
                //         became placements; those placements still display
                //         on top of the popup until we delete the image ids.
                //   SHRINK: rows that we cleared earlier now need their
                //           graph back AND ratatui's diff might leave ghost
                //           popup cells on screen — terminal.clear() forces
                //           a full re-emit. The reset of `view.clear_graph_images`
                //           triggers a fresh upload on the next frame so
                //           placements regenerate everywhere outside the
                //           current popup rectangle.
                if self.dir_input.active {
                    let height = self.dir_dropdown_area
                        .map(|a| a.height)
                        .unwrap_or(0);
                    if height != self.dir_input.last_rendered_height {
                        // Delete only graph image placements + reset their
                        // manager state — avatars (placed past the popup's
                        // right edge) keep their cache intact, so the full
                        // window doesn't blink, only the graph column does.
                        // We do NOT `continue` here — an extra immediate
                        // redraw would re-run flush_pending_graph_uploads,
                        // moving the terminal cursor around via Kitty image
                        // placement escapes (the source of the caret-jump
                        // bug). The natural next event tick will redraw.
                        let graph_ids = self.view.graph_image_ids_sorted();
                        let _ = self.ctx.image_protocol.delete_images(&graph_ids);
                        self.view.clear_graph_images();
                        self.dir_input.last_rendered_height = height;
                    }
                }
            }
            // When an animation is in flight (spinner, notification countdown,
            // file streaming, or debounce window) we need to wake up on a
            // regular cadence even with no user input. Otherwise we block
            // indefinitely — no spurious 50 ms wakeups while browsing.
            let animated = self.app_status.spinner_active
                || self.app_status.notification_timestamp.is_some()
                || !self.file_stream.is_empty()
                || (self.dir_input.active && self.dir_input.text_dirty_since.is_some());

            let event = if animated {
                match self.ec.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(ev) => {
                        needs_draw = true;
                        Some(ev)
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Animation tick — advance time-based state.
                        let notif_expiring = self
                            .app_status
                            .notification_timestamp
                            .map(|ts| ts.elapsed() >= std::time::Duration::from_secs(2))
                            .unwrap_or(false);
                        let streaming = if !self.file_stream.is_empty() {
                            let chunk_size = 100.min(self.file_stream.len());
                            let chunk: Vec<_> = self.file_stream.drain(..chunk_size).collect();
                            if let View::Diff(ref mut view) = self.view {
                                view.append_addition_lines(chunk);
                            }
                            true
                        } else {
                            false
                        };
                        if self.app_status.spinner_active {
                            let total = if self.spinner_frames.is_empty() {
                                10
                            } else {
                                crate::brand::SPINNER_FRAME_COUNT
                            };
                            self.app_status.spinner_frame =
                                (self.app_status.spinner_frame + 1) % total;
                            needs_draw = true;
                        } else {
                            needs_draw = notif_expiring || streaming;
                        }
                        None
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        panic!("event channel disconnected");
                    }
                }
            } else {
                needs_draw = true;
                Some(self.ec.recv())
            };

            let Some(event) = event else { continue };
            match event {
                AppEvent::Key(key) => {
                    // The change-directory overlay hijacks every key while
                    // open — typing extends the input, Esc cancels, Enter
                    // commits. Handled before the keybind dispatch so the
                    // user can freely type letters that are otherwise bound
                    // to view actions (`d`, `b`, etc.).
                    if self.dir_input.active {
                        let was_active = true;
                        let target_path = self.handle_dir_input_key(key);
                        // If the user just cancelled (Esc closed the
                        // overlay without committing) we force a full
                        // terminal redraw so ratatui's diff doesn't leave
                        // popup characters baked into the screen. The
                        // re-uploaded images come back via the next
                        // prepare_graph_uploads cycle.
                        if was_active && !self.dir_input.active && target_path.is_none() {
                            // Esc / cancel — force the graph re-render. The
                            // popup-open path deleted the graph placements
                            // from Kitty, so we need the manager to see
                            // "nothing uploaded" and queue fresh uploads on
                            // the next prepare_graph_uploads cycle.
                            self.view.clear_graph_images();
                            self.clear_terminal(terminal)?;
                            continue;
                        }
                        if let Some(target) = target_path {
                            // Reject invalid targets *before* doing the cd —
                            // a transient footer message is friendlier than
                            // tearing down the overlay and re-opening the
                            // current repo. The overlay stays open so the
                            // user can pick another path. We distinguish
                            // "path missing" from "path exists but isn't a
                            // git repo root" so the user knows whether to
                            // fix the typo or pick a different folder.
                            if !target.exists() {
                                self.dir_error_message = Some((
                                    "directory does not exist".to_string(),
                                    std::time::Instant::now(),
                                ));
                                continue;
                            }
                            if !crate::git::is_git_path(&target) {
                                self.dir_error_message = Some((
                                    "not a git directory".to_string(),
                                    std::time::Instant::now(),
                                ));
                                continue;
                            }
                            match std::env::set_current_dir(&target) {
                                Ok(_) => {
                                    self.dir_recents = crate::recents::push(&target, &self.dir_recents);
                                    self.dir_input.close();
                                    self.app_status.spinner_active = false;
                                    let _ = ratatui::crossterm::execute!(
                                        std::io::stdout(),
                                        ratatui::crossterm::cursor::Show
                                    );
                                    self.cleanup_graph_images()?;
                                    return Ok(Ret::Refresh(RefreshRequest {
                                        context: crate::view::RefreshViewContext::List {
                                            list_context: crate::view::ListRefreshViewContext {
                                                commit_hash: String::new(),
                                                selected: 0,
                                                height: 20,
                                                scroll_to_top: true,
                                            },
                                            pending_notification: Some(format!(
                                                "Switched to {}",
                                                target.display()
                                            )),
                                        },
                                    }));
                                }
                                Err(e) => {
                                    self.ec.send(AppEvent::NotifyError(format!(
                                        "cd failed: {}",
                                        e
                                    )));
                                    self.dir_input.close();
                                    self.dir_dropdown_area = None;
                                    self.app_status.spinner_active = false;
                                    self.header_logo_last = None;
                                    let _ = ratatui::crossterm::execute!(
                                        std::io::stdout(),
                                        ratatui::crossterm::cursor::Show
                                    );
                                    // Same re-upload trigger as the Esc path
                                    // so the graph reappears.
                                    self.view.clear_graph_images();
                                    self.clear_terminal(terminal)?;
                                }
                            }
                        }
                        continue;
                    }

                    match self.app_status.status_line {
                        StatusLine::None
                        | StatusLine::Input(_, _, _)
                        | StatusLine::Spinner(_) => {
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

                    // True when a text field is actively capturing character
                    // input: dialog Input/SecondInput, config text edit, diff
                    // search, or the list search bar (StatusLine::Input).
                    let text_input_active =
                        self.view.is_input_active()
                            || matches!(
                                self.app_status.status_line,
                                StatusLine::Input(_, _, _)
                            );

                    match user_event {
                        Some(UserEvent::ForceQuit) => {
                            // Ctrl+C always quits — it cannot produce a printable char.
                            self.ec.send(AppEvent::Quit);
                        }
                        Some(UserEvent::Quit) if !text_input_active => {
                            self.ec.send(AppEvent::Quit);
                        }
                        Some(UserEvent::Drop)
                            if matches!(self.view, View::List(_))
                                && !self.view.is_search_querying()
                                && !self.view.is_input_active() =>
                        {
                            // `d` on the commit list doubles as "change
                            // directory" — `drop_commit` is only ever
                            // relevant in the Detail view anyway, where the
                            // event falls through to the standard handler.
                            // We additionally bail out when ANY input is
                            // active (search bar, dialog text field, config
                            // edit) so the user can freely type the letter
                            // `d` while writing.
                            self.dir_input.open(&self.dir_recents);
                            // Only nuke the GRAPH placements — the popup
                            // overlays the graph column at the left, so
                            // those need to disappear. Avatars sit at the
                            // far-right columns (well past the popup width)
                            // so we leave both their Kitty placements AND
                            // the avatar-manager cache untouched — that's
                            // what makes the open instant; re-uploading
                            // every avatar SVG would otherwise add ~1s of
                            // latency on busy repos.
                            let graph_ids = self.view.graph_image_ids_sorted();
                            let _ = self.ctx.image_protocol.delete_images(&graph_ids);
                            self.view.clear_graph_images();
                            // Reuse the global spinner machinery so the
                            // header G-logo animates and the footer braille
                            // ticks at the same cadence as the Pull / Fetch
                            // background tasks.
                            self.app_status.spinner_active = true;
                            self.app_status.spinner_frame = 0;
                            self.header_logo_last = None;
                            self.app_status.numeric_prefix.clear();
                            // Hide the real terminal cursor — we draw a fake
                            // one into the buffer so image-protocol writes
                            // (avatars, graph) can't visibly teleport the
                            // hardware caret around the screen.
                            let _ = ratatui::crossterm::execute!(
                                std::io::stdout(),
                                ratatui::crossterm::cursor::Hide
                            );
                        }
                        Some(ue) => {
                            // When a text field is capturing input, only pass
                            // through structural dialog events. Everything else
                            // (letter keys, Backspace, Delete, Ctrl+arrows…)
                            // is forwarded as Unknown so the view's raw key
                            // handler inserts the character or moves the cursor.
                            let forward_as_raw = text_input_active
                                && !matches!(
                                    ue,
                                    UserEvent::Cancel
                                        | UserEvent::Close
                                        | UserEvent::Confirm
                                        | UserEvent::NavigateUp
                                        | UserEvent::NavigateDown
                                        | UserEvent::RefList
                                );
                            if forward_as_raw {
                                self.app_status.numeric_prefix.clear();
                                self.handle_view_event_clearing_detail_avatar(
                                    UserEventWithCount::from_event(UserEvent::Unknown),
                                    key,
                                    terminal,
                                )?;
                            } else {
                                let event_with_count = process_numeric_prefix(
                                    &self.app_status.numeric_prefix,
                                    *ue,
                                    key,
                                );
                                self.handle_view_event_clearing_detail_avatar(
                                    event_with_count,
                                    key,
                                    terminal,
                                )?;
                                self.app_status.numeric_prefix.clear();
                            }
                        }
                        None => {
                            if let StatusLine::Input(_, _, _) = self.app_status.status_line {
                                // In input mode, pass all key events to the view
                                // fixme: currently, the only thing that processes key_event is searching the list,
                                //        so this probably works, but it's not the right process...
                                self.app_status.numeric_prefix.clear();
                                self.handle_view_event_clearing_detail_avatar(
                                    UserEventWithCount::from_event(UserEvent::Unknown),
                                    key,
                                    terminal,
                                )?;
                            } else if self.view.is_input_active() {
                                // Config text edit mode: pass all key events
                                self.app_status.numeric_prefix.clear();
                                self.handle_view_event_clearing_detail_avatar(
                                    UserEventWithCount::from_event(UserEvent::Unknown),
                                    key,
                                    terminal,
                                )?;
                            } else if let KeyCode::Char(c) = key.code {
                                // Accumulate numeric prefix
                                if c.is_ascii_digit()
                                    && (c != '0' || !self.app_status.numeric_prefix.is_empty())
                                {
                                    self.app_status.numeric_prefix.push(c);
                                } else {
                                    needs_draw = false; // unbound non-digit key: nothing changed
                                }
                            } else {
                                needs_draw = false; // unbound non-char key: nothing changed
                            }
                        }
                    }
                }
                AppEvent::Resize(w, h) => {
                    let _ = (w, h);
                    self.invalidate_header();
                }
                AppEvent::Mouse(mouse) => {
                    needs_draw = self.handle_mouse_event(mouse, terminal)?;
                    if let Some(req) = self.pending_refresh.take() {
                        // A handler (currently: dir-input dropdown 2nd-click)
                        // asked for a full app rebuild. Mirror the keyboard
                        // Enter path: drop graph images so kitty doesn't carry
                        // them into the new repo, then bubble up Ret::Refresh.
                        self.cleanup_graph_images()?;
                        return Ok(Ret::Refresh(req));
                    }
                }
                AppEvent::Quit => {
                    self.cleanup_graph_images()?;
                    return Ok(Ret::Quit);
                }
                AppEvent::OpenDetail => {
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_detail();
                }
                AppEvent::CloseDetail => {
                    if !self.close_detail() {
                        // No commit list state available (e.g. opened from Refs panel)
                        // Force a full refresh to rebuild the list view
                        self.cleanup_graph_images()?;
                        return Ok(Ret::Refresh(RefreshRequest {
                            context: RefreshViewContext::List {
                                list_context: crate::view::ListRefreshViewContext {
                                    commit_hash: String::new(),
                                    selected: 0,
                                    height: 0,
                                    scroll_to_top: true,
                                },
                                pending_notification: None,
                            },
                        }));
                    }
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                }
                AppEvent::OpenUserCommand(n) => {
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_user_command(n, Some(terminal));
                }
                AppEvent::CloseUserCommand => {
                    self.close_user_command();
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                }
                AppEvent::OpenRefs => {
                    self.clear_image(Some(terminal))?;
                    self.open_refs();
                }
                AppEvent::CloseRefs => {
                    self.clear_image(Some(terminal))?;
                    self.close_refs();
                }
                AppEvent::OpenHelp => {
                    self.clear_image(Some(terminal))?;
                    self.open_help();
                }
                AppEvent::CloseHelp => {
                    self.close_help();
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                }
                AppEvent::OpenConfig => {
                    self.clear_image(Some(terminal))?;
                    self.open_config();
                }
                AppEvent::CloseConfig => {
                    self.close_config();
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                }
                AppEvent::GithubAuthFinished(state) => {
                    self.finish_github_auth(state);
                }
                AppEvent::OpenFileDiff { hash, file_path } => {
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_file_diff(hash, file_path);
                }
                AppEvent::OpenStashDiff { stash_ref } => {
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_stash_diff(stash_ref);
                }
                AppEvent::OpenCompareDiff { from_hash, to_hash } => {
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_compare_diff(from_hash, to_hash);
                }
                AppEvent::CloseDiff => {
                    self.close_diff();
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                }
                AppEvent::CloseDiffToDetail => {
                    self.close_diff_to_detail();
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
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
                AppEvent::CopyRawToClipboard {
                    value,
                    success_message,
                } => {
                    self.copy_raw_to_clipboard(value, success_message);
                }
                AppEvent::OpenUrl(url) => {
                    if let Err(msg) = open_url(&url) {
                        self.error_notification(msg);
                    }
                }
                AppEvent::Refresh(context) => {
                    self.stop_spinner();
                    self.cleanup_graph_images()?;
                    let request = RefreshRequest { context };
                    return Ok(Ret::Refresh(request));
                }
                AppEvent::LoadMoreCommits(context) => {
                    self.stop_spinner();
                    self.cleanup_graph_images()?;
                    let request = RefreshRequest { context };
                    return Ok(Ret::LoadMore(request));
                }
                AppEvent::FilesystemChanged => {
                    // Auto-refresh from external git activity. Three guards:
                    // 1. Don't disrupt user input — skip while a dialog or any
                    //    text-input (commit message, search bar, …) is active.
                    // 2. Throttle to at most one refresh every 2 s. The watcher
                    //    already debounces + fingerprint-compares, but a heavy
                    //    burst of legitimate changes shouldn't trigger
                    //    repeated terminal redraws within a few seconds.
                    // 3. Don't refresh if a spinner is active — gitoui itself
                    //    is currently running a git command, the post-action
                    //    refresh path will handle the UI update.
                    const THROTTLE: std::time::Duration =
                        std::time::Duration::from_secs(2);
                    let is_dialog = matches!(self.view, View::Dialog(_));
                    let is_input = matches!(
                        self.app_status.status_line,
                        StatusLine::Input(_, _, _)
                    );
                    let is_view_input = self.view.is_input_active();
                    let is_spinning = self.app_status.spinner_active;
                    let throttled = self
                        .app_status
                        .last_auto_refresh
                        .map(|t| t.elapsed() < THROTTLE)
                        .unwrap_or(false);
                    if !is_dialog
                        && !is_input
                        && !is_view_input
                        && !is_spinning
                        && !throttled
                    {
                        self.app_status.last_auto_refresh =
                            Some(std::time::Instant::now());
                        self.view.refresh();
                    }
                }
                AppEvent::AvatarsUpdated => {}
                AppEvent::ClearStatusLine => {
                    self.clear_status_line();
                }
                AppEvent::UpdateStatusInput(msg, cursor_pos, msg_r) => {
                    self.update_status_input(msg, cursor_pos, msg_r);
                }
                AppEvent::NotifyInfo(msg) => {
                    self.stop_spinner();
                    self.info_notification(msg);
                }
                AppEvent::NotifySuccess(msg) => {
                    self.stop_spinner();
                    self.success_notification(msg);
                }
                AppEvent::NotifyWarn(msg) => {
                    self.stop_spinner();
                    self.warn_notification(msg);
                }
                AppEvent::NotifyError(msg) => {
                    self.stop_spinner();
                    self.error_notification(msg);
                }
                AppEvent::PushCurrentBranch => {
                    self.execute_push_current_branch();
                }
                AppEvent::PullCurrentBranch => {
                    self.execute_pull_current_branch();
                }
                AppEvent::CheckAbortOperation => {
                    self.check_abort_operation();
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
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_branch_detail(branch_name);
                }
                AppEvent::OpenSetUpstreamDialog { branch } => {
                    let repo_path = self.repository.path();
                    match actions::get_remotes(repo_path) {
                        Ok(remotes) if !remotes.is_empty() => {
                            self.open_dialog(DialogKind::SetUpstream { remotes, branch });
                        }
                        _ => {
                            self.ec.send(AppEvent::NotifyError(
                                "No remotes configured. Add a remote first.".into(),
                            ));
                        }
                    }
                }
                AppEvent::OpenTagDetail { tag_name } => {
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_tag_detail(tag_name);
                }
                AppEvent::OpenUncommitted => {
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_uncommitted();
                }
                AppEvent::StageFile { file } => self.stage_file(file),
                AppEvent::UnstageFile { file } => self.unstage_file(file),
                AppEvent::DiscardFile { file } => self.discard_file(file),
                AppEvent::RefreshUncommitted => {
                    self.refresh_uncommitted();
                }
                AppEvent::Tick => {
                    let notif_expiring = self
                        .app_status
                        .notification_timestamp
                        .map(|ts| ts.elapsed() >= std::time::Duration::from_secs(2))
                        .unwrap_or(false);
                    // Progressive file loading: stream next chunk into the diff view
                    let streaming = if !self.file_stream.is_empty() {
                        let chunk_size = 100.min(self.file_stream.len());
                        let chunk: Vec<_> = self.file_stream.drain(..chunk_size).collect();
                        if let View::Diff(ref mut view) = self.view {
                            view.append_addition_lines(chunk);
                        }
                        true
                    } else {
                        false
                    };
                    if self.app_status.spinner_active {
                        let total = if self.spinner_frames.is_empty() {
                            10
                        } else {
                            crate::brand::SPINNER_FRAME_COUNT
                        };
                        self.app_status.spinner_frame =
                            (self.app_status.spinner_frame + 1) % total;
                        needs_draw = true;
                    } else {
                        needs_draw = notif_expiring || streaming;
                    }
                }
                AppEvent::OpenUncommittedDiff {
                    file_path,
                    is_staged,
                } => {
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_uncommitted_diff(file_path, is_staged);
                }
                AppEvent::ToggleHunkStage {
                    file_path,
                    hunk_idx,
                    currently_staged,
                } => {
                    self.toggle_hunk_stage(file_path, hunk_idx, currently_staged);
                }
                AppEvent::CloseDiffToUncommitted => {
                    self.close_diff_to_uncommitted();
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                }
                AppEvent::BackgroundFetch => {
                    self.start_spinner("Fetching\u{2026}");
                    self.start_background_fetch();
                }
                AppEvent::OpenFileHistory { file_path } => {
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_file_history(file_path);
                }
                AppEvent::CloseFileHistory => {
                    self.close_file_history();
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                }
                AppEvent::OpenBlame { file_path } => {
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_blame(file_path);
                }
                AppEvent::CloseBlame => {
                    self.close_blame();
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                }
                AppEvent::OpenDetailByHash { hash } => {
                    self.clear_image(Some(terminal))?;
                    self.clear_terminal(terminal)?;
                    self.open_detail_by_hash(hash);
                }
                AppEvent::SwitchWorktree { path } => {
                    if let Err(e) = std::env::set_current_dir(&path) {
                        self.ec.send(AppEvent::NotifyError(format!(
                            "Cannot switch to worktree: {}",
                            e
                        )));
                    } else {
                        self.cleanup_graph_images()?;
                        return Ok(Ret::Refresh(RefreshRequest {
                            context: crate::view::RefreshViewContext::List {
                                list_context: crate::view::ListRefreshViewContext {
                                    commit_hash: String::new(),
                                    selected: 0,
                                    height: 20,
                                    scroll_to_top: true,
                                },
                                pending_notification: Some(format!("Switched to {}", path)),
                            },
                        }));
                    }
                }
            }
        }
    }

    fn prepare_render(&mut self, terminal: &mut DefaultTerminal) -> Result<(), std::io::Error> {
        let area: Rect = terminal.size()?.into();
        let [_, view_area, _, _] = split_app_areas_with_header(area);
        self.update_state(view_area);
        self.view.update_layout(view_area);
        self.view.prepare_graph_uploads();
        Ok(())
    }

    /// Route a key to the dir-input overlay. Returns `Some(target)` when the
    /// user pressed Enter on a valid resolution — the caller is responsible
    /// for the actual `set_current_dir` + `Ret::Refresh`. `None` means "stay
    /// in the overlay, redraw on next iteration".
    fn handle_dir_input_key(&mut self, key: ratatui::crossterm::event::KeyEvent) -> Option<std::path::PathBuf> {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};
        match key.code {
            KeyCode::Esc => {
                self.dir_input.close();
                self.dir_dropdown_area = None;
                self.app_status.spinner_active = false;
                self.header_logo_last = None;
                let _ = ratatui::crossterm::execute!(
                    std::io::stdout(),
                    ratatui::crossterm::cursor::Show
                );
                None
            }
            KeyCode::Enter => {
                // The user may have hit Enter while typing inside the
                // debounce window — force a sync refresh so `.resolve()`
                // sees up-to-date suggestions (Enter prefers the focused
                // suggestion when one is selected). Without this, fast
                // typist → Enter could commit a stale completion.
                let recents = self.dir_recents.clone();
                self.dir_input.force_refresh(&recents);
                let cwd = std::env::current_dir().unwrap_or_default();
                self.dir_input.resolve(&cwd)
            }
            KeyCode::Down => {
                self.dir_input.select_next();
                None
            }
            KeyCode::Up => {
                self.dir_input.select_prev();
                None
            }
            KeyCode::Tab => {
                // Tab = pick the currently focused suggestion as if the user
                // had typed it, so they can keep refining (e.g. completing
                // `~/work/` → `~/work/api/`). Force a sync refresh first in
                // case the debounce window is still pending — we want the
                // *current* selected suggestion, not a stale snapshot.
                let recents = self.dir_recents.clone();
                self.dir_input.force_refresh(&recents);
                if let Some(i) = self.dir_input.selected {
                    if let Some(s) = self.dir_input.suggestions.get(i).cloned() {
                        // Append "/" so the next keystroke (or immediate
                        // refresh) shows the children of the completed dir.
                        self.dir_input.text = if s.display.ends_with('/') {
                            s.display
                        } else {
                            format!("{}/", s.display)
                        };
                        self.dir_input.cursor = self.dir_input.text.len();
                        self.dir_input.selected = None;
                        self.dir_input.refresh_suggestions_from(&recents);
                    }
                }
                None
            }
            KeyCode::Left => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.dir_input.move_cursor_word_left();
                } else {
                    self.dir_input.move_cursor_left();
                }
                None
            }
            KeyCode::Right => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.dir_input.move_cursor_word_right();
                } else {
                    self.dir_input.move_cursor_right();
                }
                None
            }
            KeyCode::Backspace => {
                let recents = self.dir_recents.clone();
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.dir_input.delete_word_left(&recents);
                } else {
                    self.dir_input.backspace(&recents);
                }
                None
            }
            KeyCode::Delete => {
                let recents = self.dir_recents.clone();
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.dir_input.delete_word_right(&recents);
                } else {
                    self.dir_input.delete_right(&recents);
                }
                None
            }
            // Some terminals (xterm, alacritty…) emit Ctrl+Backspace as the
            // BS (0x08) or DEL (0x7F) control char rather than as
            // `KeyCode::Backspace` with the Ctrl modifier — catch them both
            // explicitly before the generic Char arm so they trigger the
            // word-delete instead of inserting a control character.
            KeyCode::Char(c)
                if (c == '\u{08}' || c == '\u{7f}')
                    && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                let recents = self.dir_recents.clone();
                self.dir_input.delete_word_left(&recents);
                None
            }
            KeyCode::Char('h')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                // Ctrl+H is the legacy backspace mapping on a number of
                // terminals (kitty in particular). Without this arm, the
                // Char arm below would skip it (CONTROL filter) and the
                // word-delete would never fire.
                let recents = self.dir_recents.clone();
                self.dir_input.delete_word_left(&recents);
                None
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let recents = self.dir_recents.clone();
                self.dir_input.insert_char(c, &recents);
                None
            }
            _ => None,
        }
    }

    fn handle_view_event_clearing_detail_avatar(
        &mut self,
        event_with_count: UserEventWithCount,
        key: KeyEvent,
        _terminal: &mut DefaultTerminal,
    ) -> Result<(), std::io::Error> {
        self.view.handle_event(event_with_count, key);
        // Apply theme live when cycling in config view.
        if let View::Config(ref view) = self.view {
            if let Some(def) = crate::themes::get_theme(&view.core_config().option.theme) {
                let color_theme = def.color_theme;
                // Rebuild the graph palette from the new theme + current
                // `[graph.color]` config so the preview row in the config
                // page and the underlying list view both re-render with
                // the new background and branch colours. Without this the
                // graph images stayed baked with the previous theme's bg.
                let ctx = Rc::make_mut(&mut self.ctx);
                ctx.color_theme = color_theme.clone();
                ctx.graph_color_set = crate::color::build_graph_color_set(
                    &color_theme,
                    &ctx.graph_config.color,
                );
                self.view.update_color_theme(color_theme);
            }
        }
        Ok(())
    }

    fn flush_pending_graph_uploads(&mut self) -> Result<(), std::io::Error> {
        let mut uploads = self.view.drain_pending_graph_uploads();
        uploads.extend(
            self.ctx
                .avatar_manager
                .lock()
                .unwrap()
                .drain_pending_uploads(),
        );
        uploads.append(&mut self.brand_pending_uploads);
        uploads.append(&mut self.spinner_pending_uploads);
        if uploads.is_empty() {
            return Ok(());
        }

        let mut stdout = io::stdout().lock();
        for upload in uploads {
            stdout.write_all(upload.as_bytes())?;
        }
        stdout.flush()
    }

    fn flush_pending_avatar_deletes(&mut self) -> Result<(), std::io::Error> {
        let rows = self.view.drain_pending_avatar_deletes();
        for row in rows {
            self.ctx.image_protocol.delete_row(row)?;
        }
        Ok(())
    }

    fn cleanup_graph_images(&self) -> Result<(), std::io::Error> {
        let mut image_ids = self.view.graph_image_ids_sorted();
        image_ids.extend(self.ctx.avatar_manager.lock().unwrap().image_ids_sorted());
        self.ctx.image_protocol.delete_images(&image_ids)
    }

    fn render(&mut self, f: &mut Frame) {
        let base = Block::default().fg(self.ctx.color_theme.fg).bg(self.ctx.color_theme.bg);
        f.render_widget(base, f.area());

        let [header_area, view_area, gap_area, status_line_area] =
            split_app_areas_with_header(f.area());

        self.update_state(view_area);

        self.render_header(f, header_area);
        self.view.render(f, view_area);

        // Dir-input dropdown overlays the top of the view content while the
        // overlay is active. Drawn AFTER the view so it paints on top.
        if self.dir_input.active {
            self.render_dir_dropdown(f, view_area);
        } else {
            // Make sure stale areas don't trigger Kitty-graphics clears on
            // the next frame after the overlay closes.
            self.dir_dropdown_area = None;
        }

        // Dotted separator between view content and shortcuts bar
        let sep_style = Style::default().fg(self.ctx.color_theme.divider_fg);
        let sep_span = Span::styled("╌".repeat(gap_area.width as usize), sep_style);
        f.render_widget(Paragraph::new(Line::from(sep_span)), gap_area);

        self.render_status_line(f, status_line_area);

        // Dir-input caret: a "block" cursor painted directly into the
        // buffer — we overwrite the char that sits at the insertion point
        // with the SAME char in reversed colors (theme bg on the cursor's
        // accent color) so the letter stays visible "through" the cursor.
        // The real terminal caret is hidden on overlay-open so it can't be
        // teleported around by graph/avatar image escapes — the block on
        // screen is always exactly where we paint it.
        //
        // Blinking is driven manually off `spinner_frame` (Tick fires every
        // ~100ms while idle). A 12-frame cycle (600 ms on / 600 ms off)
        // feels closer to a relaxed hardware text-caret rate.
        // `Modifier::SLOW_BLINK` is ignored by most modern terminals, so we
        // don't rely on it.
        if self.dir_input.active {
            if let Some((anchor_x, anchor_y)) = self.dir_input_cursor_anchor.take() {
                let before_count = self.dir_input.text[..self.dir_input.cursor]
                    .chars()
                    .count() as u16;
                let cursor_x = anchor_x + 5 + before_count;
                // Char to highlight: the one to the right of the cursor, or
                // a space when the cursor is past the end of the input.
                let cursor_char: String = self.dir_input.text[self.dir_input.cursor..]
                    .chars()
                    .next()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| " ".to_string());
                let blink_on = (self.app_status.spinner_frame % 12) < 6;
                if blink_on {
                    // `virtual_cursor_fg` is `Color::Reset` in every shipped
                    // theme — using it as bg produces a transparent block
                    // (cursor invisible). Hard-code #f8f8f2 (the warm
                    // off-white shared with the wordmark's "oui") — visible
                    // across themes, matches the brand palette.
                    let style = Style::default()
                        .fg(self.ctx.color_theme.bg)
                        .bg(ratatui::style::Color::Rgb(0xf8, 0xf8, 0xf2));
                    f.buffer_mut()
                        .set_string(cursor_x, anchor_y, &cursor_char, style);
                }
                // blink_on == false: paint nothing — the input-line render
                // above already drew the underlying char with its normal
                // style, so that's the "off" half of the blink.
            }
        }
    }
}

impl App<'_> {
    /// Translate (col, row) into a suggestion index inside the dropdown.
    /// Returns `None` for clicks outside the popup body (border, header
    /// row, or outside the rectangle entirely).
    fn dir_dropdown_hit(&self, col: u16, row: u16) -> Option<usize> {
        let area = self.dir_dropdown_area?;
        if col < area.x
            || col >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return None;
        }
        // Skip the top + bottom border rows.
        if row == area.y || row == area.y + area.height - 1 {
            return None;
        }
        let row_inside = (row - area.y - 1) as usize;
        let idx = self.dir_input.scroll + row_inside;
        if idx < self.dir_input.suggestions.len() {
            Some(idx)
        } else {
            None
        }
    }

    /// Dropdown rendered below the header while the `d` overlay is open.
    /// Anchored to the top-left of the view area, capped at `MAX_VISIBLE`
    /// suggestion rows; scrolling kicks in beyond that.
    fn render_dir_dropdown(&mut self, f: &mut Frame, view_area: Rect) {
        use ratatui::{
            layout::Rect,
            style::Style,
            text::{Line, Span},
            widgets::{Block, Borders, Paragraph},
        };

        let suggestions = &self.dir_input.suggestions;
        let theme = &self.ctx.color_theme;
        let visible_count = suggestions.len().min(crate::dir_input::MAX_VISIBLE);
        // Cap height: visible rows + 2 border rows (or 1+2 when empty so the
        // placeholder hint still has room).
        let inner_rows = visible_count.max(1);
        let height = ((inner_rows + 2) as u16).min(view_area.height);
        // Width: capped so the popup never bleeds past the commit-message
        // column into the avatar / author / hash / date columns on the
        // right. Their cells stay visible and the avatar Kitty placements
        // stay alive (we never delete them on resize). 60% of the view
        // width usually sits comfortably inside the commit-message column
        // on any reasonable terminal size.
        let width = ((view_area.width as f32 * 0.6) as u16).clamp(40, 80);
        let width = width.min(view_area.width);
        let area = Rect::new(view_area.x, view_area.y, width, height);

        // Record the area so the run loop can react to popup growth (and
        // re-clear the graph in the newly covered rows). The actual delete
        // happens AFTER `terminal.draw` returns — doing it here would emit
        // escape sequences in the middle of ratatui's flush, garbling the
        // screen.
        self.dir_dropdown_area = Some(area);

        // Clear the cell buffer so we don't bleed terminal content through.
        // `Cell::reset()` zeroes EVERY style component first — using
        // `set_style` alone leaves `fg` untouched when the passed style only
        // specifies `bg`, and Kitty's Unicode-placeholder protocol encodes
        // the image ID in the foreground colour: a leftover fg is enough
        // for Kitty to keep painting the commit-graph image on top of the
        // popup. Explicit reset → set the bg → set the char.
        let popup_bg = theme.bg;
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                let cell = f.buffer_mut().get_mut(x, y);
                cell.reset();
                cell.set_style(Style::default().bg(popup_bg));
                cell.set_char(' ');
            }
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.list_head_fg))
            .style(Style::default().bg(theme.bg));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if suggestions.is_empty() {
            let hint = Line::from(Span::styled(
                "  type a path or pick from recents…",
                Style::default()
                    .fg(theme.divider_fg)
                    .add_modifier(ratatui::style::Modifier::ITALIC),
            ));
            f.render_widget(Paragraph::new(hint), inner);
            return;
        }

        let scroll = self.dir_input.scroll;
        let end = (scroll + crate::dir_input::MAX_VISIBLE).min(suggestions.len());
        let lines: Vec<Line> = (scroll..end)
            .map(|i| {
                let s = &suggestions[i];
                let is_active = self.dir_input.selected == Some(i);
                let bg = if is_active { theme.list_selected_bg } else { theme.bg };
                // Icon column padded so both kinds line up before the `│`
                // separator. The clock glyph renders as a single cell on most
                // terminals while the folder emoji renders as two cells —
                // without the extra trailing space the clock rows would sit
                // one column to the left of the folder rows.
                let kind_prefix = match s.kind {
                    crate::dir_input::SuggestionKind::Recent => " ⏱  ",
                    crate::dir_input::SuggestionKind::Filesystem => " 📁 ",
                };
                let kind_fg = match s.kind {
                    crate::dir_input::SuggestionKind::Recent => theme.list_date_fg,
                    crate::dir_input::SuggestionKind::Filesystem => theme.list_name_fg,
                };
                Line::from(vec![
                    Span::styled(
                        kind_prefix.to_string(),
                        Style::default().fg(kind_fg).bg(bg),
                    ),
                    Span::styled(
                        "│ ".to_string(),
                        Style::default().fg(theme.divider_fg).bg(bg),
                    ),
                    Span::styled(
                        s.display.clone(),
                        Style::default().fg(theme.fg).bg(bg),
                    ),
                ])
            })
            .collect();

        f.render_widget(Paragraph::new(lines), inner);

        // Tiny scroll indicator in the right border when there's more to see.
        if suggestions.len() > crate::dir_input::MAX_VISIBLE {
            let total = suggestions.len();
            let position_label = format!(" {}/{} ", end, total);
            // Drop it on the top border so it doesn't compete with content rows.
            if let Some(_) = position_label.chars().next() {
                let label_x = area.x + area.width.saturating_sub(position_label.chars().count() as u16 + 2);
                let label_y = area.y;
                let mut x = label_x;
                for c in position_label.chars() {
                    if x >= area.x + area.width { break; }
                    f.buffer_mut()
                        .get_mut(x, label_y)
                        .set_char(c)
                        .set_style(
                            Style::default()
                                .fg(theme.list_head_fg)
                                .bg(theme.bg)
                                .add_modifier(ratatui::style::Modifier::BOLD),
                        );
                    x += 1;
                }
            }
        }
    }

    fn render_header(&mut self, f: &mut Frame, area: Rect) {
        use ratatui::{
            text::{Line, Span},
            widgets::Paragraph,
        };

        // Derive a tilde-relative path from the current working directory.
        let cwd = std::env::current_dir()
            .unwrap_or_else(|_| self.repository.path().to_path_buf());
        let home = std::env::var("HOME").unwrap_or_default();
        let display_path = if !home.is_empty() && cwd.starts_with(&home) {
            format!("~{}", &cwd.to_string_lossy()[home.len()..])
        } else {
            cwd.to_string_lossy().to_string()
        };

        // Repo name (last component) in bold, rest dimmer.
        let repo_name = cwd
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let path_prefix = if display_path.ends_with(&repo_name) && repo_name.len() < display_path.len() {
            display_path[..display_path.len() - repo_name.len()].to_string()
        } else {
            String::new()
        };

        // Right icon area: [logo] [gap] [wordmark]  — or text fallback (9 cols).
        let has_brand = self.brand_logo.is_some() && self.brand_wordmark.is_some();
        let icon_cell_width = if has_brand {
            crate::brand::LOGO_CELL_WIDTH as u16
                + crate::brand::GAP_COLS
                + crate::brand::WORDMARK_CELL_WIDTH as u16
        } else {
            9u16
        };

        // Numeric prefix (vim-style count, e.g. "10" before `j` to skip 10
        // commits). Rendered in the header to the left of the logo, separated
        // by a vertical bar — when empty, no extra space is reserved and the
        // layout falls back to the previous path-then-logo arrangement.
        let prefix_str = self.app_status.numeric_prefix.as_str();
        let prefix_block_width: u16 = if prefix_str.is_empty() {
            0
        } else {
            // <digits> + " │ " (3 cells: space, bar, space)
            prefix_str.chars().count() as u16 + 3
        };
        let path_width = area
            .width
            .saturating_sub(icon_cell_width + 2 + prefix_block_width);

        let [content_row, separator_row] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Length(1),
        ])
        .areas(area);

        // Content row: path left, [optional prefix block | icon] right.
        let [left_area, right_area] = ratatui::layout::Layout::horizontal([
            ratatui::layout::Constraint::Min(0),
            ratatui::layout::Constraint::Length(icon_cell_width + 2 + prefix_block_width),
        ])
        .areas(content_row);

        // Sub-split the right side into [prefix_area, icon_area] so the icon
        // rendering below keeps using its own dedicated rectangle.
        let [prefix_area, right_area] = ratatui::layout::Layout::horizontal([
            ratatui::layout::Constraint::Length(prefix_block_width),
            ratatui::layout::Constraint::Length(icon_cell_width + 2),
        ])
        .areas(right_area);

        let white = ratatui::style::Color::White;
        let dim_white = ratatui::style::Color::Rgb(160, 160, 160);

        // When the dir-input overlay is active, replace the pwd display with
        // an editable text line. The real terminal cursor is positioned via
        // `f.set_cursor_position` at the END of `render()` (after every
        // other component has drawn) so neither the popup buffer overwrite
        // nor the Kitty image placements moves it around mid-frame.
        if self.dir_input.active {
            let accent = self.ctx.color_theme.list_head_fg;
            let input_line = Line::from(vec![
                Span::styled(
                    " cd ".to_string(),
                    ratatui::style::Style::default()
                        .fg(accent)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                ),
                Span::styled(" ".to_string(), ratatui::style::Style::default()),
                Span::styled(
                    self.dir_input.text.clone(),
                    ratatui::style::Style::default().fg(white),
                ),
            ]);
            f.render_widget(Paragraph::new(input_line), left_area);
            // Remember the cursor anchor so the main `render()` method can
            // place the cursor as the very last thing it does.
            self.dir_input_cursor_anchor = Some((left_area.x, left_area.y));
        } else {
            // Path: dim prefix + bold repo name.
            let mut path_spans = vec![];
            if !path_prefix.is_empty() {
                let prefix_truncated = if path_prefix.chars().count() > path_width as usize {
                    let skip = path_prefix.chars().count().saturating_sub(path_width as usize - 1);
                    format!("…{}", path_prefix.chars().skip(skip).collect::<String>())
                } else {
                    path_prefix.clone()
                };
                path_spans.push(Span::styled(
                    format!(" {}", prefix_truncated),
                    ratatui::style::Style::default().fg(dim_white),
                ));
            } else {
                path_spans.push(Span::raw(" "));
            }
            path_spans.push(Span::styled(
                repo_name,
                ratatui::style::Style::default()
                    .fg(white)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ));

            f.render_widget(
                Paragraph::new(Line::from(path_spans)),
                left_area,
            );
        }

        // Numeric prefix block: "<digits> │ " right before the logo. Rendered
        // here (not in the footer) so the user sees the count where their eye
        // already tracks the brand. Empty prefix → empty area, no bar shown.
        if !prefix_str.is_empty() {
            let prefix_line = Line::from(vec![
                Span::styled(
                    prefix_str,
                    ratatui::style::Style::default()
                        .fg(self.ctx.color_theme.status_input_transient_fg)
                        .add_modifier(ratatui::style::Modifier::BOLD),
                ),
                Span::styled(
                    " │ ",
                    ratatui::style::Style::default().fg(self.ctx.color_theme.divider_fg),
                ),
            ]);
            f.render_widget(
                Paragraph::new(prefix_line)
                    .alignment(ratatui::layout::Alignment::Right),
                prefix_area,
            );
        }

        // Icon: [G logo OR spinner frame] + gap + wordmark, or text fallback.
        // The spinner animation lives here in the header (safe zone — not the last row).
        if has_brand {
            // Determine current logo "identity": None = static G, Some(idx) = animation frame.
            let logo_id: Option<usize> =
                if self.app_status.spinner_active && !self.spinner_frames.is_empty() {
                    Some(self.app_status.spinner_frame % self.spinner_frames.len())
                } else {
                    None
                };

            // skip=true → ratatui never writes the cell, terminal keeps the existing image.
            // skip=false → cell is written (uploaded/placed) when content differs from prev buffer.
            let logo_skip = Some(logo_id) == self.header_logo_last;
            let wordmark_skip = self.header_wordmark_rendered;

            let y = content_row.top();

            // Compute layout positions (must be done before taking image refs due to borrow rules).
            let logo_cell_w = crate::brand::LOGO_CELL_WIDTH as u16;
            let wordmark_cell_w = crate::brand::WORDMARK_CELL_WIDTH as u16;
            let total = logo_cell_w + crate::brand::GAP_COLS + wordmark_cell_w;
            let x_logo = right_area.right().saturating_sub(total + 1);
            let x_wordmark = x_logo + logo_cell_w + crate::brand::GAP_COLS;

            // Write logo cells (or just set skip=true to preserve the existing Kitty image).
            {
                let logo_img: &crate::protocol::PreparedImage =
                    if let Some(fidx) = logo_id {
                        &self.spinner_frames[fidx]
                    } else {
                        self.brand_logo.as_ref().unwrap()
                    };
                let buf = f.buffer_mut();
                for (dx, ic) in logo_img.cells().iter().enumerate() {
                    let x = x_logo + dx as u16;
                    if x >= right_area.left() && x < right_area.right() {
                        let cell = &mut buf[(x, y)];
                        if !logo_skip {
                            cell.set_symbol(ic.symbol());
                            cell.set_style(ic.style());
                        }
                        cell.set_skip(logo_skip);
                    }
                }
            }
            // Write wordmark cells.
            {
                let wordmark_img = self.brand_wordmark.as_ref().unwrap();
                let buf = f.buffer_mut();
                for (dx, ic) in wordmark_img.cells().iter().enumerate() {
                    let x = x_wordmark + dx as u16;
                    if x >= right_area.left() && x < right_area.right() {
                        let cell = &mut buf[(x, y)];
                        if !wordmark_skip {
                            cell.set_symbol(ic.symbol());
                            cell.set_style(ic.style());
                        }
                        cell.set_skip(wordmark_skip);
                    }
                }
            }
            // Persist the rendered state so the next render can skip re-uploading.
            if !logo_skip { self.header_logo_last = Some(logo_id); }
            if !wordmark_skip { self.header_wordmark_rendered = true; }
        } else {
            let icon_line = Line::from(vec![Span::styled(
                " ◈ gitoui ",
                ratatui::style::Style::default()
                    .fg(white)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            )]);
            f.render_widget(
                Paragraph::new(icon_line).alignment(ratatui::layout::Alignment::Right),
                right_area,
            );
        }

        // Separator row: full-width line.
        let sep = Line::from(
            "─".repeat(area.width as usize)
                .fg(ratatui::style::Color::White),
        );
        f.render_widget(Paragraph::new(sep), separator_row);
    }

    fn render_status_line(&self, f: &mut Frame, area: Rect) {
        // Compute view-state flags before building spans so StatusLine::None can
        // decide whether the numeric prefix goes before or after the HEAD info.
        let is_search_active = self.view.is_search_active();
        let is_search_querying = self.view.is_search_querying();
        let is_config_active = self.view.is_config_active();
        // 2-commit compare flow: when a mark is active in the list view, the
        // entire footer swaps over to a dedicated mode — left side shows the
        // "Comparison: <a> → <b>" indicator (with the cursor side updating
        // live as the user navigates), right side replaces the usual shortcut
        // bar with compare-specific actions.
        let compare_pending = self.view.list_compare_pending();
        let show_enhanced = matches!(
            &self.app_status.status_line,
            StatusLine::None | StatusLine::NotificationInfo(_)
        ) && !is_search_active
            && !is_config_active
            && compare_pending.is_none()
            && !self.dir_input.active;

        let mut spans = if self.dir_input.active {
            // Transient validation error (e.g. "not a git repository") wins
            // over the regular "Changing directory…" idle text for ~2 s.
            // After that we fall back to the spinner without closing the
            // overlay so the user can pick a different target.
            let show_error = self
                .dir_error_message
                .as_ref()
                .filter(|(_, t)| t.elapsed() < std::time::Duration::from_secs(2))
                .map(|(msg, _)| msg.clone());
            if let Some(msg) = show_error {
                vec![Span::styled(
                    format!(" {}", msg),
                    Style::default()
                        .fg(self.ctx.color_theme.status_error_fg)
                        .add_modifier(Modifier::BOLD),
                )]
            } else {
                // Same braille spinner used by Pull / Fetch background jobs,
                // ticked off `spinner_frame` so it matches the header G-logo
                // animation cadence.
                const FRAMES: [&str; 10] = [
                    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}",
                    "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}", "\u{280f}",
                ];
                let frame = FRAMES[self.app_status.spinner_frame % 10];
                vec![Span::styled(
                    format!("{frame} Changing directory…"),
                    Style::default()
                        .fg(self.ctx.color_theme.status_info_fg)
                        .add_modifier(Modifier::BOLD),
                )]
            }
        } else { match &self.app_status.status_line {
            // Numeric prefix is now rendered in the header (left of the logo),
            // so the footer's None branch is empty regardless of prefix state.
            StatusLine::None => vec![],
            StatusLine::Input(msg, _, transient_msg) => {
                let msg_w = console::measure_text_width(msg.as_str());
                if let Some(t_msg) = transient_msg {
                    let t_msg_w = console::measure_text_width(t_msg.as_str());
                    let pad_w = area.width as usize - msg_w - t_msg_w - 2;
                    vec![
                        Span::styled(
                            msg.as_str(),
                            Style::default().fg(self.ctx.color_theme.status_input_fg),
                        ),
                        Span::raw(" ".repeat(pad_w)),
                        Span::styled(
                            t_msg.as_str(),
                            Style::default().fg(self.ctx.color_theme.status_input_transient_fg),
                        ),
                    ]
                } else {
                    vec![Span::styled(
                        msg.as_str(),
                        Style::default().fg(self.ctx.color_theme.status_input_fg),
                    )]
                }
            }
            StatusLine::NotificationInfo(msg) => {
                vec![Span::styled(
                    msg.as_str(),
                    Style::default().fg(self.ctx.color_theme.status_info_fg),
                )]
            }
            StatusLine::NotificationSuccess(msg) => {
                vec![Span::styled(
                    msg.as_str(),
                    Style::default()
                        .fg(self.ctx.color_theme.status_success_fg)
                        .add_modifier(Modifier::BOLD),
                )]
            }
            StatusLine::NotificationWarn(msg) => {
                vec![Span::styled(
                    msg.as_str(),
                    Style::default()
                        .fg(self.ctx.color_theme.status_warn_fg)
                        .add_modifier(Modifier::BOLD),
                )]
            }
            StatusLine::NotificationError(msg) => {
                vec![Span::styled(
                    format!("ERROR: {msg}"),
                    Style::default()
                        .fg(self.ctx.color_theme.status_error_fg)
                        .add_modifier(Modifier::BOLD),
                )]
            }
            StatusLine::Spinner(msg) => {
                // The G animation plays in the header logo — status bar is text-only.
                const FRAMES: [&str; 10] = [
                    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}",
                    "\u{2834}", "\u{2826}", "\u{2827}", "\u{2807}", "\u{280f}",
                ];
                let frame = FRAMES[self.app_status.spinner_frame % 10];
                vec![Span::styled(
                    format!("{frame} {msg}"),
                    Style::default()
                        .fg(self.ctx.color_theme.status_info_fg)
                        .add_modifier(Modifier::BOLD),
                )]
            }
        }};

        let dim_separator = Style::default().fg(self.ctx.color_theme.divider_fg);
        let dim_text = Style::default().fg(self.ctx.color_theme.list_ref_paren_fg);
        let show_shortcuts = matches!(
            &self.app_status.status_line,
            StatusLine::None
        ) || is_search_active
            || is_config_active;
        let _is_diff = matches!(&self.view, View::Diff(_));

        let status_area = if show_shortcuts || self.dir_input.active {
            let shortcut_text: String = if self.dir_input.active {
                // Dedicated cd-mode hints — only the keys that actually do
                // something while the overlay is open. The animated label
                // lives on the LEFT side (see the spans build above).
                "⌘ ↑↓:navigate▕▏Tab:complete▕▏Enter:cd▕▏Esc:cancel".into()
            } else if is_search_querying {
                String::new()
            } else if is_search_active {
                let (ignore_case, fuzzy, regex) = self
                    .view
                    .search_case_fuzzy_regex()
                    .unwrap_or((false, false, false));
                // ON = case-sensitive (ignore_case=false), OFF = case-insensitive (ignore_case=true)
                let case_str = if ignore_case { "[OFF]" } else { "[ON]" };
                let fuzzy_str = if fuzzy { "[ON]" } else { "[OFF]" };
                let regex_str = if regex { "[ON]" } else { "[OFF]" };
                format!(
                    "⌘ s:case{case_str}▕▏z:fuzzy{fuzzy_str}▕▏x:regex{regex_str}▕▏n:next▕▏N:prev▕▏Esc:clear"
                )
            } else if is_config_active {
                self.view
                    .config_footer_hint()
                    .unwrap_or_else(|| "⌘ Enter/⇆:cycle".into())
            } else if compare_pending.is_some() {
                // Dedicated compare-pending shortcut bar — completely
                // replaces the usual list view shortcuts so the user knows
                // unambiguously which actions are relevant in this mode.
                "⌘ Space:compare▕▏↑↓:navigate▕▏Esc:cancel".into()
            } else {
                match &self.view {
                    View::List(_) => {
                        // `c` / `C` (copy msg / hash) are deliberately kept
                        // functional but omitted from the footer hint to
                        // reduce clutter. They're discoverable via `?:help`.
                        "⌘ f:search▕▏Tab:refs▕▏P:push▕▏U:pull▕▏r:fetch▕▏d:cd▕▏p:config▕▏?:help▕▏q:quit"
                            .into()
                    }
                    View::Diff(_) => self
                        .view
                        .diff_footer_hint()
                        .unwrap_or_else(|| "⌘ c:copy-path".into()),
                    View::Detail(_) => "⌘ ⇆:prev/next▕▏H:history▕▏c:msg▕▏C:hash▕▏r:fetch".into(),
                    View::Refs(_) => "⌘ D:delete▕▏c:copy-name▕▏r:fetch▕▏?:help".into(),
                    View::Help(_) => "⌘ ?:close".into(),
                    View::UserCommand(_) => "⌘ ?:help▕▏r:fetch".into(),
                    View::Dialog(_) => "⌘ Tab:focus▕▏Enter:confirm".into(),
                    // All branch / tag actions live in the right-hand action
                    // bar — see `LOCAL_BRANCH_ACTIONS` / `REMOTE_BRANCH_ACTIONS`
                    // / `TAG_ACTIONS`. The footer only keeps what's NOT in the
                    // panel (fetch).
                    View::BranchDetail(_) => "⌘ r:fetch".into(),
                    View::TagDetail(_) => "⌘ r:fetch".into(),
                    View::Uncommitted(_) => self
                        .view
                        .uncommitted_footer_hint()
                        .unwrap_or_else(String::new),
                    View::FileHistory(_) => self
                        .view
                        .file_history_footer_hint()
                        .unwrap_or_else(|| "⌘ Esc:close".into()),
                    View::Blame(_) => self
                        .view
                        .blame_footer_hint()
                        .unwrap_or_else(|| "⌘ Esc:close".into()),
                    View::Compare(_) => self
                        .view
                        .compare_footer_hint()
                        .unwrap_or_else(|| "⌘ Tab:focus▕▏↑↓:navigate▕▏Esc:close".into()),
                    _ => "⌘ f:search▕▏Tab:refs▕▏?:help▕▏q:quit▕▏r:fetch".into(),
                }
            };

            let shortcut_display_width = shortcut_text.chars().count() as u16;
            let right_constraint = Constraint::Length(shortcut_display_width + 4);
            let [left_area, right_area] =
                Layout::horizontal([Constraint::Min(0), right_constraint]).areas(area);
            let shortcut_spans = vec![Span::styled(shortcut_text, dim_text)];
            let shortcut_line = Line::from(shortcut_spans);
            let shortcut_paragraph = Paragraph::new(shortcut_line)
                .style(Style::default().bg(self.ctx.color_theme.bg))
                .alignment(Alignment::Right)
                .block(Block::default().padding(Padding::horizontal(1)));

            f.render_widget(shortcut_paragraph, right_area);

            if is_config_active {
                let config_hint_line = match &self.app_status.status_line {
                    StatusLine::NotificationSuccess(msg) => Line::from(vec![Span::styled(
                        msg.as_str(),
                        Style::default()
                            .fg(self.ctx.color_theme.status_success_fg)
                            .add_modifier(Modifier::BOLD),
                    )]),
                    StatusLine::NotificationError(msg) => Line::from(vec![Span::styled(
                        format!("ERROR: {msg}"),
                        Style::default()
                            .fg(self.ctx.color_theme.status_error_fg)
                            .add_modifier(Modifier::BOLD),
                    )]),
                    StatusLine::NotificationInfo(msg) => Line::from(vec![Span::styled(
                        msg.as_str(),
                        Style::default().fg(self.ctx.color_theme.status_info_fg),
                    )]),
                    StatusLine::NotificationWarn(msg) => Line::from(vec![Span::styled(
                        msg.as_str(),
                        Style::default()
                            .fg(self.ctx.color_theme.status_warn_fg)
                            .add_modifier(Modifier::BOLD),
                    )]),
                    _ => Line::from(vec![Span::styled(
                        "Changes are applied to your config.toml",
                        dim_text,
                    )]),
                };
                let config_hint_paragraph = Paragraph::new(config_hint_line)
                    .style(Style::default().bg(self.ctx.color_theme.bg))
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

        // Compare-pending mode: replace the LEFT side (HEAD info + dirty
        // counts) with the live "Comparison: marked → cursor" indicator. The
        // selected hash slot updates on every render as the cursor moves;
        // shows `...` when the cursor sits on the marked commit itself or on
        // the Uncommitted row (where comparison is impossible).
        if let Some((marked, cursor)) = compare_pending.as_ref() {
            let short = |h: &str| h.chars().take(7).collect::<String>();
            let marked_short = short(marked.as_str());
            let target_short = match cursor {
                Some(c) if c != marked => short(c.as_str()),
                _ => "...".to_string(),
            };
            // The "Comparison: " label keeps the violet bg badge — it's the
            // mode indicator. The two SHAs themselves render in the regular
            // commit-list hash color (no bg) so they look exactly like the
            // SHA column the user is reading from.
            spans.push(Span::styled(
                " Comparison: ",
                Style::default()
                    .fg(self.ctx.color_theme.list_compare_marked_fg)
                    .bg(self.ctx.color_theme.list_compare_marked_bg)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                marked_short,
                Style::default()
                    .fg(self.ctx.color_theme.list_hash_fg)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                " → ",
                Style::default().fg(self.ctx.color_theme.divider_fg),
            ));
            spans.push(Span::styled(
                target_short,
                Style::default()
                    .fg(self.ctx.color_theme.list_hash_fg)
                    .add_modifier(Modifier::BOLD),
            ));
        } else if show_enhanced {
            match self.repository.head() {
                Head::Branch { name } => {
                    let branch_color = self
                        .ctx
                        .branch_color_map
                        .get(name)
                        .copied()
                        .unwrap_or(self.ctx.color_theme.status_info_fg);
                    spans.push(Span::styled(
                        "HEAD → ",
                        Style::default()
                            .fg(self.ctx.color_theme.list_head_fg)
                            .add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled("⎇ ", Style::default().fg(branch_color)));
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
                        Style::default()
                            .fg(self.ctx.color_theme.list_head_fg)
                            .add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(
                        "● detached",
                        Style::default()
                            .fg(self.ctx.color_theme.status_warn_fg)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                Head::None => {}
            }

            // Push/pull readiness indicator: ↑N (commits to push) / ↓N
            // (commits to pull) appended right after the branch name.
            // Shown only if the branch has an upstream; ✓ if in sync.
            if let Some((ahead, behind)) = self.ctx.current_branch_remote_state {
                if ahead == 0 && behind == 0 {
                    spans.push(Span::styled(
                        " ✓",
                        Style::default().fg(self.ctx.color_theme.status_success_fg),
                    ));
                } else {
                    if ahead > 0 {
                        spans.push(Span::styled(
                            format!(" ↑{}", ahead),
                            Style::default().fg(self.ctx.color_theme.detail_file_change_add_fg),
                        ));
                    }
                    if behind > 0 {
                        spans.push(Span::styled(
                            format!(" ↓{}", behind),
                            Style::default()
                                .fg(self.ctx.color_theme.detail_file_change_delete_fg),
                        ));
                    }
                }
            }

            if let Some(changes) = self.repository.uncommitted_changes() {
                let staged = changes.staged.len();
                let unstaged = changes.unstaged.len();
                let untracked = changes.untracked.len();

                if changes.is_dirty() {
                    // No trailing space after the bar — each indicator below
                    // already starts with a leading space, which doubles as
                    // the separator from one indicator to the next.
                    spans.push(Span::styled(" │", dim_separator));
                    if staged > 0 {
                        spans.push(Span::styled(
                            format!(" ✓{}", staged),
                            Style::default().fg(self.ctx.color_theme.status_success_fg),
                        ));
                    }
                    if unstaged > 0 {
                        spans.push(Span::styled(
                            format!(" ⚡{}", unstaged),
                            Style::default().fg(self.ctx.color_theme.status_warn_fg),
                        ));
                    }
                    if untracked > 0 {
                        spans.push(Span::styled(
                            format!(" ?{}", untracked),
                            Style::default().fg(self.ctx.color_theme.list_ref_stash_fg),
                        ));
                    }
                }
            }

            // (Numeric prefix moved to the header, left of the logo.)
        }

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line)
            .style(Style::default().bg(self.ctx.color_theme.bg))
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

const HEADER_HEIGHT: u16 = 2;

fn split_app_areas_with_header(area: Rect) -> [Rect; 4] {
    let [header, rest] = Layout::vertical([
        Constraint::Length(HEADER_HEIGHT),
        Constraint::Min(0),
    ])
    .areas(area);
    let [view, gap, status] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(rest);
    [header, view, gap, status]
}

impl App<'_> {
    fn update_state(&mut self, view_area: Rect) {
        self.app_status.view_area = view_area;
    }

    /// Mark header images as needing re-upload on next render.
    fn invalidate_header(&mut self) {
        self.header_logo_last = None;
        self.header_wordmark_rendered = false;
    }

    /// Clear the terminal display and mark header images dirty for re-upload.
    fn clear_terminal(&mut self, terminal: &mut DefaultTerminal) -> Result<(), std::io::Error> {
        terminal.clear()?;
        self.invalidate_header();
        Ok(())
    }

    fn clear_image(
        &mut self,
        terminal: Option<&mut DefaultTerminal>,
    ) -> Result<(), std::io::Error> {
        // Clear prepared images so they get re-uploaded after terminal clear
        self.view.clear_graph_images();
        self.ctx
            .avatar_manager
            .lock()
            .unwrap()
            .clear_prepared_images();
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
        let head_branch_name = match self.repository.head() {
            Head::Branch { name } => Some(name.clone()),
            _ => None,
        };
        let head_commit_hash = head_commit_hash_from_repository(self.repository);
        self.view = View::of_detail(
            commit_list_state,
            commit,
            changes,
            refs,
            head_branch_name,
            head_commit_hash,
            self.ctx.clone(),
            self.ec.sender(),
        );
    }

    fn close_detail(&mut self) -> bool {
        match self.view {
            View::Detail(ref mut view) => {
                let commit_list_state = view.take_list_state();
                self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
                true
            }
            View::BranchDetail(ref mut view) => {
                if let Some(commit_list_state) = view.take_list_state() {
                    self.view =
                        View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
                    true
                } else {
                    false
                }
            }
            View::TagDetail(ref mut view) => {
                if let Some(commit_list_state) = view.take_list_state() {
                    self.view =
                        View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
                    true
                } else {
                    false
                }
            }
            View::Uncommitted(ref mut view) => {
                if let Some(commit_list_state) = view.take_list_state() {
                    self.view =
                        View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn open_file_diff(&mut self, hash: String, file_path: String) {
        if crate::git::diff::is_binary_extension(&file_path) {
            self.ec.send(AppEvent::NotifyWarn(format!(
                "Binary file: diff not available for {}",
                file_path
            )));
            return;
        }
        let commit_list_state = match self.view {
            View::Detail(ref mut view) => view.take_list_state(),
            View::Diff(ref mut view) => view.take_list_state().unwrap(),
            _ => return,
        };
        let all_files = self
            .repository
            .commit_detail(&CommitHash::from(hash.as_str()))
            .1
            .iter()
            .filter(|c| !matches!(c, crate::git::FileChange::Delete { .. }))
            .map(|c| match c {
                crate::git::FileChange::Add { path, .. }
                | crate::git::FileChange::Modify { path, .. } => path.clone(),
                crate::git::FileChange::Move { to, .. } => to.clone(),
                crate::git::FileChange::Delete { .. } => unreachable!(),
            })
            .collect::<Vec<String>>();
        let all_files = all_files.into_iter().map(|p| (p, false)).collect();
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
                    self.repository.path().to_path_buf(),
                );
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
                self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
            }
        }
    }

    fn open_stash_diff(&mut self, stash_ref: String) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => view.take_list_state(),
            View::Refs(ref mut view) => view.take_list_state(),
            _ => return,
        };
        let repo_path = self.repository.path().to_path_buf();
        match DiffEntry::load_for_stash(&repo_path, &stash_ref) {
            Ok(diff_entries) => {
                let all_file_paths = diff_entries
                    .iter()
                    .filter_map(|e| e.new_path.clone().or_else(|| e.old_path.clone()))
                    .map(|p| (p, false))
                    .collect::<Vec<(String, bool)>>();
                let title = format!("Stash: {}", stash_ref);
                self.view = View::of_diff_with_entries(
                    commit_list_state,
                    diff_entries,
                    self.ctx.clone(),
                    self.ec.sender(),
                    title,
                    stash_ref,
                    all_file_paths,
                    repo_path,
                );
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
                self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
            }
        }
    }

    /// Handle the 2-commit comparison flow. Detects older/newer order from
    /// commit dates, loads `git diff <older>..<newer>`, and opens the result
    /// in DiffView. Clears the compare mark on the way out so subsequent
    /// Space starts a fresh selection.
    fn open_compare_diff(&mut self, from_hash: String, to_hash: String) {
        let mut commit_list_state = match self.view {
            View::List(ref mut view) => view.take_list_state(),
            _ => return,
        };
        // Mark is consumed on opening — UX described in the plan.
        commit_list_state.clear_compare_mark();

        let repo_path = self.repository.path().to_path_buf();

        // Order detection: find both commits in the loaded list and pick the
        // older one as the diff base. Falls back to (from, to) if either
        // can't be located (rare — only if hashes drifted out of the loaded
        // window between marking and confirming).
        let (older, newer) = {
            let from = self.repository.commit(&CommitHash::from(from_hash.as_str()));
            let to = self.repository.commit(&CommitHash::from(to_hash.as_str()));
            match (from, to) {
                (Some(f), Some(t)) if t.committer_date >= f.committer_date => {
                    (from_hash.clone(), to_hash.clone())
                }
                (Some(_), Some(_)) => (to_hash.clone(), from_hash.clone()),
                _ => (from_hash.clone(), to_hash.clone()),
            }
        };

        match DiffEntry::load_for_commit_range(&repo_path, &older, &newer, 3) {
            Ok(diff_entries) if !diff_entries.is_empty() => {
                // Open the dedicated 2-pane CompareView (file list on the
                // left, full diff of the selected file on the right). The
                // CompareView owns the commit_list_state and rebuilds an
                // internal DiffView on every file selection change.
                self.view = View::of_compare(
                    commit_list_state,
                    diff_entries,
                    older,
                    newer,
                    repo_path,
                    self.ctx.clone(),
                    self.ec.sender(),
                );
            }
            Ok(_) => {
                self.ec.send(AppEvent::NotifyInfo(
                    "No differences between the two commits.".into(),
                ));
                self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
                self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
            }
        }
    }

    fn close_diff(&mut self) {
        self.file_stream.clear();
        if let View::Diff(ref mut view) = self.view {
            let commit_list_state = view.take_list_state().unwrap();
            self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
        } else if let View::Compare(ref mut view) = self.view {
            // CompareView shares CloseDiff with the regular diff close path —
            // it owns the commit list state directly and returns to List view.
            let commit_list_state = view.take_list_state().unwrap();
            self.view = View::of_list(commit_list_state, self.ctx.clone(), self.ec.sender());
        }
    }

    fn close_diff_to_detail(&mut self) {
        if let View::Diff(ref mut view) = self.view {
            let commit_list_state = view.take_list_state().unwrap();
            let (commit, changes, refs) =
                selected_commit_details(self.repository, &commit_list_state);
            let head_branch_name = match self.repository.head() {
                Head::Branch { name } => Some(name.clone()),
                _ => None,
            };
            let head_commit_hash = head_commit_hash_from_repository(self.repository);
            self.view = View::of_detail(
                commit_list_state,
                commit,
                changes,
                refs,
                head_branch_name,
                head_commit_hash,
                self.ctx.clone(),
                self.ec.sender(),
            );
        }
    }

    fn open_file_history(&mut self, file_path: String) {
        // Load history FIRST — same rationale as `open_blame`: if git log
        // errors (rare for valid paths, but possible for files outside the
        // worktree), don't tear down the source view's state.
        let repo_path = self.repository.path().to_path_buf();
        let entries = match actions::file_history(&repo_path, &file_path) {
            Ok(e) => e,
            Err(msg) => {
                self.ec
                    .send(AppEvent::NotifyError(format!("File history failed: {}", msg)));
                return;
            }
        };

        let commit_list_state = match self.view {
            View::Diff(ref mut view) => view.take_list_state(),
            View::Detail(ref mut view) => Some(view.take_list_state()),
            View::Blame(ref mut view) => view.take_list_state(),
            View::Uncommitted(ref mut view) => view.take_list_state(),
            _ => None,
        };
        self.view = View::of_file_history(
            commit_list_state,
            file_path,
            entries,
            self.ctx.clone(),
            self.ec.sender(),
        );
    }

    fn close_file_history(&mut self) {
        if let View::FileHistory(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            if let Some(state) = commit_list_state {
                self.view = View::of_list(state, self.ctx.clone(), self.ec.sender());
            }
        }
    }

    fn open_blame(&mut self, file_path: String) {
        // Load the blame BEFORE we touch the source view's state. If git
        // blame errors (e.g. file doesn't exist in the working tree, like a
        // path that was renamed or deleted in a later commit), the source
        // view stays intact — taking its list_state before would leave it in
        // a half-broken state that panics on the next render.
        let repo_path = self.repository.path().to_path_buf();
        let lines = match crate::git::blame::load_blame(&repo_path, &file_path) {
            Ok(l) => l,
            Err(msg) => {
                self.ec
                    .send(AppEvent::NotifyError(format!("Blame failed: {}", msg)));
                return;
            }
        };

        let commit_list_state = match self.view {
            View::Diff(ref mut view) => view.take_list_state(),
            View::Detail(ref mut view) => Some(view.take_list_state()),
            View::FileHistory(ref mut view) => view.take_list_state(),
            View::Uncommitted(ref mut view) => view.take_list_state(),
            _ => None,
        };
        self.view = View::of_blame(
            commit_list_state,
            file_path,
            lines,
            self.ctx.clone(),
            self.ec.sender(),
        );
    }

    fn close_blame(&mut self) {
        if let View::Blame(ref mut view) = self.view {
            let commit_list_state = view.take_list_state();
            if let Some(state) = commit_list_state {
                self.view = View::of_list(state, self.ctx.clone(), self.ec.sender());
            }
        }
    }

    fn open_detail_by_hash(&mut self, hash: String) {
        let commit_hash = CommitHash::from(hash.as_str());

        // The Blame view can hand us any commit reachable from HEAD, including
        // ones older than the current `max_count` cap or unreachable from the
        // present refs. Bail out with a notification instead of panicking in
        // `commit_detail` (which unwraps the in-memory commit map).
        if self.repository.commit(&commit_hash).is_none() {
            let short = if hash.len() >= 7 { &hash[..7] } else { hash.as_str() };
            self.ec.send(AppEvent::NotifyWarn(format!(
                "Commit {} is not in the loaded history — press `]` to load more.",
                short
            )));
            return;
        }

        // Carry the file path forward when the user dives into a commit from a
        // file-centric view — the new Detail view will pre-select that file in
        // its change list so the user lands exactly where they were looking.
        let preselect_file: Option<String> = match self.view {
            View::Blame(ref view) => Some(view.file_path().to_string()),
            View::FileHistory(ref view) => Some(view.file_path().to_string()),
            _ => None,
        };

        let commit_list_state = match self.view {
            View::FileHistory(ref mut view) => view.take_list_state(),
            View::Blame(ref mut view) => view.take_list_state(),
            _ => None,
        };
        let (commit, changes) = self.repository.commit_detail(&commit_hash);
        let refs: Vec<Ref> = self
            .repository
            .refs(&commit_hash)
            .into_iter()
            .cloned()
            .collect();
        let head_branch_name = match self.repository.head() {
            Head::Branch { name } => Some(name.clone()),
            _ => None,
        };
        let head_commit_hash = head_commit_hash_from_repository(self.repository);
        if let Some(list_state) = commit_list_state {
            self.view = View::of_detail(
                list_state,
                commit,
                changes,
                refs,
                head_branch_name,
                head_commit_hash,
                self.ctx.clone(),
                self.ec.sender(),
            );
            // Coming from Blame / FileHistory: jump straight to the file the
            // user was looking at. `select_file_by_path` falls back to the
            // default selection (file 0) when the path isn't in the commit's
            // change list, so renames-with-no-match degrade gracefully.
            if let Some(path) = preselect_file {
                if let View::Detail(ref mut detail) = self.view {
                    detail.select_file_by_path(&path);
                }
            }
        } else {
            self.ec.send(AppEvent::NotifyError(
                "Cannot open commit detail: no list state available.".into(),
            ));
        }
    }

    fn open_uncommitted_diff(&mut self, file_path: String, is_staged: bool) {
        if crate::git::diff::is_binary_extension(&file_path) {
            self.ec.send(AppEvent::NotifyWarn(format!(
                "Binary file: diff not available for {}",
                file_path
            )));
            return;
        }
        self.file_stream.clear();

        let (commit_list_state, all_files, is_untracked) = match self.view {
            View::Uncommitted(ref mut view) => {
                let list_state = view.take_list_state();
                let is_untracked = view
                    .untracked
                    .iter()
                    .any(|f| f.path == file_path);
                let all_files: Vec<(String, bool)> = view
                    .staged
                    .iter()
                    .filter(|f| f.status != StatusType::Deleted)
                    .map(|f| (f.path.clone(), true))
                    .chain(
                        view.unstaged
                            .iter()
                            .filter(|f| f.status != StatusType::Deleted)
                            .map(|f| (f.path.clone(), false)),
                    )
                    .chain(view.untracked.iter().map(|f| (f.path.clone(), false)))
                    .collect();
                (list_state, all_files, is_untracked)
            }
            View::Diff(ref mut view) => {
                let list_state = view.take_list_state();
                let all_files = view.all_file_paths().clone();
                (list_state, all_files, false)
            }
            _ => return,
        };

        // For untracked files, show content as additions (not a real diff)
        if is_untracked {
            match DiffEntry::load_untracked_file(self.repository.path(), &file_path) {
                Err(e) if e == "binary" => {
                    // Show a placeholder entry with the "cannot open" message
                    let entry = DiffEntry::binary_placeholder(&file_path);
                    let title = format!("File: {}", file_path);
                    self.view = View::of_uncommitted_diff(
                        commit_list_state,
                        vec![entry],
                        self.ctx.clone(),
                        self.ec.sender(),
                        title,
                        all_files,
                        self.repository.path().to_path_buf(),
                    );
                }
                Err(e) => {
                    self.ec.send(AppEvent::NotifyError(e));
                }
                Ok(full_entry) => {
                    // Stream the first 200 lines immediately, then the rest via Tick
                    let total_lines: Vec<_> = full_entry
                        .hunks
                        .into_iter()
                        .flat_map(|h| h.lines)
                        .collect();
                    let chunk_size = 200.min(total_lines.len());
                    let (initial, rest): (Vec<_>, Vec<_>) =
                        total_lines.into_iter().enumerate().partition(|(i, _)| *i < chunk_size);
                    let initial: Vec<_> = initial.into_iter().map(|(_, l)| l).collect();
                    let rest: Vec<_> = rest.into_iter().map(|(_, l)| l).collect();

                    let initial_hunk = crate::git::diff::Hunk {
                        old_start: 0,
                        old_count: 0,
                        new_start: 1,
                        new_count: initial.len() as u32,
                        lines: initial,
                        origin: crate::git::diff::HunkOrigin::Untracked,
                    };
                    let initial_entry = DiffEntry {
                        old_path: Some("/dev/null".to_string()),
                        new_path: Some(file_path.clone()),
                        hunks: vec![initial_hunk],
                    };
                    let title = format!("File: {}", file_path);
                    self.view = View::of_uncommitted_diff(
                        commit_list_state,
                        vec![initial_entry],
                        self.ctx.clone(),
                        self.ec.sender(),
                        title,
                        all_files,
                        self.repository.path().to_path_buf(),
                    );
                    self.file_stream = rest;
                }
            }
            return;
        }

        // Hunk-level staging — we always load the combined staged+unstaged
        // diff so the view shows every hunk linearly with its own indicator.
        // `is_staged` only drives the initial scroll position so the user
        // lands on the first hunk of the side they clicked from.
        match DiffEntry::load_combined_uncommitted_for_file(
            self.repository.path(),
            &file_path,
            3,
        ) {
            Ok(Some(diff_entry)) => {
                let title = format!("Diff: {}", file_path);
                self.view = View::of_uncommitted_diff(
                    commit_list_state,
                    vec![diff_entry],
                    self.ctx.clone(),
                    self.ec.sender(),
                    title,
                    all_files,
                    self.repository.path().to_path_buf(),
                );
                if let View::Diff(ref mut view) = self.view {
                    let origin = if is_staged {
                        crate::git::diff::HunkOrigin::Staged
                    } else {
                        crate::git::diff::HunkOrigin::Unstaged
                    };
                    view.set_initial_scroll_origin(origin);
                }
            }
            Ok(None) => {
                self.ec
                    .send(AppEvent::NotifyWarn(format!("No changes for {}", file_path)));
            }
            Err(err) => {
                self.ec.send(AppEvent::NotifyError(err));
            }
        }
    }

    /// Stage or unstage a single hunk by reloading the combined diff, picking
    /// the hunk at `hunk_idx`, piping the patch through `git apply --cached`
    /// (with `--reverse` for unstage), then re-opening the diff so the view
    /// reflects the new state. Preserves the user's scroll position and
    /// re-focuses the same hunk so toggling doesn't snap them back to row 0.
    /// If the user has edited the file between the view render and the click,
    /// hunk_idx may no longer line up — in which case `git apply` itself
    /// surfaces an error verbatim.
    fn toggle_hunk_stage(
        &mut self,
        file_path: String,
        hunk_idx: usize,
        currently_staged: bool,
    ) {
        let repo_path = self.repository.path().to_path_buf();
        let combined = match DiffEntry::load_combined_uncommitted_for_file(&repo_path, &file_path, 3) {
            Ok(Some(d)) => d,
            Ok(None) => {
                self.ec
                    .send(AppEvent::NotifyWarn("No changes to toggle".to_string()));
                return;
            }
            Err(e) => {
                self.ec.send(AppEvent::NotifyError(e));
                return;
            }
        };
        let hunk = match combined.hunks.get(hunk_idx) {
            Some(h) => h,
            None => {
                self.ec
                    .send(AppEvent::NotifyError(format!("Hunk {} not found", hunk_idx)));
                return;
            }
        };
        let result = if currently_staged {
            crate::git::actions::unstage_hunk(&repo_path, &file_path, hunk)
        } else {
            crate::git::actions::stage_hunk(&repo_path, &file_path, hunk)
        };
        match result {
            Ok(_) => {
                // Capture the user's current scroll position from the diff view
                // before we tear it down, so we can restore it on the new view.
                let prev_scroll = match self.view {
                    View::Diff(ref v) => v.scroll_offset(),
                    _ => 0,
                };
                // Reload the combined diff so the moved hunk shows its new state.
                self.open_uncommitted_diff(file_path, false);
                // After reload, ask the new view to anchor on the toggled hunk.
                if let View::Diff(ref mut v) = self.view {
                    v.set_restore_focus(hunk_idx, prev_scroll);
                }
            }
            Err(e) => {
                self.ec
                    .send(AppEvent::NotifyError(format!("git apply failed: {}", e)));
            }
        }
    }

    fn close_diff_to_uncommitted(&mut self) {
        if let View::Diff(ref mut view) = self.view {
            let list_state = view.take_list_state();
            let repo_path = self.repository.path().to_path_buf();
            match crate::git::status::UncommittedChanges::load(&repo_path) {
                Ok(changes) => {
                    let load_diff_stats = |f: &crate::git::status::FileStatus, is_staged: bool| {
                        let diff_result = if is_staged {
                            DiffEntry::load_staged_for_file(self.repository.path(), &f.path)
                        } else {
                            DiffEntry::load_unstaged_for_file(self.repository.path(), &f.path)
                        };

                        match diff_result {
                            Ok(diff_entry) => {
                                let (additions, deletions) =
                                    diff_entry.count_additions_and_deletions();
                                (additions, deletions)
                            }
                            Err(_) => (0, 0),
                        }
                    };

                    let convert = |f: &crate::git::status::FileStatus, is_staged: bool| {
                        let (additions, deletions) = load_diff_stats(f, is_staged);
                        crate::widget::uncommitted::UncommittedFile {
                            status: f.status.clone(),
                            path: f.path.clone(),
                            old_path: f.old_path.clone(),
                            additions,
                            deletions,
                        }
                    };

                    self.view = View::Uncommitted(Box::new(
                        crate::view::uncommitted::UncommittedView::new(
                            changes.unstaged.iter().map(|f| convert(f, false)).collect(),
                            changes.staged.iter().map(|f| convert(f, true)).collect(),
                            changes
                                .untracked
                                .iter()
                                .map(|f| convert(f, false))
                                .collect(),
                            list_state,
                            self.ctx.clone(),
                            self.ec.sender(),
                        ),
                    ));
                }
                Err(err) => {
                    self.ec.send(AppEvent::NotifyError(err));
                }
            }
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
            let github_auth_state = view.github_auth_state().clone();
            let old_mouse = self.ctx.ui_config.common.mouse_enabled;
            self.view = view.take_before_view();
            let github_avatars = core.github_avatars();
            let ctx = Rc::make_mut(&mut self.ctx);
            ctx.core_config = core;
            if let Some(def) = crate::themes::get_theme(&ctx.core_config.option.theme) {
                ctx.color_theme = def.color_theme;
                ctx.core_config.option.syntax_theme = def.syntax_theme.to_owned();
            }
            // Always rebuild the graph palette from the (possibly theme-
            // overridden) color_theme + user's [graph] config so the
            // returning list view paints with the right bg + branch colours.
            ctx.graph_color_set = crate::color::build_graph_color_set(
                &ctx.color_theme,
                &ctx.graph_config.color,
            );
            ctx.ui_config = ui.clone();
            ctx.github_auth_state = github_auth_state.clone();
            ctx.avatar_manager
                .lock()
                .unwrap()
                .set_github_token(github_auth_state.token.clone());
            ctx.avatar_manager
                .lock()
                .unwrap()
                .set_github_avatars(github_avatars);
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
                pending_notification: None,
            }));
        }
    }

    fn finish_github_auth(&mut self, state: GithubAuthState) {
        let authenticated = state.is_authenticated();
        if let View::Config(ref mut view) = self.view {
            view.finish_github_auth(state.clone());
        }
        let ctx = Rc::make_mut(&mut self.ctx);
        ctx.github_auth_state = state.clone();
        ctx.avatar_manager
            .lock()
            .unwrap()
            .set_github_token(state.token.clone());
        if authenticated {
            let tx = self.ec.sender();
            thread::spawn(move || {
                thread::sleep(std::time::Duration::from_secs(3));
                let _ = tx.send(AppEvent::Refresh(RefreshViewContext::List {
                    list_context: crate::view::ListRefreshViewContext {
                        commit_hash: String::new(),
                        selected: 0,
                        height: 20,
                        scroll_to_top: false,
                    },
                    pending_notification: None,
                }));
            });
        }
    }

    fn start_background_fetch(&mut self) {
        let repo_path = self.repository.path().to_path_buf();
        let tx = self.ec.sender();
        thread::spawn(move || {
            let _ = actions::fetch(&repo_path);
            let _ = tx.send(AppEvent::Refresh(crate::view::RefreshViewContext::List {
                list_context: crate::view::ListRefreshViewContext {
                    commit_hash: String::new(),
                    selected: 0,
                    height: 20,
                    scroll_to_top: false,
                },
                pending_notification: None,
            }));
        });
    }

    fn select_older_commit(&mut self) {
        if let View::Detail(ref mut view) = self.view {
            view.select_older_commit(self.repository);
        } else if let View::Diff(ref mut view) = self.view {
            let hash = view
                .as_list_state()
                .unwrap()
                .selected_commit_hash()
                .as_str()
                .to_string();
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
            let hash = view
                .as_list_state()
                .unwrap()
                .selected_commit_hash()
                .as_str()
                .to_string();
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
            let hash = view
                .as_list_state()
                .unwrap()
                .selected_commit_hash()
                .as_str()
                .to_string();
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
            RefreshViewContext::List {
                pending_notification,
                ..
            } => {
                if let Some(msg) = pending_notification {
                    self.ec.send(AppEvent::NotifySuccess(msg));
                }
            }
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
        self.app_status.notification_timestamp = None;
    }

    fn handle_mouse_event(
        &mut self,
        mouse: ratatui::crossterm::event::MouseEvent,
        terminal: &mut DefaultTerminal,
    ) -> Result<bool, std::io::Error> {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};

        let needs_draw = match mouse.kind {
            MouseEventKind::ScrollUp => {
                if self.dir_input.active {
                    self.dir_input.scroll_by(-3);
                } else {
                    self.handle_view_event_clearing_detail_avatar(
                        crate::event::UserEventWithCount::new(crate::event::UserEvent::ScrollUp, 3),
                        ratatui::crossterm::event::KeyEvent::new(
                            ratatui::crossterm::event::KeyCode::Up,
                            ratatui::crossterm::event::KeyModifiers::NONE,
                        ),
                        terminal,
                    )?;
                }
                true
            }
            MouseEventKind::ScrollDown => {
                if self.dir_input.active {
                    self.dir_input.scroll_by(3);
                } else {
                    self.handle_view_event_clearing_detail_avatar(
                        crate::event::UserEventWithCount::new(crate::event::UserEvent::ScrollDown, 3),
                        ratatui::crossterm::event::KeyEvent::new(
                            ratatui::crossterm::event::KeyCode::Down,
                            ratatui::crossterm::event::KeyModifiers::NONE,
                        ),
                        terminal,
                    )?;
                }
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                use ratatui::crossterm::event::KeyModifiers;
                // When the dir-input overlay is open it owns every mouse
                // event — a click inside the dropdown completes the entry
                // into the input (same as Tab) so the user can keep refining
                // before pressing Enter. Clicks outside are swallowed so the
                // underlying commit list can't be interacted with.
                if self.dir_input.active {
                    if let Some(idx) =
                        self.dir_dropdown_hit(mouse.column, mouse.row)
                    {
                        if let Some(s) = self.dir_input.suggestions.get(idx).cloned() {
                            // Two-step click: first click on a suggestion just
                            // fills the input (Tab semantics). Clicking the
                            // exact same suggestion a second time — i.e. the
                            // input text already equals what we'd write —
                            // commits the cd (Enter semantics). Lets the user
                            // both refine and confirm with the mouse alone.
                            if self.dir_input.text == s.display {
                                self.dir_input.selected = Some(idx);
                                let cwd = std::env::current_dir().unwrap_or_default();
                                if let Some(target) = self.dir_input.resolve(&cwd) {
                                    // Same gates as the keyboard Enter path:
                                    // first reject missing paths with a clear
                                    // "directory does not exist" message, then
                                    // fall through to the git-root check.
                                    if !target.exists() {
                                        self.dir_error_message = Some((
                                            "directory does not exist".to_string(),
                                            std::time::Instant::now(),
                                        ));
                                        return Ok(true);
                                    }
                                    if !crate::git::is_git_path(&target) {
                                        self.dir_error_message = Some((
                                            "not a git directory".to_string(),
                                            std::time::Instant::now(),
                                        ));
                                        return Ok(true);
                                    }
                                    match std::env::set_current_dir(&target) {
                                        Ok(_) => {
                                            self.dir_recents = crate::recents::push(&target, &self.dir_recents);
                                            self.dir_input.close();
                                            self.dir_dropdown_area = None;
                                            self.app_status.spinner_active = false;
                                            let _ = ratatui::crossterm::execute!(
                                                std::io::stdout(),
                                                ratatui::crossterm::cursor::Show
                                            );
                                            // Bubble the rebuild up via the
                                            // pending_refresh slot — the main
                                            // loop will drain it right after
                                            // handle_mouse_event returns and
                                            // exit with Ret::Refresh, the same
                                            // path the keyboard Enter takes.
                                            self.pending_refresh = Some(RefreshRequest {
                                                context: crate::view::RefreshViewContext::List {
                                                    list_context:
                                                        crate::view::ListRefreshViewContext {
                                                            commit_hash: String::new(),
                                                            selected: 0,
                                                            height: 20,
                                                            scroll_to_top: true,
                                                        },
                                                    pending_notification: Some(format!(
                                                        "Switched to {}",
                                                        target.display()
                                                    )),
                                                },
                                            });
                                            return Ok(true);
                                        }
                                        Err(e) => {
                                            self.ec.send(AppEvent::NotifyError(format!(
                                                "cd failed: {}",
                                                e
                                            )));
                                            self.dir_input.close();
                                            self.dir_dropdown_area = None;
                                            self.app_status.spinner_active = false;
                                            self.header_logo_last = None;
                                            let _ = ratatui::crossterm::execute!(
                                                std::io::stdout(),
                                                ratatui::crossterm::cursor::Show
                                            );
                                            self.view.clear_graph_images();
                                            let _ = self.clear_terminal(terminal);
                                        }
                                    }
                                }
                            } else {
                                self.dir_input.text = s.display;
                                self.dir_input.cursor = self.dir_input.text.len();
                                self.dir_input.selected = None;
                                let recents = self.dir_recents.clone();
                                self.dir_input.refresh_suggestions_from(&recents);
                            }
                        }
                    }
                    // Click outside dropdown while overlay is active: do
                    // nothing — explicitly NOT propagating to view.handle_click.
                    return Ok(true);
                }
                if mouse.modifiers.contains(KeyModifiers::CONTROL) {
                    self.view.handle_shift_click(mouse.column, mouse.row);
                } else {
                    self.view.handle_click(mouse.column, mouse.row);
                }
                true
            }
            MouseEventKind::Moved => {
                if self.dir_input.active {
                    // Overlay owns hover: highlight the dropdown row under
                    // the cursor, swallow events that fall outside so the
                    // commit list doesn't react.
                    if let Some(idx) =
                        self.dir_dropdown_hit(mouse.column, mouse.row)
                    {
                        self.dir_input.selected = Some(idx);
                    }
                    return Ok(true);
                }
                self.view.handle_mouse_move(mouse.column, mouse.row)
            }
            _ => false,
        };
        Ok(needs_draw)
    }

    fn update_status_input(
        &mut self,
        msg: String,
        cursor_pos: Option<u16>,
        transient_msg: Option<String>,
    ) {
        self.app_status.status_line = StatusLine::Input(msg, cursor_pos, transient_msg);
    }

    fn start_spinner(&mut self, msg: &str) {
        self.app_status.spinner_active = true;
        self.app_status.spinner_frame = 0;
        self.app_status.status_line = StatusLine::Spinner(msg.to_string());
        self.header_logo_last = None; // logo switches from static to animated
    }

    fn stop_spinner(&mut self) {
        self.app_status.spinner_active = false;
        self.header_logo_last = None; // logo switches from animated back to static
        // status_line will be overwritten by the next NotifySuccess/NotifyError/Refresh
    }

    fn info_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationInfo(msg);
        self.app_status.notification_timestamp = Some(std::time::Instant::now());
    }

    fn success_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationSuccess(msg);
        self.app_status.notification_timestamp = Some(std::time::Instant::now());
    }

    fn warn_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationWarn(msg);
        self.app_status.notification_timestamp = Some(std::time::Instant::now());
    }

    fn error_notification(&mut self, msg: String) {
        self.app_status.status_line = StatusLine::NotificationError(msg);
        self.app_status.notification_timestamp = Some(std::time::Instant::now());
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

    fn copy_raw_to_clipboard(&mut self, value: String, success_message: String) {
        match copy_to_clipboard(value, &self.ctx.core_config.external.clipboard) {
            Ok(_) => self.success_notification(success_message),
            Err(msg) => self.error_notification(msg),
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
            self.view.clear_graph_images();
            // Also clear avatar prepared images so ratatui re-emits those cells
            // and clears any dialog text residue (image cells have skip=true which
            // would otherwise leave dialog content visible over the avatar area).
            self.ctx.avatar_manager.lock().unwrap().clear_prepared_images();
        }
    }

    fn dialog_confirm(&mut self) {
        // DialogConfirm is handled by the DialogView itself via handle_event
        // This method is called when the dialog sends DialogConfirm event
        // In practice, DialogView sends ExecuteGitAction directly on Confirm
    }

    fn check_abort_operation(&mut self) {
        let git_dir = self.repository.path();
        match actions::detect_in_progress(git_dir) {
            Some(actions::InProgressOperation::Rebase) => {
                self.open_dialog(DialogKind::ConfirmAbortOperation {
                    op_name: "rebase".into(),
                });
            }
            Some(actions::InProgressOperation::Merge) => {
                self.open_dialog(DialogKind::ConfirmAbortOperation {
                    op_name: "merge".into(),
                });
            }
            Some(actions::InProgressOperation::CherryPick) => {
                self.open_dialog(DialogKind::ConfirmAbortOperation {
                    op_name: "cherry-pick".into(),
                });
            }
            None => {
                self.ec
                    .send(AppEvent::NotifyInfo("No rebase or merge in progress".into()));
            }
        }
    }

    fn execute_push_current_branch(&mut self) {
        let repo_path = self.repository.path();
        // Detect the upstream first (synchronously, fast).
        // We need to know the branch & remote before spawning the thread.
        let branch = match self.repository.head() {
            Head::Branch { name } => name.clone(),
            _ => {
                self.ec
                    .send(AppEvent::NotifyError("Cannot push: not on a branch".into()));
                return;
            }
        };
        let upstream = actions::branch_upstream(repo_path, &branch);
        let no_upstream = upstream.as_ref().map_or(true, |u| u.is_empty());
        if no_upstream {
            // Ask the user to pick a remote before pushing.
            match actions::get_remotes(repo_path) {
                Ok(remotes) if !remotes.is_empty() => {
                    self.open_dialog(DialogKind::ChooseRemote { remotes, branch });
                }
                _ => {
                    self.ec.send(AppEvent::NotifyError(
                        "No remotes configured. Add a remote first.".into(),
                    ));
                }
            }
            return;
        }
        let remote = upstream
            .unwrap_or_default()
            .split('/')
            .next()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "origin".to_string());
        self.start_spinner("Pushing\u{2026}");
        let tx = self.ec.sender();
        let repo_path_buf = repo_path.to_path_buf();
        thread::spawn(move || {
            let result = actions::push_branch(&repo_path_buf, &remote, &branch, false);
            match result {
                Ok(msg) => {
                    let msg = if msg.is_empty() {
                        "Pushed successfully".into()
                    } else {
                        msg
                    };
                    let _ = tx.send(AppEvent::NotifySuccess(msg));
                }
                Err(msg) => {
                    let _ = tx.send(AppEvent::NotifyError(msg));
                }
            }
        });
    }

    fn execute_pull_current_branch(&mut self) {
        let repo_path = self.repository.path();
        let branch = match self.repository.head() {
            Head::Branch { name } => name.clone(),
            _ => {
                self.ec.send(AppEvent::NotifyError(
                    "Cannot pull: not on a branch".into(),
                ));
                return;
            }
        };
        let remote = actions::branch_upstream(repo_path, &branch)
            .ok()
            .and_then(|u| u.split('/').next().map(|s| s.to_string()))
            .unwrap_or_else(|| "origin".to_string());
        self.start_spinner("Pulling\u{2026}");
        let tx = self.ec.sender();
        let repo_path_buf = repo_path.to_path_buf();
        thread::spawn(move || {
            match actions::pull_branch(&repo_path_buf, &remote, &branch) {
                Ok(msg) => {
                    let msg = if msg.is_empty() {
                        "Pulled successfully".into()
                    } else {
                        msg
                    };
                    let _ = tx.send(AppEvent::Refresh(RefreshViewContext::List {
                        list_context: crate::view::ListRefreshViewContext {
                            commit_hash: String::new(),
                            selected: 0,
                            height: 20,
                            scroll_to_top: false,
                        },
                        pending_notification: Some(msg),
                    }));
                }
                Err(msg) => {
                    let _ = tx.send(AppEvent::NotifyError(msg));
                }
            }
        });
    }

    // Phase 2 - Git Actions execution
    fn execute_git_action(&mut self, target: String, action: GitAction) {
        let repo_path = self.repository.path();
        let is_refs_action = matches!(
            &action,
            GitAction::AddRemote { .. }
                | GitAction::RemoveRemote
                | GitAction::AddWorktree { .. }
                | GitAction::DeleteWorktree { .. }
        );
        let should_checkout_worktree =
            matches!(&action, GitAction::AddWorktree { checkout: true, .. });
        // Pre-compute the auto-generated worktree path before `action` is consumed.
        let worktree_raw_path = if let GitAction::AddWorktree { name, .. } = &action {
            worktree_path_for_branch(repo_path, name)
        } else {
            String::new()
        };
        let should_auto_fetch = matches!(
            action,
            GitAction::Commit { .. }
                | GitAction::Stash { .. }
                | GitAction::Push
                | GitAction::PushBranch { .. }
                | GitAction::PushTag
                | GitAction::AddTag { .. }
        );
        if matches!(&action, GitAction::Rebase { .. }) {
            self.start_spinner("Rebasing\u{2026}");
        }
        if matches!(&action, GitAction::Merge { .. }) {
            self.start_spinner("Merging\u{2026}");
        }
        let (result, success_label) = match action {
            GitAction::Checkout => {
                let r = if target.starts_with("refs/stash") {
                    actions::checkout_commit(repo_path, &target)
                } else {
                    actions::checkout_commit(repo_path, &target)
                };
                (r, None)
            }
            GitAction::CreateBranch { name, checkout } => (
                actions::create_branch_at(repo_path, &name, &target, checkout),
                None,
            ),
            GitAction::AddTag {
                name,
                annotated,
                message,
            } => {
                let msg = if annotated { message.as_deref() } else { None };
                (actions::create_tag(repo_path, &name, &target, msg), None)
            }
            GitAction::CherryPick {
                no_commit,
                record_origin,
            } => (
                actions::cherry_pick(repo_path, &target, no_commit, record_origin),
                None,
            ),
            GitAction::Revert => (actions::revert_commit(repo_path, &target), None),
            GitAction::Drop => (actions::drop_commit(repo_path, &target), None),
            GitAction::Merge {
                no_ff,
                squash,
                no_commit,
            } => (
                actions::merge_commit(repo_path, &target, no_ff, squash, no_commit),
                None,
            ),
            GitAction::Rebase {
                ignore_date,
                interactive,
            } => (
                actions::rebase_onto(repo_path, &target, ignore_date, interactive),
                None,
            ),
            GitAction::Reset { mode } => (actions::reset(repo_path, &target, &mode), None),
            GitAction::DeleteBranch { force } => {
                (actions::delete_branch(repo_path, &target, force), None)
            }
            GitAction::RenameBranch { new_name } => {
                (actions::rename_branch(repo_path, &target, &new_name), None)
            }
            GitAction::PushBranch { force } => {
                let remote = actions::branch_upstream(repo_path, &target)
                    .ok()
                    .and_then(|u| u.split('/').next().map(|s| s.to_string()))
                    .unwrap_or_else(|| "origin".to_string());
                (actions::push_branch(repo_path, &remote, &target, force), None)
            }
            GitAction::PullBranch { rebase: _ } => {
                let remote = actions::branch_upstream(repo_path, &target)
                    .ok()
                    .and_then(|u| u.split('/').next().map(|s| s.to_string()))
                    .unwrap_or_else(|| "origin".to_string());
                (actions::pull_branch(repo_path, &remote, &target), None)
            }
            GitAction::Fetch => (actions::fetch(repo_path), None),
            GitAction::DeleteTag => (actions::delete_tag(repo_path, &target), None),
            GitAction::PushTag => (actions::push_commit(repo_path, &target), None),
            GitAction::ApplyStash => (actions::apply_stash(repo_path, &target), None),
            GitAction::PopStash => (actions::pop_stash(repo_path, &target), None),
            GitAction::DropStash => (actions::drop_stash(repo_path, &target), None),
            GitAction::CreateBranchFromStash { branch_name } => (
                actions::create_branch_from_stash(repo_path, &branch_name, &target),
                None,
            ),
            GitAction::StageFile { file } => (
                actions::stage_file(repo_path, &file),
                Some(format!("Staged {}", file)),
            ),
            GitAction::StageAll => (
                actions::stage_all(repo_path),
                Some("Staged all files".into()),
            ),
            GitAction::UnstageFile { file } => (
                actions::unstage_file(repo_path, &file),
                Some(format!("Unstaged {}", file)),
            ),
            GitAction::UnstageAll => (
                actions::unstage_all(repo_path),
                Some("Unstaged all files".into()),
            ),
            GitAction::DiscardFile { file } => (
                actions::discard_file(repo_path, &file),
                Some(format!("Discarded {}", file)),
            ),
            GitAction::DiscardAll => (
                actions::discard_all(repo_path),
                Some("Discarded all changes".into()),
            ),
            GitAction::Stash { message, include_untracked } => (
                actions::stash(repo_path, message.as_deref(), include_untracked),
                Some("Stashed changes".into()),
            ),
            GitAction::Commit { message, amend } => {
                let label = if amend { "Amended commit" } else { "Committed" };
                (
                    actions::commit(repo_path, &message, amend),
                    Some(label.to_string()),
                )
            }
            GitAction::CleanUntracked => (
                actions::clean_untracked(repo_path),
                Some("Cleaned untracked files".into()),
            ),
            GitAction::Push => (actions::push_commit(repo_path, &target), None),
            GitAction::AbortRebase => (actions::abort_rebase(repo_path), None),
            GitAction::AbortMerge => (actions::abort_merge(repo_path), None),
            GitAction::AbortCherryPick => (actions::abort_cherry_pick(repo_path), None),
            GitAction::CreateArchive => {
                let r = std::process::Command::new("git")
                    .current_dir(repo_path)
                    .args([
                        "archive",
                        "--format=zip",
                        "-o",
                        &format!("{}.zip", target),
                        &target,
                    ])
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
                    .map_err(|e| format!("Failed to create archive: {}", e));
                (r, None)
            }
            GitAction::AddRemote { url } => (
                actions::add_remote(repo_path, &target, &url),
                None,
            ),
            GitAction::RemoveRemote => (
                actions::remove_remote(repo_path, &target),
                None,
            ),
            GitAction::PushSetUpstream { branch } => (
                actions::push_set_upstream(repo_path, &target, &branch),
                Some(format!("Pushed and set upstream to '{}/{}'.", target, branch)),
            ),
            GitAction::SetUpstream { branch } => (
                actions::set_upstream(repo_path, &target, &branch),
                Some(format!("Upstream set to '{}/{}'.", target, branch)),
            ),
            GitAction::AddWorktree { name, .. } => {
                let wt_path = worktree_path_for_branch(repo_path, &name);
                (
                    actions::add_worktree(repo_path, &wt_path, &name),
                    Some(format!("Worktree '{}' created", wt_path)),
                )
            }
            GitAction::DeleteWorktree { force } => (
                actions::delete_worktree(repo_path, &target, force),
                Some("Worktree removed".to_string()),
            ),
        };

        match result {
            Ok(_) => {
                self.close_dialog();
                if is_refs_action {
                    if let Some(label) = success_label {
                        self.ec.send(AppEvent::NotifySuccess(label));
                    }
                    if let View::Refs(ref mut view) = self.view {
                        view.refresh();
                    } else {
                        self.ec.send(AppEvent::Refresh(RefreshViewContext::List {
                            list_context: crate::view::ListRefreshViewContext {
                                commit_hash: String::new(),
                                selected: 0,
                                height: 20,
                                scroll_to_top: false,
                            },
                            pending_notification: None,
                        }));
                    }
                    if should_checkout_worktree && !worktree_raw_path.is_empty() {
                        // Resolve the absolute path now that git has created the worktree.
                        let abs_path = actions::list_worktrees(repo_path)
                            .into_iter()
                            .find(|wt| {
                                // Match by last path component or suffix
                                std::path::Path::new(&wt.path)
                                    .to_string_lossy()
                                    .ends_with(worktree_raw_path.trim_start_matches("../").trim_start_matches("./"))
                                    || wt.path == worktree_raw_path
                            })
                            .map(|wt| wt.path)
                            .unwrap_or_else(|| {
                                // Fallback: join with repo_path and normalize manually
                                repo_path.join(&worktree_raw_path)
                                    .to_string_lossy()
                                    .to_string()
                            });
                        self.ec.send(AppEvent::SwitchWorktree { path: abs_path });
                    }
                } else if let Some(label) = success_label {
                    self.ec.send(AppEvent::NotifySuccess(label));
                    self.ec.send(AppEvent::RefreshUncommitted);
                } else {
                    self.ec.send(AppEvent::Refresh(RefreshViewContext::List {
                        list_context: crate::view::ListRefreshViewContext {
                            commit_hash: String::new(),
                            selected: 0,
                            height: 20,
                            scroll_to_top: false,
                        },
                        pending_notification: Some("Operation completed successfully".into()),
                    }));
                }
                if should_auto_fetch {
                    self.ec.send(AppEvent::BackgroundFetch);
                }
            }
            Err(msg) => {
                self.ec.send(AppEvent::NotifyError(msg));
            }
        }
    }

    fn open_branch_detail(&mut self, branch_name: String) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => Some(view.take_list_state()),
            View::Detail(ref mut view) => Some(view.take_list_state()),
            View::UserCommand(ref mut view) => Some(view.take_list_state()),
            View::Refs(ref mut view) => Some(view.take_list_state()),
            View::BranchDetail(ref mut view) => view.take_list_state(),
            View::TagDetail(ref mut view) => view.take_list_state(),
            _ => None,
        };
        let repo_path = self.repository.path();
        let is_remote = branch_name.contains('/');
        let upstream = actions::branch_upstream(repo_path, &branch_name).ok();
        let (ahead, behind, comparison_label) = if upstream.is_some() {
            let a = actions::branch_ahead_count(repo_path, &branch_name).unwrap_or_default();
            let b = actions::branch_behind_count(repo_path, &branch_name).unwrap_or_default();
            let label = upstream.clone().unwrap_or_default();
            (a, b, label)
        } else if branch_name != self.ctx.git_default_branch {
            let default = self.ctx.git_default_branch.clone();
            let (a, b) = actions::branch_ahead_behind_vs(repo_path, &branch_name, &default)
                .unwrap_or_default();
            (a, b, default)
        } else {
            (String::new(), String::new(), String::new())
        };
        let (tip_hash, tip_commit_message) = actions::branch_tip_info(repo_path, &branch_name)
            .map(|s| {
                let mut parts = s.splitn(2, ' ');
                (
                    parts.next().unwrap_or("").to_string(),
                    parts.next().unwrap_or("").to_string(),
                )
            })
            .unwrap_or_default();
        let metadata = BranchMetadata {
            branch_name: branch_name.clone(),
            is_remote,
            tip_hash,
            tip_commit_message,
            tip_author: String::new(),
            tip_date: String::new(),
            upstream,
            ahead,
            behind,
            comparison_label,
        };
        self.view =
            View::BranchDetail(Box::new(crate::view::branch_detail::BranchDetailView::new(
                branch_name,
                metadata,
                commit_list_state,
                self.ctx.clone(),
                self.ec.sender(),
            )));
    }

    fn open_tag_detail(&mut self, tag_name: String) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => Some(view.take_list_state()),
            View::Detail(ref mut view) => Some(view.take_list_state()),
            View::UserCommand(ref mut view) => Some(view.take_list_state()),
            View::Refs(ref mut view) => Some(view.take_list_state()),
            View::BranchDetail(ref mut view) => view.take_list_state(),
            View::TagDetail(ref mut view) => view.take_list_state(),
            _ => None,
        };
        let _repo_path = self.repository.path();
        let metadata = TagMetadata {
            tag_name: tag_name.clone(),
            tag_type: "Tag".to_string(),
            target_hash: String::new(),
            target_commit_message: String::new(),
            tagger: None,
            date: None,
            message: None,
        };
        self.view = View::TagDetail(Box::new(crate::view::tag_detail::TagDetailView::new(
            tag_name,
            metadata,
            commit_list_state,
            self.ctx.clone(),
            self.ec.sender(),
        )));
    }

    fn open_uncommitted(&mut self) {
        let commit_list_state = match self.view {
            View::List(ref mut view) => Some(view.take_list_state()),
            View::Detail(ref mut view) => Some(view.take_list_state()),
            View::UserCommand(ref mut view) => Some(view.take_list_state()),
            View::Refs(ref mut view) => Some(view.take_list_state()),
            _ => None,
        };
        let changes = UncommittedChanges::load(self.repository.path()).unwrap_or_default();

        let load_diff_stats = |f: &crate::git::status::FileStatus, is_staged: bool| {
            let diff_result = if is_staged {
                DiffEntry::load_staged_for_file(self.repository.path(), &f.path)
            } else {
                DiffEntry::load_unstaged_for_file(self.repository.path(), &f.path)
            };
            match diff_result {
                Ok(diff_entry) => diff_entry.count_additions_and_deletions(),
                Err(_) => (0, 0),
            }
        };

        let convert = |f: &crate::git::status::FileStatus, is_staged: bool| {
            let (additions, deletions) = load_diff_stats(f, is_staged);
            crate::widget::uncommitted::UncommittedFile {
                status: f.status.clone(),
                path: f.path.clone(),
                old_path: f.old_path.clone(),
                additions,
                deletions,
            }
        };

        let staged: Vec<_> = changes.staged.iter().map(|f| convert(f, true)).collect();
        let unstaged: Vec<_> = changes.unstaged.iter().map(|f| convert(f, false)).collect();
        let untracked: Vec<_> = changes
            .untracked
            .iter()
            .map(|f| convert(f, false))
            .collect();
        self.view = View::Uncommitted(Box::new(crate::view::uncommitted::UncommittedView::new(
            unstaged,
            staged,
            untracked,
            commit_list_state,
            self.ctx.clone(),
            self.ec.sender(),
        )));
    }

    fn stage_file(&mut self, file: String) {
        match actions::stage_file(self.repository.path(), &file) {
            Ok(_) => {
                self.ec
                    .send(AppEvent::NotifySuccess(format!("Staged {}", file)));
                self.ec.send(AppEvent::RefreshUncommitted);
            }
            Err(msg) => self.ec.send(AppEvent::NotifyError(msg)),
        }
    }

    fn unstage_file(&mut self, file: String) {
        match actions::unstage_file(self.repository.path(), &file) {
            Ok(_) => {
                self.ec
                    .send(AppEvent::NotifySuccess(format!("Unstaged {}", file)));
                self.ec.send(AppEvent::RefreshUncommitted);
            }
            Err(msg) => self.ec.send(AppEvent::NotifyError(msg)),
        }
    }

    fn discard_file(&mut self, file: String) {
        match actions::discard_file(self.repository.path(), &file) {
            Ok(_) => {
                self.ec
                    .send(AppEvent::NotifySuccess(format!("Discarded {}", file)));
                self.ec.send(AppEvent::RefreshUncommitted);
            }
            Err(msg) => self.ec.send(AppEvent::NotifyError(msg)),
        }
    }

    fn refresh_uncommitted(&mut self) {
        if let View::Uncommitted(ref mut view) = self.view {
            let selected_path = view.selected_path().map(|s| s.to_string());
            let _was_section = view.section();

            let changes = UncommittedChanges::load(self.repository.path()).unwrap_or_default();

            let load_diff_stats = |f: &crate::git::status::FileStatus, is_staged: bool| {
                let diff_result = if is_staged {
                    DiffEntry::load_staged_for_file(self.repository.path(), &f.path)
                } else {
                    DiffEntry::load_unstaged_for_file(self.repository.path(), &f.path)
                };

                match diff_result {
                    Ok(diff_entry) => {
                        let (additions, deletions) = diff_entry.count_additions_and_deletions();
                        (additions, deletions)
                    }
                    Err(_) => (0, 0),
                }
            };

            let convert = |f: &crate::git::status::FileStatus, is_staged: bool| {
                let (additions, deletions) = load_diff_stats(f, is_staged);
                crate::widget::uncommitted::UncommittedFile {
                    status: f.status.clone(),
                    path: f.path.clone(),
                    old_path: f.old_path.clone(),
                    additions,
                    deletions,
                }
            };

            view.staged = changes.staged.iter().map(|f| convert(f, true)).collect();
            view.unstaged = changes.unstaged.iter().map(|f| convert(f, false)).collect();
            view.untracked = changes
                .untracked
                .iter()
                .map(|f| convert(f, false))
                .collect();

            if let Some(ref path) = selected_path {
                view.reselect(path);
            }
        }
    }
}

fn worktree_path_for_branch(_repo_path: &std::path::Path, name: &str) -> String {
    name.to_string()
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

fn head_commit_hash_from_repository(repository: &Repository) -> Option<crate::git::CommitHash> {
    match repository.head() {
        Head::Detached { target } => Some(target.clone()),
        Head::Branch { name } => {
            // Look up the commit hash that the branch points to via refs
            repository.all_refs().into_iter().find_map(|r| {
                if let crate::git::Ref::Branch {
                    name: ref_name,
                    target,
                } = r
                {
                    if ref_name == name {
                        Some(target.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        }
        Head::None => None,
    }
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
