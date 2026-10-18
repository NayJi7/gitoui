//! Soft-wrap layout shared by the multi-line text editors (the comment
//! editor and the compose body, in both the PR and Issues views).
//!
//! The editors store a plain `String` and a byte cursor. Rendering used to
//! cut the text on `\n` only, so a long line ran past the frame. Everything
//! that depends on where text lands on screen (drawing, the cursor, Up/Down,
//! scrolling, clicks) now goes through this module so they all agree on the
//! same visual rows.
//!
//! Rows wrap at spaces when they can and hard-break a word longer than the
//! width. One column is kept free at the end of every row so the cursor can
//! always sit after the last character without leaving the frame.

use console::measure_text_width;

/// One visual row: the byte range `start..end` of its text, without the
/// `\n`. `last` is true when the row ends its logical line (a `\n` or the
/// end of the buffer follows) rather than being cut by the wrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    pub start: usize,
    pub end: usize,
    pub last: bool,
}

fn char_width(ch: char) -> usize {
    let mut tmp = [0u8; 4];
    measure_text_width(ch.encode_utf8(&mut tmp))
}

/// Columns usable for text. `0` means "width unknown" (nothing rendered yet)
/// and disables wrapping, which matches the old line-per-`\n` behaviour.
fn limit(width: u16) -> usize {
    if width == 0 {
        usize::MAX
    } else {
        usize::from(width).saturating_sub(1).max(1)
    }
}

/// Soft-wrap `buf` for a body `width` columns wide.
pub(crate) fn rows(buf: &str, width: u16) -> Vec<Row> {
    let limit = limit(width);
    let mut out = Vec::new();
    let mut base = 0usize;
    for line in buf.split('\n') {
        let mut start = base;
        let mut col = 0usize;
        // Byte offset just after the last space seen on the current row,
        // and the row width at that point.
        let mut last_break: Option<(usize, usize)> = None;
        for (i, ch) in line.char_indices() {
            let abs = base + i;
            let w = char_width(ch);
            if col > 0 && col.saturating_add(w) > limit {
                let (split, carried) = match last_break {
                    Some((b, c)) if b > start => (b, col - c),
                    _ => (abs, 0),
                };
                out.push(Row {
                    start,
                    end: split,
                    last: false,
                });
                start = split;
                col = carried;
                last_break = None;
            }
            col += w;
            if ch == ' ' {
                last_break = Some((abs + 1, col));
            }
        }
        out.push(Row {
            start,
            end: base + line.len(),
            last: true,
        });
        base += line.len() + 1;
    }
    out
}

/// Number of visual rows (at least 1, even for an empty buffer).
pub(crate) fn row_count(buf: &str, width: u16) -> u16 {
    u16::try_from(rows(buf, width).len()).unwrap_or(u16::MAX)
}

/// `(column, row)` on screen of the byte cursor, relative to the body.
pub(crate) fn cursor_pos(buf: &str, cursor: usize, width: u16) -> (u16, u16) {
    let cursor = cursor.min(buf.len());
    let rows = rows(buf, width);
    for (idx, row) in rows.iter().enumerate() {
        if cursor >= row.start && (cursor < row.end || (cursor == row.end && row.last)) {
            let col = measure_text_width(&buf[row.start..cursor]);
            return (
                u16::try_from(col).unwrap_or(u16::MAX),
                u16::try_from(idx).unwrap_or(u16::MAX),
            );
        }
    }
    let last = rows.len().saturating_sub(1);
    let col = rows
        .last()
        .map(|r| measure_text_width(&buf[r.start..r.end]))
        .unwrap_or(0);
    (
        u16::try_from(col).unwrap_or(u16::MAX),
        u16::try_from(last).unwrap_or(u16::MAX),
    )
}

