//! GitHub Actions: workflow runs, jobs and their logs.
//!
//! Every network function returns `Result<_, String>` like the rest of the
//! GitHub client; the interesting logic (URLs, status mapping, log reading)
//! lives in pure functions so it can be tested without a server.

use std::collections::VecDeque;
use std::io::BufRead;
use std::time::Duration;

use serde::Deserialize;

use super::RepoCoords;

use super::{connection_failed, retry, CONNECT_ATTEMPTS, CONNECT_TIMEOUT};

const API: &str = "https://api.github.com";

/// Lines of a job log kept in memory: the end of a log is what matters.
pub const LOG_MAX_LINES: usize = 20_000;

/// Runs and jobs: GitHub answers the `actions/runs` endpoint slowly (seconds,
/// for payloads of hundreds of KB), more than the 15 s of the shared client.
const API_TIMEOUT: Duration = Duration::from_secs(30);

/// Logs can be large: they get a longer total timeout still.
const LOG_TIMEOUT: Duration = Duration::from_secs(120);

/// Runs shown in the list. A page of 100 is over a megabyte of JSON.
pub const LIST_PAGE: u32 = 50;

/// Clients are built once and shared: a connection that was hard to open on
/// a lossy network is then reused by the next requests.
fn client(timeout: Duration) -> Result<reqwest::blocking::Client, String> {
    use std::sync::OnceLock;
    static API: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    static LOGS: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    let cell = if timeout == LOG_TIMEOUT { &LOGS } else { &API };
    if let Some(c) = cell.get() {
        return Ok(c.clone());
    }
    let built = reqwest::blocking::Client::builder()
        .user_agent("gitoui")
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout)
        .build()
        .map_err(|e| format!("http client init: {e}"))?;
    Ok(cell.get_or_init(|| built).clone())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Queued,
    InProgress,
    Completed,
    Waiting,
    Pending,
    Requested,
    Other,
}

impl RunStatus {
    pub fn from_api(s: &str) -> Self {
        match s {
            "queued" => Self::Queued,
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "waiting" => Self::Waiting,
            "pending" => Self::Pending,
            "requested" => Self::Requested,
            _ => Self::Other,
        }
    }

