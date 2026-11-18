//! Fitting text into the room a screen has for it. Two behaviours, never a
//! crop:
//!
//! - content that matters is wrapped onto several rows (`wrap_plain`,
//!   `wrap_spans`);
//! - a fixed one-row layout shortens it with `…` (`ellipsize_end`,
//!   `ellipsize_middle`, `ellipsize_path`).
//!
//! Every width is a display width, so wide characters (Korean, emoji) count
//! for two columns.

use ratatui::{
    style::{Color, Stylize},
    text::{Line, Span},
};

/// Display width of `s`.
pub fn width(s: &str) -> usize {
    console::measure_text_width(s)
}

fn char_width(c: char) -> usize {
    let mut buf = [0u8; 4];
    console::measure_text_width(c.encode_utf8(&mut buf))
}

/// The longest prefix of `s` that fits in `max` columns.
fn take_width(s: &str, max: usize) -> &str {
    let mut used = 0;
    for (i, c) in s.char_indices() {
        let w = char_width(c);
        if used + w > max {
            return &s[..i];
        }
        used += w;
    }
    s
}

/// The longest suffix of `s` that fits in `max` columns.
fn take_width_end(s: &str, max: usize) -> &str {
    let mut used = 0;
    for (i, c) in s.char_indices().rev() {
        let w = char_width(c);
        if used + w > max {
            return &s[i + c.len_utf8()..];
        }
        used += w;
    }
    s
}

/// `s` as is when it fits in `max` columns, else cut with a `…` at the end.
pub fn ellipsize_end(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = take_width(s, max - 1).to_string();
    out.push('…');
    out
}

/// `s` as is when it fits, else its beginning and its end around a `…`:
/// for names where both ends tell something (a branch, a long identifier).
pub fn ellipsize_middle(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    if max <= 1 {
        return if max == 0 {
            String::new()
        } else {
            "…".to_string()
        };
    }
    let room = max - 1;
    let head = room.div_ceil(2);
    let tail = room - head;
    format!(
        "{}…{}",
        take_width(s, head),
        if tail == 0 {
            ""
        } else {
            take_width_end(s, tail)
        }
    )
}

/// A path shortened so the file name and the directories next to it stay:
/// `…/actions/render.rs`. The beginning is what goes, because two files with
/// the same name differ by their parent. When even the name is too long it is
/// shortened itself.
pub fn ellipsize_path(path: &str, max: usize) -> String {
    if width(path) <= max {
        return path.to_string();
    }
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    };
    // "…/" in front of the name.
    if dir.is_empty() || width(name) + 2 > max {
        return ellipsize_middle(name, max);
    }
    // As many of the last directories as fit.
    let mut kept: Vec<&str> = Vec::new();
    let mut used = width(name) + 2;
    for comp in dir.rsplit('/') {
        let w = width(comp) + 1;
        if used + w > max {
            break;
        }
        used += w;
        kept.push(comp);
    }
    kept.reverse();
    if kept.is_empty() {
        format!("…/{name}")
    } else {
        format!("…/{}/{name}", kept.join("/"))
    }
}

/// `rows` limited to `max_rows`: when some are dropped, the last one kept ends
/// with `…` so the reader knows there was more (`width` is the room of a row).
pub fn cap_rows(mut rows: Vec<String>, max_rows: usize, width: usize) -> Vec<String> {
    let max_rows = max_rows.max(1);
    if rows.len() > max_rows {
        rows.truncate(max_rows);
        if let Some(last) = rows.last_mut() {
            *last = ellipsize_end(&format!("{last} …"), width.max(1));
        }
    }
    rows
}

/// `s` padded with spaces on the right up to `w` columns (display width).
pub fn pad_to_width(s: &str, w: usize) -> String {
    let have = width(s);
    format!("{s}{}", " ".repeat(w.saturating_sub(have)))
}

