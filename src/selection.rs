//! Selecting text with the mouse, in every view: drag over the screen, the
//! text under the drag is highlighted and copied when the button is released.
//!
//! The text is read from the last frame that was drawn, so it is exactly what
//! the user sees, whatever the view. A terminal's own selection (`Shift` +
//! drag) is a different thing: the terminal does it, gitoui never sees it.
//!
//! Two modes (`selection_mode` in the config). `classic` takes whatever is
//! under the drag, borders and neighbouring panels included, like a terminal
//! does. `smart` keeps the selection inside the panel the drag started in: it
//! never takes the frame around it or the panel next to it.

use ratatui::{buffer::Buffer, style::Modifier};

/// The inside of a panel, which a smart selection is kept to: the columns
/// `left..=right` and the rows `top..=bottom`, all included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    pub top: u16,
    pub bottom: u16,
    pub left: u16,
    pub right: u16,
}

impl Bounds {
    /// The nearest cell inside, as (row, column).
    fn clamp(&self, (row, col): (u16, u16)) -> (u16, u16) {
        (
            row.clamp(self.top, self.bottom),
            col.clamp(self.left, self.right),
        )
    }
}

/// What the mouse has selected, as (row, column) cells of the screen.
/// `anchor` is where the button went down, `cursor` where the pointer is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSelection {
    anchor: (u16, u16),
    cursor: (u16, u16),
    bounds: Option<Bounds>,
}

impl TextSelection {
    /// With `bounds`, both ends are brought inside them: a drag that goes out of
    /// the panel stops at its edge.
    pub fn new(anchor: (u16, u16), cursor: (u16, u16), bounds: Option<Bounds>) -> Self {
        match bounds {
            Some(b) => Self {
                anchor: b.clamp(anchor),
                cursor: b.clamp(cursor),
                bounds,
            },
            None => Self {
                anchor,
                cursor,
                bounds,
            },
        }
    }

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