    /// Not finished yet: queued, waiting for approval, or running.
    pub fn is_active(self) -> bool {
        matches!(
            self,
            Self::Queued | Self::InProgress | Self::Waiting | Self::Pending | Self::Requested
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunConclusion {
    Success,
    Failure,
    Cancelled,
    Skipped,
    Neutral,
    TimedOut,
    ActionRequired,
    StartupFailure,
    Stale,
    Other,
}

impl RunConclusion {
    pub fn from_api(s: &str) -> Self {
        match s {
            "success" => Self::Success,
            "failure" => Self::Failure,
            "cancelled" => Self::Cancelled,
            "skipped" => Self::Skipped,
            "neutral" => Self::Neutral,
            "timed_out" => Self::TimedOut,
            "action_required" => Self::ActionRequired,
            "startup_failure" => Self::StartupFailure,
            "stale" => Self::Stale,
            _ => Self::Other,
        }
    }

    pub fn is_failure(self) -> bool {
        matches!(
            self,
            Self::Failure | Self::TimedOut | Self::ActionRequired | Self::StartupFailure
        )
    }
}

#[derive(Debug, Clone)]
pub struct Run {
    pub id: u64,
    /// Workflow name.
    pub workflow: String,
    /// Commit message or PR title.
    pub title: String,
    pub event: String,
    pub status: RunStatus,
    pub conclusion: Option<RunConclusion>,
    pub branch: String,
    /// The workflow it belongs to; its name is in `list_workflows`.
    pub workflow_id: u64,
    pub sha: String,
    pub number: u64,
    pub attempt: u64,
    pub actor: String,
    /// Number of the pull request that triggered the run, if any.
    pub pull_request: Option<u64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub updated_at: String,
    pub url: String,
    /// The workflow file, like `.github/workflows/deny.yml`.
    pub path: String,
}

/// Two values are equal when they describe the same run in the same state
/// (what changes the dot in the header), not when every field matches.
impl PartialEq for Run {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.status == other.status && self.conclusion == other.conclusion
    }
}

impl Run {
    /// Running now. A run waiting for an approval, or queued for more than a
    /// day (a stuck runner), does not count: it would keep the CI dot and the
    /// page polling for ever.
    pub fn is_live(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        match self.status {
            // Running, unless nothing moved for a day (a lost runner).
            RunStatus::InProgress => chrono::DateTime::parse_from_rfc3339(&self.updated_at)
                .map(|t| (now - t.with_timezone(&chrono::Utc)).num_hours() < 24)
                .unwrap_or(true),
            RunStatus::Queued => chrono::DateTime::parse_from_rfc3339(&self.created_at)
                .map(|t| (now - t.with_timezone(&chrono::Utc)).num_hours() < 24)
                .unwrap_or(true),
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Step {
    pub number: u64,
    pub name: String,
    pub status: RunStatus,
    pub conclusion: Option<RunConclusion>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: u64,
    pub name: String,
    pub status: RunStatus,
    pub conclusion: Option<RunConclusion>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub url: String,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Default)]
pub struct JobLog {
    pub lines: Vec<String>,
    pub truncated: bool,
    /// Shown above the log: it is cut or incomplete.
    pub warning: Option<String>,
}

// ─── Raw API shapes ──────────────────────────────────────────────

fn one() -> u64 {
    1
}

#[derive(Deserialize)]
struct ApiActor {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize)]
struct ApiPullRequest {
    #[serde(default)]
    number: u64,
}

#[derive(Deserialize)]
struct ApiRun {
    id: u64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    display_title: Option<String>,
    #[serde(default)]
    event: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    head_branch: Option<String>,
    #[serde(default)]
    workflow_id: u64,
    #[serde(default)]
    head_sha: String,
    #[serde(default)]
    run_number: u64,
    #[serde(default = "one")]
    run_attempt: u64,
    #[serde(default)]
    actor: Option<ApiActor>,
    #[serde(default)]
    pull_requests: Vec<ApiPullRequest>,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    run_started_at: Option<String>,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    path: String,
}

#[derive(Deserialize)]
struct ApiRuns {
    #[serde(default)]
    workflow_runs: Vec<ApiRun>,
}

#[derive(Deserialize)]
struct ApiStep {
    #[serde(default)]
    name: String,
    #[serde(default)]
    number: u64,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    completed_at: Option<String>,
}

#[derive(Deserialize)]
struct ApiJob {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    completed_at: Option<String>,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    steps: Vec<ApiStep>,
}

#[derive(Deserialize)]
struct ApiJobs {
    #[serde(default)]
    jobs: Vec<ApiJob>,
}

#[derive(Debug, Clone)]
pub struct Workflow {
    pub id: u64,
    pub name: String,
}

#[derive(Deserialize)]
struct ApiWorkflow {
    id: u64,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct ApiWorkflows {
    #[serde(default)]
    workflows: Vec<ApiWorkflow>,
}

pub fn parse_workflows(text: &str) -> Result<Vec<Workflow>, String> {
    let api: ApiWorkflows =
        serde_json::from_str(text).map_err(|e| format!("GitHub workflows: bad response ({e})"))?;
    Ok(api
        .workflows
        .into_iter()
        .map(|w| Workflow {
            id: w.id,
            name: w.name,
        })
        .collect())
}

fn status_of(s: Option<&str>) -> RunStatus {
    s.map_or(RunStatus::Other, RunStatus::from_api)
}

fn conclusion_of(s: Option<&str>) -> Option<RunConclusion> {
    s.map(RunConclusion::from_api)
}

pub fn parse_runs(text: &str) -> Result<Vec<Run>, String> {
    let api: ApiRuns = serde_json::from_str(text)
        .map_err(|e| format!("GitHub workflow runs: bad response ({e})"))?;
    Ok(api
        .workflow_runs
        .into_iter()
        .map(|r| Run {
            id: r.id,
            title: r
                .display_title
                .filter(|t| !t.is_empty())
                .unwrap_or_default(),
            workflow: r.name.unwrap_or_default(),
            event: r.event,
            status: status_of(r.status.as_deref()),
            conclusion: conclusion_of(r.conclusion.as_deref()),
            branch: r.head_branch.unwrap_or_default(),
            workflow_id: r.workflow_id,
            sha: r.head_sha,
            number: r.run_number,
            attempt: r.run_attempt,
            actor: r.actor.map(|a| a.login).unwrap_or_default(),
            pull_request: r.pull_requests.first().map(|p| p.number).filter(|n| *n > 0),
            created_at: r.created_at,
            started_at: r.run_started_at,
            updated_at: r.updated_at,
            url: r.html_url,
            path: r.path,
        })
        .collect())
}

/// One run, as answered by `/actions/runs/{id}` (not wrapped in a list).
pub fn parse_run(text: &str) -> Result<Run, String> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| format!("GitHub workflow runs: bad response ({e})"))?;
    let wrapped = serde_json::json!({ "workflow_runs": [value] }).to_string();
    parse_runs(&wrapped)?
        .into_iter()
        .next()
        .ok_or_else(|| "GitHub workflow runs: empty response".to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationLevel {
    Notice,
    Warning,
    Failure,
}

/// A message a job left on its check run (`::notice`, `::warning`, `::error`).
#[derive(Debug, Clone)]
pub struct Annotation {
    pub job_id: u64,
    pub level: AnnotationLevel,
    pub title: String,
    pub message: String,
    pub path: String,
    pub line: u64,
}

#[derive(Deserialize)]
struct ApiAnnotation {
    #[serde(default)]
    annotation_level: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    start_line: Option<u64>,
}

pub fn parse_annotations(job_id: u64, text: &str) -> Result<Vec<Annotation>, String> {
    let api: Vec<ApiAnnotation> = serde_json::from_str(text)
        .map_err(|e| format!("GitHub annotations: bad response ({e})"))?;
    Ok(api
        .into_iter()
        .map(|a| Annotation {
            job_id,
            level: match a.annotation_level.as_deref() {
                Some("failure") => AnnotationLevel::Failure,
                Some("warning") => AnnotationLevel::Warning,
                _ => AnnotationLevel::Notice,
            },
            title: a.title.unwrap_or_default(),
            // GitHub wraps some messages in literal quotes.
            message: a
                .message
                .unwrap_or_default()
                .trim_matches('"')
                .trim()
                .to_string(),
            path: a.path.unwrap_or_default(),
            line: a.start_line.unwrap_or(0),
        })
        .collect())
}

pub fn parse_jobs(text: &str) -> Result<Vec<Job>, String> {
    let api: ApiJobs = serde_json::from_str(text)
        .map_err(|e| format!("GitHub workflow jobs: bad response ({e})"))?;
    Ok(api
        .jobs
        .into_iter()
        .map(|j| Job {
            id: j.id,
            name: j.name,
            status: status_of(j.status.as_deref()),
            conclusion: conclusion_of(j.conclusion.as_deref()),
            started_at: j.started_at,
            completed_at: j.completed_at,
            url: j.html_url,
            steps: j
                .steps
                .into_iter()
                .map(|s| Step {
                    number: s.number,
                    name: s.name,
                    status: status_of(s.status.as_deref()),
                    conclusion: conclusion_of(s.conclusion.as_deref()),
                    started_at: s.started_at,
                    completed_at: s.completed_at,
                })
                .collect(),
        })
        .collect())
}

// ─── URLs ────────────────────────────────────────────────────────

fn repo_base(c: &RepoCoords) -> String {
    format!("{API}/repos/{}/{}", c.owner, c.repo)
}

pub fn runs_url(c: &RepoCoords, branch: Option<&str>, per_page: u32) -> String {
    runs_page_url(c, branch, per_page, 1)
}

/// One page of the runs (the first one has no `page` parameter).
pub fn runs_page_url(c: &RepoCoords, branch: Option<&str>, per_page: u32, page: u32) -> String {
    let base = format!("{}/actions/runs", repo_base(c));
    let per_page = per_page.to_string();
    let page = page.to_string();
    let mut params = vec![("per_page", per_page.as_str())];
    if let Some(b) = branch {
        params.push(("branch", b));
    }
    if page != "1" {
        params.push(("page", page.as_str()));
    }
    reqwest::Url::parse_with_params(&base, params)
        .map(|u| u.to_string())
        .unwrap_or(base)
}

pub fn run_url(c: &RepoCoords, run_id: u64) -> String {
    format!("{}/actions/runs/{run_id}", repo_base(c))
}

pub fn annotations_url(c: &RepoCoords, job_id: u64) -> String {
    format!(
        "{}/check-runs/{job_id}/annotations?per_page=100",
        repo_base(c)
    )
}

pub fn workflows_url(c: &RepoCoords) -> String {
    format!("{}/actions/workflows?per_page=100", repo_base(c))
}

pub fn jobs_url(c: &RepoCoords, run_id: u64) -> String {
    format!("{}/actions/runs/{run_id}/jobs?per_page=100", repo_base(c))
}

pub fn logs_url(c: &RepoCoords, job_id: u64) -> String {
    format!("{}/actions/jobs/{job_id}/logs", repo_base(c))
}

pub fn rerun_url(c: &RepoCoords, run_id: u64) -> String {
    format!("{}/actions/runs/{run_id}/rerun", repo_base(c))
}

pub fn rerun_failed_url(c: &RepoCoords, run_id: u64) -> String {
    format!("{}/actions/runs/{run_id}/rerun-failed-jobs", repo_base(c))
}

pub fn cancel_url(c: &RepoCoords, run_id: u64) -> String {
    format!("{}/actions/runs/{run_id}/cancel", repo_base(c))
}

// ─── Log reading ─────────────────────────────────────────────────

/// Reads a log line by line, keeping only the last `cap` lines, and keeps
/// what was received when the stream breaks half way.
pub fn read_log<R: BufRead>(mut reader: R, cap: usize) -> JobLog {
    let mut lines: VecDeque<String> = VecDeque::with_capacity(cap.min(4096));
    let mut total = 0usize;
    let mut error: Option<std::io::Error> = None;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        let read = reader.read_until(b'\n', &mut buf);
        match read {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                error = Some(e);
                if buf.is_empty() {
                    break;
                }
            }
        }
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        if lines.len() == cap {
            lines.pop_front();
        }
        lines.push_back(String::from_utf8_lossy(&buf).into_owned());
        total += 1;
        if error.is_some() {
            break;
        }
    }
    let truncated = total > cap;
    let warning = match (&error, truncated) {
        (Some(e), true) => Some(format!(
            "Log incomplete: {e} (showing the last {cap} lines)"
        )),
        (Some(e), false) => Some(format!("Log incomplete: {e}")),
        (None, true) => Some(format!("Showing the last {cap} lines of {total}")),
        (None, false) => None,
    };
    JobLog {
        lines: lines.into(),
        truncated,
        warning,
    }
}

// ─── Network ─────────────────────────────────────────────────────

/// A network error with its causes ("error sending request: connect: ...").
fn net_error(label: &str, e: &reqwest::Error) -> String {
    use std::error::Error as _;
    let mut text = format!("{label}: {e}");
    let mut source = e.source();
    while let Some(cause) = source {
        text.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    text
}

fn failure(label: &str, status: reqwest::StatusCode, body: &str) -> String {
    format!(
        "{label}: {}",
        super::pr::humanize_github_error(status, body)
    )
}

/// A GET that is tried a second time when the connection could not be opened
/// or timed out: a flaky network should not show an error right away.
fn send_get(
    timeout: Duration,
    token: &str,
    url: &str,
    label: &str,
) -> Result<reqwest::blocking::Response, String> {
    send_get_if_changed(timeout, token, url, label, None)
}

/// Like `send_get`, and when `etag` is given asks GitHub to answer `304` if
/// nothing changed: such an answer does not count against the rate limit.
fn send_get_if_changed(
    timeout: Duration,
    token: &str,
    url: &str,
    label: &str,
    etag: Option<&str>,
) -> Result<reqwest::blocking::Response, String> {
    let client = client(timeout)?;
    retry(CONNECT_ATTEMPTS, connection_failed, || {
        let mut req = client
            .get(url)
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json");
        if let Some(tag) = etag {
            req = req.header("If-None-Match", tag);
        }
        req.send()
    })
    .map_err(|e| net_error(label, &e))
}

fn get_text(token: &str, url: &str, label: &str) -> Result<String, String> {
    let resp = send_get(API_TIMEOUT, token, url, label)?;
    let status = resp.status();
    let body = resp.text().map_err(|e| net_error(label, &e))?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(failure(label, status, &body))
    }
}

/// What a conditional request found.
#[derive(Debug, PartialEq)]
pub enum Listed<T> {
    /// New data, with the tag to send next time.
    Fresh(T, Option<String>),
    /// Nothing changed since the tag we sent (a free answer).
    Unchanged,
}

/// The most recent runs of a branch, or `Unchanged` when GitHub says the list
/// is the same as when `etag` was given. The CI monitor asks every few
/// seconds: with this, an idle repository costs no quota at all.
pub fn list_runs_if_changed(
    token: &str,
    coords: &RepoCoords,
    branch: Option<&str>,
    per_page: u32,
    etag: Option<&str>,
) -> Result<Listed<Vec<Run>>, String> {
    let label = "GitHub /actions/runs";
    let resp = send_get_if_changed(
        API_TIMEOUT,
        token,
        &runs_url(coords, branch, per_page),
        label,
        etag,
    )?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(Listed::Unchanged);
    }
    let new_tag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = resp.text().map_err(|e| net_error(label, &e))?;
    if status.is_success() {
        Ok(Listed::Fresh(parse_runs(&body)?, new_tag))
    } else {
        Err(failure(label, status, &body))
    }
}

/// The most recent runs of the repository (all workflows), optionally of
/// one branch.
pub fn list_runs(
    token: &str,
    coords: &RepoCoords,
    branch: Option<&str>,
    per_page: u32,
) -> Result<Vec<Run>, String> {
    list_runs_page(token, coords, branch, per_page, 1)
}

/// A later page of the runs: the page of the list that scrolls past its end.
pub fn list_runs_page(
    token: &str,
    coords: &RepoCoords,
    branch: Option<&str>,
    per_page: u32,
    page: u32,
) -> Result<Vec<Run>, String> {
    parse_runs(&get_text(
        token,
        &runs_page_url(coords, branch, per_page, page),
        "GitHub /actions/runs",
    )?)
}

/// The workflows of the repository: the name of a run is its own title for
/// some kinds of runs (Dependabot), the workflow name is only here.
pub fn list_workflows(token: &str, coords: &RepoCoords) -> Result<Vec<Workflow>, String> {
    parse_workflows(&get_text(
        token,
        &workflows_url(coords),
        "GitHub /actions/workflows",
    )?)
}

/// One run, to refresh the header of its page.
pub fn get_run(token: &str, coords: &RepoCoords, run_id: u64) -> Result<Run, String> {
    parse_run(&get_text(
        token,
        &run_url(coords, run_id),
        "GitHub /actions/runs/{id}",
    )?)
}

/// The annotations of several jobs, in order. A job whose annotations cannot
/// be read is skipped: they are a bonus, not worth an error.
pub fn list_annotations(token: &str, coords: &RepoCoords, job_ids: &[u64]) -> Vec<Annotation> {
    let mut all = Vec::new();
    for &id in job_ids {
        let url = annotations_url(coords, id);
        if let Ok(text) = get_text(token, &url, "GitHub check-run annotations") {
            if let Ok(mut list) = parse_annotations(id, &text) {
                all.append(&mut list);
            }
        }
    }
    all
}

pub fn list_jobs(token: &str, coords: &RepoCoords, run_id: u64) -> Result<Vec<Job>, String> {
    parse_jobs(&get_text(
        token,
        &jobs_url(coords, run_id),
        "GitHub /actions/runs/jobs",
    )?)
}

/// The log of one job. GitHub answers with a redirect to signed storage:
/// reqwest's default redirect policy drops the `Authorization` header when
/// the host changes, which is what we want (no custom policy on purpose).
/// GitHub keeps the logs of a job 90 days, and only gives them once the job is
/// over: before that (and for a moment after) the answer is a 404.
pub const LOGS_GONE: &str = "Logs not available (GitHub keeps them 90 days)";

/// The logs of a job that has just finished: they are uploaded a moment after
/// the end, so a 404 is tried again a few times before it is believed.
pub fn fetch_job_logs_settled(
    token: &str,
    coords: &RepoCoords,
    job_id: u64,
) -> Result<JobLog, String> {
    const WAITS_SECS: [u64; 4] = [1, 2, 3, 4];
    for wait in WAITS_SECS {
        match fetch_job_logs(token, coords, job_id) {
            Err(e) if e == LOGS_GONE => std::thread::sleep(Duration::from_secs(wait)),
            other => return other,
        }
    }
    fetch_job_logs(token, coords, job_id)
}

pub fn fetch_job_logs(token: &str, coords: &RepoCoords, job_id: u64) -> Result<JobLog, String> {
    let resp = send_get(
        LOG_TIMEOUT,
        token,
        &logs_url(coords, job_id),
        "GitHub job logs",
    )?;
    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(LOGS_GONE.into());
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(failure("GitHub job logs", status, &body));
    }
    let log = read_log(std::io::BufReader::new(resp), LOG_MAX_LINES);
    match (&log.warning, log.lines.is_empty()) {
        (Some(w), true) => Err(w.clone()),
        _ => Ok(log),
    }
}