/// Byte offset for a cursor placed at visual `(col, row)`. A row past the
/// end gives the end of the buffer; a column past the end of a row gives
/// the end of that row.
pub(crate) fn cursor_at(buf: &str, width: u16, col: usize, row: usize) -> usize {
    let rows = rows(buf, width);
    let Some(r) = rows.get(row) else {
        return buf.len();
    };
    let mut pos = r.start;
    let mut acc = 0usize;
    for (i, ch) in buf[r.start..r.end].char_indices() {
        if acc >= col {
            pos = r.start + i;
            return clamp_in_row(buf, r, pos);
        }
        acc += char_width(ch);
        pos = r.start + i + ch.len_utf8();
    }
    clamp_in_row(buf, r, pos)
}

/// A cursor sitting at the end of a wrapped (non-last) row would be drawn at
/// the start of the next row, so keep it on the row's last character.
fn clamp_in_row(buf: &str, row: &Row, pos: usize) -> usize {
    if !row.last && pos >= row.end && row.end > row.start {
        buf[row.start..row.end]
            .char_indices()
            .next_back()
            .map(|(i, _)| row.start + i)
            .unwrap_or(row.start)
    } else {
        pos
    }
}

/// `true` when Up (`delta < 0`) or Down (`delta > 0`) would move the cursor
/// to another visual row instead of leaving the field.
pub(crate) fn can_move_vertically(buf: &str, cursor: usize, width: u16, delta: i32) -> bool {
    let (_, row) = cursor_pos(buf, cursor, width);
    let total = rows(buf, width).len();
    if delta < 0 {
        row > 0
    } else {
        usize::from(row) + 1 < total
    }
}

