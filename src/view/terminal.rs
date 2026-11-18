//! The integrated terminal: the shell's screen, shown in place of the commit
//! graph.

use std::rc::Rc;

use ratatui::{buffer::Buffer, crossterm::event::KeyEvent, layout::Rect, style::Color, Frame};

use crate::{
    app::AppContext,
    event::{Sender, UserEventWithCount},
    shell::session::{Scroll, SessionView, Status},
    view::View,
};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum FooterTone {
    Info,
    Error,
}

/// Labels of the keys shown in the footer, resolved from the user's keybinds.
pub(crate) struct FooterKeys {
    pub back: String,
}

/// `Ctrl+q` is shown as `Ctrl+Q`, the way people write it.
fn display_key(key: &str) -> String {
    match key.strip_prefix("Ctrl+") {
        Some(rest) if rest.len() == 1 && rest.chars().all(|c| c.is_ascii_lowercase()) => {
            format!("Ctrl+{}", rest.to_ascii_uppercase())
        }
        _ => key.to_string(),
    }
}

fn hint(key: &str, label: &str) -> String {
    if key.is_empty() {
        label.to_string()
    } else {
        format!("{}:{label}", display_key(key))
    }
}

/// Left text, its tone, and the right-hand key hints of the footer.
pub(crate) fn terminal_footer(status: &Status, keys: &FooterKeys) -> (String, FooterTone, String) {
    let back = hint(&keys.back, "back to the graph");
    match status {
        Status::Running => (
            "$ Terminal".to_string(),
            FooterTone::Info,
            format!("⌘ keys go to the shell▕▏{back}"),
        ),
        Status::Exited(code) => (
            format!("✗ Shell exited (exit {code})"),
            FooterTone::Error,
            format!("⌘ {back}"),
        ),
    }
}

/// The emulator leaves the default colors as `Reset`, which would show the
/// terminal's own palette instead of the gitoui theme.
fn apply_theme_defaults(buf: &mut Buffer, area: Rect, fg: Color, bg: Color) {
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            let Some(cell) = buf.cell_mut((x, y)) else {
                continue;
            };
            if cell.fg == Color::Reset {
                cell.fg = fg;
            }
            if cell.bg == Color::Reset {
                cell.bg = bg;
            }
        }
    }
}

#[derive(Debug)]
pub struct TerminalView<'a> {
    before: View<'a>,
    session: SessionView,
    scroll: Scroll,
    ctx: Rc<AppContext>,
}

impl<'a> TerminalView<'a> {
    pub fn new(before: View<'a>, session: SessionView, ctx: Rc<AppContext>, _tx: Sender) -> Self {
        Self {
            before,
            session,
            scroll: Scroll::new(),
            ctx,
        }
    }

    pub fn session(&self) -> &SessionView {
        &self.session
    }

    /// Every key goes to the shell; the app handles them before the views.
    pub fn handle_event(&mut self, _event: UserEventWithCount, _key: KeyEvent) {}

    /// Mouse wheel: positive moves towards older lines.
    pub fn scroll_by(&mut self, delta: isize) {
        if delta >= 0 {
            self.scroll.up(delta as usize);
        } else {
            self.scroll.down(delta.unsigned_abs());
        }
    }

    /// Jump to the oldest line kept.
    pub fn scroll_to_top(&mut self) {
        self.scroll.top();
    }

    /// Typing brings the view back to the live screen.
    pub fn snap_to_bottom(&mut self) {
        self.scroll.bottom();
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let (fg, bg) = (theme.fg, theme.bg);
        let buf = f.buffer_mut();
        // The graph cells underneath must not show through.
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.reset();
                    cell.set_char(' ');
                }
            }
        }
        self.session.render(area, buf, &mut self.scroll);
        apply_theme_defaults(buf, area, fg, bg);
    }

    pub fn take_before_view(&mut self) -> View<'a> {
        std::mem::take(&mut self.before)
    }

    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        self.before.graph_image_ids_sorted()
    }

    pub fn clear_graph_images(&mut self) {
        self.before.clear_graph_images();
    }

    pub fn update_color_theme(&mut self, theme: crate::color::ColorTheme) {
        self.before.update_color_theme(theme);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> FooterKeys {
        FooterKeys {
            back: "Ctrl+q".into(),
        }
    }

    #[test]
    fn footer_while_the_shell_runs() {
        let (left, tone, right) = terminal_footer(&Status::Running, &keys());
        assert_eq!(left, "$ Terminal");
        assert_eq!(tone, FooterTone::Info);
        assert_eq!(right, "⌘ keys go to the shell▕▏Ctrl+Q:back to the graph");
    }

    #[test]
    fn footer_when_the_shell_ended() {
        let (left, tone, right) = terminal_footer(&Status::Exited(2), &keys());
        assert_eq!(left, "✗ Shell exited (exit 2)");
        assert_eq!(tone, FooterTone::Error);
        assert_eq!(right, "⌘ Ctrl+Q:back to the graph");
    }

    #[test]
    fn an_unbound_key_drops_its_prefix() {
        let none = FooterKeys {
            back: String::new(),
        };
        let (_, _, right) = terminal_footer(&Status::Running, &none);
        assert_eq!(right, "⌘ keys go to the shell▕▏back to the graph");
    }

    #[test]
    fn default_colors_follow_the_theme() {
        let area = Rect::new(0, 0, 3, 1);
        let mut buf = Buffer::empty(area);
        buf[(1, 0)].fg = Color::Red;
        apply_theme_defaults(&mut buf, area, Color::White, Color::Black);
        assert_eq!(
            (buf[(0, 0)].fg, buf[(0, 0)].bg),
            (Color::White, Color::Black)
        );
        assert_eq!((buf[(1, 0)].fg, buf[(1, 0)].bg), (Color::Red, Color::Black));
    }
}
