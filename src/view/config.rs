use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph},
    Frame,
};

use crate::{
    app::AppContext,
    config::{save, CoreConfig, DiffMode, UiConfig},
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
    highlight::SyntaxHighlighter,
    view::View,
    GraphStyle, ImageProtocolType,
};

#[derive(Debug)]
pub struct ConfigView<'a> {
    before: View<'a>,
    selected: usize,
    ctx: Rc<AppContext>,
    tx: Sender,
    core_config: CoreConfig,
    ui_config: UiConfig,
}

impl<'a> ConfigView<'a> {
    pub fn new(before: View<'a>, ctx: Rc<AppContext>, tx: Sender) -> Self {
        let core_config = ctx.core_config.clone();
        let ui_config = ctx.ui_config.clone();
        Self {
            before,
            selected: 0,
            ctx,
            tx,
            core_config,
            ui_config,
        }
    }

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, _: KeyEvent) {
        let event = event_with_count.event;
        let count = event_with_count.count;
        match event {
            UserEvent::NavigateUp | UserEvent::SelectUp => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
            }
            UserEvent::NavigateDown | UserEvent::SelectDown => {
                if self.selected < 5 {
                    self.selected += 1;
                }
            }
            UserEvent::Confirm => {
                self.cycle_option();
            }
            UserEvent::NavigateRight => {
                self.cycle_option();
            }
            UserEvent::NavigateLeft => {
                self.cycle_option_prev();
            }
            UserEvent::Cancel | UserEvent::Close | UserEvent::Config => {
                self.tx.send(AppEvent::CloseConfig);
            }
            UserEvent::PageDown => {
                for _ in 0..count {
                    if self.selected < 5 {
                        self.selected += 1;
                    }
                }
            }
            UserEvent::PageUp => {
                for _ in 0..count {
                    if self.selected > 0 {
                        self.selected -= 1;
                    }
                }
            }
            UserEvent::GoToTop => {
                self.selected = 0;
            }
            UserEvent::GoToBottom => {
                self.selected = 5;
            }
            _ => {}
        }
    }

    fn cycle_option_prev(&mut self) {
        match self.selected {
            0 => {
                let current = self.core_config.graph_style();
                let prev = match current {
                    GraphStyle::Rounded => GraphStyle::Smooth,
                    GraphStyle::Angular => GraphStyle::Rounded,
                    GraphStyle::Smooth => GraphStyle::Angular,
                };
                self.core_config.set_graph_style(prev);
            }
            1 => {
                let prev = match self.ui_config.common.diff_mode {
                    DiffMode::Enhanced => DiffMode::Raw,
                    DiffMode::Raw => DiffMode::Enhanced,
                };
                self.ui_config.common.set_diff_mode(prev);
            }
            2 => {
                self.ui_config.common.set_mouse_enabled(!self.ui_config.common.mouse_enabled);
            }
            3 => {
                let current = self.core_config.protocol().unwrap_or(ImageProtocolType::Auto);
                let prev = match current {
                    ImageProtocolType::Auto => ImageProtocolType::KittyUnicode,
                    ImageProtocolType::Kitty => ImageProtocolType::Auto,
                    ImageProtocolType::Iterm => ImageProtocolType::Kitty,
                    ImageProtocolType::Sixel => ImageProtocolType::Iterm,
                    ImageProtocolType::KittyUnicode => ImageProtocolType::Sixel,
                };
                self.core_config.set_protocol(prev);
            }
            4 => {
                let themes = ["base16-ocean.dark", "base16-ocean.light", "base16-mocha.dark", "base16-eighties.dark", "InspiredGitHub", "Solarized (dark)", "Solarized (light)", "Dracula", "Monokai", "3024 Day", "Agola Dark", "Blackboard", "Cobalt"];
                let current = self.core_config.option.syntax_theme.as_str();
                let idx = themes.iter().position(|&t| t == current).unwrap_or(0);
                let prev_idx = if idx == 0 { themes.len() - 1 } else { idx - 1 };
                self.core_config.option.syntax_theme = themes[prev_idx].to_string();
            }
            5 => {
                let prev = self.core_config.date_time_format().cycle_prev();
                self.core_config.set_date_time_format(prev);
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
                let current = self.core_config.graph_style();
                let next = match current {
                    GraphStyle::Rounded => GraphStyle::Angular,
                    GraphStyle::Angular => GraphStyle::Smooth,
                    GraphStyle::Smooth => GraphStyle::Rounded,
                };
                self.core_config.set_graph_style(next);
            }
            1 => {
                let next = match self.ui_config.common.diff_mode {
                    DiffMode::Enhanced => DiffMode::Raw,
                    DiffMode::Raw => DiffMode::Enhanced,
                };
                self.ui_config.common.set_diff_mode(next);
            }
            2 => {
                self.ui_config.common.set_mouse_enabled(!self.ui_config.common.mouse_enabled);
            }
            3 => {
                let current = self.core_config.protocol().unwrap_or(ImageProtocolType::Auto);
                let next = match current {
                    ImageProtocolType::Auto => ImageProtocolType::Kitty,
                    ImageProtocolType::Kitty => ImageProtocolType::Iterm,
                    ImageProtocolType::Iterm => ImageProtocolType::Sixel,
                    ImageProtocolType::Sixel => ImageProtocolType::KittyUnicode,
                    ImageProtocolType::KittyUnicode => ImageProtocolType::Auto,
                };
                self.core_config.set_protocol(next);
            }
            4 => {
                let themes = ["base16-ocean.dark", "base16-ocean.light", "base16-mocha.dark", "base16-eighties.dark", "InspiredGitHub", "Solarized (dark)", "Solarized (light)", "Dracula", "Monokai", "3024 Day", "Agola Dark", "Blackboard", "Cobalt"];
                let current = self.core_config.option.syntax_theme.as_str();
                let idx = themes.iter().position(|&t| t == current).unwrap_or(0);
                let next_idx = (idx + 1) % themes.len();
                self.core_config.option.syntax_theme = themes[next_idx].to_string();
            }
            5 => {
                let next = self.core_config.date_time_format().cycle_next();
                self.core_config.set_date_time_format(next);
            }
            _ => {}
        }

        if let Err(e) = save(&self.core_config, &self.ui_config) {
            self.tx.send(AppEvent::NotifyError(e.to_string()));
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .padding(Padding::new(2, 2, 1, 1));
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Title left-aligned
        let title = Line::from(vec![
            Span::styled("Configuration", Style::default().add_modifier(Modifier::BOLD)),
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
        let sep_line = Line::from("─".repeat(inner.width as usize)).style(sep_style);
        f.render_widget(Paragraph::new(sep_line), sep_area);

        // Split remaining area into two columns
        let content_area = Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height - 3,
        };
        let [left_area, right_area] = Layout::horizontal([
            Constraint::Percentage(45),
            Constraint::Percentage(55),
        ]).areas(content_area);

        // Items list (left column)
        let items = vec![
            ("Graph Style", graph_style_display(self.core_config.graph_style())),
            ("Diff Mode", diff_mode_display(self.ui_config.common.diff_mode)),
            ("Mouse", mouse_display(self.ui_config.common.mouse_enabled)),
            ("Image Protocol", protocol_display(self.core_config.protocol())),
            ("Syntax Theme", self.core_config.option.syntax_theme.clone()),
            ("Date Format", self.core_config.date_time_format().display_name().to_string()),
        ];

        let lines: Vec<Line> = items
            .iter()
            .enumerate()
            .map(|(i, (name, value))| {
                let display = format!("< {} >", value);
                let spans = vec![
                    Span::raw(format!("{:<18}", name)),
                    Span::styled(display, Style::default().fg(self.ctx.color_theme.fg)),
                ];
                let mut line = Line::from(spans);
                if i == self.selected {
                    line = line.style(Style::default().add_modifier(Modifier::REVERSED));
                }
                line
            })
            .collect();

        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, left_area);

        // Description / preview (right column)
        let descriptions = [
            "Controls how commit connection lines are rendered in the graph.",
            "Enhanced shows contextual line numbers; Raw shows plain git diff output.",
            "Enable mouse support for clicking and scrolling.",
            "Terminal image protocol used for rendering commit graph images.",
            "Color theme for syntax highlighting in code diffs.",
            "Date and time display format for commits in the list and detail views.",
        ];

        let mut right_lines: Vec<Line> = vec![
            Line::from(vec![
                Span::styled("Description", Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(""),
            Line::from(descriptions[self.selected]),
        ];

        match self.selected {
            4 => {
                // Syntax Theme preview
                right_lines.push(Line::from(""));
                right_lines.push(Line::from(vec![
                    Span::styled("Preview", Style::default().add_modifier(Modifier::BOLD)),
                ]));
                right_lines.push(Line::from(""));

                let preview_code = vec![
                    "// Example code",
                    "fn greet(name: &str) -> String {",
                    "    let count = 42;",
                    "    format!(\"Hello {}!\", name)",
                    "}",
                ];

                if let Some(mut highlighter) = SyntaxHighlighter::new_with_theme(
                    "test.rs",
                    &self.core_config.option.syntax_theme,
                ) {
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
            _ => {}
        }

        let right_paragraph = Paragraph::new(right_lines);
        f.render_widget(right_paragraph, right_area);
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        let inner_y = 3; // block padding top + title + sep
        let item_y_start = inner_y;
        let item_idx = (row as usize).saturating_sub(item_y_start);
        if item_idx < 5 {
            self.selected = item_idx;
            self.cycle_option();
        }
    }

    pub fn handle_mouse_move(&mut self, _col: u16, row: u16) {
        let inner_y = 3;
        let item_y_start = inner_y;
        let item_idx = (row as usize).saturating_sub(item_y_start);
        if item_idx < 5 {
            self.selected = item_idx;
        }
    }
}

impl<'a> ConfigView<'a> {
    pub fn take_before_view(&mut self) -> View<'a> {
        std::mem::take(&mut self.before)
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        self.before.graph_image_ids_sorted()
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
