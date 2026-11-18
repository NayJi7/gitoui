//! Drawing of the Actions page: the list of runs, the page of a run and the
//! workflow picker that opens over them.

use chrono::{Local, Utc};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::{
    color::ColorTheme,
    github::actions::{Annotation, AnnotationLevel, Run},
};

use super::{
    logic::{
        duration_text, event_phrase, fit_text, format_when, job_phrase, plan_window, run_duration,
        status_glyph, status_label, wrap_text, LogKind, LogLine, RunFilter, Tone,
    },
    ActionsView, Focus, Load, Note, PRow, RunPage, SIDE_WIDTH, WIDE_FROM,
};

pub(super) fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

fn tone_color(theme: &ColorTheme, tone: Tone) -> ratatui::style::Color {
    match tone {
        Tone::Success => theme.status_success_fg,
        Tone::Failure => theme.status_error_fg,
        Tone::Active => theme.status_warn_fg,
        Tone::Queued => theme.detail_label_fg,
        Tone::Muted => theme.divider_fg,
    }
}

// ─── Log lines ───────────────────────────────────────────────────

fn ansi_spans(text: &str) -> Vec<Span<'static>> {
    // Progress bars rewrite the line with carriage returns: keep the last state.
    let text = text.rsplit('\r').next().unwrap_or(text);
    if !text.contains('\x1b') {
        return vec![Span::raw(text.replace('\t', "    "))];
    }
    use ansi_to_tui::IntoText as _;
    match text.replace('\t', "    ").into_text() {
        Ok(t) => t
            .lines
            .into_iter()
            .flat_map(|l| l.spans)
            .map(|s| Span::styled(s.content.to_string(), s.style))
            .collect(),
        Err(_) => vec![Span::raw(text.to_string())],
    }
}

/// One styled log line. Built at render time for the lines on screen only.
pub(super) fn render_log_line(
    l: &LogLine,
    show_timestamps: bool,
    group_open: bool,
    theme: &ColorTheme,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if show_timestamps && !l.timestamp.is_empty() {
        let ts = l.timestamp.get(11..19).unwrap_or(&l.timestamp);
        spans.push(Span::styled(
            format!("{ts} "),
            Style::default().fg(theme.divider_fg),
        ));
    }
    match l.kind {
        LogKind::Group => spans.push(Span::styled(
            format!("{} {}", chevron(group_open), l.text),
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD),
        )),
        LogKind::EndGroup => {}
        LogKind::Error => spans.push(Span::styled(
            format!("✗ {}", l.text),
            Style::default()
                .fg(theme.status_error_fg)
                .add_modifier(Modifier::BOLD),
        )),
        LogKind::Warning => spans.push(Span::styled(
            format!("! {}", l.text),
            Style::default().fg(theme.status_warn_fg),
        )),
        LogKind::Notice => spans.push(Span::styled(
            format!("ℹ {}", l.text),
            Style::default().fg(theme.status_info_fg),
        )),
        LogKind::Debug => spans.push(Span::styled(
            l.text.clone(),
            Style::default().fg(theme.divider_fg),
        )),
        LogKind::Command => spans.push(Span::styled(
            l.text.clone(),
            Style::default()
                .fg(theme.list_name_fg)
                .add_modifier(Modifier::BOLD),
        )),
        LogKind::Plain => spans.extend(ansi_spans(&l.text)),
    }
    spans
}

// ─── Rows ────────────────────────────────────────────────────────

fn span_width(spans: &[Span]) -> usize {
    spans
        .iter()
        .map(|s| console::measure_text_width(&s.content))
        .sum()
}

/// Paints the selection on a row and stretches it to the right edge.
fn select_row(spans: &mut Vec<Span<'static>>, theme: &ColorTheme, width: usize) {
    let bg = theme.list_selected_bg;
    for s in spans.iter_mut() {
        s.style = s.style.bg(bg);
    }
    let w = span_width(spans);
    if w < width {
        spans.push(Span::styled(" ".repeat(width - w), Style::default().bg(bg)));
    }
}

fn chevron(open: bool) -> &'static str {
    if open {
        "▾"
    } else {
        "▸"
    }
}

/// The frame counter of the running icons, set at the top of each render
/// (the icons are drawn by plain functions that do not see the view).
static ANIM_TICK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Whether the terminal font has the fancy glyphs (the `nerd-font` flag).
static ANIM_NERD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// The status icon. A running one turns like the dot in the header of the
/// commit list: same half circles, same color, same pace.
///
/// A run or a job that is queued turns too (it is about to run, and a re-run
/// starts queued for a few seconds): only one waiting for an approval stays a
/// plain circle.
fn status_mark(
    theme: &ColorTheme,
    status: crate::github::actions::RunStatus,
    conclusion: Option<crate::github::actions::RunConclusion>,
) -> Span<'static> {
    use crate::github::actions::RunStatus;
    if matches!(
        status,
        RunStatus::InProgress | RunStatus::Queued | RunStatus::Pending | RunStatus::Requested
    ) {
        return spinning(theme);
    }
    let (glyph, tone) = status_glyph(status, conclusion);
    glyph_span(theme, glyph, tone)
}

/// The icon of a step: only the one that runs turns, the ones still to come
/// stay plain circles.
fn step_mark(
    theme: &ColorTheme,
    status: crate::github::actions::RunStatus,
    conclusion: Option<crate::github::actions::RunConclusion>,
) -> Span<'static> {
    if status == crate::github::actions::RunStatus::InProgress {
        return spinning(theme);
    }
    let (glyph, tone) = status_glyph(status, conclusion);
    glyph_span(theme, glyph, tone)
}

