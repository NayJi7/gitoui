//! The terminal emulator screen (vt100) and its rendering into a ratatui buffer.

use std::num::NonZeroU16;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

pub(crate) const SCROLLBACK_LINES: usize = 10_000;

/// Collects what the emulator must answer.
#[derive(Default)]
struct Hooks {
    replies: Vec<u8>,
}

impl vt100::Callbacks for Hooks {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        let (row, col) = screen.cursor_position();
        match (i1, c, first) {
            (None, 'c', 0) => self.replies.extend_from_slice(b"\x1b[?62;22c"),
            (Some(b'>'), 'c', 0) => self.replies.extend_from_slice(b"\x1b[>0;0;0c"),
            (None, 'n', 5) => self.replies.extend_from_slice(b"\x1b[0n"),
            (None, 'n', 6) => self
                .replies
                .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes()),
            (Some(b'?'), 'n', 6) => self
                .replies
                .extend_from_slice(format!("\x1b[?{};{}R", row + 1, col + 1).as_bytes()),
            _ => {}
        }
    }
}

pub(crate) struct TermScreen {
    parser: vt100::Parser<Hooks>,
}

fn dim(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n).unwrap_or(NonZeroU16::MIN)
}

impl TermScreen {
    pub(crate) fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: vt100::Parser::new_with_callbacks(
                dim(rows),
                dim(cols),
                SCROLLBACK_LINES,
                Hooks::default(),
            ),
        }
    }

    pub(crate) fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Bytes the terminal must send back to the program (answers to queries).
    pub(crate) fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.parser.callbacks_mut().replies)
    }

    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.set_size(dim(rows), dim(cols));
    }

    pub(crate) fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub(crate) fn screen_mut(&mut self) -> &mut vt100::Screen {
        self.parser.screen_mut()
    }
}

fn convert_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub(crate) fn render_screen(
    screen: &vt100::Screen,
    area: Rect,
    buf: &mut Buffer,
    show_cursor: bool,
) {
    let (rows, cols) = screen.size();
    let height = area.height.min(rows.get());
    let width = area.width.min(cols.get());
    for row in 0..height {
        for col in 0..width {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            let Some(target) = buf.cell_mut((area.x + col, area.y + row)) else {
                continue;
            };
            if cell.is_wide_continuation() {
                target.reset();
                continue;
            }
            let symbol = if cell.has_contents() {
                cell.contents()
            } else {
                " "
            };
            target.set_symbol(symbol);
            target.fg = convert_color(cell.fgcolor());
            target.bg = convert_color(cell.bgcolor());
            let mut modifier = Modifier::empty();
            if cell.bold() {
                modifier |= Modifier::BOLD;
            }
            if cell.dim() {
                modifier |= Modifier::DIM;
            }
            if cell.italic() {
                modifier |= Modifier::ITALIC;
            }
            if cell.underline() {
                modifier |= Modifier::UNDERLINED;
            }
            if cell.inverse() {
                modifier |= Modifier::REVERSED;
            }
            target.modifier = modifier;
        }
    }
    if show_cursor && !screen.hide_cursor() && screen.scrollback() == 0 {
        let (row, col) = screen.cursor_position();
        if row < height && col < width {
            if let Some(target) = buf.cell_mut((area.x + col, area.y + row)) {
                target.modifier |= Modifier::REVERSED;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier};

    #[test]
    fn answers_cursor_position() {
        let mut s = TermScreen::new(5, 20);
        s.process(b"ab\x1b[6n");
        assert_eq!(s.take_replies(), b"\x1b[1;3R");
        assert!(s.take_replies().is_empty());
    }

    #[test]
    fn answers_device_attributes_and_status() {
        let mut s = TermScreen::new(5, 20);
        s.process(b"\x1b[c");
        s.process(b"\x1b[0c");
        assert_eq!(s.take_replies(), b"\x1b[?62;22c\x1b[?62;22c");
        s.process(b"\x1b[>c");
        assert_eq!(s.take_replies(), b"\x1b[>0;0;0c");
        s.process(b"\x1b[5n");
        assert_eq!(s.take_replies(), b"\x1b[0n");
        s.process(b"ab\x1b[?6n");
        assert_eq!(s.take_replies(), b"\x1b[?1;3R");
    }

    #[test]
    fn scrolled_view_stays_put_when_output_arrives() {
        let mut s = TermScreen::new(3, 10);
        let lines: String = (0..10).map(|i| format!("{i}\r\n")).collect();
        s.process(lines.as_bytes());
        s.screen_mut().set_scrollback(2);
        assert_eq!(s.screen().scrollback(), 2);
        s.process(b"a\r\nb\r\nc\r\n");
        assert_eq!(s.screen().scrollback(), 5);
    }

    fn cell_at(buf: &Buffer, x: u16, y: u16) -> &ratatui::buffer::Cell {
        buf.cell((x, y)).unwrap()
    }

    #[test]
    fn renders_colors_and_attributes() {
        let mut s = TermScreen::new(2, 10);
        s.process(b"\x1b[1;31mR\x1b[0m\x1b[38;2;1;2;3mG");
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        render_screen(s.screen(), area, &mut buf, false);
        let r = cell_at(&buf, 0, 0);
        assert_eq!(r.symbol(), "R");
        assert_eq!(r.fg, Color::Indexed(1));
        assert!(r.modifier.contains(Modifier::BOLD));
        let g = cell_at(&buf, 1, 0);
        assert_eq!(g.symbol(), "G");
        assert_eq!(g.fg, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn renders_wide_characters_once() {
        let mut s = TermScreen::new(2, 10);
        s.process("日x".as_bytes());
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        render_screen(s.screen(), area, &mut buf, false);
        assert_eq!(cell_at(&buf, 0, 0).symbol(), "日");
        assert_eq!(cell_at(&buf, 2, 0).symbol(), "x");
    }

    #[test]
    fn cursor_is_reversed_only_when_asked() {
        let mut s = TermScreen::new(2, 10);
        s.process(b"ab");
        let area = Rect::new(0, 0, 10, 2);
        let mut buf = Buffer::empty(area);
        render_screen(s.screen(), area, &mut buf, true);
        assert!(cell_at(&buf, 2, 0).modifier.contains(Modifier::REVERSED));
        let mut buf = Buffer::empty(area);
        render_screen(s.screen(), area, &mut buf, false);
        assert!(!cell_at(&buf, 2, 0).modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn scrollback_is_capped() {
        let mut s = TermScreen::new(2, 10);
        let lines: String = (0..12_000).map(|i| format!("{i}\r\n")).collect();
        s.process(lines.as_bytes());
        s.screen_mut().set_scrollback(usize::MAX);
        assert_eq!(s.screen().scrollback(), SCROLLBACK_LINES);
    }
}
