use std::rc::Rc;

use ratatui::{
    crossterm::event::KeyEvent,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Padding, Paragraph},
    Frame,
};

use crate::{
    app::AppContext,
    config::{save, CoreConfig, DiffMode, UiConfig},
    event::{AppEvent, Sender, UserEvent, UserEventWithCount},
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
                if self.selected < 3 {
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
                    if self.selected < 3 {
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
                self.selected = 3;
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

        // Items list
        let items = vec![
            ("Graph Style", graph_style_display(self.core_config.graph_style())),
            ("Diff Mode", diff_mode_display(self.ui_config.common.diff_mode)),
            ("Mouse", mouse_display(self.ui_config.common.mouse_enabled)),
            ("Image Protocol", protocol_display(self.core_config.protocol())),
        ];

        let lines: Vec<Line> = items
            .iter()
            .enumerate()
            .map(|(i, (name, value))| {
                let display = format!("< {} >", value);
                let spans = vec![
                    Span::raw(format!("{:<20}", name)),
                    Span::styled(display, Style::default().fg(self.ctx.color_theme.fg)),
                ];
                let mut line = Line::from(spans);
                if i == self.selected {
                    line = line.style(Style::default().add_modifier(Modifier::REVERSED));
                }
                line
            })
            .collect();

        let list_area = Rect {
            x: inner.x,
            y: inner.y + 2,
            width: inner.width,
            height: inner.height - 3,
        };
        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, list_area);
    }

    pub fn handle_click(&mut self, col: u16, row: u16) {
        let inner_y = 3; // block padding top + title + sep
        let item_y_start = inner_y;
        let item_idx = (row as usize).saturating_sub(item_y_start);
        if item_idx < 4 {
            self.selected = item_idx;
            self.cycle_option();
        }
    }

    pub fn handle_mouse_move(&mut self, _col: u16, row: u16) {
        let inner_y = 3;
        let item_y_start = inner_y;
        let item_idx = (row as usize).saturating_sub(item_y_start);
        if item_idx < 4 {
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
