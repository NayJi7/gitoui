use std::{rc::Rc, thread};

use ratatui::{
    crossterm::event::KeyEvent,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph},
    Frame,
};

use crate::{
    app::AppContext,
    config::{save, CoreConfig, DiffMode, UiConfig},
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    github_auth::{self, GithubAuthState},
    highlight::SyntaxHighlighter,
    view::{graph_preview::GraphPreview, View},
    GraphStyle, ImageProtocolType,
};

const CONFIG_ITEM_COUNT: usize = 11;
const TEXT_EDIT_START_INDEX: usize = 6;
const GITHUB_AUTH_INDEX: usize = 9;
const GITHUB_AVATARS_INDEX: usize = 10;
const CONFIG_ITEM_INDENT: &str = " ";

#[derive(Debug, Clone)]
struct PendingGithubDevice {
    user_code: String,
    verification_uri: String,
}

impl<'a> std::fmt::Debug for ConfigView<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigView")
            .field("selected", &self.selected)
            .field("editing_text", &self.editing_text)
            .field("editing_value", &self.editing_value)
            .finish()
    }
}

pub struct ConfigView<'a> {
    before: View<'a>,
    selected: usize,
    ctx: Rc<AppContext>,
    tx: Sender,
    core_config: CoreConfig,
    ui_config: UiConfig,
    github_auth_state: GithubAuthState,
    github_auth_pending: bool,
    pending_github_device: Option<PendingGithubDevice>,
    editing_text: bool,
    editing_value: String,
    theme_preview: Option<SyntaxHighlighter>,
    graph_preview: Option<GraphPreview>,
    pending_preview_uploads: Vec<String>,
    cumulative_preview_image_ids: Vec<u32>,
    last_preview_rows: Option<(u16, u16)>,
    pending_preview_deletes: Vec<u16>,
    left_area: Rect,
    left_item_rows: Vec<Option<usize>>,
}