/// Splits `text` into rows of at most `max` columns, breaking at spaces and
/// inside a word only when the word alone is wider than a row (a URL, a
/// path). Blank lines are kept. A line that starts with spaces keeps them on
/// the rows it continues onto, so an indented list or code stays aligned.
pub fn wrap_plain(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut rows = Vec::new();
    // The newline that ends a text (a commit body from git has one) is not
    // an empty row of its own.
    let text = text.trim_end_matches('\n');
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            rows.push(String::new());
            continue;
        }
        // A line that fits is kept exactly as written: aligned columns, code
        // and tables keep their spaces.
        if width(line) <= max {
            rows.push(line.to_string());
            continue;
        }
        let indent_len = line.len() - line.trim_start_matches(' ').len();
        let indent_w = indent_len.min(max / 2);
        let indent = " ".repeat(indent_w);
        let body = line.trim_start_matches(' ');
        let mut cur = indent.clone();
        let mut cur_w = indent_w;
        let mut first_word = true;
        for word in body.split(' ') {
            if word.is_empty() {
                continue;
            }
            let ww = width(word);
            let sep = if first_word { 0 } else { 1 };
            if cur_w + sep + ww <= max {
                if sep == 1 {
                    cur.push(' ');
                }
                cur.push_str(word);
                cur_w += sep + ww;
                first_word = false;
                continue;
            }
            // Does not fit on this row.
            if !first_word {
                rows.push(std::mem::take(&mut cur));
                cur = indent.clone();
                cur_w = indent_w;
            }
            if indent_w + ww <= max {
                cur.push_str(word);
                cur_w = indent_w + ww;
                first_word = false;
                continue;
            }
            // A word wider than a whole row: cut it.
            let mut rest = word;
            while !rest.is_empty() {
                let room = max - cur_w;
                let piece = take_width(rest, room.max(1));
                let piece = if piece.is_empty() {
                    // A character wider than the room: take it anyway.
                    &rest[..rest.chars().next().map_or(0, |c| c.len_utf8())]
                } else {
                    piece
                };
                cur.push_str(piece);
                rest = &rest[piece.len()..];
                if !rest.is_empty() {
                    rows.push(std::mem::take(&mut cur));
                    cur = indent.clone();
                    cur_w = indent_w;
                } else {
                    cur_w += width(piece);
                }
            }
            first_word = false;
        }
        rows.push(cur);
    }
    rows
}

