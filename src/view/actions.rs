//! GitHub Actions page, modelled on github.com.
//!
//! Two pages. The list of runs (filtered by status with the arrows, by
//! workflow with Tab, by text with `f`), and the page of one run: `Summary`
//! and its jobs on the left, and on the right either the summary or the steps
//! of the selected job. The steps are an accordion, and the groups of a log are
//! one inside it. Everything that touches the network runs in a thread and
//! answers with an `AppEvent` carrying a generation number, so an old answer
//! never replaces a newer one.

pub(crate) mod logic;
mod render;

use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    layout::Rect,
};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    app::AppContext,
    event::{ActionsRunKind, AppEvent, DialogKind, Sender, UserEvent, UserEventWithCount},
    github::{
        actions::{self, Annotation, Job, JobLog, Run, RunConclusion, RunStatus, Workflow},
        RepoCoords,
    },
    widget::commit_list::CommitListState,
};
use logic::{assign_ranges, parse_log, visible_lines, wrap_text, LogKind, LogLine, RunFilter};

/// Answers carry a generation number so an old one never replaces a newer
/// one. The counter is global: a page that was closed and reopened must not
/// accept the answers meant for the previous one.
fn next_generation() -> u64 {
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// While something is running, the page reloads this often.
const REFRESH_EVERY: Duration = Duration::from_secs(10);

/// From this width the jobs are a list on the left of the run page; below,
/// they are one line above the panel.
pub(super) const WIDE_FROM: u16 = 90;
pub(super) const SIDE_WIDTH: u16 = 26;

/// Annotations are read job by job: stop at a reasonable matrix size.
const ANNOTATION_JOBS_MAX: usize = 30;

/// Something that is fetched on demand.
#[derive(Debug)]
enum Load<T> {
    NotLoaded,
    Loading,
    Loaded(T),
    Failed(String),
}

#[derive(Debug)]
struct LogData {
    lines: Vec<LogLine>,
    /// One range of `lines` per step of the job.
    ranges: Vec<Range<usize>>,
}

#[derive(Debug)]
struct JobNode {
    job: Job,
    log: Load<LogData>,
    log_gen: u64,
    log_inflight: bool,
}

impl JobNode {
    /// GitHub only gives the log of a job once it is over: before, asking
    /// for it can only fail.
    fn logs_ready(&self) -> bool {
        !self.job.status.is_active()
    }

    /// The steps changed (a poll found new ones): split the log again, or the
    /// last steps would show "No output" for a log that has their lines.
    fn refit(&mut self) {
        if let Load::Loaded(data) = &mut self.log {
            data.ranges = assign_ranges(&data.lines, &self.job.steps);
        }
    }
}

/// What a re-run or a cancel was asked for, until GitHub shows it: the line
/// moves at once, then short checks follow (not one every second for ever).
#[derive(Debug)]
struct Pending {
    run_id: u64,
    cancel: bool,
    attempt_before: u64,
    deadline: Instant,
    /// How many of `FOLLOW_UP` were used.
    step: usize,
}

impl Pending {
    /// Compares what GitHub answers with what was asked. Returns true once the
    /// change is visible; until then an answer that still shows the old state
    /// is corrected so the line does not jump back.
    fn reconcile(&self, fresh: &mut Run) -> bool {
        if fresh.id != self.run_id {
            return false;
        }
        let settled = if self.cancel {
            !fresh.status.is_active()
        } else {
            fresh.status.is_active() || fresh.attempt > self.attempt_before
        };
        if !settled && !self.cancel {
            fresh.status = RunStatus::Queued;
            fresh.conclusion = None;
            fresh.attempt = self.attempt_before + 1;
        }
        settled
    }
}

/// Seconds to wait before each look at the run after an action.
const FOLLOW_UP: [u64; 6] = [1, 2, 3, 5, 8, 12];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    /// The list of `Summary` and jobs.
    Side,
    Panel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Note {
    Text(&'static str),
    JobsFailed,
    LogFailed,
}

/// A line of the panel of the run page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PRow {
    /// "Annotations: 1 notice", folds the annotations of the entry.
    AnnHeader,
    /// Index into the annotations of the page.
    Annotation(usize),
    /// Index of a step of the selected job.
    Step(usize),
    /// A step, then a line of the log (index into `LogData::lines`).
    Line(usize, usize),
    Note(Note),
}

/// What a row is, independent of its position: used to keep the selection
/// on the same thing when the rows are rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PKey {
    AnnHeader,
    Annotation(usize),
    Step(u64),
    Line(usize, usize),
    Note,
}

#[derive(Debug)]
struct RunPage {
    run: Run,
    jobs: Load<Vec<JobNode>>,
    jobs_gen: u64,
    jobs_inflight: bool,
    run_gen: u64,
    /// The run itself is being fetched (the header of the page is refreshed).
    run_inflight: bool,
    annotations: Vec<Annotation>,
    ann_gen: u64,
    ann_requested: bool,
    /// Asked once the run was over: they cannot change any more.
    ann_final: bool,
    /// 0 is `Summary`, then one entry per job.
    entry: usize,
    focus: Focus,
    rows: Vec<PRow>,
    selected: usize,
    offset: usize,
    /// The wheel scrolled the view: the offset no longer follows the cursor.
    free_scroll: bool,
    expanded_steps: FxHashSet<(u64, u64)>,
    /// `(job id, index of the `##[group]` line)` of the open groups.
    open_groups: FxHashSet<(u64, usize)>,
    /// Entries whose annotations are unfolded (0 is the summary).
    ann_open: FxHashSet<u64>,
    /// Jobs whose failed steps were already opened by themselves.
    opened_jobs: FxHashSet<u64>,
    /// A refresh failed and the user was told: say it once, not every poll.
    warned: bool,
}

impl RunPage {
    fn new(run: Run) -> Self {
        Self {
            run,
            jobs: Load::NotLoaded,
            jobs_gen: 0,
            jobs_inflight: false,
            run_gen: 0,
            run_inflight: false,
            annotations: Vec::new(),
            ann_gen: 0,
            ann_requested: false,
            ann_final: false,
            entry: 0,
            focus: Focus::Panel,
            rows: Vec::new(),
            selected: 0,
            offset: 0,
            free_scroll: false,
            expanded_steps: FxHashSet::default(),
            open_groups: FxHashSet::default(),
            ann_open: std::iter::once(0).collect(),
            opened_jobs: FxHashSet::default(),
            warned: false,
        }
    }

    /// A run that is active again (a re-run) can still produce annotations.
    fn settle_annotations_flag(&mut self) {
        if self.run.status.is_active() {
            self.ann_final = false;
        }
    }

    fn job_nodes(&self) -> &[JobNode] {
        match &self.jobs {
            Load::Loaded(jobs) => jobs,
            _ => &[],
        }
    }

    fn entry_count(&self) -> usize {
        1 + self.job_nodes().len()
    }

    /// The job of the selected entry (`None` on `Summary`).
    fn entry_job(&self) -> Option<&JobNode> {
        self.job_nodes().get(self.entry.checked_sub(1)?)
    }

    fn any_inflight(&self) -> bool {
        self.run_inflight || self.jobs_inflight || self.job_nodes().iter().any(|j| j.log_inflight)
    }

    fn step_open(&self, job_id: u64, number: u64) -> bool {
        self.expanded_steps.contains(&(job_id, number))
    }

    fn has_open_step(&self, job: &JobNode) -> bool {
        job.job
            .steps
            .iter()
            .any(|s| self.step_open(job.job.id, s.number))
    }