fn spinning(theme: &ColorTheme) -> Span<'static> {
    let tick = ANIM_TICK.load(std::sync::atomic::Ordering::Relaxed);
    Span::styled(
        crate::ci_monitor::spinner_frame(
            tick,
            ANIM_NERD.load(std::sync::atomic::Ordering::Relaxed),
        ),
        Style::default()
            .fg(theme.list_hash_fg)
            .add_modifier(Modifier::BOLD),
    )
}

fn glyph_span(theme: &ColorTheme, glyph: &'static str, tone: Tone) -> Span<'static> {
    Span::styled(
        glyph,
        Style::default()
            .fg(tone_color(theme, tone))
            .add_modifier(Modifier::BOLD),
    )
}

const DATE_W: usize = 18;
const WORKFLOW_W: usize = 26;
const BRANCH_W: usize = 24;

/// A run on one line: status, title, then (as the width allows) the workflow
/// and its number, the branch and the date. The rest is on the run page.
pub(super) fn run_line(
    theme: &ColorTheme,
    run: &Run,
    selected: bool,
    width: usize,
    now: chrono::DateTime<Local>,
) -> Line<'static> {
    let (show_workflow, show_branch, show_date) = if width >= 110 {
        (true, true, true)
    } else if width >= 80 {
        (true, false, true)
    } else if width >= 44 {
        (false, false, true)
    } else {
        (false, false, false)
    };
    let mut right = 0;
    for (shown, w) in [
        (show_workflow, WORKFLOW_W),
        (show_branch, BRANCH_W),
        (show_date, DATE_W),
    ] {
        if shown {
            right += w + 2;
        }
    }
    let title_w = width.saturating_sub(right + 5 + 1);
    let dim = Style::default().fg(theme.detail_label_fg);
    let title = if run.title.is_empty() {
        &run.workflow
    } else {
        &run.title
    };
    // Like the lists of pull requests and issues: a small triangle flags the
    // row under the cursor.
    let marker = if selected {
        Span::styled(
            "▶ ",
            Style::default()
                .fg(theme.list_head_fg)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw("  ")
    };
    let mut spans: Vec<Span<'static>> = vec![
        marker,
        status_mark(theme, run.status, run.conclusion),
        Span::raw("  "),
        Span::styled(
            fit_text(title, title_w),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
    ];
    if show_workflow {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            fit_text(&format!("{} #{}", run.workflow, run.number), WORKFLOW_W),
            dim,
        ));
    }
    if show_branch {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            fit_text(&format!(" {} ", run.branch), BRANCH_W),
            Style::default()
                .fg(theme.list_ref_stash_fg)
                .bg(theme.list_selected_bg),
        ));
    }
    if show_date {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            fit_text(&format_when(&run.created_at, now), DATE_W),
            dim,
        ));
    }
    spans.push(Span::raw(" "));
    if selected {
        select_row(&mut spans, theme, width);
    }
    Line::from(spans)
}

const DURATION_W: usize = 8;

/// A step of a job: chevron, status, name, duration on the right.
#[allow(clippy::too_many_arguments)]
fn step_line(
    theme: &ColorTheme,
    open: bool,
    status: crate::github::actions::RunStatus,
    conclusion: Option<crate::github::actions::RunConclusion>,
    name: &str,
    duration: &str,
    selected: bool,
    width: usize,
) -> Line<'static> {
    let name_w = width.saturating_sub(1 + 2 + 1 + 2 + 2 + DURATION_W + 1);
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("{} ", chevron(open)),
            Style::default().fg(theme.detail_label_fg),
        ),
        step_mark(theme, status, conclusion),
        Span::raw("  "),
        Span::styled(fit_text(name, name_w), Style::default().fg(theme.fg)),
        Span::raw("  "),
        Span::styled(
            format!("{duration:>DURATION_W$} "),
            Style::default().fg(theme.detail_label_fg),
        ),
    ];
    if selected {
        select_row(&mut spans, theme, width);
    }
    Line::from(spans)
}

/// A line of a log, numbered like github.com (the number is its position in
/// the step, `##[endgroup]` markers included).
/// Cells before the text of a log line: the indent and the line number.
pub(super) const LOG_GUTTER: usize = 9;

/// Cuts a styled line into rows of at most `width` cells, keeping the styles.
/// A long log line wraps instead of running out of the panel.
pub(super) fn wrap_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let mut rows: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut used = 0usize;
    for span in spans {
        let style = span.style;
        let mut piece = String::new();
        for c in span.content.chars() {
            let cell = console::measure_text_width(c.encode_utf8(&mut [0; 4]));
            if used + cell > width && used > 0 {
                if !piece.is_empty() {
                    if let Some(row) = rows.last_mut() {
                        row.push(Span::styled(std::mem::take(&mut piece), style));
                    }
                }
                rows.push(Vec::new());
                used = 0;
            }
            piece.push(c);
            used += cell;
        }
        if !piece.is_empty() {
            if let Some(row) = rows.last_mut() {
                row.push(Span::styled(piece, style));
            }
        }
    }
    rows
}

/// How many rows a log line takes in a text area `area` cells wide.
pub(super) fn log_line_height(l: &LogLine, show_timestamps: bool, area: usize) -> usize {
    // What is drawn is never wider than the raw text plus the timestamp and
    // the marker (the colors of a line are not drawn), and a character takes
    // at most as many cells as it has bytes: most lines are short.
    if l.text.len() + 11 <= area.max(1) {
        return 1;
    }
    let spans = render_log_line(l, show_timestamps, false, &ColorTheme::default());
    wrap_spans(spans, area).len().max(1)
}