    /// The columns a row of the selection spans: from `first` on the first
    /// row, up to `last` on the last, the whole width (or panel) in between.
    fn span_in_row(&self, row: u16, area: ratatui::layout::Rect) -> (u16, u16) {
        let (first, last) = self.ordered();
        let (left, right) = match self.bounds {
            Some(b) => (b.left, b.right),
            None => (area.x, area.right().saturating_sub(1)),
        };
        let from = if row == first.0 { first.1 } else { left };
        let to = if row == last.0 { last.1 } else { right };
        (from, to)
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
        let (from, to) = selection.span_in_row(row, area);
        for col in from..=to.min(area.right().saturating_sub(1)) {
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
        let (from, to) = selection.span_in_row(row, area);
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

/// Part of a vertical line: `│`, `┃`, `║`, the dashed variants, and the corners
/// and junctions a frame is made of (`╭ ╮ ╰ ╯ ├ ┤ ┬ ┴ ┼ ╔ ╗ ╚ ╝ ┏ ┓ ┗ ┛`…).
fn is_frame_vertical(symbol: &str) -> bool {
    const GLYPHS: &str = "│┃║┆┇┊┋╎╏▏▕╷╵╹╻╭╮╰╯┌┐└┘├┤┬┴┼╔╗╚╝╠╣╦╩╬┏┓┗┛┣┫┳┻╋";
    let mut chars = symbol.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if GLYPHS.contains(c))
}

/// A corner of a frame: the row of it is the top or the bottom edge, which
/// holds no text of the panel.
fn is_frame_corner(symbol: &str) -> bool {
    const CORNERS: &str = "╭╮╰╯┌┐└┘╔╗╚╝┏┓┗┛";
    let mut chars = symbol.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if CORNERS.contains(c))
}

/// The rows, as (top, bottom), of the vertical line that goes through `(row, col)`.
fn vertical_run(buf: &Buffer, col: u16, row: u16) -> (u16, u16) {
    let area = buf.area;
    let part = |r: u16| {
        buf.cell((col, r))
            .is_some_and(|c| is_frame_vertical(c.symbol()))
    };
    let mut top = row;
    while top > area.top() && part(top - 1) {
        top -= 1;
    }
    let mut bottom = row;
    while bottom + 1 < area.bottom() && part(bottom + 1) {
        bottom += 1;
    }
    (top, bottom)
}

/// A vertical line this long is a frame, not a `│` typed in some text.
const MIN_FRAME_HEIGHT: u16 = 3;

/// The nearest frame line on `row`, going from `col` in `step` direction
/// (`-1` left, `1` right): its column and the rows it covers.
fn frame_side(buf: &Buffer, row: u16, col: u16, step: i32) -> Option<(u16, (u16, u16))> {
    let area = buf.area;
    let mut c = col as i32 + step;
    while c >= area.left() as i32 && c < area.right() as i32 {
        let x = c as u16;
        if buf
            .cell((x, row))
            .is_some_and(|cell| is_frame_vertical(cell.symbol()))
        {
            let run = vertical_run(buf, x, row);
            if run.1 - run.0 + 1 >= MIN_FRAME_HEIGHT {
                return Some((x, run));
            }
        }
        c += step;
    }
    None
}

/// The inside of the panel that holds `(row, col)`, found from the frame
/// lines around it: the nearest vertical line on each side, and the rows they
/// have in common. A panel with a frame on one side only (the commit list
/// has a line on its left and the screen's edge on its right) is bounded on
/// that side and by the screen on the other. `None` when there is no frame
/// at all, or when the point is on one.
pub fn smart_bounds(buf: &Buffer, (row, col): (u16, u16)) -> Option<Bounds> {
    let area = buf.area;
    if buf
        .cell((col, row))
        .is_some_and(|c| is_frame_vertical(c.symbol()))
    {
        return None;
    }
    let left = frame_side(buf, row, col, -1);
    let right = frame_side(buf, row, col, 1);
    if left.is_none() && right.is_none() {
        return None;
    }
    let (mut top, mut bottom) = (area.top(), area.bottom().saturating_sub(1));
    for (_, run) in left.iter().chain(right.iter()) {
        top = top.max(run.0);
        bottom = bottom.min(run.1);
    }
    // The top and bottom edges of a frame hold its corners, not text.
    let is_corner_row = |r: u16| {
        [left, right].iter().flatten().any(|(x, _)| {
            buf.cell((*x, r))
                .is_some_and(|c| is_frame_corner(c.symbol()))
        })
    };
    if is_corner_row(top) && top < row {
        top += 1;
    }
    if is_corner_row(bottom) && bottom > row {
        bottom -= 1;
    }
    // A frame usually keeps one blank column between itself and its text:
    // it is not part of what is copied.
    let blank_column = |x: u16| {
        (top..=bottom).all(|r| {
            buf.cell((x, r))
                .is_none_or(|c| c.symbol().trim().is_empty())
        })
    };
    let mut inner_left = left.map_or(area.left(), |(x, _)| x + 1);
    if left.is_some() && inner_left < col && blank_column(inner_left) {
        inner_left += 1;
    }
    let mut inner_right =
        right.map_or(area.right().saturating_sub(1), |(x, _)| x.saturating_sub(1));
    if right.is_some() && inner_right > col && blank_column(inner_right) {
        inner_right -= 1;
    }
    let bounds = Bounds {
        top,
        bottom,
        left: inner_left,
        right: inner_right,
    };
    (bounds.left <= bounds.right && bounds.top <= bounds.bottom).then_some(bounds)
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
        TextSelection::new(anchor, cursor, None)
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

    /// Two panels side by side, each in its own frame, like the jobs list and
    /// the log of a CI run.
    fn two_panels() -> Buffer {
        screen(&[
            "╭────╮╭──────╮",
            "│ ab ││ log1 │",
            "│ cd ││ log2 │",
            "│ ef ││ log3 │",
            "╰────╯╰──────╯",
        ])
    }

    #[test]
    fn the_panel_around_the_press_is_found() {
        let buf = two_panels();
        // In the right panel: its inside, with the frame, the corners and the
        // blank margin left out.
        assert_eq!(
            smart_bounds(&buf, (2, 9)),
            Some(Bounds {
                top: 1,
                bottom: 3,
                left: 8,
                right: 11
            })
        );
        // In the left panel.
        assert_eq!(
            smart_bounds(&buf, (1, 2)),
            Some(Bounds {
                top: 1,
                bottom: 3,
                left: 2,
                right: 3
            })
        );
    }

    #[test]
    fn a_smart_selection_never_takes_the_frame_or_the_next_panel() {
        let buf = two_panels();
        let bounds = smart_bounds(&buf, (1, 8));
        // Dragged from the right panel, far to the left and down: into the
        // other panel, which is not taken.
        let selection = TextSelection::new((1, 8), (3, 0), bounds);
        assert_eq!(selected_text(&buf, &selection), "log1\nlog2\nl");
    }

    #[test]
    fn a_smart_selection_stops_at_the_edge_of_its_panel() {
        let buf = two_panels();
        let bounds = smart_bounds(&buf, (2, 9));
        // From the end of "log2", up and out of the panel to the top left.
        let selection = TextSelection::new((2, 11), (0, 0), bounds);
        assert_eq!(selected_text(&buf, &selection), "log1\nlog2");
        // The highlight stays in the panel too.
        let mut drawn = two_panels();
        highlight(&mut drawn, &selection);
        let reversed = |row: u16, col: u16| drawn[(col, row)].modifier.contains(Modifier::REVERSED);
        assert!(reversed(1, 8) && reversed(1, 11) && reversed(2, 8) && reversed(2, 11));
        assert!(!reversed(1, 6) && !reversed(1, 12) && !reversed(1, 13) && !reversed(0, 8));
        assert!(!reversed(3, 8) && !reversed(1, 2));
    }

    #[test]
    fn without_a_frame_there_is_no_bound() {
        let buf = screen(&["plain text", "more text"]);
        assert_eq!(smart_bounds(&buf, (0, 3)), None);
    }

    #[test]
    fn a_pipe_inside_some_text_is_not_a_frame() {
        // One `│` in a single row is text, not a frame line.
        let buf = screen(&["a │ b", "c d"]);
        assert_eq!(smart_bounds(&buf, (0, 4)), None);
    }

    #[test]
    fn a_panel_with_a_line_on_one_side_only_is_bounded_on_that_side() {
        // Like the commit list: a vertical line on its left, the screen's edge on its right.
        let buf = screen(&["gr│msg one", "gr│msg two", "gr│msg 3", "gr│msg 4"]);
        assert_eq!(
            smart_bounds(&buf, (1, 6)),
            Some(Bounds {
                top: 0,
                bottom: 3,
                left: 3,
                right: 13
            })
        );
    }

    #[test]
    fn the_margin_of_a_frame_is_kept_when_text_touches_it() {
        // No blank column between the frame and the text: nothing to skip.
        let buf = screen(&["╭────╮", "│abcd│", "│efgh│", "│ijkl│", "╰────╯"]);
        assert_eq!(
            smart_bounds(&buf, (2, 3)),
            Some(Bounds {
                top: 1,
                bottom: 3,
                left: 1,
                right: 4
            })
        );
    }

    #[test]
    fn a_press_on_the_frame_itself_is_not_smart() {
        let buf = two_panels();
        assert_eq!(smart_bounds(&buf, (2, 0)), None);
    }
}