impl<'a> ConfigView<'a> {
    fn config_left_width(&self, total_width: u16) -> u16 {
        let values = [
            graph_style_display(self.core_config.graph_style()),
            diff_mode_display(self.ui_config.common.diff_mode),
            mouse_display(self.ui_config.common.mouse_enabled),
            protocol_display(self.core_config.protocol()),
            self.core_config.option.theme.clone(),
            self.core_config
                .date_time_format()
                .display_name()
                .to_string(),
            self.core_config
                .user_name()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "(from git)".into()),
            self.core_config
                .user_email()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "(from git)".into()),
            self.core_config
                .default_branch()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "(from git)".into()),
            github_auth_display(&self.github_auth_state, self.github_auth_pending),
            if self.core_config.github_avatars() {
                "enabled".to_string()
            } else {
                "disabled".to_string()
            },
        ];
        let names = [
            "Theme",
            "Graph Style",
            "Diff Mode",
            "Mouse",
            "Date Format",
            "Image Protocol",
            "Git Name",
            "Git Email",
            "Default Branch",
            "GitHub Auth",
            "Github Avatars",
        ];
        let content_width = names
            .iter()
            .zip(values.iter())
            .enumerate()
            .map(|(i, (_name, value))| {
                let display = config_value_display(i, value, false, "");
                CONFIG_ITEM_INDENT.len() + 18 + console::measure_text_width(&display)
            })
            .max()
            .unwrap_or(36) as u16;
        let min_width = 34.min(total_width);
        let max_width = (total_width.saturating_mul(48) / 100).max(min_width);
        (content_width + 4).max(min_width).min(max_width)
    }

    fn render_vertical_separator(&self, f: &mut Frame, area: Rect) {
        if area.width == 0 {
            return;
        }
        let height = vertical_separator_height(area.height);
        if height == 0 {
            return;
        }
        let y = area.y;
        let line = Line::from(Span::styled(
            "│",
            Style::default().fg(self.ctx.color_theme.divider_fg),
        ));
        for offset in 0..height {
            let segment = Rect {
                x: area.x,
                y: y + offset,
                width: 1,
                height: 1,
            };
            f.render_widget(Paragraph::new(line.clone()), segment);
        }
    }

    pub fn new(before: View<'a>, ctx: Rc<AppContext>, tx: Sender) -> Self {
        let core_config = ctx.core_config.clone();
        let ui_config = ctx.ui_config.clone();
        let github_auth_state = ctx.github_auth_state.clone();
        Self {
            before,
            selected: 0,
            ctx,
            tx,
            core_config,
            ui_config,
            github_auth_state,
            github_auth_pending: false,
            pending_github_device: None,
            editing_text: false,
            editing_value: String::new(),
            theme_preview: None,
            graph_preview: None,
            pending_preview_uploads: Vec::new(),
            cumulative_preview_image_ids: Vec::new(),
            last_preview_rows: None,
            pending_preview_deletes: Vec::new(),
            left_area: Rect::default(),
            left_item_rows: Vec::new(),
        }
    }

    fn github_avatars_selectable(&self) -> bool {
        self.github_auth_state.is_authenticated()
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        if self.editing_text {
            self.handle_text_edit_event(event_with_count, key);
            return;
        }

        let event = event_with_count.event;
        let count = event_with_count.count;
        match event {
            UserEvent::NavigateUp | UserEvent::SelectUp => {
                if self.selected > 0 {
                    self.selected -= 1;
                    if self.selected == GITHUB_AVATARS_INDEX && !self.github_avatars_selectable() {
                        self.selected -= 1;
                    }
                }
            }
            UserEvent::NavigateDown | UserEvent::SelectDown => {
                if self.selected + 1 < CONFIG_ITEM_COUNT {
                    self.selected += 1;
                    if self.selected == GITHUB_AVATARS_INDEX && !self.github_avatars_selectable() {
                        if self.selected + 1 < CONFIG_ITEM_COUNT {
                            self.selected += 1;
                        } else {
                            self.selected -= 1;
                        }
                    }
                }
            }
            UserEvent::Confirm => {
                if self.selected == GITHUB_AUTH_INDEX {
                    self.handle_github_auth();
                } else if self.selected == GITHUB_AVATARS_INDEX {
                    if self.github_avatars_selectable() {
                        self.cycle_option();
                    }
                } else if self.selected >= TEXT_EDIT_START_INDEX {
                    self.start_text_edit();
                } else {
                    self.cycle_option();
                }
            }
            UserEvent::NavigateRight => {
                if self.selected == GITHUB_AUTH_INDEX {
                    self.handle_github_auth();
                } else if self.selected == GITHUB_AVATARS_INDEX {
                    if self.github_avatars_selectable() {
                        self.cycle_option();
                    }
                } else if self.selected >= TEXT_EDIT_START_INDEX {
                    self.start_text_edit();
                } else {
                    self.cycle_option();
                }
            }
            UserEvent::NavigateLeft => {
                if self.selected == GITHUB_AUTH_INDEX {
                    self.handle_github_auth();
                } else if self.selected == GITHUB_AVATARS_INDEX {
                    if self.github_avatars_selectable() {
                        self.cycle_option_prev();
                    }
                } else if self.selected >= TEXT_EDIT_START_INDEX {
                    self.start_text_edit();
                } else {
                    self.cycle_option_prev();
                }
            }
            UserEvent::Cancel | UserEvent::Close | UserEvent::Config => {
                self.tx.send(AppEvent::CloseConfig);
            }
            UserEvent::PageDown => {
                for _ in 0..count {
                    if self.selected + 1 < CONFIG_ITEM_COUNT {
                        self.selected += 1;
                    }
                }
                if self.selected == GITHUB_AVATARS_INDEX
                    && !self.github_avatars_selectable()
                    && self.selected + 1 < CONFIG_ITEM_COUNT
                {
                    self.selected += 1;
                }
            }
            UserEvent::PageUp => {
                for _ in 0..count {
                    if self.selected > 0 {
                        self.selected -= 1;
                    }
                }
                if self.selected == GITHUB_AVATARS_INDEX
                    && !self.github_avatars_selectable()
                    && self.selected > 0
                {
                    self.selected -= 1;
                }
            }
            UserEvent::GoToTop => {
                self.selected = 0;
            }
            UserEvent::GoToBottom => {
                self.selected = CONFIG_ITEM_COUNT - 1;
                if self.selected == GITHUB_AVATARS_INDEX
                    && !self.github_avatars_selectable()
                    && self.selected > 0
                {
                    self.selected -= 1;
                }
            }
            _ => {}
        }
    }

    fn start_text_edit(&mut self) {
        let current_value = match self.selected {
            6 => self.core_config.user_name().unwrap_or("").to_string(),
            7 => self.core_config.user_email().unwrap_or("").to_string(),
            8 => self.core_config.default_branch().unwrap_or("").to_string(),
            _ => return,
        };
        self.editing_text = true;
        self.editing_value = current_value;
    }

    fn handle_github_auth(&mut self) {
        if self.github_auth_pending {
            self.copy_and_open_pending_github_device();
            return;
        }

        if self.github_auth_state.is_authenticated() {
            match github_auth::clear_state() {
                Ok(()) => {
                    self.pending_github_device = None;
                    self.github_auth_state = GithubAuthState {
                        token: None,
                        login: None,
                        message: None,
                    };
                    self.tx
                        .send(AppEvent::NotifySuccess("Logged out successfully".into()));
                }
                Err(e) => {
                    self.github_auth_state.message = Some(e.clone());
                    self.tx.send(AppEvent::NotifyError(e));
                }
            }
            return;
        }

        match github_auth::request_device_code() {
            Ok(device) => {
                self.github_auth_pending = true;
                self.pending_github_device = Some(PendingGithubDevice {
                    user_code: device.user_code.clone(),
                    verification_uri: device.verification_uri.clone(),
                });
                self.github_auth_state.message = Some(format!(
                    "Enter {} at {}",
                    device.user_code, device.verification_uri
                ));
                self.copy_and_open_pending_github_device();
                let tx = self.tx.clone();
                thread::spawn(move || {
                    let state = match github_auth::poll_for_token(&device) {
                        Ok(state) => state,
                        Err(e) => GithubAuthState {
                            token: None,
                            login: None,
                            message: Some(e),
                        },
                    };
                    tx.send(AppEvent::GithubAuthFinished(state));
                });
            }
            Err(e) => {
                self.github_auth_state.message = Some(e.clone());
                self.tx.send(AppEvent::NotifyError(e));
            }
        }
    }

    fn copy_and_open_pending_github_device(&self) {
        if let Some(device) = &self.pending_github_device {
            self.tx.send(AppEvent::CopyRawToClipboard {
                value: device.user_code.clone(),
                success_message: format!("{} copied to your clipboard", device.user_code),
            });
            self.tx
                .send(AppEvent::OpenUrl(device.verification_uri.clone()));
        }
    }

    pub fn finish_github_auth(&mut self, state: GithubAuthState) {
        self.github_auth_pending = false;
        self.pending_github_device = None;
        let authenticated = state.is_authenticated();
        let login = state.login.clone();
        let message = state.message.clone();
        self.github_auth_state = state;
        if authenticated {
            self.tx.send(AppEvent::NotifySuccess(format!(
                "GitHub auth connected as {}",
                login.unwrap_or_else(|| "unknown".into())
            )));
        } else if let Some(message) = message {
            self.tx.send(AppEvent::NotifyError(message));
        }
    }

    fn handle_text_edit_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        use ratatui::crossterm::event::KeyCode;
        let event = event_with_count.event;
        match event {
            UserEvent::Confirm => {
                self.finish_text_edit();
                return;
            }
            UserEvent::Cancel => {
                self.cancel_text_edit();
                return;
            }
            _ => {}
        }
        use ratatui::crossterm::event::KeyModifiers;
        match key.code {
            KeyCode::Char(c) => {
                self.editing_value.push(c);
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::CONTROL) => {
                while self.editing_value.chars().last().map(|c| !c.is_alphanumeric()).unwrap_or(false) {
                    self.editing_value.pop();
                }
                while self.editing_value.chars().last().map(|c| c.is_alphanumeric()).unwrap_or(false) {
                    self.editing_value.pop();
                }
            }
            KeyCode::Backspace => {
                self.editing_value.pop();
            }
            _ => {}
        }
    }

    fn finish_text_edit(&mut self) {
        self.editing_text = false;
        let value = if self.editing_value.trim().is_empty() {
            None
        } else {
            Some(self.editing_value.clone())
        };
        match self.selected {
            6 => self.core_config.set_user_name(value),
            7 => self.core_config.set_user_email(value),
            8 => self.core_config.set_default_branch(value),
            _ => {}
        }
        if let Err(e) = save(&self.core_config, &self.ui_config) {
            self.tx.send(AppEvent::NotifyError(e.to_string()));
        }
    }

    fn cancel_text_edit(&mut self) {
        self.editing_text = false;
    }

    fn cycle_option_prev(&mut self) {
        match self.selected {
            0 => {
                let themes = crate::themes::list_themes();
                let current = self.core_config.option.theme.as_str();
                let idx = themes.iter().position(|&t| t == current).unwrap_or(0);
                let prev_idx = if idx == 0 { themes.len() - 1 } else { idx - 1 };
                let new_theme = themes[prev_idx];
                self.core_config.option.theme = new_theme.to_string();
                if let Some(def) = crate::themes::get_theme(new_theme) {
                    self.core_config.option.syntax_theme = def.syntax_theme.to_owned();
                }
                self.theme_preview = None;
            }
            1 => {
                let current = self.core_config.graph_style();
                let prev = match current {
                    GraphStyle::Rounded => GraphStyle::Smooth,
                    GraphStyle::Angular => GraphStyle::Rounded,
                    GraphStyle::Smooth => GraphStyle::Angular,
                };
                self.core_config.set_graph_style(prev);
            }
            2 => {
                let prev = match self.ui_config.common.diff_mode {
                    DiffMode::Enhanced => DiffMode::Raw,
                    DiffMode::Raw => DiffMode::Enhanced,
                };
                self.ui_config.common.set_diff_mode(prev);
            }
            3 => {
                self.ui_config
                    .common
                    .set_mouse_enabled(!self.ui_config.common.mouse_enabled);
            }
            4 => {
                let prev = self.core_config.date_time_format().cycle_prev();
                self.core_config.set_date_time_format(prev);
            }
            5 => {
                let current = self
                    .core_config
                    .protocol()
                    .unwrap_or(ImageProtocolType::Auto);
                let prev = match current {
                    ImageProtocolType::Auto => ImageProtocolType::KittyUnicode,
                    ImageProtocolType::Kitty => ImageProtocolType::Auto,
                    ImageProtocolType::Iterm => ImageProtocolType::Kitty,
                    ImageProtocolType::Sixel => ImageProtocolType::Iterm,
                    ImageProtocolType::KittyUnicode => ImageProtocolType::Sixel,
                };
                self.core_config.set_protocol(prev);
            }
            GITHUB_AVATARS_INDEX => {
                self.core_config
                    .set_github_avatars(!self.core_config.github_avatars());
            }
            _ => {}
        }

        if let Err(e) = save(&self.core_config, &self.ui_config) {
            self.tx.send(AppEvent::NotifyError(e.to_string()));
        }
    }

    fn cycle_option(&mut self) {
        match self.selected {
            0 => {
                let themes = crate::themes::list_themes();
                let current = self.core_config.option.theme.as_str();
                let idx = themes.iter().position(|&t| t == current).unwrap_or(0);
                let next_idx = (idx + 1) % themes.len();
                let new_theme = themes[next_idx];
                self.core_config.option.theme = new_theme.to_string();
                if let Some(def) = crate::themes::get_theme(new_theme) {
                    self.core_config.option.syntax_theme = def.syntax_theme.to_owned();
                }
                self.theme_preview = None;
            }
            1 => {
                let current = self.core_config.graph_style();
                let next = match current {
                    GraphStyle::Rounded => GraphStyle::Angular,
                    GraphStyle::Angular => GraphStyle::Smooth,
                    GraphStyle::Smooth => GraphStyle::Rounded,
                };
                self.core_config.set_graph_style(next);
            }
            2 => {
                let next = match self.ui_config.common.diff_mode {
                    DiffMode::Enhanced => DiffMode::Raw,
                    DiffMode::Raw => DiffMode::Enhanced,
                };
                self.ui_config.common.set_diff_mode(next);
            }
            3 => {
                self.ui_config
                    .common
                    .set_mouse_enabled(!self.ui_config.common.mouse_enabled);
            }
            4 => {
                let next = self.core_config.date_time_format().cycle_next();
                self.core_config.set_date_time_format(next);
            }
            5 => {
                let current = self
                    .core_config
                    .protocol()
                    .unwrap_or(ImageProtocolType::Auto);
                let next = match current {
                    ImageProtocolType::Auto => ImageProtocolType::Kitty,
                    ImageProtocolType::Kitty => ImageProtocolType::Iterm,
                    ImageProtocolType::Iterm => ImageProtocolType::Sixel,
                    ImageProtocolType::Sixel => ImageProtocolType::KittyUnicode,
                    ImageProtocolType::KittyUnicode => ImageProtocolType::Auto,
                };
                self.core_config.set_protocol(next);
            }
            GITHUB_AVATARS_INDEX => {
                self.core_config
                    .set_github_avatars(!self.core_config.github_avatars());
            }
            _ => {}
        }

        if let Err(e) = save(&self.core_config, &self.ui_config) {
            self.tx.send(AppEvent::NotifyError(e.to_string()));
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let block = Block::default().padding(Padding::new(2, 2, 1, 1));
        let inner = block.inner(area);
        f.render_widget(block, area);

        let title = Line::from(vec![
            Span::styled(
                "Configuration",
                Style::default()
                    .fg(self.ctx.color_theme.list_ref_stash_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  Settings & preferences",
                Style::default().fg(self.ctx.color_theme.divider_fg),
            ),
        ]);
        let title_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        f.render_widget(Paragraph::new(title), title_area);

        // Separator line
        let sep_area = Rect {
            x: inner.x,
            y: inner.y + 1,
            width: inner.width,
            height: 1,
        };
        let sep_style = Style::default().fg(self.ctx.color_theme.divider_fg);
        let sep_line = Line::from("╌".repeat(inner.width as usize)).style(sep_style);
        f.render_widget(Paragraph::new(sep_line), sep_area);

        // Split remaining area into two columns
        let content_area = Rect {
            x: inner.x,
            y: inner.y + 3,
            width: inner.width,
            height: inner.height.saturating_sub(4),
        };
        let left_width = self.config_left_width(content_area.width);
        let separator_area = Rect {
            x: content_area.x + left_width,
            y: content_area.y,
            width: 1,
            height: content_area.height,
        };
        let left_area = Rect {
            x: content_area.x,
            y: content_area.y,
            width: left_width.saturating_sub(1),
            height: content_area.height,
        };
        let right_area = Rect {
            x: separator_area.x + 2,
            y: content_area.y,
            width: content_area.right().saturating_sub(separator_area.x + 2),
            height: content_area.height,
        };
        self.left_area = left_area;
        self.render_vertical_separator(f, separator_area);

        // Items list (left column)
        let items = vec![
            (
                "Theme",
                self.core_config.option.theme.clone(),
                false,
            ),
            (
                "Graph Style",
                graph_style_display(self.core_config.graph_style()),
                false,
            ),
            (
                "Diff Mode",
                diff_mode_display(self.ui_config.common.diff_mode),
                false,
            ),
            (
                "Mouse",
                mouse_display(self.ui_config.common.mouse_enabled),
                false,
            ),
            (
                "Date Format",
                self.core_config
                    .date_time_format()
                    .display_name()
                    .to_string(),
                false,
            ),
            (
                "Image Protocol",
                protocol_display(self.core_config.protocol()),
                false,
            ),
            (
                "Git Name",
                self.core_config
                    .user_name()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "(from git)".into()),
                false,
            ),
            (
                "Git Email",
                self.core_config
                    .user_email()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "(from git)".into()),
                false,
            ),
            (
                "Default Branch",
                self.core_config
                    .default_branch()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "(from git)".into()),
                false,
            ),
            (
                "GitHub Auth",
                github_auth_display(&self.github_auth_state, self.github_auth_pending),
                false,
            ),
            (
                "Github Avatars",
                if self.core_config.github_avatars() {
                    "enabled".to_string()
                } else {
                    "disabled".to_string()
                },
                true,
            ),
        ];

        let avatars_selectable = self.github_avatars_selectable();
        let dim_fg = self.ctx.color_theme.divider_fg;
        let mut lines: Vec<Line> = vec![config_section_line("Interface", &self.ctx.color_theme)];
        let mut item_rows: Vec<Option<usize>> = vec![None];
        for (i, (name, value, indented)) in items.iter().enumerate() {
            if i == 6 {
                lines.push(Line::from(""));
                item_rows.push(None);
                lines.push(config_section_line("Git", &self.ctx.color_theme));
                item_rows.push(None);
            } else if i == GITHUB_AUTH_INDEX {
                lines.push(Line::from(""));
                item_rows.push(None);
                lines.push(config_section_line("GitHub", &self.ctx.color_theme));
                item_rows.push(None);
            }

            let display = config_value_display(
                i,
                value,
                self.editing_text && i == self.selected,
                &self.editing_value,
            );
            let is_grayed = *indented && !avatars_selectable;
            let label = format!("{CONFIG_ITEM_INDENT}{name}");
            let value_prefix = "";
            let label_fg = if is_grayed {
                dim_fg
            } else {
                self.ctx.color_theme.fg
            };
            let value_fg = if is_grayed {
                dim_fg
            } else {
                config_value_fg(config_value_kind(i), &self.ctx.color_theme)
            };
            let value_style = if self.editing_text && i == self.selected {
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .bg(self.ctx.color_theme.list_selected_bg)
            } else if config_value_kind(i) == ConfigValueKind::Input && !is_grayed {
                Style::default()
                    .fg(self.ctx.color_theme.detail_label_fg)
                    .bg(self.ctx.color_theme.list_selected_bg)
                    .add_modifier(config_value_modifier(i))
            } else {
                Style::default()
                    .fg(value_fg)
                    .add_modifier(config_value_modifier(i))
            };
            let spans = vec![
                Span::styled(format!("{:<18}", label), Style::default().fg(label_fg)),
                Span::styled(format!("{value_prefix}{display}"), value_style),
            ];
            let mut line = Line::from(spans);
            if i == self.selected && !is_grayed {
                line = line.style(
                    Style::default()
                        .fg(self.ctx.color_theme.list_selected_fg)
                        .bg(self.ctx.color_theme.list_selected_bg)
                        .add_modifier(Modifier::BOLD),
                );
            }
            lines.push(line);
            item_rows.push(Some(i));
        }
        self.left_item_rows = item_rows;

        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, left_area);

        // Description / preview (right column)
        let git_name = &self.ctx.git_user_name;
        let git_email = &self.ctx.git_user_email;
        let descriptions: Vec<String> = vec![
            "Color theme applied to the entire interface, including diff syntax highlighting.".into(),
            "Controls how commit connection lines are rendered in the graph.".into(),
            "Enhanced shows contextual line numbers; Raw shows plain git diff output.".into(),
            "Enable mouse support for clicking and scrolling.".into(),
            "Date and time display format for commits in the list and detail views.".into(),
            "Terminal image protocol used for rendering commit graph images.".into(),
            format!(
                "Override git user.name for commits.\n\nCurrent git config: '{}'",
                git_name
            ),
            format!(
                "Override git user.email for commits.\n\nCurrent git config: '{}'",
                git_email
            ),
            format!(
                "Override the branch name used by gitoui when initializing a new git repository.\n\nCurrent git config: '{}'",
                self.ctx.git_default_branch
            ),
            github_auth_description(&self.github_auth_state, self.github_auth_pending),
            github_avatars_description(&self.github_auth_state, self.core_config.github_avatars()),
        ];

        let selected_name = items
            .get(self.selected)
            .map(|(name, _, _)| *name)
            .unwrap_or("Description");
        let mut right_lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled(
                    "Details",
                    Style::default()
                        .fg(self.ctx.color_theme.list_ref_stash_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  / {selected_name}"),
                    Style::default().fg(self.ctx.color_theme.divider_fg),
                ),
            ]),
            Line::from(""),
        ];
        for line in descriptions[self.selected].lines() {
            let style = if line.contains("Might cause") {
                Style::default().fg(self.ctx.color_theme.status_warn_fg)
            } else if line.starts_with("Current git config") {
                Style::default().fg(self.ctx.color_theme.status_success_fg)
            } else {
                Style::default().fg(self.ctx.color_theme.fg)
            };
            right_lines.push(Line::from(Span::styled(line.to_string(), style)));
        }

        match self.selected {
            0 => {
                right_lines.push(Line::from(""));
                right_lines.push(Line::from(vec![Span::styled(
                    "Preview",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                right_lines.push(Line::from(""));

                let preview_code = vec![
                    "// Example code",
                    "fn greet(name: &str) -> String {",
                    "    let count = 42;",
                    "    format!(\"Hello {}!\", name)",
                    "}",
                ];

                if self.theme_preview.is_none() {
                    self.theme_preview = SyntaxHighlighter::new_with_theme(
                        "test.rs",
                        &self.core_config.option.syntax_theme,
                    );
                }
                if let Some(ref mut highlighter) = self.theme_preview {
                    for line in preview_code {
                        let spans = highlighter.highlight_line(line, Style::default(), None);
                        right_lines.push(Line::from(spans));
                    }
                } else {
                    for line in preview_code {
                        right_lines.push(Line::from(line));
                    }
                }
            }
            1 => {
                right_lines.push(Line::from(""));
                right_lines.push(Line::from(vec![Span::styled(
                    "Preview",
                    Style::default().add_modifier(Modifier::BOLD),
                )]));
                right_lines.push(Line::from(""));
                // Reserve 5 blank lines under "Preview" (3 image rows + 2 vertical
                // padding rows). Image cells will be written directly to the buffer
                // after the paragraph is rendered.
                for _ in 0..5 {
                    right_lines.push(Line::from(""));
                }
            }
            _ => {}
        }

        // Leaving the Graph Style item — pre-clear the cells that previously held
        // the preview so labels/image-trailing-spaces are overwritten with plain
        // spaces. Paragraph rendering will then write the new content (e.g. the
        // theme code preview) over those spaces. Kitty persistent placements are
        // evicted via the post-draw `pending_preview_deletes` queue.
        if self.selected != 1 {
            if let Some((y, count)) = self.last_preview_rows.take() {
                self.clear_preview_cells(f, right_area, y, count);
                for i in 0..count {
                    self.pending_preview_deletes.push(y + i);
                }
            }
        }

        let right_paragraph = Paragraph::new(right_lines);
        f.render_widget(right_paragraph, right_area);

        if self.selected == 1 {
            self.render_graph_style_preview(f, right_area);
        }

        if self.editing_text
            && self.selected >= TEXT_EDIT_START_INDEX
            && self.selected != GITHUB_AUTH_INDEX
            && self.selected != GITHUB_AVATARS_INDEX
        {
            let cursor_x = left_area.x + 18 + self.editing_value.len() as u16;
            let cursor_y = self
                .left_item_rows
                .iter()
                .position(|idx| *idx == Some(self.selected))
                .map(|row| left_area.y + row as u16)
                .unwrap_or(left_area.y);
            f.set_cursor_position((cursor_x, cursor_y));
        }
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        let Some(item_idx) = self.left_area_item_index(col, row) else {
            return;
        };
        if item_idx < CONFIG_ITEM_COUNT {
            if item_idx == GITHUB_AVATARS_INDEX && !self.github_avatars_selectable() {
                return;
            }
            self.selected = item_idx;
            if self.selected == GITHUB_AUTH_INDEX {
                self.handle_github_auth();
            } else if self.selected == GITHUB_AVATARS_INDEX {
                self.cycle_option();
            } else if self.selected >= TEXT_EDIT_START_INDEX {
                self.start_text_edit();
            } else {
                self.cycle_option();
            }
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        let Some(item_idx) = self.left_area_item_index(col, row) else {
            return;
        };
        if item_idx < CONFIG_ITEM_COUNT && !self.editing_text {
            self.selected = item_idx;
        }
    }

    fn left_area_item_index(&self, col: u16, row: u16) -> Option<usize> {
        if col < self.left_area.x || col >= self.left_area.x.saturating_add(self.left_area.width) {
            return None;
        }
        if row < self.left_area.y || row >= self.left_area.y.saturating_add(self.left_area.height) {
            return None;
        }
        let row_idx = (row - self.left_area.y) as usize;
        self.left_item_rows.get(row_idx).and_then(|idx| *idx)
    }

    fn render_graph_style_preview(&mut self, f: &mut Frame, right_area: Rect) {
        let style: crate::graph::GraphStyle = Some(self.core_config.graph_style()).into();
        let needs_rebuild = self
            .graph_preview
            .as_ref()
            .map(|p| p.style != style)
            .unwrap_or(true);
        if needs_rebuild {
            let bg_rgb = if let ratatui::style::Color::Rgb(r, g, b) = self.ctx.color_theme.bg {
                Some((r, g, b))
            } else {
                None
            };
            let mut preview = GraphPreview::build(
                style,
                &self.ctx.graph_color_set,
                self.ctx.image_protocol,
                bg_rgb,
            );
            self.pending_preview_uploads
                .append(&mut preview.pending_uploads);
            self.cumulative_preview_image_ids
                .extend(preview.image_ids.iter().copied());
            self.graph_preview = Some(preview);
        }

        let preview = match &self.graph_preview {
            Some(p) => p,
            None => return,
        };

        // Image rows live just below the "Preview" header/blank within right_area.
        // Layout: 0=Details header, 1=blank, 2=description (1 line), 3=blank,
        //         4="Preview", 5=blank, 6..8=image rows (3 commits).
        let preview_top = right_area.y.saturating_add(6);
        // Labels: "main" matches the lane-0 branch color, "feature" the lane-1
        // branch color. Third row (fork commit) intentionally has no label.
        let main_color = self.ctx.graph_color_set.get(0).to_ratatui_color();
        let feature_color = self.ctx.graph_color_set.get(1).to_ratatui_color();
        let labels: [&str; 3] = ["main", "feature", ""];
        let label_styles = [
            Style::default().fg(main_color).add_modifier(Modifier::BOLD),
            Style::default().fg(feature_color).add_modifier(Modifier::BOLD),
            Style::default(),
        ];

        let buf = f.buffer_mut();
        let max_image_cells = right_area.width as usize;
        let mut max_used_cells = 0usize;
        for (i, image) in preview.rows.iter().enumerate() {
            let y = preview_top + i as u16;
            if y >= right_area.y.saturating_add(right_area.height) {
                break;
            }
            let used = image.cells().len().min(max_image_cells);
            if used > max_used_cells {
                max_used_cells = used;
            }
            for (x, cell) in image.cells().iter().take(max_image_cells).enumerate() {
                let col = right_area.x + x as u16;
                if col >= right_area.x.saturating_add(right_area.width) {
                    break;
                }
                let buf_cell = &mut buf[(col, y)];
                buf_cell.set_symbol(cell.symbol());
                buf_cell.set_style(cell.style().bg(self.ctx.color_theme.bg));
                buf_cell.set_skip(cell.skip());
            }
        }

        // Labels are written to the right of the widest image row, with a 2-cell gap.
        let label_x = right_area.x + max_used_cells as u16 + 2;
        for (i, label) in labels.iter().enumerate() {
            if i >= preview.rows.len() {
                break;
            }
            if label.is_empty() {
                continue;
            }
            let y = preview_top + i as u16;
            if y >= right_area.y.saturating_add(right_area.height) {
                break;
            }
            let area = Rect {
                x: label_x.min(right_area.x.saturating_add(right_area.width)),
                y,
                width: right_area
                    .x
                    .saturating_add(right_area.width)
                    .saturating_sub(label_x),
                height: 1,
            };
            if area.width == 0 {
                continue;
            }
            let line = Line::from(Span::styled((*label).to_string(), label_styles[i]));
            f.render_widget(Paragraph::new(line), area);
        }

        // Record where we drew the preview so we can delete the Kitty placements
        // when the user navigates to a different config item.
        self.last_preview_rows = Some((preview_top, preview.rows.len() as u16));
    }

    fn clear_preview_cells(&self, f: &mut Frame, right_area: Rect, y_start: u16, count: u16) {
        // We only need to overwrite the previous frame's text-bearing cells (image
        // placeholders and labels) with a regular space so the buffer diff sends an
        // update. We deliberately avoid embedding the protocol's clear-cell escape
        // here, because for Kitty proper that escape is a long string whose unicode
        // width pushes ratatui's `to_skip` counter past the trailing cells, causing
        // their updates to be dropped. Kitty graphics layers are removed via the
        // post-draw `delete_row` calls in `pending_preview_deletes`.
        let buf = f.buffer_mut();
        let right_end = right_area.x.saturating_add(right_area.width);
        let bottom = right_area.y.saturating_add(right_area.height);
        let bg_style = Style::default()
            .fg(self.ctx.color_theme.fg)
            .bg(self.ctx.color_theme.bg);
        for i in 0..count {
            let y = y_start + i;
            if y >= bottom {
                break;
            }
            for col in right_area.x..right_end {
                let buf_cell = &mut buf[(col, y)];
                buf_cell.set_symbol(" ");
                buf_cell.set_style(bg_style);
                buf_cell.set_skip(false);
            }
        }
    }

    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_preview_uploads)
    }

    pub fn drain_pending_avatar_deletes(&mut self) -> Vec<u16> {
        std::mem::take(&mut self.pending_preview_deletes)
    }
}

impl<'a> ConfigView<'a> {
    pub fn take_before_view(&mut self) -> View<'a> {
        std::mem::take(&mut self.before)
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        std::rc::Rc::make_mut(&mut self.ctx).color_theme = theme.clone();
        self.before.update_color_theme(theme);
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        let mut ids = self.before.graph_image_ids_sorted();
        ids.extend(self.cumulative_preview_image_ids.iter().copied());
        ids.sort();
        ids.dedup();
        ids
    }

    pub fn is_search_active(&self) -> bool {
        self.before.is_search_active()
    }

    pub fn is_search_querying(&self) -> bool {
        self.before.is_search_querying()
    }

    pub fn search_case_fuzzy(&self) -> Option<(bool, bool)> {
        self.before.search_case_fuzzy()
    }

    pub fn core_config(&self) -> &CoreConfig {
        &self.core_config
    }

    pub fn ui_config(&self) -> &UiConfig {
        &self.ui_config
    }

    pub fn github_auth_state(&self) -> &GithubAuthState {
        &self.github_auth_state
    }

    pub fn is_editing_text(&self) -> bool {
        self.editing_text
    }

    pub fn footer_hint(&self) -> String {
        config_footer_hint(
            self.selected,
            &self.github_auth_state,
            self.github_auth_pending,
        )
    }
}