/// A log line, numbered like github.com (the number is its position in the
/// step, `##[endgroup]` markers included). A long line wraps: the number is
/// only on its first row and the whole line is highlighted, so it reads as
/// one line.
fn log_rows(
    theme: &ColorTheme,
    number: usize,
    line: &LogLine,
    show_timestamps: bool,
    group_open: bool,
    selected: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let text_width = width.saturating_sub(LOG_GUTTER);
    let pieces = wrap_spans(
        render_log_line(line, show_timestamps, group_open, theme),
        text_width,
    );
    pieces
        .into_iter()
        .enumerate()
        .map(|(i, piece)| {
            let gutter = if i == 0 {
                format!("   {number:>4}  ")
            } else {
                " ".repeat(LOG_GUTTER)
            };
            let mut spans = vec![Span::styled(gutter, Style::default().fg(theme.divider_fg))];
            spans.extend(piece);
            if selected {
                select_row(&mut spans, theme, width);
            }
            Line::from(spans)
        })
        .collect()
}

/// `1 error, 2 warnings, 1 notice`
pub(super) fn annotation_summary(annotations: &[&Annotation]) -> String {
    let count = |level| annotations.iter().filter(|a| a.level == level).count();
    let mut parts = Vec::new();
    for (level, one, many) in [
        (AnnotationLevel::Failure, "error", "errors"),
        (AnnotationLevel::Warning, "warning", "warnings"),
        (AnnotationLevel::Notice, "notice", "notices"),
    ] {
        match count(level) {
            0 => {}
            1 => parts.push(format!("1 {one}")),
            n => parts.push(format!("{n} {many}")),
        }
    }
    parts.join(", ")
}

