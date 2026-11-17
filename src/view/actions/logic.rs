//! Pure logic of the Actions page: filters, status glyphs, durations, the
//! jobs tree and the parsing of job logs. No rendering, no network.

use chrono::{DateTime, Utc};

use crate::github::actions::{Run, RunConclusion, RunStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunFilter {
    All,
    Running,
    Failed,
    Succeeded,
}

impl RunFilter {
    pub const ALL: [RunFilter; 4] = [Self::All, Self::Running, Self::Failed, Self::Succeeded];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Running => "Running",
            Self::Failed => "Failed",
            Self::Succeeded => "Succeeded",
        }
    }

    pub fn matches(self, run: &Run) -> bool {
        match self {
            Self::All => true,
            Self::Running => run.status.is_active(),
            Self::Failed => run.conclusion.is_some_and(RunConclusion::is_failure),
            Self::Succeeded => run.conclusion == Some(RunConclusion::Success),
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Which theme color a status uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Success,
    Failure,
    Active,
    Queued,
    Muted,
}

pub fn status_glyph(status: RunStatus, conclusion: Option<RunConclusion>) -> (&'static str, Tone) {
    if status.is_active() {
        return match status {
            RunStatus::InProgress => ("●", Tone::Active),
            _ => ("○", Tone::Queued),
        };
    }
    match conclusion {
        Some(RunConclusion::Success) => ("✓", Tone::Success),
        Some(c) if c.is_failure() => ("✗", Tone::Failure),
        Some(RunConclusion::Cancelled) => ("⊘", Tone::Muted),
        Some(RunConclusion::Skipped)
        | Some(RunConclusion::Neutral)
        | Some(RunConclusion::Stale) => ("–", Tone::Muted),
        _ => ("?", Tone::Muted),
    }
}

/// What GitHub writes next to the status icon: `Success`, `In progress`...
pub fn status_label(status: RunStatus, conclusion: Option<RunConclusion>) -> &'static str {
    match status {
        RunStatus::InProgress => "In progress",
        RunStatus::Queued | RunStatus::Pending | RunStatus::Requested => "Queued",
        RunStatus::Waiting => "Waiting",
        _ => match conclusion {
            Some(RunConclusion::Success) => "Success",
            Some(RunConclusion::Failure) | Some(RunConclusion::StartupFailure) => "Failure",
            Some(RunConclusion::TimedOut) => "Timed out",
            Some(RunConclusion::ActionRequired) => "Action required",
            Some(RunConclusion::Cancelled) => "Cancelled",
            Some(RunConclusion::Skipped) => "Skipped",
            Some(RunConclusion::Neutral) | Some(RunConclusion::Stale) => "Neutral",
            _ => "Completed",
        },
    }
}

/// How GitHub words a job under its name: `succeeded 4 hours ago in 48s`.
pub fn job_phrase(status: RunStatus, conclusion: Option<RunConclusion>) -> &'static str {
    match status {
        RunStatus::InProgress => "started",
        RunStatus::Queued | RunStatus::Pending | RunStatus::Requested => "queued",
        RunStatus::Waiting => "waiting",
        _ => match conclusion {
            Some(RunConclusion::Success) => "succeeded",
            Some(RunConclusion::Failure) | Some(RunConclusion::StartupFailure) => "failed",
            Some(RunConclusion::TimedOut) => "timed out",
            Some(RunConclusion::Cancelled) => "was cancelled",
            Some(RunConclusion::Skipped) => "was skipped",
            _ => "completed",
        },
    }
}