fn graph_style_display(style: GraphStyle) -> String {
    match style {
        GraphStyle::Rounded => "Rounded".to_string(),
        GraphStyle::Angular => "Angular".to_string(),
        GraphStyle::Smooth => "Smooth".to_string(),
    }
}

fn diff_mode_display(mode: DiffMode) -> String {
    match mode {
        DiffMode::Enhanced => "Enhanced".to_string(),
        DiffMode::Raw => "Raw".to_string(),
    }
}

fn mouse_display(enabled: bool) -> String {
    if enabled {
        "On".to_string()
    } else {
        "Off".to_string()
    }
}

fn protocol_display(protocol: Option<ImageProtocolType>) -> String {
    match protocol {
        Some(ImageProtocolType::Auto) => "Auto".to_string(),
        Some(ImageProtocolType::Iterm) => "iTerm2".to_string(),
        Some(ImageProtocolType::Kitty) => "Kitty".to_string(),
        Some(ImageProtocolType::KittyUnicode) => "Kitty Unicode".to_string(),
        Some(ImageProtocolType::Sixel) => "Sixel".to_string(),
        None => "Auto".to_string(),
    }
}

fn github_auth_display(state: &GithubAuthState, pending: bool) -> String {
    if pending {
        return "- Authenticating... -".to_string();
    }
    if state.is_authenticated() {
        return format!(
            "- Authenticated as {} -",
            state.login.as_deref().unwrap_or("unknown")
        );
    }
    "- Press to auth -".to_string()
}