    /// Indices of the annotations of the entry, and the id that owns its
    /// fold state.
    fn entry_annotations(&self) -> (Vec<usize>, u64) {
        match self.entry_job() {
            None => ((0..self.annotations.len()).collect(), 0),
            Some(jn) => (
                self.annotations
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.job_id == jn.job.id)
                    .map(|(i, _)| i)
                    .collect(),
                jn.job.id,
            ),
        }
    }

    fn build_rows(&self) -> Vec<PRow> {
        let mut rows = Vec::new();
        if self.entry == 0 {
            match &self.jobs {
                Load::NotLoaded | Load::Loading => {
                    rows.push(PRow::Note(Note::Text("Loading jobs…")))
                }
                Load::Failed(_) => rows.push(PRow::Note(Note::JobsFailed)),
                Load::Loaded(jobs) if jobs.is_empty() => {
                    rows.push(PRow::Note(Note::Text("No jobs")))
                }
                Load::Loaded(_) => {}
            }
        }
        let (anns, owner) = self.entry_annotations();
        if !anns.is_empty() {
            rows.push(PRow::AnnHeader);
            if self.ann_open.contains(&owner) {
                rows.extend(anns.into_iter().map(PRow::Annotation));
            }
        }
        let Some(jn) = self.entry_job() else {
            return rows;
        };
        if jn.job.steps.is_empty() {
            rows.push(PRow::Note(Note::Text("No steps yet")));
        }
        for (si, step) in jn.job.steps.iter().enumerate() {
            rows.push(PRow::Step(si));
            if !self.step_open(jn.job.id, step.number) {
                continue;
            }
            match &jn.log {
                Load::NotLoaded if !jn.logs_ready() => rows.push(PRow::Note(Note::Text(
                    "Logs appear when the job finishes (GitHub does not stream them). o: follow it in the browser",
                ))),
                Load::NotLoaded => {}
                Load::Loading => rows.push(PRow::Note(Note::Text("Loading logs…"))),
                Load::Failed(_) => rows.push(PRow::Note(Note::LogFailed)),
                Load::Loaded(data) => {
                    let range = data.ranges.get(si).cloned().unwrap_or(0..0);
                    let shown = visible_lines(&data.lines, range, |l| {
                        self.open_groups.contains(&(jn.job.id, l))
                    });
                    if shown.is_empty() {
                        rows.push(PRow::Note(Note::Text("No output")));
                    } else {
                        rows.extend(shown.into_iter().map(|li| PRow::Line(si, li)));
                    }
                }
            }
        }
        rows
    }

    fn key_of(&self, row: PRow) -> PKey {
        match row {
            PRow::AnnHeader => PKey::AnnHeader,
            PRow::Annotation(i) => PKey::Annotation(i),
            PRow::Step(si) => self
                .entry_job()
                .and_then(|jn| jn.job.steps.get(si))
                .map_or(PKey::Note, |s| PKey::Step(s.number)),
            PRow::Line(si, li) => PKey::Line(si, li),
            PRow::Note(_) => PKey::Note,
        }
    }

    /// Terminal lines taken by row `i`: an annotation wraps.
    fn row_height(&self, i: usize, width: usize, show_timestamps: bool) -> usize {
        match self.rows.get(i) {
            Some(PRow::Annotation(a)) => self.annotations.get(*a).map_or(1, |a| {
                1 + wrap_text(&a.message, width.saturating_sub(7)).len()
            }),
            // A long log line wraps.
            Some(PRow::Line(_, li)) => match self.entry_job().map(|j| &j.log) {
                Some(Load::Loaded(d)) => d.lines.get(*li).map_or(1, |l| {
                    render::log_line_height(
                        l,
                        show_timestamps,
                        width.saturating_sub(render::LOG_GUTTER),
                    )
                }),
                _ => 1,
            },
            _ => 1,
        }
    }

    fn is_error_row(&self, row: PRow) -> bool {
        let PRow::Line(_, li) = row else {
            return false;
        };
        match self.entry_job().map(|j| &j.log) {
            Some(Load::Loaded(data)) => {
                data.lines.get(li).is_some_and(|l| l.kind == LogKind::Error)
            }
            _ => false,
        }
    }
}

/// The index of the first rectangle that holds the point.
fn rect_hit(rects: &[(usize, Rect)], col: u16, row: u16) -> Option<usize> {
    rects
        .iter()
        .find(|(_, r)| render::rect_contains(*r, col, row))
        .map(|(i, _)| *i)
}

#[derive(Debug)]
pub struct ActionsView<'a> {
    commit_list_state: Option<CommitListState<'a>>,
    coords: RepoCoords,
    token: String,
    /// Branch of HEAD, for the "current branch" filter.
    branch: Option<String>,
    runs: Vec<Run>,
    /// Indices into `runs` of what the filters let through.
    visible: Vec<usize>,
    selected: usize,
    offset: usize,
    /// The wheel scrolled the list: the offset no longer follows the cursor.
    free_scroll: bool,
    workflows: Vec<Workflow>,
    workflows_gen: u64,
    filter: RunFilter,
    only_branch: bool,
    /// Workflow chosen in the picker (`None` = all workflows).
    workflow: Option<String>,
    /// The text search (`f`), applied to the runs already loaded.
    query: String,
    search_input: bool,
    /// The field of the search: the same editor as the commit list, so every
    /// editing shortcut works the same.
    search: tui_input::Input,
    /// Cursor of the workflow picker while it is open.
    picker: Option<usize>,
    /// The workflow filter when the picker opened, to put back on Esc.
    picker_before: Option<String>,
    page: Option<RunPage>,
    show_timestamps: bool,
    runs_loading: bool,
    runs_gen: u64,
    /// Runs beyond the first page exist on GitHub, and the next page to ask.
    has_more: bool,
    next_page: u32,
    loading_more: bool,
    more_gen: u64,
    last_error: Option<String>,
    last_refresh: Instant,
    /// When the page opened: the running icons turn from this clock.
    started: Instant,
    reload_at: Option<Instant>,
    /// A re-run or a cancel that GitHub has not shown yet.
    pending: Option<Pending>,
    body_height: u16,
    // Filled by the last render
    layout_wide: bool,
    filter_tab_rects: Vec<(RunFilter, Rect)>,
    hovered_filter: Option<RunFilter>,
    row_rects: Vec<(usize, Rect)>,
    side_rects: Vec<(usize, Rect)>,
    picker_rects: Vec<(usize, Rect)>,
    /// The entry of the jobs list under the mouse (feedback only).
    hovered_side: Option<usize>,
    panel_rects: Vec<(usize, Rect)>,
    ctx: Rc<AppContext>,
    tx: Sender,
}

impl<'a> ActionsView<'a> {
    pub fn new(
        commit_list_state: Option<CommitListState<'a>>,
        coords: RepoCoords,
        token: String,
        branch: Option<String>,
        open_run: Option<Run>,
        ctx: Rc<AppContext>,
        tx: Sender,
    ) -> Self {
        let mut view = Self {
            commit_list_state,
            coords,
            token,
            branch,
            runs: Vec::new(),
            visible: Vec::new(),
            selected: 0,
            offset: 0,
            free_scroll: false,
            workflows: Vec::new(),
            workflows_gen: 0,
            filter: RunFilter::All,
            only_branch: false,
            workflow: None,
            query: String::new(),
            search_input: false,
            search: tui_input::Input::default(),
            picker: None,
            picker_before: None,
            page: None,
            show_timestamps: false,
            runs_loading: false,
            runs_gen: 0,
            has_more: false,
            next_page: 2,
            loading_more: false,
            more_gen: 0,
            last_error: None,
            last_refresh: Instant::now(),
            started: Instant::now(),
            reload_at: None,
            pending: None,
            body_height: 0,
            layout_wide: true,
            filter_tab_rects: Vec::new(),
            hovered_filter: None,
            row_rects: Vec::new(),
            side_rects: Vec::new(),
            picker_rects: Vec::new(),
            hovered_side: None,
            panel_rects: Vec::new(),
            ctx,
            tx,
        };
        view.fetch_runs();
        view.fetch_workflows();
        if let Some(run) = open_run {
            view.open_page(run);
        }
        view
    }