fn post_empty(token: &str, url: &str, label: &str) -> Result<(), String> {
    let client = client(API_TIMEOUT)?;
    let resp = retry(CONNECT_ATTEMPTS, connection_failed, || {
        client
            .post(url)
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("Content-Length", "0")
            .send()
    })
    .map_err(|e| net_error(label, &e))?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = resp.text().unwrap_or_default();
    Err(failure(label, status, &body))
}

pub fn rerun(token: &str, coords: &RepoCoords, run_id: u64) -> Result<(), String> {
    post_empty(token, &rerun_url(coords, run_id), "Re-run")
}

pub fn rerun_failed(token: &str, coords: &RepoCoords, run_id: u64) -> Result<(), String> {
    post_empty(
        token,
        &rerun_failed_url(coords, run_id),
        "Re-run failed jobs",
    )
}

pub fn cancel(token: &str, coords: &RepoCoords, run_id: u64) -> Result<(), String> {
    post_empty(token, &cancel_url(coords, run_id), "Cancel")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const RUNS_JSON: &str = r#"{
      "total_count": 2,
      "workflow_runs": [
        {"id": 101, "name": "CI", "display_title": "fix(open): launcher", "event": "push",
         "status": "completed", "conclusion": "failure", "head_branch": "master",
         "head_sha": "a76a01d0000000000000000000000000000000aa", "run_number": 42, "run_attempt": 2,
         "actor": {"login": "NayJi7"}, "pull_requests": [{"number": 93}], "created_at": "2026-11-02T09:00:00Z",
         "run_started_at": "2026-11-02T09:00:05Z", "updated_at": "2026-11-02T09:04:00Z",
         "html_url": "https://github.com/NayJi7/gitoui/actions/runs/101"},
        {"id": 102, "name": "Pages", "display_title": "docs", "event": "workflow_dispatch",
         "status": "brand_new_status", "conclusion": "something_new", "head_branch": null,
         "head_sha": "bf60985", "run_number": 7, "actor": null,
         "created_at": "2026-11-02T10:00:00Z", "updated_at": "2026-11-02T10:00:00Z",
         "html_url": "https://github.com/NayJi7/gitoui/actions/runs/102"}
      ]}"#;

    const JOBS_JSON: &str = r#"{"jobs": [
      {"id": 7, "name": "lint (stable)", "status": "completed", "conclusion": "failure",
       "started_at": "2026-11-02T09:00:10Z", "completed_at": "2026-11-02T09:03:10Z",
       "html_url": "https://github.com/NayJi7/gitoui/actions/runs/101/job/7",
       "steps": [
         {"name": "Set up job", "status": "completed", "conclusion": "success", "number": 1,
          "started_at": "2026-11-02T09:00:10Z", "completed_at": "2026-11-02T09:00:12Z"},
         {"name": "Run tests", "status": "completed", "conclusion": "failure", "number": 2,
          "started_at": "2026-11-02T09:00:12Z", "completed_at": "2026-11-02T09:03:10Z"}]},
      {"id": 8, "name": "build", "status": "queued", "conclusion": null, "steps": []}]}"#;

    fn coords() -> crate::github::RepoCoords {
        crate::github::RepoCoords {
            owner: "NayJi7".into(),
            repo: "gitoui".into(),
        }
    }

    #[test]
    fn parses_runs_and_tolerates_unknown_values() {
        let runs = parse_runs(RUNS_JSON).unwrap();
        assert_eq!(runs.len(), 2);
        let first = &runs[0];
        assert_eq!(
            (
                first.id,
                first.workflow.as_str(),
                first.number,
                first.attempt
            ),
            (101, "CI", 42, 2)
        );
        assert_eq!(first.title, "fix(open): launcher");
        assert_eq!(first.status, RunStatus::Completed);
        assert_eq!(first.conclusion, Some(RunConclusion::Failure));
        assert_eq!(first.branch, "master");
        assert_eq!(first.actor, "NayJi7");
        assert_eq!(first.pull_request, Some(93));
        let second = &runs[1];
        assert_eq!(second.status, RunStatus::Other);
        assert_eq!(second.conclusion, Some(RunConclusion::Other));
        assert_eq!(
            (
                second.branch.as_str(),
                second.actor.as_str(),
                second.attempt
            ),
            ("", "", 1)
        );
    }

    #[test]
    fn parses_workflows_and_the_workflow_id_of_a_run() {
        let ws = parse_workflows(
            r#"{"total_count":2,"workflows":[{"id":11,"name":"Build","state":"active"},{"id":12,"name":"Dependabot Updates","state":"active"}]}"#,
        )
        .unwrap();
        assert_eq!(ws.len(), 2);
        assert_eq!((ws[1].id, ws[1].name.as_str()), (12, "Dependabot Updates"));
        let runs = parse_runs(
            r#"{"workflow_runs":[{"id":1,"workflow_id":12,"name":"cargo in /. - Update #1"}]}"#,
        )
        .unwrap();
        assert_eq!(runs[0].workflow_id, 12);
        assert_eq!(
            workflows_url(&coords()),
            "https://api.github.com/repos/NayJi7/gitoui/actions/workflows?per_page=100"
        );
    }

    #[test]
    fn parses_jobs_with_steps() {
        let jobs = parse_jobs(JOBS_JSON).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].steps.len(), 2);
        assert_eq!(jobs[0].steps[1].name, "Run tests");
        assert_eq!(jobs[0].steps[1].conclusion, Some(RunConclusion::Failure));
        assert_eq!(jobs[1].status, RunStatus::Queued);
        assert_eq!(jobs[1].conclusion, None);
        assert!(jobs[1].steps.is_empty());
    }

    #[test]
    fn parse_errors_name_the_endpoint() {
        let err = parse_runs("not json").unwrap_err();
        assert!(err.contains("workflow runs"), "{err}");
    }

    #[test]
    fn status_helpers() {
        assert!(RunStatus::Queued.is_active() && RunStatus::InProgress.is_active());
        assert!(RunStatus::Waiting.is_active() && RunStatus::Pending.is_active());
        assert!(!RunStatus::Completed.is_active());
        assert!(RunConclusion::Failure.is_failure() && RunConclusion::TimedOut.is_failure());
        assert!(
            RunConclusion::StartupFailure.is_failure()
                && RunConclusion::ActionRequired.is_failure()
        );
        assert!(!RunConclusion::Success.is_failure() && !RunConclusion::Cancelled.is_failure());
    }

    #[test]
    fn a_run_is_live_unless_it_waits_forever() {
        use chrono::TimeZone;
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 11, 16, 12, 0, 0)
            .unwrap();
        let run = |status: &str, created: &str| {
            parse_runs(&format!(
                r#"{{"workflow_runs":[{{"id":1,"status":"{status}","created_at":"{created}"}}]}}"#
            ))
            .unwrap()
            .remove(0)
        };
        assert!(
            run("in_progress", "2026-11-10T00:00:00Z").is_live(now),
            "running is always live"
        );
        assert!(run("queued", "2026-11-16T11:00:00Z").is_live(now));
        assert!(
            !run("queued", "2026-11-13T11:00:00Z").is_live(now),
            "queued for days: stuck"
        );
        assert!(
            !run("waiting", "2026-11-16T11:59:00Z").is_live(now),
            "waiting for an approval"
        );
        assert!(!run("pending", "2026-11-16T11:59:00Z").is_live(now));
        assert!(!run("completed", "2026-11-16T11:59:00Z").is_live(now));
        assert!(run("queued", "").is_live(now), "unknown date: assume live");
        let stuck = parse_runs(
            r#"{"workflow_runs":[{"id":1,"status":"in_progress","created_at":"2026-11-01T00:00:00Z","updated_at":"2026-11-02T00:00:00Z"}]}"#,
        )
        .unwrap()
        .remove(0);
        assert!(!stuck.is_live(now), "in progress but silent for days");
    }

    #[test]
    fn one_run_and_its_annotations_parse() {
        let run = parse_run(
            r#"{"id":7,"name":"Deny","status":"completed","conclusion":"success",
            "path":".github/workflows/deny.yml","created_at":"2026-11-16T10:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(
            (run.id, run.path.as_str()),
            (7, ".github/workflows/deny.yml")
        );
        assert!(parse_run("[]").is_err());
        let list = parse_annotations(
            9,
            r#"[{"annotation_level":"notice","title":"","message":"\"hello\"","path":".github","start_line":1},
                {"annotation_level":"failure","title":"T","message":"bad","path":"a.rs","start_line":4}]"#,
        )
        .unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].message, "hello");
        assert_eq!(list[0].level, AnnotationLevel::Notice);
        assert_eq!((list[1].level, list[1].line), (AnnotationLevel::Failure, 4));
        assert_eq!(list[1].job_id, 9);
    }

    #[test]
    fn retry_stops_at_the_first_success_or_unretryable_error() {
        let mut calls = 0;
        let ok: Result<u32, &str> = retry(
            4,
            |_| true,
            || {
                calls += 1;
                if calls < 3 {
                    Err("connect")
                } else {
                    Ok(7)
                }
            },
        );
        assert_eq!((ok, calls), (Ok(7), 3));

        let mut calls = 0;
        let fatal: Result<u32, &str> = retry(
            4,
            |e| *e == "connect",
            || {
                calls += 1;
                Err("fatal")
            },
        );
        assert_eq!((fatal, calls), (Err("fatal"), 1));

        let mut calls = 0;
        let never: Result<u32, &str> = retry(
            4,
            |_| true,
            || {
                calls += 1;
                Err("connect")
            },
        );
        assert_eq!((never, calls), (Err("connect"), 4));
    }

    #[test]
    fn urls_are_built_and_encoded() {
        let c = coords();
        assert_eq!(
            runs_url(&c, None, 50),
            "https://api.github.com/repos/NayJi7/gitoui/actions/runs?per_page=50"
        );
        assert_eq!(
            runs_url(&c, Some("feat/a b&c"), 10),
            "https://api.github.com/repos/NayJi7/gitoui/actions/runs?per_page=10&branch=feat%2Fa+b%26c"
        );
        assert_eq!(
            runs_page_url(&c, Some("main"), 50, 3),
            "https://api.github.com/repos/NayJi7/gitoui/actions/runs?per_page=50&branch=main&page=3"
        );
        assert_eq!(runs_page_url(&c, None, 50, 1), runs_url(&c, None, 50));
        assert_eq!(
            jobs_url(&c, 5),
            "https://api.github.com/repos/NayJi7/gitoui/actions/runs/5/jobs?per_page=100"
        );
        assert_eq!(
            logs_url(&c, 9),
            "https://api.github.com/repos/NayJi7/gitoui/actions/jobs/9/logs"
        );
        assert_eq!(
            rerun_url(&c, 5),
            "https://api.github.com/repos/NayJi7/gitoui/actions/runs/5/rerun"
        );
        assert_eq!(
            rerun_failed_url(&c, 5),
            "https://api.github.com/repos/NayJi7/gitoui/actions/runs/5/rerun-failed-jobs"
        );
        assert_eq!(
            cancel_url(&c, 5),
            "https://api.github.com/repos/NayJi7/gitoui/actions/runs/5/cancel"
        );
    }

    #[test]
    fn log_reader_keeps_only_the_last_lines() {
        let text: String = (0..25_000).map(|n| format!("line {n}\n")).collect();
        let log = read_log(Cursor::new(text.into_bytes()), LOG_MAX_LINES);
        assert_eq!(log.lines.len(), LOG_MAX_LINES);
        assert_eq!(log.lines.first().map(String::as_str), Some("line 5000"));
        assert_eq!(log.lines.last().map(String::as_str), Some("line 24999"));
        assert!(log.truncated);
        assert_eq!(
            log.warning.as_deref(),
            Some("Showing the last 20000 lines of 25000")
        );
    }

    #[test]
    fn log_reader_handles_crlf_invalid_utf8_and_a_missing_final_newline() {
        let bytes = b"first\r\nsecond \xff bad\nlast".to_vec();
        let log = read_log(Cursor::new(bytes), 100);
        assert_eq!(log.lines, vec!["first", "second \u{fffd} bad", "last"]);
        assert!(!log.truncated && log.warning.is_none());
    }

    struct Failing(Vec<u8>, bool);
    impl std::io::Read for Failing {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.0.is_empty() {
                let n = self.0.len().min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0.drain(..n);
                Ok(n)
            } else if !self.1 {
                self.1 = true;
                Err(std::io::Error::other("connection reset"))
            } else {
                Ok(0)
            }
        }
    }

    #[test]
    fn a_log_cut_in_the_middle_keeps_what_was_received() {
        let log = read_log(
            std::io::BufReader::new(Failing(b"one\ntwo\npart".to_vec(), false)),
            100,
        );
        assert_eq!(log.lines, vec!["one", "two", "part"]);
        let warning = log.warning.unwrap();
        assert!(
            warning.starts_with("Log incomplete:") && warning.contains("connection reset"),
            "{warning}"
        );
    }
}