fn github_auth_description(state: &GithubAuthState, pending: bool) -> String {
    if pending {
        let message = state
            .message
            .clone()
            .unwrap_or_else(|| "Complete the GitHub device authentication in your browser.".into());
        return format!(
            "{message}\n\nPress to copy the code again and re-open https://github.com/login/device"
        );
    }
    if state.is_authenticated() {
        return "GitHub auth is active. Press to logout.".into();
    }
    state
        .message
        .clone()
        .unwrap_or_else(|| "GitHub auth is inactive.".into())
}

fn github_avatars_description(state: &GithubAuthState, _enabled: bool) -> String {
    if !state.is_authenticated() {
        return "Authenticate with GitHub\nto enable avatars.".into();
    }
    "Show author avatars in the commit list and detail views.\n\nMight cause some lags.".into()
}

fn config_footer_hint(selected: usize, state: &GithubAuthState, pending: bool) -> String {
    if selected == GITHUB_AUTH_INDEX {
        if pending {
            "Enter:re-copy Esc:close".into()
        } else if state.is_authenticated() {
            "Enter:logout Esc:close".into()
        } else {
            "Enter:auth Esc:close".into()
        }
    } else if selected == GITHUB_AVATARS_INDEX {
        if state.is_authenticated() {
            "Enter/⇆:toggle Esc:close".into()
        } else {
            "Esc:close".into()
        }
    } else if selected >= TEXT_EDIT_START_INDEX {
        "Enter:edit Esc:close".into()
    } else {
        "Enter/⇆:cycle Esc:close".into()
    }
}