/// Cursor after moving `delta` visual rows (-1 = up, 1 = down), keeping its
/// column. Up on the first row goes to the start of the buffer and Down on
/// the last row to its end, like the previous line-based behaviour.
pub(crate) fn move_vertical(buf: &str, cursor: usize, width: u16, delta: i32) -> usize {
    let (col, row) = cursor_pos(buf, cursor, width);
    let total = rows(buf, width).len();
    if delta < 0 {
        if row == 0 {
            return 0;
        }
        cursor_at(buf, width, usize::from(col), usize::from(row) - 1)
    } else {
        if usize::from(row) + 1 >= total {
            return buf.len();
        }
        cursor_at(buf, width, usize::from(col), usize::from(row) + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text of each row, borrowed from `buf`.
    fn row_texts<'a>(buf: &'a str, rows: &[Row]) -> Vec<&'a str> {
        rows.iter().map(|r| &buf[r.start..r.end]).collect()
    }

    fn texts(buf: &str, width: u16) -> Vec<String> {
        let rs = rows(buf, width);
        row_texts(buf, &rs).into_iter().map(String::from).collect()
    }

    #[test]
    fn empty_buffer_is_one_empty_row() {
        assert_eq!(texts("", 10), vec![""]);
        assert_eq!(row_count("", 10), 1);
    }

    #[test]
    fn short_lines_are_untouched_and_split_on_newlines() {
        assert_eq!(texts("ab\ncd", 10), vec!["ab", "cd"]);
        assert_eq!(texts("ab\n", 10), vec!["ab", ""]);
    }

    #[test]
    fn long_line_wraps_at_spaces_and_keeps_one_column_free() {
        // width 10 => 9 usable columns
        assert_eq!(
            texts("hello brave new world", 10),
            vec!["hello ", "brave ", "new world"]
        );
    }

    #[test]
    fn every_row_fits_the_width() {
        let text = "the quick brown fox jumps over the lazy dog and keeps running far away";
        for width in [5u16, 8, 13, 20, 40] {
            for t in texts(text, width) {
                assert!(
                    measure_text_width(&t) < usize::from(width),
                    "row {t:?} too wide for {width}"
                );
            }
        }
    }

    #[test]
    fn wrapping_loses_no_text() {
        let text = "aaaa bbbbbbbbbbbbbbbbbbbb cc\n\ndddd";
        let rs = rows(text, 7);
        let rebuilt: String = row_texts(text, &rs).concat();
        assert_eq!(rebuilt, text.replace('\n', ""));
    }

    #[test]
    fn word_longer_than_the_width_is_hard_broken() {
        assert_eq!(texts("abcdefghij", 5), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn width_zero_disables_wrapping() {
        assert_eq!(
            texts("a very long line indeed", 0),
            vec!["a very long line indeed"]
        );
    }

    #[test]
    fn wide_characters_count_two_columns() {
        // width 6 => 5 usable columns => two double-width chars (4) per row
        let rows_t = texts("ああああ", 6);
        assert_eq!(rows_t, vec!["ああ", "ああ"]);
    }

    #[test]
    fn cursor_at_a_wrap_boundary_belongs_to_the_next_row() {
        let buf = "hello brave";
        // "hello " | "brave"; the byte after the space starts row 1
        assert_eq!(cursor_pos(buf, 6, 8), (0, 1));
        assert_eq!(cursor_pos(buf, 5, 8), (5, 0));
    }

    #[test]
    fn cursor_at_end_of_buffer_stays_in_the_frame() {
        let buf = "abcdefgh";
        let width = 5u16;
        let (col, _) = cursor_pos(buf, buf.len(), width);
        assert!(col < width, "col {col} outside width {width}");
    }

    #[test]
    fn cursor_after_a_trailing_newline_is_on_the_new_row() {
        assert_eq!(cursor_pos("ab\n", 3, 10), (0, 1));
    }

    #[test]
    fn cursor_pos_and_cursor_at_round_trip() {
        let buf = "alpha beta gamma\ndelta epsilon zeta";
        for width in [6u16, 9, 14, 40] {
            for (i, _) in buf.char_indices() {
                let (c, r) = cursor_pos(buf, i, width);
                let back = cursor_at(buf, width, usize::from(c), usize::from(r));
                assert_eq!(
                    cursor_pos(buf, back, width),
                    (c, r),
                    "width {width} cursor {i}"
                );
            }
        }
    }

    #[test]
    fn moving_down_then_up_stays_on_the_same_visual_column() {
        let buf = "alpha beta gamma delta";
        let width = 9u16;
        let start = 2; // inside "alpha "
        let down = move_vertical(buf, start, width, 1);
        let (c0, _) = cursor_pos(buf, start, width);
        let (c1, r1) = cursor_pos(buf, down, width);
        assert_eq!(r1, 1);
        assert_eq!(c1, c0);
        assert_eq!(move_vertical(buf, down, width, -1), start);
    }

    #[test]
    fn up_on_first_row_goes_to_start_and_down_on_last_row_to_end() {
        let buf = "alpha beta gamma";
        assert_eq!(move_vertical(buf, 3, 8, -1), 0);
        assert_eq!(move_vertical(buf, 3, 80, 1), buf.len());
    }

    #[test]
    fn vertical_movement_walks_logical_lines_too() {
        let buf = "one\ntwo\nthree";
        assert_eq!(move_vertical(buf, 1, 80, 1), 5);
        assert_eq!(move_vertical(buf, 5, 80, -1), 1);
    }

    #[test]
    fn can_move_vertically_follows_visual_rows() {
        let buf = "alpha beta gamma";
        assert!(!can_move_vertically(buf, 2, 80, -1));
        assert!(!can_move_vertically(buf, 2, 80, 1));
        assert!(can_move_vertically(buf, 2, 8, 1));
        assert!(can_move_vertically(buf, buf.len(), 8, -1));
    }

    #[test]
    fn click_below_the_text_lands_at_the_end() {
        let buf = "abc";
        assert_eq!(cursor_at(buf, 10, 0, 5), buf.len());
    }

    #[test]
    fn click_past_the_end_of_a_wrapped_row_stays_on_that_row() {
        let buf = "hello brave";
        let pos = cursor_at(buf, 8, 50, 0);
        assert_eq!(cursor_pos(buf, pos, 8).1, 0);
    }
}