/// Same for a styled line: every piece keeps its style across the wrap.
pub fn wrap_spans(spans: Vec<Span<'static>>, max: usize) -> Vec<Vec<Span<'static>>> {
    crate::view::pr::wrap_styled_spans(spans, max.max(1))
}

/// A styled line on one row: kept as is when it fits `max` columns, else cut
/// with a `…` after the last piece that fits. Each piece keeps its style.
pub fn ellipsize_spans(spans: Vec<Span<'_>>, max: usize) -> Vec<Span<'static>> {
    let owned = |s: Span<'_>| Span::styled(s.content.into_owned(), s.style);
    let total: usize = spans.iter().map(|s| width(s.content.as_ref())).sum();
    if total <= max {
        return spans.into_iter().map(owned).collect();
    }
    if max == 0 {
        return Vec::new();
    }
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for span in spans {
        let w = width(span.content.as_ref());
        if used + w < max {
            used += w;
            out.push(owned(span));
            continue;
        }
        // This piece crosses the limit: keep what fits before the `…`.
        let room = max - 1 - used;
        let mut cut = take_width(span.content.as_ref(), room).to_string();
        cut.push('…');
        out.push(Span::styled(cut, span.style));
        return out;
    }
    out
}

/// A line being typed in, on one row of `max` columns with the cursor at
/// column `cursor`: when it is too long the window slides so the cursor stays
/// visible, and a `…` in front says the beginning is hidden. Returns the
/// spans and the new column of the cursor.
pub fn scroll_spans_to_cursor(
    spans: Vec<Span<'_>>,
    max: usize,
    cursor: usize,
) -> (Vec<Span<'static>>, usize) {
    let owned = |s: Span<'_>| Span::styled(s.content.into_owned(), s.style);
    let total: usize = spans.iter().map(|s| width(s.content.as_ref())).sum();
    if total <= max || max < 2 {
        return (spans.into_iter().map(owned).collect(), cursor);
    }
    // The cursor may sit just after the last character: one blank cell.
    let (from, lead, to) = if cursor < max {
        // It is in the first window: nothing to hide in front.
        (0, false, max)
    } else {
        let end = cursor + 1;
        (end - (max - 1), true, end)
    };
    let mut out: Vec<Span<'static>> = Vec::new();
    if lead {
        out.push(Span::raw("…"));
    }
    let mut col = 0;
    for span in spans {
        let mut piece = String::new();
        for c in span.content.chars() {
            let w = char_width(c);
            if col >= from && col + w <= to {
                piece.push(c);
            }
            col += w;
        }
        if !piece.is_empty() {
            out.push(Span::styled(piece, span.style));
        }
    }
    let new_cursor = if lead { cursor + 1 - from } else { cursor };
    (out, new_cursor)
}

/// The shortcut hints of a footer (`⌘ a:one▕▏b:two▕▏c:three`) in `max`
/// columns: the last hints are dropped until it fits, the first one is kept
/// and shortened with a `…` if even it is too wide.
pub fn fit_hints(hint: &str, max: usize) -> String {
    const SEP: &str = "▕▏";
    if width(hint) <= max {
        return hint.to_string();
    }
    let (prefix, body) = match hint.strip_prefix("⌘ ") {
        Some(rest) => ("⌘ ", rest),
        None => ("", hint),
    };
    let mut parts: Vec<&str> = body.split(SEP).collect();
    while parts.len() > 1 && width(&format!("{prefix}{}", parts.join(SEP))) > max {
        parts.pop();
    }
    ellipsize_end(&format!("{prefix}{}", parts.join(SEP)), max)
}

/// One styled line cut into rows that fit `max`, each keeping its styles.
pub fn wrap_line(line: Line<'_>, max: usize) -> Vec<Line<'static>> {
    let style = line.style;
    let spans: Vec<Span<'static>> = line
        .spans
        .into_iter()
        .map(|s| Span::styled(s.content.into_owned(), s.style))
        .collect();
    wrap_spans(spans, max)
        .into_iter()
        .map(|row| Line::from(row).style(style))
        .collect()
}

/// Room the value column of a detail panel keeps free: one column on the
/// left of the value, two on the right.
pub const VALUE_PADDING: usize = 3;

/// A panel of `Label:  value` rows (commit, tag, branch details). A value
/// that is wider than its column goes on the next rows, and the column of
/// labels gets blank rows under its label so both stay aligned.
pub struct LabelledRows {
    pub labels: Vec<Line<'static>>,
    pub values: Vec<Line<'static>>,
    value_width: usize,
    label_fg: Color,
}

impl LabelledRows {
    /// `value_area_width` is the width of the area the values are drawn in,
    /// padding included.
    pub fn new(value_area_width: u16, label_fg: Color) -> Self {
        Self {
            labels: Vec::new(),
            values: Vec::new(),
            value_width: (value_area_width as usize)
                .saturating_sub(VALUE_PADDING)
                .max(1),
            label_fg,
        }
    }

    /// A value, wrapped as needed, with its label on the first row.
    pub fn row(&mut self, label: &str, value: Line<'_>) {
        let rows = wrap_line(value, self.value_width);
        self.push(label, rows);
    }

    /// Several lines of text (a message): kept as written, blank lines and
    /// indentation included, long lines wrapped.
    pub fn text(&mut self, label: &str, text: &str) {
        let rows: Vec<Line<'static>> = wrap_plain(text, self.value_width)
            .into_iter()
            .map(Line::from)
            .collect();
        self.push(label, rows);
    }

    /// A row with no label (a divider).
    pub fn bare(&mut self, value: Line<'static>) {
        self.labels.push(Line::from(""));
        self.values.push(value);
    }

    fn push(&mut self, label: &str, rows: Vec<Line<'static>>) {
        self.labels
            .push(Line::from(label.to_string()).fg(self.label_fg));
        for _ in 1..rows.len() {
            self.labels.push(Line::from(""));
        }
        self.values.extend(rows);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_left_alone() {
        assert_eq!(ellipsize_end("abc", 3), "abc");
        assert_eq!(ellipsize_middle("abc", 3), "abc");
        assert_eq!(ellipsize_path("a/b.rs", 6), "a/b.rs");
        assert_eq!(wrap_plain("abc def", 7), vec!["abc def"]);
    }

    #[test]
    fn ends_are_shortened_with_an_ellipsis_within_the_width() {
        assert_eq!(ellipsize_end("abcdefgh", 5), "abcd…");
        assert_eq!(ellipsize_end("abcdefgh", 1), "…");
        assert_eq!(ellipsize_end("abcdefgh", 0), "");
        let m = ellipsize_middle("feature/very-long-branch-name", 12);
        assert_eq!(width(&m), 12);
        assert!(
            m.starts_with("featu") && m.ends_with("name") && m.contains('…'),
            "{m}"
        );
    }

    #[test]
    fn wide_characters_count_for_two_columns() {
        // Four Korean characters are eight columns.
        assert_eq!(width("오병준이"), 8);
        let cut = ellipsize_end("오병준이", 5);
        assert!(width(&cut) <= 5, "{cut}");
        assert!(cut.ends_with('…'));
        for row in wrap_plain("오병준 오병준 오병준 오병준", 8) {
            assert!(width(&row) <= 8, "{row}");
        }
    }

    #[test]
    fn a_path_keeps_its_file_name() {
        let p = ellipsize_path("src/view/actions/render.rs", 20);
        assert_eq!(p, "…/actions/render.rs");
        assert!(width(&p) <= 20, "{p}");
        // Two files of the same name stay different: the parent is kept.
        let a = ellipsize_path("src/view/actions/mod.rs", 16);
        let b = ellipsize_path("src/view/rebase/mod.rs", 16);
        assert_ne!(a, b, "{a} / {b}");
        // The name alone is too long: it is shortened itself.
        let q = ellipsize_path("src/a_really_long_file_name.rs", 10);
        assert!(width(&q) <= 10, "{q}");
    }

    #[test]
    fn wrapping_breaks_at_spaces_and_loses_nothing() {
        let rows = wrap_plain("the quick brown fox jumps over the lazy dog", 12);
        assert!(rows.iter().all(|r| width(r) <= 12), "{rows:?}");
        assert_eq!(
            rows.join(" "),
            "the quick brown fox jumps over the lazy dog"
        );
    }

    #[test]
    fn a_word_wider_than_a_row_is_cut() {
        let url = "https://example.com/a/very/long/path/that/never/ends";
        let rows = wrap_plain(url, 10);
        assert!(rows.iter().all(|r| width(r) <= 10), "{rows:?}");
        assert_eq!(rows.concat(), url);
    }

    #[test]
    fn spaces_inside_a_line_that_fits_are_kept_and_the_last_newline_is_not_a_row() {
        assert_eq!(wrap_plain("a    b   c", 20), vec!["a    b   c"]);
        assert_eq!(wrap_plain("x\n", 10), vec!["x"]);
        assert_eq!(wrap_plain("x\n\ny\n", 10), vec!["x", "", "y"]);
    }

    #[test]
    fn blank_lines_and_indentation_are_kept() {
        let rows = wrap_plain("first\n\n    indented words that wrap around", 14);
        assert_eq!(rows[0], "first");
        assert_eq!(rows[1], "");
        assert!(rows[2].starts_with("    indented"), "{rows:?}");
        assert!(rows[3].starts_with("    "), "{rows:?}");
        assert!(rows.iter().all(|r| width(r) <= 14), "{rows:?}");
    }

    #[test]
    fn labelled_rows_keep_both_columns_aligned() {
        let mut r = LabelledRows::new(23, Color::Gray);
        r.row("Author: ", Line::from("short"));
        r.row("Message: ", Line::from("one two three four five six seven"));
        r.text(
            "Body: ",
            "first line\n\nsecond line that is long enough to wrap",
        );
        r.bare(Line::from("---"));
        assert_eq!(r.labels.len(), r.values.len());
        // 23 columns of value area leave 20 for the text.
        assert!(r.values.iter().all(|l| width(&l.to_string()) <= 20));
        assert!(r.values.len() > 5);
    }

    #[test]
    fn hints_drop_from_the_end_and_spans_get_an_ellipsis() {
        let h = "⌘ f:search▕▏Tab:refs▕▏?:help▕▏q:quit";
        assert_eq!(fit_hints(h, 100), h);
        let short = fit_hints(h, 24);
        assert!(width(&short) <= 24, "{short}");
        assert!(short.starts_with("⌘ f:search"), "{short}");
        assert!(!short.contains("q:quit"));
        let spans = vec![Span::raw("HEAD → "), Span::raw("a-long-branch-name")];
        let cut = ellipsize_spans(spans, 12);
        let text: String = cut.iter().map(|s| s.content.as_ref()).collect();
        assert!(width(&text) <= 12 && text.ends_with('…'), "{text}");
    }

    #[test]
    fn a_line_being_typed_slides_to_keep_the_cursor_visible() {
        let spans = vec![Span::raw("Search: "), Span::raw("0123456789abcdefghij")];
        // Cursor at the end of 28 columns shown in 14.
        let (out, cur) = scroll_spans_to_cursor(spans.clone(), 14, 28);
        let text: String = out.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.starts_with('…') && text.ends_with("ghij"), "{text}");
        assert!(width(&text) <= 14 && cur < 14, "{text} {cur}");
        // Cursor back at the start: the beginning is shown, no leading `…`.
        let (out, cur) = scroll_spans_to_cursor(spans, 14, 8);
        let text: String = out.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.starts_with("Search: "), "{text}");
        assert_eq!(cur, 8);
    }

    #[test]
    fn degenerate_widths_do_not_panic() {
        assert_eq!(ellipsize_end("abc", 0), "");
        assert_eq!(ellipsize_middle("abcdef", 1), "…");
        assert_eq!(ellipsize_path("a/b/c", 1), "c");
        assert!(!wrap_plain("abc", 0).is_empty());
        assert!(!wrap_plain("", 5).is_empty());
    }
}