fn config_section_line(title: &str, theme: &crate::color::ColorTheme) -> Line<'static> {
    let color = theme.list_ref_stash_fg;
    Line::from(vec![
        Span::styled("▍ ", Style::default().fg(color)),
        Span::styled(
            title.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn vertical_separator_height(content_height: u16) -> u16 {
    content_height.saturating_sub(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigValueKind {
    Cycle,
    Input,
    Button,
}

fn config_value_kind(index: usize) -> ConfigValueKind {
    if index == GITHUB_AUTH_INDEX {
        ConfigValueKind::Button
    } else if index == GITHUB_AVATARS_INDEX || index < TEXT_EDIT_START_INDEX {
        ConfigValueKind::Cycle
    } else {
        ConfigValueKind::Input
    }
}

fn config_value_fg(kind: ConfigValueKind, theme: &crate::color::ColorTheme) -> Color {
    match kind {
        ConfigValueKind::Cycle => theme.status_info_fg,
        ConfigValueKind::Input => theme.status_success_fg,
        ConfigValueKind::Button => theme.status_warn_fg,
    }
}

fn config_value_modifier(index: usize) -> Modifier {
    match config_value_kind(index) {
        ConfigValueKind::Button => Modifier::BOLD,
        ConfigValueKind::Cycle | ConfigValueKind::Input => Modifier::empty(),
    }
}

// Config value display conventions:
//   < value >  = cycle between several options (left/right to change)
//   [ value ]  = text input field (enter to edit)
//   [ (value) ]= info display (read-only, e.g. "from git")
//   - value -  = button (enter/press to activate)
fn config_value_display(index: usize, value: &str, editing: bool, editing_value: &str) -> String {
    if editing {
        editing_value.to_string()
    } else {
        match config_value_kind(index) {
            ConfigValueKind::Button => value.to_string(),
            ConfigValueKind::Input => value.to_string(),
            ConfigValueKind::Cycle => format!("< {value} >"),
        }
    }
}


#[cfg(test)]
mod tests {
    use crate::github_auth::GithubAuthState;

    #[test]
    fn github_auth_display_shows_authenticated_login() {
        let state = GithubAuthState {
            token: Some("token".into()),
            login: Some("octocat".into()),
            message: None,
        };

        assert_eq!(
            super::github_auth_display(&state, false),
            "- Authenticated as octocat -"
        );
    }

    #[test]
    fn github_auth_display_shows_pending_before_saved_token() {
        assert_eq!(
            super::github_auth_display(&GithubAuthState::default(), true),
            "- Authenticating... -"
        );
    }

    #[test]
    fn github_auth_display_prompts_enter_to_auth_when_logged_out() {
        assert_eq!(
            super::github_auth_display(&GithubAuthState::default(), false),
            "- Press to auth -"
        );
    }

    #[test]
    fn github_auth_description_does_not_include_enter_prompt_when_logged_out() {
        let description = super::github_auth_description(&GithubAuthState::default(), false);

        assert!(!description.contains("Press Enter"));
        assert!(!description.contains("Press to"));
        assert!(description.contains("inactive"));
    }

    #[test]
    fn github_auth_description_includes_logout_prompt_when_authenticated() {
        let state = GithubAuthState {
            token: Some("token".into()),
            login: Some("octocat".into()),
            message: None,
        };
        let description = super::github_auth_description(&state, false);

        assert!(description.contains("logout"));
    }

    #[test]
    fn github_auth_pending_description_includes_copy_reopen_hint() {
        let mut state = GithubAuthState::default();
        state.message = Some("Enter EB39-EEF2 at https://github.com/login/device".into());
        let description = super::github_auth_description(&state, true);

        assert!(description.contains("Enter EB39-EEF2 at https://github.com/login/device"));
        assert!(description
            .contains("Press to copy the code again and re-open https://github.com/login/device"));
    }

    #[test]
    fn config_footer_prompts_auth_for_github_auth_row_when_logged_out() {
        assert_eq!(
            super::config_footer_hint(super::GITHUB_AUTH_INDEX, &GithubAuthState::default(), false),
            "Enter:auth Esc:close"
        );
    }

    #[test]
    fn config_footer_prompts_logout_for_github_auth_row_when_logged_in() {
        let state = GithubAuthState {
            token: Some("token".into()),
            login: Some("octocat".into()),
            message: None,
        };

        assert_eq!(
            super::config_footer_hint(super::GITHUB_AUTH_INDEX, &state, false),
            "Enter:logout Esc:close"
        );
    }

    #[test]
    fn config_footer_prompts_recopy_during_pending_auth() {
        assert_eq!(
            super::config_footer_hint(super::GITHUB_AUTH_INDEX, &GithubAuthState::default(), true),
            "Enter:re-copy Esc:close"
        );
    }

    #[test]
    fn github_auth_row_value_is_not_wrapped_in_angle_brackets() {
        assert_eq!(
            super::config_value_display(super::GITHUB_AUTH_INDEX, "- Press to auth -", false, "",),
            "- Press to auth -"
        );
    }

    #[test]
    fn github_avatars_row_value_is_wrapped_in_angle_brackets() {
        assert_eq!(
            super::config_value_display(super::GITHUB_AVATARS_INDEX, "enabled", false, ""),
            "< enabled >"
        );
        assert_eq!(
            super::config_value_display(super::GITHUB_AVATARS_INDEX, "disabled", false, ""),
            "< disabled >"
        );
    }

    #[test]
    fn config_value_kind_matches_display_conventions() {
        assert_eq!(
            super::config_value_kind(super::GITHUB_AUTH_INDEX),
            super::ConfigValueKind::Button
        );
        assert_eq!(
            super::config_value_kind(super::GITHUB_AVATARS_INDEX),
            super::ConfigValueKind::Cycle
        );
        assert_eq!(super::config_value_kind(6), super::ConfigValueKind::Input);
    }

    #[test]
    fn config_item_indent_is_lightweight_section_indent() {
        assert_eq!(super::CONFIG_ITEM_INDENT, " ");
    }

    #[test]
    fn vertical_separator_height_keeps_one_line_before_footer() {
        assert_eq!(super::vertical_separator_height(10), 9);
        assert_eq!(super::vertical_separator_height(1), 0);
    }
}
