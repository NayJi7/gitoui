//! Selecting text with the mouse, in every view: drag over the screen, the
//! text under the drag is highlighted and copied when the button is released.
//!
//! The text is read from the last frame that was drawn, so it is exactly what
//! the user sees, whatever the view. A terminal's own selection (`Shift` +
//! drag) is a different thing: the terminal does it, gitoui never sees it.

use ratatui::{buffer::Buffer, style::Modifier};

/// What the mouse has selected, as (row, column) cells of the screen.
/// `anchor` is where the button went down, `cursor` where the pointer is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSelection {
    pub anchor: (u16, u16),
    pub cursor: (u16, u16),
}

impl TextSelection {
    /// The two ends, first one first.
    fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    /// A press and release on the same cell is a click, not a selection.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.cursor
    }

    fn contains(&self, row: u16, col: u16) -> bool {
        let (first, last) = self.ordered();
        (row, col) >= first && (row, col) <= last
    }
}

/// Selected cells are drawn reversed, like a terminal does.
pub fn highlight(buf: &mut Buffer, selection: &TextSelection) {
    if selection.is_empty() {
        return;
    }
    let (first, last) = selection.ordered();
    let area = buf.area;
    for row in first.0..=last.0.min(area.bottom().saturating_sub(1)) {
        let from = if row == first.0 { first.1 } else { area.x };
        let to = if row == last.0 {
            last.1
        } else {
            area.right().saturating_sub(1)
        };
        for col in from..=to.min(area.right().saturating_sub(1)) {
            if !selection.contains(row, col) {
                continue;
            }
            if let Some(cell) = buf.cell_mut((col, row)) {
                cell.modifier.toggle(Modifier::REVERSED);
            }
        }
    }
}

/// What a cell shows as text. Cells that carry an image (the escape sequence
/// of the picture, or Kitty's placeholder character) show as a blank.
fn cell_text(symbol: &str) -> &str {
    if symbol.is_empty() || symbol.chars().any(|c| c.is_control() || c == '\u{10EEEE}') {
        " "
    } else {
        symbol
    }
}

/// The text between the two ends of the selection, both cells included: from
/// the first cell to the end of its row, whole rows in between, then up to
/// the last cell. Trailing blanks of each row are dropped, so a copied line
/// does not carry the padding of the screen, and so are blank rows at the end.
pub fn selected_text(buf: &Buffer, selection: &TextSelection) -> String {
    let (first, last) = selection.ordered();
    let area = buf.area;
    if area.is_empty() {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::new();
    for row in first.0..=last.0.min(area.bottom() - 1) {
        let from = if row == first.0 { first.1 } else { area.x };
        let to = if row == last.0 {
            last.1
        } else {
            area.right() - 1
        };
        let mut line = String::new();
        let mut col = from.max(area.x);
        while col <= to.min(area.right() - 1) {
            let Some(cell) = buf.cell((col, row)) else {
                break;
            };
            let text = cell_text(cell.symbol());
            line.push_str(text);
            // A wide character covers the cell after it too.
            let width = crate::text_fit::width(text).max(1) as u16;
            col = col.saturating_add(width);
        }
        lines.push(line.trim_end().to_string());
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn screen(lines: &[&str]) -> Buffer {
        let area = Rect::new(0, 0, 14, lines.len() as u16);
        let mut buf = Buffer::empty(area);
        for (y, line) in lines.iter().enumerate() {
            buf.set_string(0, y as u16, line, ratatui::style::Style::default());
        }
        buf
    }

    fn sel(anchor: (u16, u16), cursor: (u16, u16)) -> TextSelection {
        TextSelection { anchor, cursor }
    }

    #[test]
    fn a_span_in_one_row_and_a_selection_over_several_rows() {
        let buf = screen(&["hello world", "second line", "third", ""]);
        assert_eq!(selected_text(&buf, &sel((0, 6), (0, 10))), "world");
        assert_eq!(
            selected_text(&buf, &sel((0, 6), (2, 2))),
            "world\nsecond line\nthi"
        );
        // Dragged backwards, same text.
        assert_eq!(
            selected_text(&buf, &sel((2, 2), (0, 6))),
            "world\nsecond line\nthi"
        );
    }

    #[test]
    fn padding_and_blank_rows_are_not_copied() {
        let buf = screen(&["hello world", "", "", ""]);
        assert_eq!(selected_text(&buf, &sel((0, 0), (3, 13))), "hello world");
    }

    #[test]
    fn a_wide_character_is_copied_once() {
        let buf = screen(&["日本x"]);
        assert_eq!(selected_text(&buf, &sel((0, 0), (0, 8))), "日本x");
    }

    #[test]
    fn image_cells_are_not_copied_as_escape_sequences() {
        let mut buf = screen(&["ab cd"]);
        buf[(2, 0)].set_symbol("\x1bPq#0;2;0;0;0#0!10~-\x1b\\");
        buf[(3, 0)].set_symbol("\u{10EEEE}");
        assert_eq!(selected_text(&buf, &sel((0, 0), (0, 4))), "ab  d");
    }

    #[test]
    fn a_click_is_not_a_selection_and_highlights_nothing() {
        let mut buf = screen(&["abc"]);
        let click = sel((0, 1), (0, 1));
        assert!(click.is_empty());
        highlight(&mut buf, &click);
        assert!(!buf[(1, 0)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn the_selected_cells_are_reversed_from_the_first_to_the_last() {
        let mut buf = screen(&["abcdef", "ghijkl", "mnopqr"]);
        highlight(&mut buf, &sel((1, 4), (0, 2)));
        let reversed = |row: u16, col: u16| buf[(col, row)].modifier.contains(Modifier::REVERSED);
        assert!(!reversed(0, 1));
        assert!(reversed(0, 2) && reversed(0, 13));
        assert!(reversed(1, 0) && reversed(1, 4));
        assert!(!reversed(1, 5));
        assert!(!reversed(2, 0));
    }
}