fn parse_time(iso: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// `45s`, `1m 32s`, `2h 3m`. Empty when the start is unknown.
pub fn duration_text(start: Option<&str>, end: Option<&str>, now: DateTime<Utc>) -> String {
    let Some(start) = start.and_then(parse_time) else {
        return String::new();
    };
    let end = end.and_then(parse_time).unwrap_or(now);
    let secs = (end - start).num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn run_duration(run: &Run, now: DateTime<Utc>) -> String {
    let start = run.started_at.as_deref().or(Some(run.created_at.as_str()));
    let end = (!run.status.is_active()).then_some(run.updated_at.as_str());
    duration_text(start, end, now)
}

/// Truncates to `width` display cells with an ellipsis, then pads.
pub fn fit_text(s: &str, width: usize) -> String {
    let w = console::measure_text_width(s);
    if w <= width {
        return format!("{s}{}", " ".repeat(width - w));
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = console::measure_text_width(&c.to_string());
        if used + cw + 1 > width {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    used += 1;
    format!("{out}{}", " ".repeat(width.saturating_sub(used)))
}

// ─── Logs ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogKind {
    Plain,
    Group,
    /// `##[endgroup]`: never drawn, but it counts in the line numbers.
    EndGroup,
    Command,
    Error,
    Warning,
    Notice,
    Debug,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// `2026-11-16T09:42:58.1234567Z`, empty when the line has none.
    pub timestamp: String,
    /// The text without timestamp and without the `##[...]` marker.
    pub text: String,
    pub kind: LogKind,
}

fn split_timestamp(raw: &str) -> (&str, &str) {
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    if let Some((first, rest)) = raw.split_once(' ') {
        if first.len() >= 20 && first.ends_with('Z') && first.as_bytes().get(10) == Some(&b'T') {
            return (first, rest);
        }
    }
    ("", raw)
}

pub fn parse_log_line(raw: &str) -> LogLine {
    let (timestamp, rest) = split_timestamp(raw);
    let (kind, text) = if let Some(t) = rest.strip_prefix("##[group]") {
        (LogKind::Group, t)
    } else if rest.trim_end() == "##[endgroup]" {
        (LogKind::EndGroup, "")
    } else if let Some(t) = rest.strip_prefix("##[error]") {
        (LogKind::Error, t)
    } else if let Some(t) = rest.strip_prefix("##[warning]") {
        (LogKind::Warning, t)
    } else if let Some(t) = rest.strip_prefix("##[notice]") {
        (LogKind::Notice, t)
    } else if let Some(t) = rest.strip_prefix("##[debug]") {
        (LogKind::Debug, t)
    } else if rest.starts_with("[command]") {
        (LogKind::Command, rest)
    } else {
        (LogKind::Plain, rest)
    };
    LogLine {
        timestamp: timestamp.to_string(),
        text: text.to_string(),
        kind,
    }
}

/// The text of a log line as it should be pasted: without the colors and with
/// only the last state of a progress bar that rewrites its line.
pub fn plain_log_text(text: &str) -> String {
    let last = text.rsplit('\r').next().unwrap_or(text);
    console::strip_ansi_codes(last).trim_end().to_string()
}

/// Parses a whole log. The `##[endgroup]` lines are kept: github.com counts
/// them in its line numbers, and they say where a group stops.
pub fn parse_log(lines: &[String]) -> Vec<LogLine> {
    lines.iter().map(|l| parse_log_line(l)).collect()
}

/// The lines of `range` worth drawing: a closed group shows only its header,
/// and the `##[endgroup]` markers never show.
pub fn visible_lines(
    lines: &[LogLine],
    range: std::ops::Range<usize>,
    is_open: impl Fn(usize) -> bool,
) -> Vec<usize> {
    let end = range.end.min(lines.len());
    let mut out = Vec::new();
    let mut i = range.start;
    while i < end {
        match lines[i].kind {
            LogKind::EndGroup => i += 1,
            LogKind::Group => {
                out.push(i);
                i += 1;
                if !is_open(i - 1) {
                    while i < end && !matches!(lines[i].kind, LogKind::EndGroup | LogKind::Group) {
                        i += 1;
                    }
                }
            }
            _ => {
                out.push(i);
                i += 1;
            }
        }
    }
    out
}

/// Word-wraps `text` to `width` display cells (at least one line).
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;
    for word in text.split_whitespace() {
        let w = console::measure_text_width(word);
        let gap = usize::from(used > 0);
        if used + gap + w <= width {
            if gap == 1 {
                current.push(' ');
            }
            current.push_str(word);
            used += gap + w;
            continue;
        }
        if used > 0 {
            lines.push(std::mem::take(&mut current));
            used = 0;
        }
        // A word longer than a line is cut where it overflows.
        for c in word.chars() {
            let cw = console::measure_text_width(&c.to_string());
            if used + cw > width {
                lines.push(std::mem::take(&mut current));
                used = 0;
            }
            current.push(c);
            used += cw;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

// ─── GitHub-like texts ───────────────────────────────────────────

/// What triggered a run, the way GitHub words it under the title.
pub fn event_phrase(run: &Run) -> String {
    let who = &run.actor;
    match run.event.as_str() {
        "pull_request" | "pull_request_target" => match run.pull_request {
            Some(n) => format!("Pull request #{n} opened by {who}"),
            None => format!("Pull request opened by {who}"),
        },
        "push" => {
            let sha: String = run.sha.chars().take(7).collect();
            format!("Commit {sha} pushed by {who}")
        }
        "workflow_dispatch" => format!("Manually run by {who}"),
        "schedule" => "Scheduled".to_string(),
        "release" => format!("Release by {who}"),
        "" | "dynamic" => format!("by {who}"),
        other => format!("{other} by {who}"),
    }
}

/// `Today at 07:19`, `Yesterday at 21:02`, `Nov 15, 14:26`, `Nov 15, 2025`.
pub fn format_when(iso: &str, now: chrono::DateTime<chrono::Local>) -> String {
    use chrono::{Datelike, Local};
    let Ok(t) = DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let t = t.with_timezone(&Local);
    let days = (now.date_naive() - t.date_naive()).num_days();
    match days {
        0 => format!("Today at {}", t.format("%H:%M")),
        1 => format!("Yesterday at {}", t.format("%H:%M")),
        _ if t.year() == now.year() => t.format("%b %-d, %H:%M").to_string(),
        _ => t.format("%b %-d, %Y").to_string(),
    }
}

/// The line a step opens with, when it has a recognisable one: the
/// `##[group]Run ...` of a script or an action, the build of a container
/// action, the cleanup of a post step, the end of the job.
fn is_step_opener(line: &LogLine) -> bool {
    match line.kind {
        LogKind::Group => ["Run ", "Build ", "Pre "]
            .iter()
            .any(|p| line.text.starts_with(p)),
        LogKind::Plain => [
            "Post job cleanup.",
            "Cleaning up orphan processes",
            "Evaluate and set job outputs",
        ]
        .iter()
        .any(|p| line.text.starts_with(p)),
        _ => false,
    }
}

/// Splits the lines of a job log between its steps. Steps are timed to the
/// second and a short step starts and ends in the same second as the ones
/// around it, so the time only says where to start looking: the boundary is
/// the first line that opens a step (see `is_step_opener`) before the step
/// ends, or else the first line written once the step has started. A skipped
/// step has times but no output. Returns one contiguous range per step.
pub fn assign_ranges(
    lines: &[LogLine],
    steps: &[crate::github::actions::Step],
) -> Vec<std::ops::Range<usize>> {
    let mut ranges = vec![0..0; steps.len()];
    // (index, start second, end second) of the steps that ran.
    let timed: Vec<(usize, i64, Option<i64>)> = steps
        .iter()
        .enumerate()
        .filter(|(_, s)| s.conclusion != Some(RunConclusion::Skipped))
        .filter_map(|(i, s)| {
            let start = s.started_at.as_deref().and_then(parse_time)?;
            let end = s.completed_at.as_deref().and_then(parse_time);
            Some((i, start.timestamp(), end.map(|t| t.timestamp())))
        })
        .collect();
    let Some(&(first, _, _)) = timed.first() else {
        return ranges;
    };
    let second_of = |i: usize| parse_time(&lines[i].timestamp).map(|t| t.timestamp());
    // The very first lines belong to the first step, whatever their time.
    let (mut open, mut begin) = (first, 0usize);
    for (k, &(j, start, end)) in timed.iter().enumerate().skip(1) {
        let limit = end.or(timed.get(k + 1).map(|next| next.1));
        let Some(first_written) =
            (begin + 1..lines.len()).find(|&i| second_of(i).is_some_and(|s| s >= start))
        else {
            break;
        };
        let mut boundary = first_written;
        for (i, line) in lines.iter().enumerate().skip(first_written) {
            if second_of(i).is_some_and(|s| limit.is_some_and(|l| s > l)) {
                break;
            }
            if is_step_opener(line) {
                boundary = i;
                break;
            }
        }
        ranges[open] = begin..boundary;
        begin = boundary;
        open = j;
    }
    ranges[open] = begin..lines.len();
    ranges
}

/// Height in terminal rows of each list row, so a run can take two.
pub fn rows_that_fit(
    offset: usize,
    height: usize,
    count: usize,
    row_height: impl Fn(usize) -> usize,
) -> usize {
    let mut used = 0;
    let mut n = 0;
    for i in offset..count {
        let h = row_height(i);
        if used + h > height {
            break;
        }
        used += h;
        n += 1;
    }
    n
}

/// The largest scroll offset: the one that puts the last row at the bottom
/// of a screen of `height` lines, so a wheel never scrolls into empty space.
pub fn max_offset(count: usize, height: usize, row_height: impl Fn(usize) -> usize) -> usize {
    let mut used = 0;
    let mut offset = count;
    while offset > 0 {
        let h = row_height(offset - 1);
        if used + h > height {
            break;
        }
        used += h;
        offset -= 1;
    }
    // A row taller than the screen still has to be reachable.
    offset.min(count.saturating_sub(1))
}

/// The list after a refresh of its first page: the fresh runs, then the older
/// runs that were loaded by scrolling (they are not in the first page any more
/// than before). Returns the list and whether such older runs were kept.
pub fn merge_first_page(fresh: Vec<Run>, old: &[Run]) -> (Vec<Run>, bool) {
    let Some(oldest) = fresh.last().map(|r| r.created_at.clone()) else {
        return (fresh, false);
    };
    let ids: std::collections::HashSet<u64> = fresh.iter().map(|r| r.id).collect();
    let tail: Vec<Run> = old
        .iter()
        .filter(|r| !ids.contains(&r.id) && r.created_at < oldest)
        .cloned()
        .collect();
    let kept = !tail.is_empty();
    let mut merged = fresh;
    merged.extend(tail);
    (merged, kept)
}

/// Where the visible window starts, how many lines the rows may use, and
/// whether a "…" must be drawn: when rows are hidden below, the last line is
/// kept free for the dots, like the commit list does while it loads.
pub fn plan_window(
    offset_hint: usize,
    selected: usize,
    free_scroll: bool,
    height: usize,
    count: usize,
    more_remote: bool,
    row_height: impl Fn(usize) -> usize + Copy,
) -> (usize, usize, bool) {
    let offset_for = |h: usize| {
        if free_scroll {
            offset_hint.min(max_offset(count, h, row_height))
        } else {
            fit_offset(offset_hint, selected, h, count, row_height)
        }
    };
    let offset = offset_for(height);
    let shown = rows_that_fit(offset, height, count, row_height);
    // `more_remote`: more rows exist on GitHub than are loaded, the dots stay.
    if (offset + shown >= count && !more_remote) || height < 3 {
        return (offset, height, false);
    }
    let usable = height - 1;
    let offset = offset_for(usable);
    let shown = rows_that_fit(offset, usable, count, row_height);
    (offset, usable, offset + shown < count || more_remote)
}

/// The scroll offset that keeps `selected` fully on screen.
pub fn fit_offset(
    offset: usize,
    selected: usize,
    height: usize,
    count: usize,
    row_height: impl Fn(usize) -> usize + Copy,
) -> usize {
    let mut offset = offset.min(count.saturating_sub(1));
    if selected < offset {
        return selected;
    }
    while offset < selected
        && selected >= offset + rows_that_fit(offset, height, count, row_height).max(1)
    {
        offset += 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::actions::parse_runs;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| i.to_string()).collect()
    }

    fn run(status: &str, conclusion: Option<&str>) -> Run {
        let c = conclusion.map_or("null".to_string(), |c| format!("\"{c}\""));
        let json = format!(
            r#"{{"workflow_runs":[{{"id":1,"name":"CI","status":"{status}","conclusion":{c},
            "created_at":"2026-11-16T10:00:00Z","updated_at":"2026-11-16T10:01:30Z",
            "run_started_at":"2026-11-16T10:00:00Z"}}]}}"#
        );
        parse_runs(&json).unwrap().remove(0)
    }

    fn step(n: u64, start: Option<&str>) -> crate::github::actions::Step {
        crate::github::actions::Step {
            number: n,
            name: format!("step {n}"),
            status: RunStatus::Completed,
            conclusion: Some(RunConclusion::Success),
            started_at: start.map(str::to_string),
            completed_at: None,
        }
    }

    #[test]
    fn log_lines_are_split_between_steps_by_time() {
        let steps = vec![
            step(1, Some("2026-11-16T10:00:00Z")),
            step(2, Some("2026-11-16T10:00:05Z")),
            step(3, None), // skipped
            step(4, Some("2026-11-16T10:00:09Z")),
        ];
        let lines = parse_log(&s(&[
            "2026-11-16T09:59:59.9Z before the first step",
            "2026-11-16T10:00:00.5Z setup",
            "2026-11-16T10:00:05.1Z checkout",
            "2026-11-16T10:00:08.0Z still checkout",
            "2026-11-16T10:00:09.9Z build",
            "no timestamp stays with its neighbour",
        ]));
        let r = assign_ranges(&lines, &steps);
        assert_eq!(r[0], 0..2, "the very first lines belong to the first step");
        assert_eq!(r[1], 2..4);
        assert_eq!(r[2], 0..0, "a skipped step has no output");
        assert_eq!(r[3], 4..6);
    }

    #[test]
    fn steps_that_start_in_the_same_second_split_on_their_group_line() {
        let steps = vec![
            step(1, Some("2026-11-16T10:00:00Z")),
            step(2, Some("2026-11-16T10:00:01Z")),
            step(3, Some("2026-11-16T10:00:01Z")),
        ];
        let lines = parse_log(&s(&[
            "2026-11-16T10:00:00.1Z Current runner version",
            "2026-11-16T10:00:01.0Z Complete job name: x",
            "2026-11-16T10:00:01.2Z ##[group]Build container",
            "2026-11-16T10:00:01.3Z building",
            "2026-11-16T10:00:01.4Z ##[endgroup]",
            "2026-11-16T10:00:01.6Z ##[group]Run checkout",
            "2026-11-16T10:00:01.7Z checking out",
        ]));
        let r = assign_ranges(&lines, &steps);
        assert_eq!(r[0], 0..2, "the last line of the setup stays with it");
        assert_eq!(r[1], 2..5, "one group line opens one step");
        assert_eq!(r[2], 5..7);
    }

    #[test]
    fn a_real_log_where_every_step_lasts_less_than_a_second() {
        let mut steps = vec![
            step(1, Some("2026-11-17T09:15:53Z")),
            step(2, Some("2026-11-17T09:15:54Z")),
            step(3, Some("2026-11-17T09:15:54Z")),
            step(4, Some("2026-11-17T09:15:54Z")),
            step(5, Some("2026-11-17T09:15:54Z")),
        ];
        for st in &mut steps {
            st.completed_at = Some("2026-11-17T09:15:54Z".into());
        }
        steps[3].conclusion = Some(RunConclusion::Skipped);
        let lines = parse_log(&s(&[
            "2026-11-17T09:15:53.91Z Current runner version: '2.338.2'",
            "2026-11-17T09:15:53.92Z ##[group]Runner Image Provisioner",
            "2026-11-17T09:15:53.93Z Hosted Compute Agent",
            "2026-11-17T09:15:53.94Z ##[endgroup]",
            "2026-11-17T09:15:53.95Z Complete job name: fails",
            "2026-11-17T09:15:54.05Z ##[group]Run echo warning",
            "2026-11-17T09:15:54.06Z echo warning",
            "2026-11-17T09:15:54.10Z ##[endgroup]",
            "2026-11-17T09:15:54.13Z ##[warning]This is a warning annotation",
            "2026-11-17T09:15:54.14Z ##[group]Run echo group",
            "2026-11-17T09:15:54.15Z exit 1",
            "2026-11-17T09:15:54.16Z ##[endgroup]",
            "2026-11-17T09:15:54.17Z ##[group]Details of the failure",
            "2026-11-17T09:15:54.18Z line inside a group 1",
            "2026-11-17T09:15:54.19Z ##[endgroup]",
            "2026-11-17T09:15:54.20Z ##[error]Process completed with exit code 1.",
            "2026-11-17T09:15:54.21Z Cleaning up orphan processes",
        ]));
        let r = assign_ranges(&lines, &steps);
        assert_eq!(r[0], 0..5, "set up job keeps its last line");
        assert_eq!(r[1], 5..9, "the warning step");
        assert_eq!(
            r[2],
            9..16,
            "the failing step keeps its user group and its errors"
        );
        assert_eq!(r[3], 0..0, "a skipped step has no output");
        assert_eq!(r[4], 16..17, "complete job");
    }

    #[test]
    fn closed_groups_hide_their_lines_but_not_their_numbers() {
        let lines = parse_log(&s(&[
            "2026-11-16T10:00:00Z first",
            "2026-11-16T10:00:00Z ##[group]Details",
            "2026-11-16T10:00:00Z inside 1",
            "2026-11-16T10:00:00Z inside 2",
            "2026-11-16T10:00:00Z ##[endgroup]",
            "2026-11-16T10:00:00Z last",
        ]));
        assert_eq!(lines[4].kind, LogKind::EndGroup, "markers are kept");
        assert_eq!(visible_lines(&lines, 0..6, |_| false), vec![0, 1, 5]);
        assert_eq!(visible_lines(&lines, 0..6, |i| i == 1), vec![0, 1, 2, 3, 5]);
        assert_eq!(visible_lines(&lines, 2..4, |_| false), vec![2, 3]);
        // A group that never ends stops at the next one.
        let open_ended = parse_log(&s(&["##[group]A", "x", "##[group]B", "y"]));
        assert_eq!(visible_lines(&open_ended, 0..4, |_| false), vec![0, 2]);
    }

    #[test]
    fn a_copied_log_line_has_no_colors_and_no_progress_noise() {
        assert_eq!(
            plain_log_text("\u{1b}[31mred text\u{1b}[0m done  "),
            "red text done"
        );
        assert_eq!(
            plain_log_text("progress 10%\rprogress 100%"),
            "progress 100%"
        );
        assert_eq!(plain_log_text(""), "");
    }

    #[test]
    fn text_wraps_by_display_width() {
        assert_eq!(wrap_text("one two three", 7), vec!["one two", "three"]);
        assert_eq!(wrap_text("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(wrap_text("", 10), vec![String::new()]);
        assert_eq!(wrap_text("日本語 日本", 6), vec!["日本語", "日本"]);
        assert_eq!(wrap_text("x", 0), vec!["x"], "zero width is clamped");
    }

    #[test]
    fn a_log_with_no_timed_step_gives_empty_ranges() {
        let steps = vec![step(1, None), step(2, None)];
        let lines = parse_log(&s(&["2026-11-16T10:00:00Z x"]));
        assert_eq!(assign_ranges(&lines, &steps), vec![0..0, 0..0]);
        assert!(assign_ranges(&[], &[]).is_empty());
    }

    fn run_of(event: &str, actor: &str, pr: Option<u64>) -> Run {
        let mut r = run("completed", Some("success"));
        r.event = event.into();
        r.actor = actor.into();
        r.pull_request = pr;
        r.sha = "b04187f1234".into();
        r
    }

    #[test]
    fn what_triggered_a_run_is_worded_like_github() {
        assert_eq!(
            event_phrase(&run_of("pull_request", "dependabot[bot]", Some(93))),
            "Pull request #93 opened by dependabot[bot]"
        );
        assert_eq!(
            event_phrase(&run_of("pull_request", "a", None)),
            "Pull request opened by a"
        );
        assert_eq!(
            event_phrase(&run_of("push", "NayJi7", None)),
            "Commit b04187f pushed by NayJi7"
        );
        assert_eq!(
            event_phrase(&run_of("workflow_dispatch", "me", None)),
            "Manually run by me"
        );
        assert_eq!(event_phrase(&run_of("schedule", "x", None)), "Scheduled");
        assert_eq!(
            event_phrase(&run_of("dynamic", "dependabot[bot]", None)),
            "by dependabot[bot]"
        );
        assert_eq!(event_phrase(&run_of("fork", "z", None)), "fork by z");
    }

    #[test]
    fn dates_are_worded_like_github() {
        use chrono::{Local, TimeZone};
        let now = Local.with_ymd_and_hms(2026, 11, 16, 12, 0, 0).unwrap();
        let at = |y, m, d, h, mi| {
            Local
                .with_ymd_and_hms(y, m, d, h, mi, 0)
                .unwrap()
                .to_rfc3339()
        };
        assert_eq!(format_when(&at(2026, 11, 16, 7, 19), now), "Today at 07:19");
        assert_eq!(
            format_when(&at(2026, 11, 15, 21, 2), now),
            "Yesterday at 21:02"
        );
        assert_eq!(format_when(&at(2026, 11, 3, 14, 26), now), "Nov 3, 14:26");
        assert_eq!(format_when(&at(2025, 11, 15, 14, 26), now), "Nov 15, 2025");
        assert_eq!(format_when("garbage", now), "garbage");
    }

    #[test]
    fn scrolling_keeps_the_selection_on_screen_with_two_line_rows() {
        // Rows 0, 3 and 6 take two lines, the others one: a screen of 6 lines.
        let height = |i: usize| if i.is_multiple_of(3) { 2 } else { 1 };
        assert_eq!(rows_that_fit(0, 6, 10, height), 4); // 2+1+1+2 = 6
        assert_eq!(rows_that_fit(1, 6, 10, height), 5); // 1+1+2+1+1
        assert_eq!(fit_offset(0, 2, 6, 10, height), 0);
        assert_eq!(
            fit_offset(0, 5, 6, 10, height),
            1,
            "row 5 needs the offset to move"
        );
        assert_eq!(fit_offset(4, 1, 6, 10, height), 1, "going back up");
        assert_eq!(fit_offset(0, 0, 6, 0, height), 0, "no rows");
    }

    #[test]
    fn a_line_is_kept_for_the_dots_only_when_rows_are_hidden_below() {
        let one = |_| 1;
        // Everything fits: no dots, the whole height is used.
        assert_eq!(plan_window(0, 0, false, 10, 5, false, one), (0, 10, false));
        // 20 rows on 10 lines: 9 rows and the dots.
        assert_eq!(plan_window(0, 0, false, 10, 20, false, one), (0, 9, true));
        // The cursor on the 10th row must not hide under the dots.
        let (offset, usable, dots) = plan_window(0, 9, false, 10, 20, false, one);
        assert!(
            9 >= offset && 9 < offset + usable && dots,
            "{offset} {usable}"
        );
        // At the very end there is nothing below.
        assert_eq!(
            plan_window(0, 19, false, 10, 20, false, one),
            (10, 10, false)
        );
        // Scrolled with the wheel to the end: no dots either.
        assert_eq!(
            plan_window(99, 0, true, 10, 20, false, one),
            (10, 10, false)
        );
        // A screen too small for dots uses all of its lines.
        assert_eq!(plan_window(0, 0, false, 2, 20, false, one), (0, 2, false));
        // More runs on GitHub: the dots stay at the end, even when all fit.
        assert_eq!(plan_window(0, 0, false, 10, 5, true, one), (0, 9, true));
        assert_eq!(plan_window(10, 19, false, 10, 20, true, one), (11, 9, true));
    }

    #[test]
    fn a_refreshed_first_page_keeps_the_older_runs_loaded_by_scrolling() {
        let run = |id: u64, created: &str| {
            parse_runs(&format!(
                r#"{{"workflow_runs":[{{"id":{id},"created_at":"{created}"}}]}}"#
            ))
            .unwrap()
            .remove(0)
        };
        let old = vec![
            run(5, "2026-11-05T00:00:00Z"),
            run(4, "2026-11-04T00:00:00Z"),
            run(3, "2026-11-03T00:00:00Z"),
            run(2, "2026-11-02T00:00:00Z"),
        ];
        // The first page now holds a new run and the 5 and 4: 3 and 2 were loaded later.
        let fresh = vec![
            run(6, "2026-11-06T00:00:00Z"),
            run(5, "2026-11-05T00:00:00Z"),
            run(4, "2026-11-04T00:00:00Z"),
        ];
        let (merged, kept) = merge_first_page(fresh, &old);
        assert!(kept);
        assert_eq!(
            merged.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![6, 5, 4, 3, 2]
        );
        // Nothing loaded beyond the first page: nothing to keep.
        let (merged, kept) = merge_first_page(vec![run(6, "2026-11-06T00:00:00Z")], &[]);
        assert!(!kept && merged.len() == 1);
        assert!(merge_first_page(Vec::new(), &old).0.is_empty());
    }

    #[test]
    fn the_wheel_stops_when_the_last_row_reaches_the_bottom() {
        let one = |_| 1;
        assert_eq!(max_offset(10, 4, one), 6);
        assert_eq!(max_offset(3, 10, one), 0, "everything fits");
        assert_eq!(max_offset(0, 10, one), 0, "no rows");
        // The last row takes two lines: 3 + 2 fit in 5 lines from row 7.
        let tall_last = |i: usize| if i == 9 { 2 } else { 1 };
        assert_eq!(max_offset(10, 5, tall_last), 6);
        assert_eq!(max_offset(2, 1, |_| 3), 1, "a row taller than the screen");
    }

    #[test]
    fn filters_match_the_right_runs() {
        let running = run("in_progress", None);
        let queued = run("queued", None);
        let failed = run("completed", Some("failure"));
        let timed_out = run("completed", Some("timed_out"));
        let ok = run("completed", Some("success"));
        let cancelled = run("completed", Some("cancelled"));
        assert!(RunFilter::Running.matches(&running) && RunFilter::Running.matches(&queued));
        assert!(!RunFilter::Running.matches(&ok));
        assert!(RunFilter::Failed.matches(&failed) && RunFilter::Failed.matches(&timed_out));
        assert!(!RunFilter::Failed.matches(&cancelled) && !RunFilter::Failed.matches(&ok));
        assert!(RunFilter::Succeeded.matches(&ok) && !RunFilter::Succeeded.matches(&failed));
        assert!(RunFilter::All.matches(&cancelled));
    }

    #[test]
    fn filters_cycle_both_ways() {
        assert_eq!(RunFilter::All.next(), RunFilter::Running);
        assert_eq!(RunFilter::Succeeded.next(), RunFilter::All);
        assert_eq!(RunFilter::All.prev(), RunFilter::Succeeded);
    }

    #[test]
    fn glyphs_and_tones() {
        assert_eq!(
            status_glyph(RunStatus::InProgress, None),
            ("●", Tone::Active)
        );
        assert_eq!(status_glyph(RunStatus::Queued, None), ("○", Tone::Queued));
        assert_eq!(status_glyph(RunStatus::Waiting, None), ("○", Tone::Queued));
        let done = RunStatus::Completed;
        assert_eq!(
            status_glyph(done, Some(RunConclusion::Success)),
            ("✓", Tone::Success)
        );
        assert_eq!(
            status_glyph(done, Some(RunConclusion::Failure)),
            ("✗", Tone::Failure)
        );
        assert_eq!(
            status_glyph(done, Some(RunConclusion::TimedOut)),
            ("✗", Tone::Failure)
        );
        assert_eq!(
            status_glyph(done, Some(RunConclusion::Cancelled)),
            ("⊘", Tone::Muted)
        );
        assert_eq!(
            status_glyph(done, Some(RunConclusion::Skipped)),
            ("–", Tone::Muted)
        );
        assert_eq!(status_glyph(done, None), ("?", Tone::Muted));
    }

    #[test]
    fn status_labels_read_like_github() {
        assert_eq!(
            status_label(RunStatus::Completed, Some(RunConclusion::Success)),
            "Success"
        );
        assert_eq!(status_label(RunStatus::InProgress, None), "In progress");
        assert_eq!(
            status_label(RunStatus::Completed, Some(RunConclusion::TimedOut)),
            "Timed out"
        );
        assert_eq!(status_label(RunStatus::Queued, None), "Queued");
    }

    #[test]
    fn durations() {
        let now = parse_time("2026-11-16T12:00:00Z").unwrap();
        let d = |a: &str, b: Option<&str>| duration_text(Some(a), b, now);
        assert_eq!(
            d("2026-11-16T10:00:00Z", Some("2026-11-16T10:00:45Z")),
            "45s"
        );
        assert_eq!(
            d("2026-11-16T10:00:00Z", Some("2026-11-16T10:01:32Z")),
            "1m 32s"
        );
        assert_eq!(
            d("2026-11-16T10:00:00Z", Some("2026-11-16T12:03:10Z")),
            "2h 3m"
        );
        assert_eq!(
            d("2026-11-16T11:59:30Z", None),
            "30s",
            "running: measured up to now"
        );
        assert_eq!(duration_text(None, None, now), "");
        let finished = run("completed", Some("success"));
        assert_eq!(run_duration(&finished, now), "1m 30s");
    }

    #[test]
    fn fit_text_truncates_and_pads_by_display_width() {
        assert_eq!(fit_text("abc", 5), "abc  ");
        assert_eq!(fit_text("abcdefgh", 5), "abcd…");
        assert_eq!(console::measure_text_width(&fit_text("日本語日本語", 7)), 7);
        assert_eq!(fit_text("abc", 0), "");
    }

    #[test]
    fn log_lines_lose_their_timestamp_and_markers() {
        let l = parse_log_line(
            "2026-11-16T09:42:58.1234567Z ##[error]Process completed with exit code 1.",
        );
        assert_eq!(l.timestamp, "2026-11-16T09:42:58.1234567Z");
        assert_eq!(
            (l.kind, l.text.as_str()),
            (LogKind::Error, "Process completed with exit code 1.")
        );
        let g = parse_log_line("2026-11-16T09:42:58.1Z ##[group]Run cargo test");
        assert_eq!(
            (g.kind, g.text.as_str()),
            (LogKind::Group, "Run cargo test")
        );
        let c = parse_log_line("2026-11-16T09:42:58.1Z [command]/usr/bin/git status");
        assert_eq!(c.kind, LogKind::Command);
        let w = parse_log_line("2026-11-16T09:42:58.1Z ##[warning]careful");
        assert_eq!(w.kind, LogKind::Warning);
        let plain = parse_log_line("no timestamp here");
        assert_eq!((plain.timestamp.as_str(), plain.kind), ("", LogKind::Plain));
        let bom = parse_log_line("\u{feff}2026-11-16T09:42:58.1Z first line");
        assert_eq!(
            (bom.timestamp.as_str(), bom.text.as_str()),
            ("2026-11-16T09:42:58.1Z", "first line")
        );
    }
}