    pub fn take_list_state(&mut self) -> Option<CommitListState<'a>> {
        self.commit_list_state.take()
    }

    // ─── Loading ─────────────────────────────────────────────────

    fn fetch_runs(&mut self) {
        self.runs_gen = next_generation();
        self.runs_loading = true;
        self.last_refresh = Instant::now();
        let generation = self.runs_gen;
        let (token, coords, tx) = (self.token.clone(), self.coords.clone(), self.tx.clone());
        let branch = if self.only_branch {
            self.branch.clone()
        } else {
            None
        };
        crate::panic_guard::spawn_protected("actions-runs", move || {
            let result = actions::list_runs(&token, &coords, branch.as_deref(), actions::LIST_PAGE);
            tx.send(AppEvent::ActionsRunsFetched { generation, result });
        });
    }

    fn fetch_workflows(&mut self) {
        self.workflows_gen = next_generation();
        let generation = self.workflows_gen;
        let (token, coords, tx) = (self.token.clone(), self.coords.clone(), self.tx.clone());
        crate::panic_guard::spawn_protected("actions-workflows", move || {
            let result = actions::list_workflows(&token, &coords);
            tx.send(AppEvent::ActionsWorkflowsFetched { generation, result });
        });
    }

    /// A run of a "dynamic" workflow carries its own title as its name:
    /// the workflow name comes from the list of workflows.
    fn named(&self, mut run: Run) -> Run {
        if let Some(w) = self.workflows.iter().find(|w| w.id == run.workflow_id) {
            run.workflow = w.name.clone();
        }
        run
    }

    pub fn on_workflows_fetched(&mut self, generation: u64, result: Result<Vec<Workflow>, String>) {
        if generation != self.workflows_gen {
            return;
        }
        // Without names the runs keep their own: not worth an error.
        if let Ok(list) = result {
            self.workflows = list;
            let keep = self.selected_run().map(|r| r.id);
            let runs = std::mem::take(&mut self.runs);
            self.runs = runs.into_iter().map(|r| self.named(r)).collect();
            let renamed = self.page.as_ref().map(|p| self.named(p.run.clone()));
            if let (Some(page), Some(run)) = (self.page.as_mut(), renamed) {
                page.run = run;
            }
            self.refresh_visible(keep);
        }
    }

    pub fn on_runs_fetched(&mut self, generation: u64, result: Result<Vec<Run>, String>) {
        if generation != self.runs_gen {
            return;
        }
        self.runs_loading = false;
        match result {
            Ok(runs) => {
                self.last_error = None;
                // The key is read before the runs change: it is the run that
                // must keep the selection, not its index.
                let keep = self.selected_run().map(|r| r.id);
                let mut runs: Vec<Run> = runs.into_iter().map(|r| self.named(r)).collect();
                for run in &mut runs {
                    self.reconcile(run);
                }
                let first_page_full = runs.len() as u32 >= actions::LIST_PAGE;
                // The runs loaded by scrolling stay under the fresh first page.
                let (merged, kept_tail) = logic::merge_first_page(runs, &self.runs);
                if !kept_tail {
                    self.next_page = 2;
                    self.has_more = first_page_full;
                }
                self.runs = merged;
                self.refresh_visible(keep);
                self.resync_applied_search();
            }
            Err(e) => self.last_error = Some(e),
        }
    }

    // ─── The runs list ───────────────────────────────────────────

    fn workflow_names(&self) -> Vec<String> {
        let mut names: Vec<String> = if self.workflows.is_empty() {
            self.runs.iter().map(|r| r.workflow.clone()).collect()
        } else {
            self.workflows.iter().map(|w| w.name.clone()).collect()
        };
        names.sort();
        names.dedup();
        names
    }

    fn query_matches(&self, run: &Run) -> bool {
        if self.query.is_empty() {
            return true;
        }
        let q = self.query.to_lowercase();
        [
            run.title.as_str(),
            run.workflow.as_str(),
            run.branch.as_str(),
            run.actor.as_str(),
            run.event.as_str(),
        ]
        .iter()
        .any(|f| f.to_lowercase().contains(&q))
            || format!("#{}", run.number).contains(&q)
    }

    /// Whether a run passes everything but the status filter.
    fn passes_scope(&self, run: &Run) -> bool {
        self.workflow.as_ref().is_none_or(|w| *w == run.workflow) && self.query_matches(run)
    }

    fn count_for(&self, filter: RunFilter) -> usize {
        self.runs
            .iter()
            .filter(|r| self.passes_scope(r) && filter.matches(r))
            .count()
    }

    fn refresh_visible(&mut self, keep: Option<u64>) {
        self.visible = self
            .runs
            .iter()
            .enumerate()
            .filter(|(_, r)| self.passes_scope(r) && self.filter.matches(r))
            .map(|(i, _)| i)
            .collect();
        let found = keep.and_then(|id| self.visible.iter().position(|&i| self.runs[i].id == id));
        self.selected = found
            .unwrap_or(self.selected)
            .min(self.visible.len().saturating_sub(1));
    }

    /// The first list of runs is still on its way: nothing to count yet.
    fn first_load(&self) -> bool {
        self.runs_loading && self.runs.is_empty()
    }

    fn selected_run(&self) -> Option<&Run> {
        self.runs.get(*self.visible.get(self.selected)?)
    }

    fn reset_selection(&mut self) {
        self.selected = 0;
        self.offset = 0;
        self.free_scroll = false;
        self.refresh_visible(None);
        self.resync_applied_search();
    }

    /// The count in the footer follows the filters while a search is applied.
    fn resync_applied_search(&self) {
        if !self.query.is_empty() && !self.search_input && self.page.is_none() {
            self.sync_search_footer();
        }
    }

    fn set_filter(&mut self, filter: RunFilter) {
        self.filter = filter;
        self.reset_selection();
    }

    // ─── The run page ────────────────────────────────────────────

    fn open_page(&mut self, run: Run) {
        let run = self.named(run);
        self.page = Some(RunPage::new(run));
        let had_search = self.search_input || !self.query.is_empty();
        self.search_input = false;
        if had_search {
            // The search belongs to the list: its footer line goes away.
            self.tx.send(AppEvent::ClearStatusLine);
        }
        self.hovered_side = None;
        self.fetch_jobs();
        self.rebuild_panel();
    }

    fn close_page(&mut self) {
        self.page = None;
        self.hovered_side = None;
        if !self.query.is_empty() {
            self.sync_search_footer();
        }
        // What happened meanwhile (a re-run, a new run) shows on the list.
        self.fetch_runs();
    }

    fn fetch_page_run(&mut self) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        page.run_gen = next_generation();
        page.run_inflight = true;
        let (generation, run_id) = (page.run_gen, page.run.id);
        let (token, coords, tx) = (self.token.clone(), self.coords.clone(), self.tx.clone());
        crate::panic_guard::spawn_protected("actions-run", move || {
            let result = actions::get_run(&token, &coords, run_id);
            tx.send(AppEvent::ActionsRunFetched { generation, result });
        });
    }

    fn fetch_jobs(&mut self) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        page.jobs_gen = next_generation();
        page.jobs_inflight = true;
        if matches!(page.jobs, Load::NotLoaded) {
            page.jobs = Load::Loading;
        }
        let (generation, run_id) = (page.jobs_gen, page.run.id);
        let (token, coords, tx) = (self.token.clone(), self.coords.clone(), self.tx.clone());
        crate::panic_guard::spawn_protected("actions-jobs", move || {
            let result = actions::list_jobs(&token, &coords, run_id);
            tx.send(AppEvent::ActionsJobsFetched {
                generation,
                run_id,
                result,
            });
        });
    }

    fn fetch_logs(&mut self, job_idx: usize) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let Load::Loaded(jobs) = &mut page.jobs else {
            return;
        };
        let Some(job) = jobs.get_mut(job_idx) else {
            return;
        };
        job.log_gen = next_generation();
        job.log_inflight = true;
        if matches!(job.log, Load::NotLoaded) {
            job.log = Load::Loading;
        }
        let (generation, job_id) = (job.log_gen, job.job.id);
        let (token, coords, tx) = (self.token.clone(), self.coords.clone(), self.tx.clone());
        crate::panic_guard::spawn_protected("actions-logs", move || {
            let result = actions::fetch_job_logs_settled(&token, &coords, job_id);
            tx.send(AppEvent::ActionsLogsFetched {
                generation,
                job_id,
                result,
            });
        });
    }

    /// Fetches the log of a job that has an open step and none yet.
    fn ensure_logs(&mut self, job_idx: usize) {
        let needs = self.page.as_ref().is_some_and(|p| {
            p.job_nodes().get(job_idx).is_some_and(|j| {
                j.logs_ready()
                    && p.has_open_step(j)
                    && matches!(j.log, Load::NotLoaded | Load::Failed(_))
            })
        });
        if needs {
            self.fetch_logs(job_idx);
        }
    }

    fn fetch_annotations(&mut self) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let ids: Vec<u64> = page
            .job_nodes()
            .iter()
            .take(ANNOTATION_JOBS_MAX)
            .map(|j| j.job.id)
            .collect();
        if ids.is_empty() {
            return;
        }
        page.ann_gen = next_generation();
        page.ann_requested = true;
        page.ann_final = !page.run.status.is_active();
        let (generation, run_id) = (page.ann_gen, page.run.id);
        let (token, coords, tx) = (self.token.clone(), self.coords.clone(), self.tx.clone());
        crate::panic_guard::spawn_protected("actions-annotations", move || {
            let annotations = actions::list_annotations(&token, &coords, &ids);
            tx.send(AppEvent::ActionsAnnotationsFetched {
                generation,
                run_id,
                annotations,
            });
        });
    }

    pub fn on_run_fetched(&mut self, generation: u64, result: Result<Run, String>) {
        if let Some(page) = self.page.as_mut().filter(|p| p.run_gen == generation) {
            page.run_inflight = false;
        }
        let Some(mut run) = result.ok().map(|r| self.named(r)) else {
            return;
        };
        self.reconcile(&mut run);
        let now = chrono::Utc::now();
        let mut ended = false;
        let mut wants_annotations = false;
        if let Some(page) = self.page.as_mut().filter(|p| p.run_gen == generation) {
            ended = page.run.is_live(now) && !run.is_live(now);
            page.run = run;
            page.settle_annotations_flag();
            wants_annotations =
                page.ann_requested && !page.run.status.is_active() && !page.ann_final;
        }
        if ended {
            // One last look at what the end of the run left behind.
            self.reload_at = Some(Instant::now() + Duration::from_secs(2));
        }
        if wants_annotations {
            self.fetch_annotations();
        }
    }

    pub fn on_jobs_fetched(
        &mut self,
        generation: u64,
        run_id: u64,
        result: Result<Vec<Job>, String>,
    ) {
        let Some(page) = self
            .page
            .as_mut()
            .filter(|p| p.run.id == run_id && p.jobs_gen == generation)
        else {
            return;
        };
        page.jobs_inflight = false;
        match result {
            Ok(jobs) => {
                page.warned = false;
                let mut previous: FxHashMap<u64, JobNode> =
                    match std::mem::replace(&mut page.jobs, Load::NotLoaded) {
                        Load::Loaded(old) => old.into_iter().map(|j| (j.job.id, j)).collect(),
                        _ => FxHashMap::default(),
                    };
                // Jobs that ended since the last answer: their log can be read now.
                let mut finished: Vec<u64> = Vec::new();
                let nodes: Vec<JobNode> = jobs
                    .into_iter()
                    .map(|job| match previous.remove(&job.id) {
                        Some(mut old) => {
                            let was_active = old.job.status.is_active();
                            old.job = job;
                            old.refit();
                            if was_active && !old.job.status.is_active() {
                                finished.push(old.job.id);
                            }
                            old
                        }
                        None => JobNode {
                            job,
                            log: Load::NotLoaded,
                            log_gen: 0,
                            log_inflight: false,
                        },
                    })
                    .collect();
                page.jobs = Load::Loaded(nodes);
                // What failed in a job that just ended opens by itself, like
                // on a job seen after the run.
                for id in &finished {
                    page.opened_jobs.remove(id);
                }
                page.entry = page.entry.min(page.entry_count() - 1);
                page.settle_annotations_flag();
                let wants_annotations =
                    !page.ann_requested || (!page.run.status.is_active() && !page.ann_final);
                let entry = page.entry;
                if wants_annotations {
                    self.fetch_annotations();
                }
                if entry > 0 {
                    self.open_failed_steps(entry - 1);
                }
                // The log of a job that has just ended comes by itself: no
                // key to press, no waiting for the next poll.
                for id in finished {
                    let idx = self
                        .page
                        .as_ref()
                        .and_then(|p| p.job_nodes().iter().position(|j| j.job.id == id));
                    if let Some(idx) = idx {
                        self.ensure_logs(idx);
                    }
                }
                self.rebuild_panel();
            }
            Err(e) => {
                if matches!(page.jobs, Load::Loaded(_)) {
                    // Keep what is shown: a flaky network must not empty it.
                    if !page.warned {
                        page.warned = true;
                        self.tx.send(AppEvent::NotifyWarn(e));
                    }
                } else {
                    page.jobs = Load::Failed(e);
                }
                self.rebuild_panel();
            }
        }
    }

    pub fn on_annotations_fetched(
        &mut self,
        generation: u64,
        run_id: u64,
        annotations: Vec<Annotation>,
    ) {
        if let Some(page) = self
            .page
            .as_mut()
            .filter(|p| p.run.id == run_id && p.ann_gen == generation)
        {
            page.annotations = annotations;
            self.rebuild_panel();
        }
    }

    pub fn on_logs_fetched(
        &mut self,
        generation: u64,
        job_id: u64,
        result: Result<JobLog, String>,
    ) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let Load::Loaded(jobs) = &mut page.jobs else {
            return;
        };
        let Some(job) = jobs
            .iter_mut()
            .find(|j| j.job.id == job_id && j.log_gen == generation)
        else {
            return;
        };
        job.log_inflight = false;
        match result {
            Ok(log) => {
                page.warned = false;
                let lines = parse_log(&log.lines);
                let ranges = assign_ranges(&lines, &job.job.steps);
                if let Some(w) = log.warning {
                    self.tx.send(AppEvent::NotifyWarn(w));
                }
                job.log = Load::Loaded(LogData { lines, ranges });
            }
            Err(e) => {
                if matches!(job.log, Load::Loaded(_)) {
                    if !page.warned {
                        page.warned = true;
                        self.tx.send(AppEvent::NotifyWarn(e));
                    }
                } else {
                    job.log = Load::Failed(e);
                }
            }
        }
        self.rebuild_panel();
    }

    pub fn on_action_done(&mut self, label: String, run_id: u64, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.tx
                    .send(AppEvent::NotifySuccess(format!("{label}: requested")));
                let cancel = label == "Cancel";
                let attempt_before = self
                    .page
                    .as_ref()
                    .map(|p| &p.run)
                    .filter(|r| r.id == run_id)
                    .or_else(|| self.runs.iter().find(|r| r.id == run_id))
                    .map_or(1, |r| r.attempt);
                if !cancel {
                    self.mark_rerun(run_id);
                }
                let now = Instant::now();
                self.pending = Some(Pending {
                    run_id,
                    cancel,
                    attempt_before,
                    deadline: now + Duration::from_secs(40),
                    step: 0,
                });
                // GitHub takes a moment: look again soon, a few times.
                self.reload_at = Some(now + Duration::from_secs(FOLLOW_UP[0]));
            }
            Err(e) => self.tx.send(AppEvent::NotifyError(e)),
        }
    }

    /// A re-run starts queued: show it before GitHub says so.
    fn mark_rerun(&mut self, run_id: u64) {
        let queue = |run: &mut Run| {
            run.status = RunStatus::Queued;
            run.conclusion = None;
            run.attempt += 1;
        };
        let keep = self.selected_run().map(|r| r.id);
        if let Some(run) = self.runs.iter_mut().find(|r| r.id == run_id) {
            queue(run);
        }
        if let Some(page) = self.page.as_mut().filter(|p| p.run.id == run_id) {
            queue(&mut page.run);
        }
        self.refresh_visible(keep);
    }

    /// An answer that still shows the state from before the action must not
    /// undo what the user just asked for.
    fn reconcile(&mut self, fresh: &mut Run) {
        let Some(p) = &self.pending else {
            return;
        };
        if p.reconcile(fresh) {
            self.pending = None;
            // The change is visible: no more quick checks, the normal
            // refresh takes over.
            self.reload_at = None;
        }
    }

    /// Like GitHub: what failed or still runs is open, the first time a job
    /// is shown.
    fn open_failed_steps(&mut self, job_idx: usize) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let Some((id, numbers, has_steps)) = page.job_nodes().get(job_idx).map(|jn| {
            let numbers: Vec<u64> = jn
                .job
                .steps
                .iter()
                .filter(|s| {
                    s.conclusion.is_some_and(RunConclusion::is_failure)
                        || matches!(s.status, RunStatus::InProgress)
                })
                .map(|s| s.number)
                .collect();
            (jn.job.id, numbers, !jn.job.steps.is_empty())
        }) else {
            return;
        };
        // A job still queued has no steps: wait for them before calling it seen.
        if has_steps && page.opened_jobs.insert(id) {
            for n in numbers {
                page.expanded_steps.insert((id, n));
            }
        }
        self.ensure_logs(job_idx);
    }

    fn select_entry(&mut self, entry: usize) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let entry = entry.min(page.entry_count() - 1);
        if entry == page.entry {
            return;
        }
        page.entry = entry;
        page.selected = 0;
        page.offset = 0;
        page.free_scroll = false;
        // The old rows belong to the old entry: no selection to carry over.
        page.rows.clear();
        if entry > 0 {
            self.open_failed_steps(entry - 1);
        }
        self.rebuild_panel();
    }

    fn rebuild_panel(&mut self) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let keep = page.rows.get(page.selected).map(|r| page.key_of(*r));
        let rows = page.build_rows();
        page.rows = rows;
        if let Some(key) = keep {
            if let Some(i) = page.rows.iter().position(|r| page.key_of(*r) == key) {
                page.selected = i;
            }
        }
        page.selected = page.selected.min(page.rows.len().saturating_sub(1));
    }

    // ─── Reloading ───────────────────────────────────────────────

    /// The file watcher asks for it: never on top of requests in flight.
    pub fn refresh(&mut self) {
        if self.runs_loading || self.page.as_ref().is_some_and(RunPage::any_inflight) {
            return;
        }
        self.reload();
    }

    /// Reloads the list, or the page and every log that is open.
    fn reload(&mut self) {
        self.last_refresh = Instant::now();
        if self.page.is_none() {
            self.fetch_runs();
            if self.workflows.is_empty() {
                self.fetch_workflows();
            }
            return;
        }
        self.fetch_page_run();
        self.fetch_jobs();
        // `r` refreshes everything on the page, the annotations included.
        self.fetch_annotations();
        // The log of a finished job never changes: only the ones that are not
        // there yet are fetched, not all of them at every poll.
        let open: Vec<usize> = self.page.as_ref().map_or_else(Vec::new, |p| {
            p.job_nodes()
                .iter()
                .enumerate()
                .filter(|(_, j)| p.has_open_step(j))
                .map(|(i, _)| i)
                .collect()
        });
        for i in open {
            self.ensure_logs(i);
        }
    }

    /// Called by the app loop: reloads by itself while something runs.
    pub fn tick(&mut self, now: Instant) {
        if self.reload_at.is_some_and(|at| now >= at) {
            self.reload_at = None;
            self.reload();
            // After an action: a few more short looks until GitHub shows it.
            if let Some(p) = self.pending.as_mut() {
                p.step += 1;
                if p.step < FOLLOW_UP.len() && now < p.deadline {
                    self.reload_at = Some(now + Duration::from_secs(FOLLOW_UP[p.step]));
                } else {
                    self.pending = None;
                }
            }
            return;
        }
        if now.saturating_duration_since(self.last_refresh) < REFRESH_EVERY {
            return;
        }
        let busy = self.runs_loading || self.page.as_ref().is_some_and(RunPage::any_inflight);
        if !busy && self.something_runs() {
            self.reload();
        }
    }

    /// Whether what is on screen is really running: worth polling.
    fn something_runs(&self) -> bool {
        let now = chrono::Utc::now();
        match &self.page {
            Some(page) => page.run.is_live(now),
            None => self.runs.iter().any(|r| r.is_live(now)),
        }
    }

    /// A running icon is on screen: the app redraws on every tick.
    pub fn is_animating(&self) -> bool {
        self.something_runs()
    }

    /// True when the loop must keep waking up for this view.
    pub fn wants_ticks(&self) -> bool {
        self.reload_at.is_some() || self.something_runs()
    }

    // ─── Keys ────────────────────────────────────────────────────

    pub fn handle_event(&mut self, event_with_count: UserEventWithCount, key: KeyEvent) {
        if self.search_input {
            self.handle_search_key(key);
            return;
        }
        if self.picker.is_some() {
            // The key that opened the box closes it.
            let toggles = self
                .ctx
                .keybind
                .resolve_scoped(&["actions"], key)
                .is_some_and(|action| action == "switch");
            if toggles {
                // Like Enter: the filter that is shown stays.
                self.close_picker(false);
            } else {
                self.handle_picker_event(event_with_count.event);
            }
            return;
        }
        let action = self
            .ctx
            .keybind
            .resolve_scoped(&["actions"], key)
            .map(str::to_string);
        if let Some(action) = action {
            if self.handle_action(&action) {
                return;
            }
        }
        let count = event_with_count.count.clamp(1, 100_000) as isize;
        let page = (self.body_height.max(2) - 1) as isize;
        match event_with_count.event {
            UserEvent::NavigateUp | UserEvent::SelectUp => self.move_by(-count),
            UserEvent::NavigateDown | UserEvent::SelectDown => self.move_by(count),
            // The wheel scrolls the view, the cursor stays where it is.
            UserEvent::ScrollUp => self.scroll_view(-count),
            UserEvent::ScrollDown => self.scroll_view(count),
            UserEvent::PageUp => self.move_by(-page * count),
            UserEvent::PageDown => self.move_by(page * count),
            UserEvent::HalfPageUp => self.move_by(-(page / 2).max(1) * count),
            UserEvent::HalfPageDown => self.move_by((page / 2).max(1) * count),
            UserEvent::GoToTop => self.move_by(isize::MIN / 2),
            UserEvent::GoToBottom => self.move_by(isize::MAX / 2),
            UserEvent::NavigateRight => self.horizontal(true),
            UserEvent::NavigateLeft => self.horizontal(false),
            UserEvent::Confirm => self.enter(),
            UserEvent::Cancel | UserEvent::Close => self.back(),
            _ => {}
        }
    }

    /// Scoped actions; false when the action does not apply.
    fn handle_action(&mut self, action: &str) -> bool {
        match action {
            "reload" => self.reload(),
            "open_web" => self.open_web(),
            "rerun" | "rerun_failed" | "cancel" => self.request(action),
            "current_branch" if self.page.is_none() => {
                if self.branch.is_none() {
                    self.tx.send(AppEvent::NotifyWarn(
                        "No current branch (detached HEAD)".into(),
                    ));
                } else {
                    self.only_branch = !self.only_branch;
                    // Another list: what was loaded does not apply any more.
                    self.runs.clear();
                    self.has_more = false;
                    self.next_page = 2;
                    // A page of the old list still on its way must be ignored.
                    self.more_gen = next_generation();
                    self.loading_more = false;
                    self.reset_selection();
                    self.fetch_runs();
                }
            }
            "search" if self.page.is_none() => {
                self.search = tui_input::Input::new(self.query.clone());
                self.search_input = true;
                self.sync_search_footer();
            }
            "switch" => self.switch(),
            "timestamps" => self.show_timestamps = !self.show_timestamps,
            "copy" => self.copy_selected(),
            "next_error" => self.jump_to_error(true),
            "prev_error" => self.jump_to_error(false),
            _ => return false,
        }
        true
    }

    /// Tab: the workflow picker on the list, the jobs list on the run page.
    fn switch(&mut self) {
        match self.page.as_mut() {
            None => {
                let names = self.workflow_names();
                let at = self
                    .workflow
                    .as_ref()
                    .and_then(|w| names.iter().position(|n| n == w))
                    .map_or(0, |i| i + 1);
                self.picker_before = self.workflow.clone();
                self.picker = Some(at);
            }
            Some(page) => {
                if self.layout_wide {
                    page.focus = match page.focus {
                        Focus::Side => Focus::Panel,
                        Focus::Panel => Focus::Side,
                    };
                }
            }
        }
    }

    /// The search lives in the footer, like the search of the commit list:
    /// `Search: text` with the cursor while typing, then the number of
    /// matches once it is applied.
    fn sync_search_footer(&self) {
        if self.search_input {
            let cursor = self.search.visual_cursor() as u16 + 8;
            self.tx.send(AppEvent::UpdateStatusInput(
                format!("Search: {}", self.query),
                Some(cursor),
                None,
            ));
        } else if !self.query.is_empty() && self.page.is_none() {
            let n = self.visible.len();
            let (msg, warn) = if n == 0 {
                (
                    format!("No matches found (query: \"{}\")", self.query),
                    true,
                )
            } else {
                (format!("{n} runs match (query: \"{}\")", self.query), false)
            };
            self.tx.send(AppEvent::SetSearchStatus { msg, warn });
        } else {
            self.tx.send(AppEvent::ClearStatusLine);
        }
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.search_input = false;
                self.query.clear();
                self.search = tui_input::Input::default();
            }
            KeyCode::Enter => {
                self.search_input = false;
                self.sync_search_footer();
                return;
            }
            _ => {
                crate::text_edit::edit_input(&mut self.search, &key);
                let typed = self.search.value().to_string();
                if typed == self.query {
                    // Only the cursor moved: show it, keep the selection.
                    self.sync_search_footer();
                    return;
                }
                self.query = typed;
            }
        }
        self.reset_selection();
        self.sync_search_footer();
    }

    /// The filter follows the cursor of the picker: no need to press Enter.
    fn move_picker(&mut self, cursor: usize) {
        let names = self.workflow_names();
        let cursor = cursor.min(names.len());
        if self.picker == Some(cursor) {
            return;
        }
        self.picker = Some(cursor);
        self.workflow = cursor.checked_sub(1).and_then(|i| names.get(i).cloned());
        self.reset_selection();
    }

    /// Closes the picker. `cancel` puts back the filter it was opened with.
    fn close_picker(&mut self, cancel: bool) {
        if self.picker.take().is_none() {
            return;
        }
        let before = self.picker_before.take();
        if cancel && self.workflow != before {
            self.workflow = before;
            self.reset_selection();
        }
    }

    fn handle_picker_event(&mut self, event: UserEvent) {
        let len = self.workflow_names().len() + 1;
        let Some(cursor) = self.picker else {
            return;
        };
        match event {
            UserEvent::NavigateUp | UserEvent::SelectUp | UserEvent::ScrollUp => {
                self.move_picker(cursor.saturating_sub(1));
            }
            UserEvent::NavigateDown | UserEvent::SelectDown | UserEvent::ScrollDown => {
                self.move_picker((cursor + 1).min(len - 1));
            }
            UserEvent::GoToTop => self.move_picker(0),
            UserEvent::GoToBottom => self.move_picker(len - 1),
            UserEvent::Confirm => self.close_picker(false),
            UserEvent::Cancel | UserEvent::Close => self.close_picker(true),
            _ => {}
        }
    }

    fn side_active(&self) -> bool {
        self.layout_wide && self.page.as_ref().is_some_and(|p| p.focus == Focus::Side)
    }

    /// Left and right: the status tabs on the list, the panes on a run.
    fn horizontal(&mut self, right: bool) {
        let Some(page) = self.page.as_mut() else {
            let next = if right {
                self.filter.next()
            } else {
                self.filter.prev()
            };
            self.set_filter(next);
            return;
        };
        if self.layout_wide {
            page.focus = if right { Focus::Panel } else { Focus::Side };
        } else {
            let entry = if right {
                page.entry + 1
            } else {
                page.entry.saturating_sub(1)
            };
            self.select_entry(entry);
        }
    }

    /// Enter: opens the run, or toggles the selected accordion row.
    fn enter(&mut self) {
        if self.page.is_none() {
            if let Some(run) = self.selected_run().cloned() {
                self.open_page(run);
            }
            return;
        }
        if self.side_active() {
            if let Some(page) = self.page.as_mut() {
                page.focus = Focus::Panel;
            }
            return;
        }
        self.toggle_selected();
    }

    /// Esc: from a run back to the list, from the list out of the page.
    fn back(&mut self) {
        if !self.query.is_empty() && self.page.is_none() {
            self.query.clear();
            self.reset_selection();
            self.sync_search_footer();
        } else if self.page.is_some() {
            self.close_page();
        } else {
            self.tx.send(AppEvent::CloseActions);
        }
    }

    fn toggle_selected(&mut self) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let Some(row) = page.rows.get(page.selected).copied() else {
            return;
        };
        // Acting on a row brings the view back to it.
        page.free_scroll = false;
        let mut fetch = None;
        match row {
            PRow::AnnHeader => {
                let (_, owner) = page.entry_annotations();
                if !page.ann_open.remove(&owner) {
                    page.ann_open.insert(owner);
                }
            }
            PRow::Step(si) => {
                let Some((id, number)) = page
                    .entry_job()
                    .and_then(|jn| jn.job.steps.get(si).map(|s| (jn.job.id, s.number)))
                else {
                    return;
                };
                if !page.expanded_steps.remove(&(id, number)) {
                    page.expanded_steps.insert((id, number));
                    fetch = page.entry.checked_sub(1);
                }
            }
            PRow::Line(_, li) => {
                let Some(jn) = page.entry_job() else {
                    return;
                };
                let id = jn.job.id;
                let is_group = matches!(
                    &jn.log,
                    Load::Loaded(d) if d.lines.get(li).is_some_and(|l| l.kind == LogKind::Group)
                );
                if is_group && !page.open_groups.remove(&(id, li)) {
                    page.open_groups.insert((id, li));
                }
            }
            PRow::Annotation(_) | PRow::Note(_) => return,
        }
        if let Some(job_idx) = fetch {
            self.ensure_logs(job_idx);
        }
        self.rebuild_panel();
    }

    /// `c`: puts the selected line (a log line, a step name or an annotation)
    /// on the clipboard.
    fn copy_selected(&mut self) {
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let text = match page.rows.get(page.selected).copied() {
            Some(PRow::Line(_, li)) => page.entry_job().and_then(|jn| match &jn.log {
                Load::Loaded(d) => d.lines.get(li).map(|l| logic::plain_log_text(&l.text)),
                _ => None,
            }),
            Some(PRow::Step(si)) => page
                .entry_job()
                .and_then(|jn| jn.job.steps.get(si).map(|s| s.name.clone())),
            Some(PRow::Annotation(a)) => page.annotations.get(a).map(|a| a.message.clone()),
            _ => None,
        };
        match text.filter(|t| !t.is_empty()) {
            Some(value) => self.tx.send(AppEvent::CopyRawToClipboard {
                value,
                success_message: "Copied the line to the clipboard".into(),
            }),
            None => self
                .tx
                .send(AppEvent::NotifyInfo("Nothing to copy here".into())),
        }
    }

    fn jump_to_error(&mut self, forward: bool) {
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let found = if forward {
            (page.selected + 1..page.rows.len()).find(|&i| page.is_error_row(page.rows[i]))
        } else {
            (0..page.selected)
                .rev()
                .find(|&i| page.is_error_row(page.rows[i]))
        };
        match found {
            Some(i) => {
                page.selected = i;
                page.free_scroll = false;
            }
            None => self.tx.send(AppEvent::NotifyInfo(
                "No more errors in the unfolded logs".into(),
            )),
        }
    }

    fn target_run(&self) -> Option<&Run> {
        match &self.page {
            Some(page) => Some(&page.run),
            None => self.selected_run(),
        }
    }

    /// Asks for confirmation in the same dialog as the rest of the app, after
    /// checking the run is in the right state.
    fn request(&mut self, action: &str) {
        let Some(run) = self.target_run() else {
            return;
        };
        let active = run.status.is_active();
        let kind = match action {
            "cancel" if !active => {
                self.tx
                    .send(AppEvent::NotifyWarn("This run is not running".into()));
                return;
            }
            "cancel" => ActionsRunKind::Cancel,
            _ if active => {
                self.tx
                    .send(AppEvent::NotifyWarn("This run is still in progress".into()));
                return;
            }
            "rerun_failed" => ActionsRunKind::RerunFailed,
            _ => ActionsRunKind::Rerun,
        };
        let title = if run.title.is_empty() {
            run.workflow.clone()
        } else {
            run.title.clone()
        };
        self.tx
            .send(AppEvent::OpenDialog(DialogKind::ConfirmActionsRun {
                run_id: run.id,
                workflow: run.workflow.clone(),
                number: run.number,
                title,
                kind,
            }));
    }

    /// The user validated the dialog: ask GitHub.
    pub fn perform_confirmed(&mut self, id: u64, kind: ActionsRunKind) {
        let (token, coords, tx) = (self.token.clone(), self.coords.clone(), self.tx.clone());
        crate::panic_guard::spawn_protected("actions-write", move || {
            let (label, result) = match kind {
                ActionsRunKind::Rerun => ("Re-run", actions::rerun(&token, &coords, id)),
                ActionsRunKind::RerunFailed => (
                    "Re-run failed jobs",
                    actions::rerun_failed(&token, &coords, id),
                ),
                ActionsRunKind::Cancel => ("Cancel", actions::cancel(&token, &coords, id)),
            };
            tx.send(AppEvent::ActionsActionDone {
                label: label.to_string(),
                run_id: id,
                result,
            });
        });
    }

    fn open_web(&mut self) {
        let url = match &self.page {
            Some(page) => page
                .entry_job()
                .map(|j| j.job.url.clone())
                .or_else(|| Some(page.run.url.clone())),
            None => self.selected_run().map(|r| r.url.clone()),
        };
        let Some(url) = url.filter(|u| !u.is_empty()) else {
            return;
        };
        match crate::external::open_url(&url) {
            Ok(()) => self
                .tx
                .send(AppEvent::NotifySuccess(format!("Opened {url}"))),
            Err(e) => self.tx.send(AppEvent::NotifyError(e)),
        }
    }

    fn move_by(&mut self, delta: isize) {
        let clamp = |current: usize, len: usize| -> usize {
            (current as isize)
                .saturating_add(delta)
                .clamp(0, len.saturating_sub(1) as isize) as usize
        };
        if self.side_active() {
            let next = self.page.as_ref().map(|p| clamp(p.entry, p.entry_count()));
            if let Some(next) = next {
                self.select_entry(next);
            }
            return;
        }
        match self.page.as_mut() {
            Some(page) => {
                page.selected = clamp(page.selected, page.rows.len());
                page.free_scroll = false;
            }
            None => {
                self.selected = clamp(self.selected, self.visible.len());
                self.free_scroll = false;
                // On the last line of what is loaded: fetch the next page. Nothing
                // is loaded ahead in the background, no cache to keep.
                if delta > 0 && self.selected + 1 >= self.visible.len() {
                    self.maybe_load_more();
                }
            }
        }
    }

    /// Asks for the next page of runs when there is one and none is on its way.
    fn maybe_load_more(&mut self) {
        if !self.has_more || self.loading_more || self.runs_loading || self.page.is_some() {
            return;
        }
        self.loading_more = true;
        self.more_gen = next_generation();
        let (generation, page) = (self.more_gen, self.next_page);
        let (token, coords, tx) = (self.token.clone(), self.coords.clone(), self.tx.clone());
        let branch = if self.only_branch {
            self.branch.clone()
        } else {
            None
        };
        crate::panic_guard::spawn_protected("actions-runs-more", move || {
            let result = actions::list_runs_page(
                &token,
                &coords,
                branch.as_deref(),
                actions::LIST_PAGE,
                page,
            );
            tx.send(AppEvent::ActionsMoreRunsFetched { generation, result });
        });
    }

    pub fn on_more_runs_fetched(&mut self, generation: u64, result: Result<Vec<Run>, String>) {
        if generation != self.more_gen {
            return;
        }
        self.loading_more = false;
        match result {
            Ok(list) => {
                let keep = self.selected_run().map(|r| r.id);
                self.has_more = list.len() as u32 >= actions::LIST_PAGE;
                self.next_page += 1;
                let known: FxHashSet<u64> = self.runs.iter().map(|r| r.id).collect();
                let added: Vec<Run> = list
                    .into_iter()
                    .filter(|r| !known.contains(&r.id))
                    .map(|r| self.named(r))
                    .collect();
                self.runs.extend(added);
                self.refresh_visible(keep);
                self.resync_applied_search();
            }
            // Try again at the next key at the end of the list.
            Err(e) => self.tx.send(AppEvent::NotifyWarn(e)),
        }
    }

    /// The wheel: moves the visible window, not the cursor. The render caps
    /// the offset so the last row stops at the bottom.
    fn scroll_view(&mut self, delta: isize) {
        let shift = |offset: usize| (offset as isize).saturating_add(delta).max(0) as usize;
        match self.page.as_mut() {
            Some(page) => {
                page.offset = shift(page.offset).min(page.rows.len().saturating_sub(1));
                page.free_scroll = true;
            }
            None => {
                self.offset = shift(self.offset).min(self.visible.len().saturating_sub(1));
                self.free_scroll = true;
                // The wheel reached the last line: same thing.
                let usable = self.body_height.saturating_sub(1) as usize;
                if delta > 0 && self.offset + usable >= self.visible.len() {
                    self.maybe_load_more();
                }
            }
        }
    }

    // ─── Mouse ───────────────────────────────────────────────────

    pub fn handle_click(&mut self, col: u16, row: u16) {
        let hit = |rects: &[(usize, Rect)]| rect_hit(rects, col, row);
        if self.picker.is_some() {
            // A click on a workflow chooses it, anywhere else closes the box.
            match hit(&self.picker_rects) {
                Some(i) => {
                    self.move_picker(i);
                    self.close_picker(false);
                }
                None => self.close_picker(true),
            }
            return;
        }
        if self.page.is_none() {
            if let Some((f, _)) = self
                .filter_tab_rects
                .iter()
                .find(|(_, r)| render::rect_contains(*r, col, row))
                .copied()
            {
                self.set_filter(f);
                return;
            }
            if let Some(i) = hit(&self.row_rects) {
                self.selected = i;
                self.enter();
            }
            return;
        }
        if let Some(i) = hit(&self.side_rects) {
            if let Some(page) = self.page.as_mut() {
                page.focus = Focus::Side;
            }
            self.select_entry(i);
            return;
        }
        if let Some(i) = hit(&self.panel_rects) {
            if let Some(page) = self.page.as_mut() {
                page.focus = Focus::Panel;
                page.selected = i;
            }
            self.toggle_selected();
        }
    }

    pub fn handle_mouse_move(&mut self, col: u16, row: u16) {
        // The box is modal: the row under the mouse takes the cursor.
        if self.picker.is_some() {
            if let Some(i) = rect_hit(&self.picker_rects, col, row) {
                self.move_picker(i);
            }
            return;
        }
        self.hovered_filter = self
            .filter_tab_rects
            .iter()
            .find(|(_, r)| render::rect_contains(*r, col, row))
            .map(|(f, _)| *f);
        if self.page.is_none() {
            // Like the other lists: the row under the mouse is the cursor.
            if let Some(i) = rect_hit(&self.row_rects, col, row) {
                self.selected = i;
            }
            return;
        }
        self.hovered_side = rect_hit(&self.side_rects, col, row);
        if let Some(i) = rect_hit(&self.panel_rects, col, row) {
            if let Some(page) = self.page.as_mut() {
                page.focus = Focus::Panel;
                page.selected = i;
            }
        }
    }

    // ─── Footer and lifecycle no-ops ─────────────────────────────

    pub fn footer_hint(&self) -> String {
        let kb = &self.ctx.keybind;
        let scoped = |action: &str, label: &str| {
            let key = kb.primary_scoped_key(&["actions"], action);
            if key.is_empty() {
                label.to_string()
            } else {
                format!("{key}:{label}")
            }
        };
        if self.search_input || self.picker.is_some() {
            return String::new();
        }
        let mut parts: Vec<String> = Vec::new();
        // What can be done to the run in front of you depends on its state:
        // cancel while it runs, re-run once it is over, and re-run only the
        // failed jobs when some failed.
        let mut run_keys = Vec::new();
        if let Some(run) = self.target_run() {
            if run.status.is_active() {
                run_keys.push(scoped("cancel", "cancel"));
            } else {
                run_keys.push(scoped("rerun", "re-run"));
                if run.conclusion.is_some_and(RunConclusion::is_failure) {
                    run_keys.push(scoped("rerun_failed", "re-run failed"));
                }
            }
        }
        match &self.page {
            None => {
                parts.push(scoped("switch", "workflow"));
                parts.push(scoped("search", "search"));
                parts.push(scoped("current_branch", "branch"));
                parts.extend(run_keys);
            }
            Some(page) => {
                let in_log = page
                    .rows
                    .get(page.selected)
                    .is_some_and(|r| matches!(r, PRow::Step(_) | PRow::Line(..)));
                if self.layout_wide {
                    parts.push(scoped("switch", "jobs"));
                }
                if in_log {
                    parts.push(scoped("copy", "copy"));
                    parts.push(scoped("timestamps", "timestamps"));
                }
                parts.extend(run_keys);
            }
        }
        parts.push(scoped("open_web", "web"));
        parts.push(scoped("reload", "reload"));
        format!("⌘ {}", parts.join("▕▏"))
    }

    pub fn update_layout(&mut self, _area: Rect) {}
    pub fn prepare_graph_uploads(&mut self) {}
    pub fn drain_pending_graph_uploads(&mut self) -> Vec<String> {
        Vec::new()
    }
    pub fn graph_image_ids_sorted(&self) -> Vec<u32> {
        Vec::new()
    }
    pub fn update_color_theme(&mut self, _theme: crate::color::ColorTheme) {}

    /// While a box or the search field is open every key is read by the view.
    pub fn is_input_active(&self) -> bool {
        self.search_input || self.picker.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::actions::{parse_runs, AnnotationLevel, Step};

    fn run() -> Run {
        parse_runs(
            r#"{"workflow_runs":[{"id":1,"name":"Deny","status":"completed","conclusion":"success",
            "created_at":"2026-11-16T10:00:00Z","updated_at":"2026-11-16T10:01:00Z"}]}"#,
        )
        .unwrap()
        .remove(0)
    }

    fn step(number: u64) -> Step {
        Step {
            number,
            name: format!("step {number}"),
            status: RunStatus::Completed,
            conclusion: Some(RunConclusion::Success),
            started_at: None,
            completed_at: None,
        }
    }

    fn job_node(id: u64, steps: u64, log: Load<LogData>) -> JobNode {
        JobNode {
            job: Job {
                id,
                name: format!("job {id}"),
                status: RunStatus::Completed,
                conclusion: Some(RunConclusion::Success),
                started_at: None,
                completed_at: None,
                url: String::new(),
                steps: (1..=steps).map(step).collect(),
            },
            log,
            log_gen: 0,
            log_inflight: false,
        }
    }

    fn annotation(job_id: u64) -> Annotation {
        Annotation {
            job_id,
            level: AnnotationLevel::Notice,
            title: String::new(),
            message: "a rather long message that has to wrap on a narrow panel".into(),
            path: String::new(),
            line: 1,
        }
    }

    fn page_with(jobs: Vec<JobNode>) -> RunPage {
        let mut page = RunPage::new(run());
        page.jobs = Load::Loaded(jobs);
        page
    }

    #[test]
    fn the_summary_lists_the_annotations_of_every_job_and_folds_them() {
        let mut page = page_with(vec![
            job_node(10, 1, Load::NotLoaded),
            job_node(11, 1, Load::NotLoaded),
        ]);
        page.annotations = vec![annotation(10), annotation(11)];
        assert_eq!(
            page.build_rows(),
            vec![PRow::AnnHeader, PRow::Annotation(0), PRow::Annotation(1)]
        );
        page.ann_open.clear();
        assert_eq!(page.build_rows(), vec![PRow::AnnHeader]);
    }

    #[test]
    fn a_job_shows_its_own_annotations_then_its_steps_and_open_logs() {
        let lines = logic::parse_log(
            &[
                "2026-11-16T10:00:00Z first",
                "2026-11-16T10:00:00Z ##[group]Details",
                "2026-11-16T10:00:00Z hidden",
                "2026-11-16T10:00:00Z ##[endgroup]",
                "2026-11-16T10:00:00Z last",
            ]
            .map(String::from),
        );
        let data = LogData {
            lines,
            ranges: vec![0..5, 0..0, 0..0],
        };
        let mut page = page_with(vec![job_node(10, 3, Load::Loaded(data))]);
        page.annotations = vec![annotation(99), annotation(10)];
        page.entry = 1;
        page.expanded_steps.insert((10, 1));
        assert_eq!(
            page.build_rows(),
            vec![
                PRow::AnnHeader,
                PRow::Step(0),
                PRow::Line(0, 0),
                PRow::Line(0, 1),
                PRow::Line(0, 4),
                PRow::Step(1),
                PRow::Step(2),
            ],
            "the annotation of another job is not shown, the group is closed"
        );
        page.open_groups.insert((10, 1));
        let opened = page.build_rows();
        assert!(opened.contains(&PRow::Line(0, 2)), "{opened:?}");
        assert!(
            !opened.contains(&PRow::Line(0, 3)),
            "the endgroup marker never shows"
        );
    }

    #[test]
    fn an_open_step_with_no_log_yet_says_so() {
        let mut page = page_with(vec![job_node(10, 1, Load::Loading)]);
        page.entry = 1;
        page.expanded_steps.insert((10, 1));
        assert_eq!(
            page.build_rows(),
            vec![PRow::Step(0), PRow::Note(Note::Text("Loading logs…"))]
        );
        page.entry = 2; // a job that does not exist (yet)
        assert!(page.entry_job().is_none());
        assert!(page.build_rows().is_empty() || matches!(page.build_rows()[0], PRow::Note(_)));
    }

    #[test]
    fn new_steps_get_their_lines_when_a_poll_brings_them() {
        let lines = logic::parse_log(
            &["2026-11-16T10:00:00Z setup", "2026-11-16T10:00:05Z build"].map(String::from),
        );
        let mut node = job_node(10, 1, Load::NotLoaded);
        node.job.steps[0].started_at = Some("2026-11-16T10:00:00Z".into());
        node.log = Load::Loaded(LogData {
            ranges: assign_ranges(&lines, &node.job.steps),
            lines,
        });
        // The poll finds a second step that started at 10:00:05.
        let mut second = step(2);
        second.started_at = Some("2026-11-16T10:00:05Z".into());
        node.job.steps.push(second);
        node.refit();
        let Load::Loaded(data) = &node.log else {
            panic!("log lost");
        };
        assert_eq!(data.ranges, vec![0..1, 1..2]);
    }

    #[test]
    fn a_rerun_lets_the_annotations_be_read_again() {
        let mut page = page_with(vec![job_node(10, 1, Load::NotLoaded)]);
        page.ann_final = true;
        page.settle_annotations_flag();
        assert!(page.ann_final, "a finished run keeps its final annotations");
        page.run.status = RunStatus::InProgress;
        page.settle_annotations_flag();
        assert!(!page.ann_final, "an active run will have new ones");
    }

    fn pending(cancel: bool) -> Pending {
        Pending {
            run_id: 1,
            cancel,
            attempt_before: 1,
            deadline: Instant::now() + Duration::from_secs(40),
            step: 0,
        }
    }

    #[test]
    fn an_old_answer_does_not_undo_a_rerun() {
        let p = pending(false);
        // GitHub still says "completed, attempt 1": the line stays queued.
        let mut stale = run();
        assert!(!p.reconcile(&mut stale));
        assert_eq!(
            (stale.status, stale.conclusion, stale.attempt),
            (RunStatus::Queued, None, 2)
        );
        // Once GitHub shows it running, or a new attempt, it is settled.
        let mut live = run();
        live.status = RunStatus::InProgress;
        assert!(p.reconcile(&mut live));
        assert_eq!(live.status, RunStatus::InProgress, "the real state is kept");
        let mut next = run();
        next.attempt = 2;
        assert!(p.reconcile(&mut next));
        // Another run is none of its business.
        let mut other = run();
        other.id = 99;
        assert!(!p.reconcile(&mut other));
        assert_eq!(other.status, RunStatus::Completed);
    }

    #[test]
    fn a_cancel_settles_when_the_run_is_over() {
        let p = pending(true);
        let mut running = run();
        running.status = RunStatus::InProgress;
        assert!(!p.reconcile(&mut running));
        assert_eq!(running.status, RunStatus::InProgress, "no invented state");
        assert!(p.reconcile(&mut run()));
    }

    #[test]
    fn a_running_job_has_no_log_to_fetch_and_says_why() {
        let mut node = job_node(10, 2, Load::NotLoaded);
        node.job.status = RunStatus::InProgress;
        node.job.conclusion = None;
        assert!(!node.logs_ready());
        let mut page = page_with(vec![node]);
        page.entry = 1;
        page.expanded_steps.insert((10, 1));
        let rows = page.build_rows();
        assert!(
            matches!(rows[1], PRow::Note(Note::Text(t)) if t.contains("when the job finishes")),
            "{rows:?}"
        );
        // Once it is over, nothing is shown until the log is fetched.
        page.jobs = Load::Loaded(vec![job_node(10, 2, Load::NotLoaded)]);
        assert!(page.job_nodes()[0].logs_ready());
        assert_eq!(page.build_rows(), vec![PRow::Step(0), PRow::Step(1)]);
    }

    #[test]
    fn keys_survive_rows_that_no_longer_exist() {
        let page = page_with(vec![job_node(10, 2, Load::NotLoaded)]);
        // An index from before a reload that returned fewer steps or jobs.
        assert_eq!(page.key_of(PRow::Step(99)), PKey::Note);
        let mut page = page;
        page.entry = 1;
        assert_eq!(page.key_of(PRow::Step(1)), PKey::Step(2));
        assert_eq!(page.key_of(PRow::Step(99)), PKey::Note);
    }

    #[test]
    fn a_long_log_line_takes_as_many_rows_as_it_wraps_to() {
        let lines = logic::parse_log(&[
            "2026-11-16T10:00:00Z short".to_string(),
            format!("2026-11-16T10:00:00Z {}", "x".repeat(200)),
        ]);
        let data = LogData {
            lines,
            ranges: std::iter::once(0..2).collect(),
        };
        let mut page = page_with(vec![job_node(10, 1, Load::Loaded(data))]);
        page.entry = 1;
        page.expanded_steps.insert((10, 1));
        page.rows = page.build_rows();
        let at = |li: usize| {
            page.rows
                .iter()
                .position(|r| *r == PRow::Line(0, li))
                .unwrap()
        };
        // 80 columns: 71 cells of text next to the gutter.
        assert_eq!(page.row_height(at(0), 80, false), 1);
        assert_eq!(page.row_height(at(1), 80, false), 3, "200 cells in 71");
        assert_eq!(page.row_height(at(1), 209, false), 1, "wide enough");
        assert!(page.row_height(at(1), 40, true) > 6, "narrower: more rows");
    }

    #[test]
    fn a_line_with_wide_characters_is_not_taken_for_a_short_one() {
        // 40 characters but 80 cells: it does not fit in 60 and must wrap.
        let wide: String = "日".repeat(40);
        let lines = logic::parse_log(&[format!("2026-11-16T10:00:00Z {wide}")]);
        assert!(render::log_line_height(&lines[0], false, 60) >= 2);
    }

    #[test]
    fn an_annotation_takes_as_many_lines_as_its_message_wraps_to() {
        let mut page = page_with(vec![job_node(10, 1, Load::NotLoaded)]);
        page.annotations = vec![annotation(10)];
        page.rows = vec![PRow::AnnHeader, PRow::Annotation(0)];
        assert_eq!(page.row_height(0, 40, false), 1);
        let wide = page.row_height(1, 80, false);
        let narrow = page.row_height(1, 20, false);
        assert!(wide >= 2 && narrow > wide, "{wide} {narrow}");
        assert_eq!(page.row_height(5, 40, false), 1, "out of range is one line");
    }
}