fn annotation_lines(
    theme: &ColorTheme,
    a: &Annotation,
    job_name: &str,
    selected: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let (icon, color) = match a.level {
        AnnotationLevel::Failure => ("✗", theme.status_error_fg),
        AnnotationLevel::Warning => ("!", theme.status_warn_fg),
        AnnotationLevel::Notice => ("ℹ", theme.status_info_fg),
    };
    let head = if a.title.is_empty() {
        job_name.to_string()
    } else {
        format!("{job_name} · {}", a.title)
    };
    let mut first = vec![
        Span::raw("    "),
        Span::styled(
            icon,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            fit_text(&head, width.saturating_sub(7))
                .trim_end()
                .to_string(),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
    ];
    if selected {
        select_row(&mut first, theme, width);
    }
    let mut lines = vec![Line::from(first)];
    for part in wrap_text(&a.message, width.saturating_sub(7)) {
        let mut spans = vec![
            Span::raw("       "),
            Span::styled(part, Style::default().fg(theme.detail_label_fg)),
        ];
        if selected {
            select_row(&mut spans, theme, width);
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// A discreet "…" centered on the last line of `area`: more rows below.
fn more_below(f: &mut Frame, area: Rect, theme: &ColorTheme, text: &'static str) {
    if area.height == 0 {
        return;
    }
    let line = Rect::new(area.x, area.y + area.height - 1, area.width, 1);
    f.render_widget(
        Paragraph::new(Span::styled(
            text,
            Style::default().fg(theme.detail_label_fg),
        ))
        .alignment(Alignment::Center),
        line,
    );
}

fn hint_line(theme: &ColorTheme, text: String) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::default().fg(theme.detail_label_fg),
    ))
}

impl ActionsView<'_> {
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // One step every 50 ms, like the animation of the header.
        ANIM_NERD.store(
            self.ctx.ui_config.common.nerd_font,
            std::sync::atomic::Ordering::Relaxed,
        );
        ANIM_TICK.store(
            (self.started.elapsed().as_millis() / 50) as usize,
            std::sync::atomic::Ordering::Relaxed,
        );
        self.filter_tab_rects.clear();
        self.row_rects.clear();
        self.side_rects.clear();
        self.picker_rects.clear();
        self.panel_rects.clear();
        if self.page.is_some() {
            self.render_page(f, area);
        } else {
            self.render_list(f, area);
        }
        if self.picker.is_some() {
            self.render_picker(f, area);
        }
    }

    /// The ` GitHub` mark at the right of a title line, like the pages of pull
    /// requests and issues: only with Nerd Font glyphs (an explicit config
    /// flag, there is no reliable way to detect them), and only when it fits.
    fn push_github_mark(&self, spans: &mut Vec<Span<'static>>, width: usize) {
        // The glyph needs a Nerd Font; without it the word stands alone.
        let gh_tag: &str = if self.ctx.ui_config.common.nerd_font {
            "\u{f09b} GitHub"
        } else {
            "GitHub"
        };
        const RIGHT_PAD: usize = 2;
        let used: usize = spans
            .iter()
            .map(|s| span_width(std::slice::from_ref(s)))
            .sum();
        let tag_w = console::measure_text_width(gh_tag);
        if width > used + tag_w + RIGHT_PAD {
            spans.push(Span::raw(" ".repeat(width - used - tag_w - RIGHT_PAD)));
            spans.push(Span::styled(
                gh_tag.to_string(),
                Style::default()
                    .fg(self.ctx.color_theme.fg)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }

    /// Columns the mark takes (with its padding) when it is shown.
    fn github_mark_width(&self) -> usize {
        let tag = if self.ctx.ui_config.common.nerd_font {
            "\u{f09b} GitHub"
        } else {
            "GitHub"
        };
        console::measure_text_width(tag) + 2 + 2
    }

    fn divider(&self, f: &mut Frame, area: Rect) {
        f.render_widget(
            Paragraph::new(Span::styled(
                "─".repeat(area.width as usize),
                Style::default().fg(self.ctx.color_theme.divider_fg),
            )),
            area,
        );
    }

    // ─── The list of runs ────────────────────────────────────────

    fn render_list(&mut self, f: &mut Frame, area: Rect) {
        let theme = self.ctx.color_theme.clone();
        let banner = if self.last_error.is_some() && area.height > 8 {
            4
        } else {
            0
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(banner),
                Constraint::Min(0),
            ])
            .split(area);

        let mut head = vec![
            Span::raw("  "),
            Span::styled(
                if self.ctx.ui_config.common.nerd_font {
                    "⪢ "
                } else {
                    "» "
                },
                Style::default()
                    .fg(theme.status_success_fg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "Actions ",
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("{}/{}", self.coords.owner, self.coords.repo),
                Style::default()
                    .fg(crate::view::pr::REPO_TEAL)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                {
                    let scope = self.workflow.as_deref().unwrap_or("All workflows");
                    if self.first_load() {
                        format!("  ·  {scope}")
                    } else {
                        let more = if self.has_more { "+" } else { "" };
                        format!("  ·  {scope}  ·  {}{more} runs", self.visible.len())
                    }
                },
                Style::default().fg(theme.detail_label_fg),
            ),
        ];
        if self.only_branch {
            if let Some(b) = &self.branch {
                head.push(Span::styled(
                    format!("  ·  on {b}"),
                    Style::default().fg(theme.list_ref_stash_fg),
                ));
            }
        }
        if self.runs_loading {
            head.push(Span::styled(
                "  loading…",
                Style::default().fg(theme.detail_label_fg),
            ));
        }
        self.push_github_mark(&mut head, chunks[0].width as usize);
        f.render_widget(Paragraph::new(Line::from(head)), chunks[0]);
        self.divider(f, chunks[1]);
        if banner > 0 {
            self.render_error_banner(f, chunks[2]);
        }

        let (tabs, rects) = self.filter_title(chunks[3]);
        self.filter_tab_rects = rects;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.divider_fg))
            .title(Line::from(tabs));
        let inner = block.inner(chunks[3]);
        f.render_widget(block, chunks[3]);
        // One empty row under the status tabs, like the lists of pull
        // requests and issues.
        let rows = Rect::new(
            inner.x,
            inner.y.saturating_add(1),
            inner.width,
            inner.height.saturating_sub(1),
        );
        self.render_runs(f, rows);
    }

    fn render_error_banner(&self, f: &mut Frame, area: Rect) {
        let theme = &self.ctx.color_theme;
        let Some(msg) = self.last_error.as_ref() else {
            return;
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.status_error_fg))
            .title(Line::from(Span::styled(
                " ✗ GitHub fetch failed ",
                Style::default()
                    .fg(theme.status_error_fg)
                    .add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let retry = self.ctx.keybind.primary_scoped_key(&["actions"], "reload");
        let hint = if retry.is_empty() {
            String::new()
        } else {
            format!(" Press {retry} to retry.")
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("{msg}{hint}"),
                Style::default().fg(theme.status_error_fg),
            ))
            .wrap(Wrap { trim: true }),
            inner,
        );
    }

    /// The status tabs, on the top border of the list.
    fn filter_title(&self, area: Rect) -> (Vec<Span<'static>>, Vec<(RunFilter, Rect)>) {
        let theme = &self.ctx.color_theme;
        let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
        let mut rects = Vec::new();
        let mut x = area.x.saturating_add(2);
        for (i, filter) in RunFilter::ALL.iter().enumerate() {
            let active = *filter == self.filter;
            let hovered = self.hovered_filter == Some(*filter) && !active;
            // No count while the first answer is awaited: "0" would be a lie.
            let label = if self.first_load() {
                filter.label().to_string()
            } else {
                format!("{} {}", filter.label(), self.count_for(*filter))
            };
            let fg = if active {
                theme.list_head_fg
            } else if hovered {
                theme.fg
            } else {
                theme.detail_label_fg
            };
            let mut style = Style::default().fg(fg);
            if active {
                style = style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            } else if hovered {
                style = style.add_modifier(Modifier::BOLD);
            }
            let w = label.chars().count() as u16;
            rects.push((*filter, Rect::new(x, area.y, w, 1)));
            spans.push(Span::styled(label, style));
            x = x.saturating_add(w);
            if i + 1 < RunFilter::ALL.len() {
                spans.push(Span::raw("   "));
                x = x.saturating_add(3);
            }
        }
        spans.push(Span::raw(" "));
        (spans, rects)
    }

    fn render_runs(&mut self, f: &mut Frame, body: Rect) {
        let theme = self.ctx.color_theme.clone();
        self.body_height = body.height;
        if self.visible.is_empty() {
            let msg = if self.runs_loading && self.runs.is_empty() {
                "Loading workflow runs…"
            } else if self.runs.is_empty() {
                "No workflow runs."
            } else {
                "No run matches the filters."
            };
            f.render_widget(
                Paragraph::new(Span::styled(
                    msg,
                    Style::default().fg(theme.detail_label_fg),
                ))
                .alignment(Alignment::Center),
                body,
            );
            return;
        }
        let height = body.height as usize;
        let (offset, usable, dots) = plan_window(
            self.offset,
            self.selected,
            self.free_scroll,
            height,
            self.visible.len(),
            self.has_more || self.loading_more,
            |_| 1,
        );
        self.offset = offset;
        let width = body.width as usize;
        let now = Local::now();
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (y, pos) in (self.offset..self.visible.len()).take(usable).enumerate() {
            let run = &self.runs[self.visible[pos]];
            lines.push(run_line(&theme, run, pos == self.selected, width, now));
            self.row_rects
                .push((pos, Rect::new(body.x, body.y + y as u16, body.width, 1)));
        }
        f.render_widget(Paragraph::new(lines), body);
        if dots {
            let text = if self.loading_more {
                "Loading more runs…"
            } else {
                "…"
            };
            more_below(f, body, &theme, text);
        }
    }

    // ─── The page of a run ───────────────────────────────────────

    fn render_page(&mut self, f: &mut Frame, area: Rect) {
        let theme = self.ctx.color_theme.clone();
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(area);

        let run = &page.run;
        let title = if run.title.is_empty() {
            &run.workflow
        } else {
            &run.title
        };
        let number = format!("  #{}", run.number);
        let title_w = (chunks[0].width as usize)
            .saturating_sub(console::measure_text_width(&run.workflow) + number.len() + 8)
            .saturating_sub(self.github_mark_width());
        let head = vec![
            Span::raw("  "),
            Span::styled(
                run.workflow.clone(),
                Style::default().fg(theme.detail_label_fg),
            ),
            Span::raw("   "),
            status_mark(&theme, run.status, run.conclusion),
            Span::raw(" "),
            Span::styled(
                fit_text(title, title_w).trim_end().to_string(),
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(number, Style::default().fg(theme.detail_label_fg)),
        ];
        let mut head = head;
        if page.any_inflight() {
            // `r` (or the refresh of a running run) is being answered.
            head.push(Span::styled(
                "   loading…",
                Style::default().fg(theme.detail_label_fg),
            ));
        }
        self.push_github_mark(&mut head, chunks[0].width as usize);
        f.render_widget(Paragraph::new(Line::from(head)), chunks[0]);
        self.divider(f, chunks[1]);

        let body = chunks[2];
        self.layout_wide = body.width >= WIDE_FROM;
        if self.layout_wide {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(SIDE_WIDTH), Constraint::Min(0)])
                .split(body);
            self.render_side(f, cols[0]);
            self.render_panel(f, cols[1]);
        } else {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Min(0)])
                .split(body);
            self.render_selector(f, rows[0]);
            self.render_panel(f, rows[1]);
        }
    }

    /// `Summary` and the jobs, on the left.
    fn render_side(&mut self, f: &mut Frame, area: Rect) {
        let theme = self.ctx.color_theme.clone();
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let focused = page.focus == Focus::Side;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused {
                theme.list_head_fg
            } else {
                theme.divider_fg
            }))
            .title(Line::from(Span::styled(
                " Jobs ",
                Style::default().fg(theme.detail_label_fg),
            )));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let width = inner.width as usize;

        let hovered_side = self.hovered_side;
        let mut items: Vec<(Option<usize>, Line<'static>)> = Vec::new();
        let mut entry_line = |entry: usize, glyph: Option<Span<'static>>, label: &str| {
            let current = page.entry == entry;
            let mut spans = vec![Span::styled(
                if current { "▌ " } else { "  " },
                Style::default().fg(theme.list_head_fg),
            )];
            let used = if glyph.is_some() { 5 } else { 2 };
            if let Some(g) = glyph {
                spans.push(g);
                spans.push(Span::raw("  "));
            }
            let mut style = Style::default().fg(theme.fg);
            if current {
                style = style.add_modifier(Modifier::BOLD);
            } else if hovered_side == Some(entry) {
                style = Style::default()
                    .fg(theme.list_head_fg)
                    .add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled(
                fit_text(label, width.saturating_sub(used)),
                style,
            ));
            if current && focused {
                select_row(&mut spans, &theme, width);
            }
            items.push((Some(entry), Line::from(spans)));
        };
        entry_line(0, None, "Summary");
        for (i, jn) in page.job_nodes().iter().enumerate() {
            let mark = status_mark(&theme, jn.job.status, jn.job.conclusion);
            entry_line(i + 1, Some(mark), &jn.job.name);
        }
        if matches!(page.jobs, Load::Loading) {
            items.push((None, hint_line(&theme, "  Loading jobs…".into())));
        }

        // Scroll so the current entry stays on screen.
        let height = inner.height as usize;
        let current_at = items
            .iter()
            .position(|(e, _)| *e == Some(page.entry))
            .unwrap_or(0);
        let start = (current_at + 1).saturating_sub(height);
        let mut lines = Vec::new();
        for (y, (entry, line)) in items.into_iter().skip(start).take(height).enumerate() {
            if let Some(e) = entry {
                self.side_rects
                    .push((e, Rect::new(inner.x, inner.y + y as u16, inner.width, 1)));
            }
            lines.push(line);
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    /// The jobs when there is no room for a list: one line, `←` / `→`.
    fn render_selector(&mut self, f: &mut Frame, area: Rect) {
        let theme = self.ctx.color_theme.clone();
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let total = page.entry_count();
        let (glyph, name) = match page.entry_job() {
            Some(jn) => (
                Some(status_mark(&theme, jn.job.status, jn.job.conclusion)),
                jn.job.name.clone(),
            ),
            None => (None, "Summary".to_string()),
        };
        let dim = Style::default().fg(theme.detail_label_fg);
        let mut spans = vec![Span::styled(" ‹ ", dim)];
        if let Some(g) = glyph {
            spans.push(g);
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            name,
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!("  {}/{total} ›", page.entry + 1), dim));
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn summary_header(&self, page: &RunPage) -> Vec<Line<'static>> {
        let theme = &self.ctx.color_theme;
        let run = &page.run;
        let dim = Style::default().fg(theme.detail_label_fg);
        let bold = Style::default().fg(theme.fg).add_modifier(Modifier::BOLD);
        let file = run.path.rsplit('/').next().unwrap_or("").to_string();
        let sha: String = run.sha.chars().take(7).collect();
        let mut meta = vec![
            Span::styled("⎇ ", dim),
            Span::styled(
                run.branch.clone(),
                Style::default().fg(theme.list_ref_stash_fg),
            ),
            Span::styled(format!("   commit {sha}"), dim),
        ];
        if !file.is_empty() {
            meta.push(Span::styled(format!("   {file}"), dim));
        }
        let mut state = vec![
            Span::styled("Status ", dim),
            status_mark(theme, run.status, run.conclusion),
            Span::styled(
                format!(" {}", status_label(run.status, run.conclusion)),
                bold,
            ),
            Span::styled("     Duration ", dim),
            Span::styled(run_duration(run, Utc::now()), bold),
        ];
        if run.attempt > 1 {
            state.push(Span::styled(format!("     Attempt #{}", run.attempt), dim));
        }
        vec![
            Line::from(vec![Span::styled(
                format!(
                    "{}  ·  {}",
                    event_phrase(run),
                    crate::github::issue::short_relative(&run.created_at)
                ),
                dim,
            )]),
            Line::from(meta),
            Line::from(state),
            Line::raw(""),
        ]
    }

    fn job_header(&self, page: &RunPage) -> Vec<Line<'static>> {
        let theme = &self.ctx.color_theme;
        let Some(jn) = page.entry_job() else {
            return Vec::new();
        };
        let job = &jn.job;
        let when = job
            .completed_at
            .as_deref()
            .or(job.started_at.as_deref())
            .map(crate::github::issue::short_relative)
            .unwrap_or_default();
        let took = duration_text(
            job.started_at.as_deref(),
            job.completed_at.as_deref(),
            Utc::now(),
        );
        let mut text = job_phrase(job.status, job.conclusion).to_string();
        if !when.is_empty() {
            text = format!("{text} {when}");
        }
        // A job still going has no total yet: "started 16s ago" says it all.
        if !took.is_empty() && !job.status.is_active() {
            text = format!("{text} in {took}");
        }
        vec![hint_line(theme, text), Line::raw("")]
    }

    fn render_panel(&mut self, f: &mut Frame, area: Rect) {
        let theme = self.ctx.color_theme.clone();
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let focused = page.focus == Focus::Panel || !self.layout_wide;
        let title = match page.entry_job() {
            None => " Summary ".to_string(),
            Some(jn) => format!(" {} ", jn.job.name),
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused {
                theme.list_head_fg
            } else {
                theme.divider_fg
            }))
            .title(Line::from(Span::styled(
                title,
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            )));
        let inner = block.inner(area);
        f.render_widget(block, area);
        let width = inner.width as usize;

        let header = if page.entry == 0 {
            self.summary_header(page)
        } else {
            self.job_header(page)
        };
        let header_h = (header.len() as u16).min(inner.height);
        let header: Vec<Line<'static>> = header
            .into_iter()
            .map(|mut l| {
                l.spans.insert(0, Span::raw(" "));
                l
            })
            .collect();
        f.render_widget(
            Paragraph::new(header),
            Rect::new(inner.x, inner.y, inner.width, header_h),
        );
        let body = Rect::new(
            inner.x,
            inner.y + header_h,
            inner.width,
            inner.height - header_h,
        );
        let height = body.height as usize;

        let (offset, usable, dots) = plan_window(
            page.offset,
            page.selected,
            page.free_scroll,
            height,
            page.rows.len(),
            false,
            |i| page.row_height(i, width, self.show_timestamps),
        );
        let wall = Utc::now();
        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut rects: Vec<(usize, Rect)> = Vec::new();
        let mut y = 0usize;
        let (anns, owner) = page.entry_annotations();
        let ann_refs: Vec<&Annotation> = anns
            .iter()
            .filter_map(|&i| page.annotations.get(i))
            .collect();
        for i in offset..page.rows.len() {
            let mut h = page.row_height(i, width, self.show_timestamps);
            if y == 0 {
                // A row taller than the panel (a huge line on a small window)
                // is still shown, cut to what fits: never an empty panel.
                h = h.min(usable.max(1));
            }
            if y + h > usable {
                break;
            }
            let selected = i == page.selected && focused;
            let row = page.rows[i];
            let drawn: Vec<Line<'static>> = match row {
                PRow::AnnHeader => {
                    let mut spans = vec![
                        Span::styled(
                            format!(" {} ", chevron(page.ann_open.contains(&owner))),
                            Style::default().fg(theme.detail_label_fg),
                        ),
                        Span::styled(
                            "Annotations",
                            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!("   {}", annotation_summary(&ann_refs)),
                            Style::default().fg(theme.detail_label_fg),
                        ),
                    ];
                    if selected {
                        select_row(&mut spans, &theme, width);
                    }
                    vec![Line::from(spans)]
                }
                PRow::Annotation(a) => match page.annotations.get(a) {
                    Some(ann) => {
                        let job_name = page
                            .job_nodes()
                            .iter()
                            .find(|j| j.job.id == ann.job_id)
                            .map_or("", |j| j.job.name.as_str());
                        annotation_lines(&theme, ann, job_name, selected, width)
                    }
                    None => vec![Line::raw("")],
                },
                PRow::Step(si) => match page
                    .entry_job()
                    .and_then(|jn| jn.job.steps.get(si).map(|s| (jn.job.id, s)))
                {
                    Some((job_id, step)) => {
                        let d = duration_text(
                            step.started_at.as_deref(),
                            step.completed_at.as_deref(),
                            wall,
                        );
                        vec![step_line(
                            &theme,
                            page.step_open(job_id, step.number),
                            step.status,
                            step.conclusion,
                            &step.name,
                            &d,
                            selected,
                            width,
                        )]
                    }
                    None => vec![Line::raw("")],
                },
                PRow::Line(si, li) => {
                    let line = page.entry_job().and_then(|jn| match &jn.log {
                        Load::Loaded(d) => d.lines.get(li).map(|l| {
                            let start = d.ranges.get(si).map_or(0, |r| r.start);
                            let open = page.open_groups.contains(&(jn.job.id, li));
                            log_rows(
                                &theme,
                                li.saturating_sub(start) + 1,
                                l,
                                self.show_timestamps,
                                open,
                                selected,
                                width,
                            )
                        }),
                        _ => None,
                    });
                    line.unwrap_or_else(|| vec![Line::default()])
                }
                PRow::Note(note) => {
                    let (text, failed) = match note {
                        Note::Text(t) => (t.to_string(), false),
                        Note::JobsFailed => (
                            match &page.jobs {
                                Load::Failed(e) => e.clone(),
                                _ => String::new(),
                            },
                            true,
                        ),
                        Note::LogFailed => (
                            match page.entry_job().map(|j| &j.log) {
                                Some(Load::Failed(e)) => e.clone(),
                                _ => String::new(),
                            },
                            true,
                        ),
                    };
                    let color = if failed {
                        theme.status_warn_fg
                    } else {
                        theme.detail_label_fg
                    };
                    vec![Line::from(vec![
                        Span::raw("   "),
                        Span::styled(text, Style::default().fg(color)),
                    ])]
                }
            };
            rects.push((
                i,
                Rect::new(body.x, body.y + y as u16, body.width, h as u16),
            ));
            lines.extend(drawn.into_iter().take(h));
            y += h;
        }
        f.render_widget(Paragraph::new(lines), body);
        if dots {
            more_below(f, body, &theme, "…");
        }
        self.panel_rects = rects;
        self.body_height = body.height;
        if let Some(page) = self.page.as_mut() {
            page.offset = offset;
        }
    }

    // ─── Boxes ───────────────────────────────────────────────────

    fn centered(area: Rect, width: u16, height: u16) -> Rect {
        let width = width.min(area.width);
        let height = height.min(area.height);
        Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        )
    }

    /// The workflows to choose from, over the list.
    fn render_picker(&mut self, f: &mut Frame, area: Rect) {
        let theme = self.ctx.color_theme.clone();
        let Some(cursor) = self.picker else {
            return;
        };
        let mut names = vec!["All".to_string()];
        names.extend(self.workflow_names());
        let widest = names
            .iter()
            .map(|n| console::measure_text_width(n))
            .max()
            .unwrap_or(10);
        // Leave room for the empty margin around the box.
        let rect = Self::centered(
            area.inner(ratatui::layout::Margin::new(1, 1)),
            widest as u16 + 8,
            names.len() as u16 + 2,
        );
        crate::popup::clear_with_margin(f, rect, area, &theme);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(crate::popup::border_style(&theme))
            .style(Style::default().bg(theme.bg))
            .title(Line::from(Span::styled(
                " Workflow ",
                crate::popup::title_style(&theme),
            )));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let height = inner.height as usize;
        let start = (cursor + 1).saturating_sub(height);
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (y, (i, n)) in names
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .enumerate()
        {
            let mut spans = vec![Span::styled(format!(" {n}"), Style::default().fg(theme.fg))];
            if i == cursor {
                select_row(&mut spans, &theme, inner.width as usize);
            }
            lines.push(Line::from(spans));
            self.picker_rects
                .push((i, Rect::new(inner.x, inner.y + y as u16, inner.width, 1)));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::actions::parse_runs;
    use crate::view::actions::logic::parse_log;
    use chrono::TimeZone;

    fn run(event: &str, actor: &str, branch: &str) -> Run {
        let json = format!(
            r#"{{"workflow_runs":[{{"id":1,"name":"Deny","display_title":"chore(deps): bump resvg","event":"{event}",
            "status":"completed","conclusion":"success","head_branch":"{branch}","head_sha":"b04187f0000","run_number":152,
            "actor":{{"login":"{actor}"}},"pull_requests":[{{"number":93}}],
            "created_at":"2026-11-16T07:19:00Z","run_started_at":"2026-11-16T07:19:00Z","updated_at":"2026-11-16T07:19:52Z"}}]}}"#
        );
        parse_runs(&json).unwrap().remove(0)
    }

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.to_string()).collect()
    }

    fn now() -> chrono::DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap()
    }

    #[test]
    fn a_run_is_one_line_with_its_workflow_branch_and_date() {
        let theme = ColorTheme::default();
        let r = run("pull_request", "dependabot[bot]", "deps/resvg-0.50.0");
        let t = text(&run_line(&theme, &r, false, 130, now()));
        assert!(t.contains("✓"), "{t}");
        assert!(t.contains("chore(deps): bump resvg"), "{t}");
        assert!(t.contains("Deny #152"), "workflow and number: {t}");
        assert!(t.contains("deps/resvg-0.50.0"), "branch: {t}");
        assert!(t.contains("Today at") || t.contains("Nov"), "date: {t}");
        assert!(!t.contains("Bot"), "no actor chip any more: {t}");
        assert_eq!(console::measure_text_width(&t), 130);
    }

    #[test]
    fn narrow_terminals_drop_the_right_columns_first() {
        let theme = ColorTheme::default();
        let r = run("push", "NayJi7", "master");
        let at = |w| text(&run_line(&theme, &r, false, w, now()));
        let mid = at(95);
        assert!(
            mid.contains("Deny #152") && !mid.contains("master"),
            "{mid}"
        );
        let small = at(60);
        assert!(!small.contains("Deny #152") && !small.contains("master"));
        assert!(
            small.contains("Today at") || small.contains("Nov"),
            "{small}"
        );
        let tiny = at(30);
        assert!(
            !tiny.contains("Today at") && tiny.contains("chore"),
            "{tiny}"
        );
        for w in [130, 95, 60, 30] {
            assert!(console::measure_text_width(&at(w)) <= w, "width {w}");
        }
    }

    #[test]
    fn the_row_under_the_cursor_is_flagged_with_a_triangle() {
        let theme = ColorTheme::default();
        let r = run("push", "NayJi7", "master");
        let on = text(&run_line(&theme, &r, true, 100, now()));
        let off = text(&run_line(&theme, &r, false, 100, now()));
        assert!(on.starts_with("▶ ✓"), "{on:?}");
        assert!(off.starts_with("  ✓"), "{off:?}");
        assert_eq!(
            console::measure_text_width(&on),
            console::measure_text_width(&off),
            "the marker takes the place of two blanks"
        );
    }

    #[test]
    fn a_selected_row_is_painted_to_the_edge() {
        let theme = ColorTheme::default();
        let r = run("push", "NayJi7", "master");
        let line = run_line(&theme, &r, true, 100, now());
        assert_eq!(console::measure_text_width(&text(&line)), 100);
        assert!(line
            .spans
            .iter()
            .all(|s| s.style.bg == Some(theme.list_selected_bg)));
    }

    #[test]
    fn log_rendering_marks_errors_groups_and_keeps_ansi() {
        let theme = ColorTheme::default();
        let lines = parse_log(&[
            "2026-11-16T09:00:00.1Z ##[group]Build".to_string(),
            "2026-11-16T09:00:01.1Z \u{1b}[31mred text\u{1b}[0m done".to_string(),
            "2026-11-16T09:00:02.1Z ##[error]boom".to_string(),
            "2026-11-16T09:00:03.1Z progress 10%\rprogress 100%".to_string(),
        ]);
        let t = |l: &LogLine, ts, open| {
            render_log_line(l, ts, open, &theme)
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        assert_eq!(t(&lines[0], false, false), "▸ Build");
        assert_eq!(t(&lines[0], false, true), "▾ Build");
        assert_eq!(t(&lines[1], false, false), "red text done");
        assert_eq!(
            render_log_line(&lines[1], false, false, &theme)[0].style.fg,
            Some(ratatui::style::Color::Red)
        );
        assert_eq!(t(&lines[2], false, false), "✗ boom");
        assert_eq!(t(&lines[3], false, false), "progress 100%");
        assert_eq!(t(&lines[2], true, false), "09:00:02 ✗ boom");
    }

    #[test]
    fn log_rows_carry_their_line_number() {
        let theme = ColorTheme::default();
        let lines = parse_log(&["2026-11-16T09:00:00.1Z hello".to_string()]);
        let row = text(&log_rows(&theme, 29, &lines[0], false, false, false, 60)[0]);
        assert!(row.contains("  29  hello"), "{row:?}");
        let selected = log_rows(&theme, 1, &lines[0], false, false, true, 60);
        assert_eq!(console::measure_text_width(&text(&selected[0])), 60);
    }

    #[test]
    fn a_long_log_line_wraps_and_still_reads_as_one_line() {
        let theme = ColorTheme::default();
        let long = "word ".repeat(30); // 150 cells
        let lines = parse_log(&[format!("2026-11-16T09:00:00.1Z {long}")]);
        let rows = log_rows(&theme, 7, &lines[0], false, false, true, 60);
        // 60 - 9 of gutter = 51 cells of text per row.
        assert_eq!(rows.len(), 3, "{rows:?}");
        let t: Vec<String> = rows.iter().map(text).collect();
        assert!(t[0].contains("   7  word"), "{:?}", t[0]);
        assert!(
            t[1].starts_with("         ") && !t[1].contains('7'),
            "number only on the first row: {:?}",
            t[1]
        );
        for row in &rows {
            assert_eq!(
                console::measure_text_width(&text(row)),
                60,
                "painted to the edge"
            );
            assert!(row
                .iter()
                .all(|s| s.style.bg == Some(theme.list_selected_bg)));
        }
        // The height is what is drawn, and a short line stays on one row.
        assert_eq!(log_line_height(&lines[0], false, 51), 3);
        let short = parse_log(&["2026-11-16T09:00:00.1Z hi".to_string()]);
        assert_eq!(log_line_height(&short[0], false, 51), 1);
    }

    #[test]
    fn wrapping_keeps_the_styles_and_cuts_by_cells() {
        let red = Style::default().fg(ratatui::style::Color::Red);
        let rows = wrap_spans(
            vec![
                Span::raw("ab"),
                Span::styled("cdef", red),
                Span::raw("日本"),
            ],
            3,
        );
        let flat: Vec<String> = rows
            .iter()
            .map(|r| r.iter().map(|s| s.content.to_string()).collect())
            .collect();
        assert_eq!(flat, vec!["abc", "def", "日", "本"]);
        assert_eq!(
            rows[0][1].style, red,
            "the colored part keeps its color after the cut"
        );
        // A character wider than the area still gets its own row.
        assert_eq!(wrap_spans(vec![Span::raw("日日")], 1).len(), 2);
    }

    #[test]
    fn steps_align_their_duration_on_the_right() {
        use crate::github::actions::{RunConclusion, RunStatus};
        let theme = ColorTheme::default();
        let line = step_line(
            &theme,
            true,
            RunStatus::Completed,
            Some(RunConclusion::Success),
            "Set up job",
            "1s",
            false,
            50,
        );
        let t = text(&line);
        assert!(t.starts_with(" ▾ ✓"), "{t:?}");
        assert!(t.trim_end().ends_with("      1s"), "{t:?}");
        assert_eq!(console::measure_text_width(&t), 50);
    }

    #[test]
    fn annotations_are_summed_up_and_wrapped() {
        let notice = Annotation {
            job_id: 1,
            level: AnnotationLevel::Notice,
            title: String::new(),
            message: "The ubuntu-latest label will migrate to Ubuntu 26".into(),
            path: ".github".into(),
            line: 1,
        };
        let mut error = notice.clone();
        error.level = AnnotationLevel::Failure;
        assert_eq!(annotation_summary(&[&notice]), "1 notice");
        assert_eq!(
            annotation_summary(&[&notice, &error, &error]),
            "2 errors, 1 notice"
        );
        let theme = ColorTheme::default();
        let lines = annotation_lines(&theme, &notice, "cargo-deny", false, 30);
        assert!(text(&lines[0]).contains("ℹ  cargo-deny"));
        assert!(lines.len() > 2, "the message wraps: {}", lines.len());
        for l in &lines {
            assert!(console::measure_text_width(&text(l)) <= 30);
        }
    }
}
