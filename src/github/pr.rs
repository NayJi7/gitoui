//! GitHub Pull Request data layer.
//!
//! Endpoints used (all REST v3):
//! - `GET /repos/{owner}/{repo}/pulls?state=open` — list summaries
//! - `GET /repos/{owner}/{repo}/pulls/{number}` — full detail (description, head/base)
//! - `GET /repos/{owner}/{repo}/pulls/{number}/files` — file changes
//! - `GET /repos/{owner}/{repo}/pulls/{number}/reviews` — review summary
//! - `GET /repos/{owner}/{repo}/commits/{sha}/check-runs` — CI status
//!
//! All fetches block; callers should run them off the UI tick if they
//! care about responsiveness on slow networks.

use serde::Deserialize;

use super::{http_client, RepoCoords};

/// Compact PR summary — populates the list panel.
#[derive(Debug, Clone)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: PullState,
    pub draft: bool,
    /// Head branch (`<user>:<branch>` for forks, `<branch>` for same-repo PRs).
    pub head_label: String,
    /// Base branch ref name (usually `main` or `master`).
    pub base_ref: String,
    /// Head commit SHA — used to look up CI status lazily.
    pub head_sha: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullState {
    Open,
    Closed,
    Merged,
}

/// Full PR data — populates the detail panel. We fetch this on demand
/// when the user moves selection so the list stays cheap.
#[derive(Debug, Clone)]
pub struct PullRequestDetail {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: PullState,
    pub draft: bool,
    pub head_label: String,
    pub base_ref: String,
    pub body: String,
    pub additions: u64,
    pub deletions: u64,
    pub commits: u64,
    pub changed_files: u64,
    pub reviewers: Vec<String>,
    pub reviews: ReviewsSummary,
    pub ci: CiSummary,
    pub files: Vec<PullFile>,
}

#[derive(Debug, Clone, Default)]
pub struct ReviewsSummary {
    pub approved: usize,
    pub changes_requested: usize,
    pub commented: usize,
}

/// Aggregated CI status — totals across all check-runs on the head SHA.
#[derive(Debug, Clone, Default)]
pub struct CiSummary {
    pub total: usize,
    pub success: usize,
    pub failure: usize,
    pub pending: usize,
}

#[derive(Debug, Clone)]
pub struct PullFile {
    pub filename: String,
    pub status: FileStatus,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Modified,
    Removed,
    Renamed,
    Other,
}

// ---------- Raw response shapes — internal, deserialised then mapped. ----------

#[derive(Deserialize)]
struct ApiUser {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize)]
struct ApiRef {
    #[serde(default)]
    label: String,
    #[serde(default, rename = "ref")]
    ref_name: String,
    #[serde(default)]
    sha: String,
}

#[derive(Deserialize)]
struct ApiPullSummary {
    number: u64,
    #[serde(default)]
    title: String,
    user: Option<ApiUser>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    merged_at: Option<String>,
    head: ApiRef,
    base: ApiRef,
}

#[derive(Deserialize)]
struct ApiPullDetail {
    number: u64,
    #[serde(default)]
    title: String,
    user: Option<ApiUser>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    merged_at: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
    #[serde(default)]
    commits: u64,
    #[serde(default)]
    changed_files: u64,
    head: ApiRef,
    base: ApiRef,
    #[serde(default)]
    requested_reviewers: Vec<ApiUser>,
}

#[derive(Deserialize)]
struct ApiReview {
    #[serde(default)]
    state: String,
}

#[derive(Deserialize)]
struct ApiCheckRunsResponse {
    #[serde(default)]
    check_runs: Vec<ApiCheckRun>,
}

#[derive(Deserialize)]
struct ApiCheckRun {
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
}

#[derive(Deserialize)]
struct ApiPullFile {
    #[serde(default)]
    filename: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
}

// ---------- Public API ----------

/// Fetch the open PRs on the given repo, sorted by number descending
/// (newest first — matches GitHub's web UI default).
pub fn list_pull_requests(token: &str, coords: &RepoCoords) -> Result<Vec<PullRequest>, String> {
    let client = http_client()?;
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls?state=open&per_page=50",
        coords.owner, coords.repo
    );
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("GitHub /pulls: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "GitHub /pulls HTTP {}: {}",
            status,
            body.lines().next().unwrap_or("")
        ));
    }
    let body = resp.text().map_err(|e| format!("GitHub /pulls read: {}", e))?;
    let raw: Vec<ApiPullSummary> =
        serde_json::from_str(&body).map_err(|e| format!("GitHub /pulls JSON: {}", e))?;
    Ok(raw.into_iter().map(map_summary).collect())
}

/// Fetch a single PR with reviews + CI + files. The 3 secondary endpoints
/// run in parallel threads against a shared client so total latency is
/// roughly `max(primary, secondary)` instead of `primary + sum(secondary)`.
pub fn fetch_pull_request_detail(
    token: &str,
    coords: &RepoCoords,
    number: u64,
) -> Result<PullRequestDetail, String> {
    use std::sync::Arc;

    let client = Arc::new(http_client()?);
    let base = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}",
        coords.owner, coords.repo, number
    );

    // Primary PR fetch — we need its head SHA for the CI call, but we
    // can start reviews + files in parallel since they don't depend on
    // that SHA.
    let pr_resp = client
        .get(&base)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("GitHub PR detail: {}", e))?;
    if !pr_resp.status().is_success() {
        return Err(format!("PR detail HTTP {}", pr_resp.status()));
    }
    let pr_body = pr_resp.text().map_err(|e| format!("PR read: {}", e))?;
    let pr: ApiPullDetail =
        serde_json::from_str(&pr_body).map_err(|e| format!("PR JSON: {}", e))?;

    // Spawn the 3 secondary fetches concurrently.
    let reviews_handle = {
        let client = Arc::clone(&client);
        let token = token.to_string();
        let url = format!("{}/reviews?per_page=30", base);
        std::thread::spawn(move || fetch_reviews(&client, &token, &url))
    };
    let files_handle = {
        let client = Arc::clone(&client);
        let token = token.to_string();
        let url = format!("{}/files?per_page=30", base);
        std::thread::spawn(move || fetch_files(&client, &token, &url))
    };
    let ci_handle = {
        let client = Arc::clone(&client);
        let token = token.to_string();
        let coords = coords.clone();
        let sha = pr.head.sha.clone();
        std::thread::spawn(move || fetch_ci_summary(&client, &token, &coords, &sha))
    };

    let reviews = reviews_handle.join().unwrap_or_default();
    let files = files_handle.join().unwrap_or_default();
    let ci = ci_handle
        .join()
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default();

    Ok(PullRequestDetail {
        number: pr.number,
        title: pr.title,
        author: pr.user.map(|u| u.login).unwrap_or_else(|| "?".into()),
        state: derive_state(&pr.state, pr.merged_at.as_deref()),
        draft: pr.draft,
        head_label: if pr.head.label.is_empty() {
            pr.head.ref_name.clone()
        } else {
            pr.head.label
        },
        base_ref: pr.base.ref_name,
        body: pr.body.unwrap_or_default(),
        additions: pr.additions,
        deletions: pr.deletions,
        commits: pr.commits,
        changed_files: pr.changed_files,
        reviewers: pr
            .requested_reviewers
            .into_iter()
            .map(|u| u.login)
            .collect(),
        reviews,
        ci,
        files,
    })
}

// ---------- Helpers ----------

fn fetch_reviews(
    client: &reqwest::blocking::Client,
    token: &str,
    url: &str,
) -> ReviewsSummary {
    let mut summary = ReviewsSummary::default();
    let resp = match client
        .get(url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
    {
        Ok(r) => r,
        Err(_) => return summary,
    };
    if !resp.status().is_success() {
        return summary;
    }
    let body = resp.text().unwrap_or_default();
    if let Ok(list) = serde_json::from_str::<Vec<ApiReview>>(&body) {
        for r in list {
            match r.state.as_str() {
                "APPROVED" => summary.approved += 1,
                "CHANGES_REQUESTED" => summary.changes_requested += 1,
                "COMMENTED" => summary.commented += 1,
                _ => {}
            }
        }
    }
    summary
}

fn fetch_files(
    client: &reqwest::blocking::Client,
    token: &str,
    url: &str,
) -> Vec<PullFile> {
    let resp = match client
        .get(url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let body = resp.text().unwrap_or_default();
    serde_json::from_str::<Vec<ApiPullFile>>(&body)
        .unwrap_or_default()
        .into_iter()
        .map(map_file)
        .collect()
}

fn fetch_ci_summary(
    client: &reqwest::blocking::Client,
    token: &str,
    coords: &RepoCoords,
    sha: &str,
) -> Result<CiSummary, String> {
    if sha.is_empty() {
        return Ok(CiSummary::default());
    }
    let url = format!(
        "https://api.github.com/repos/{}/{}/commits/{}/check-runs?per_page=50",
        coords.owner, coords.repo, sha
    );
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("CI: {}", e))?;
    if !resp.status().is_success() {
        return Ok(CiSummary::default());
    }
    let body = resp.text().map_err(|e| format!("CI read: {}", e))?;
    let runs: ApiCheckRunsResponse =
        serde_json::from_str(&body).map_err(|e| format!("CI JSON: {}", e))?;
    let mut summary = CiSummary::default();
    for run in runs.check_runs {
        summary.total += 1;
        // GitHub treats `status` (queued/in_progress/completed) and
        // `conclusion` (success/failure/cancelled/...) separately. A run
        // counts as "pending" until it has a conclusion.
        match (run.status.as_str(), run.conclusion.as_deref()) {
            ("completed", Some("success")) | ("completed", Some("neutral")) => {
                summary.success += 1
            }
            ("completed", Some("failure"))
            | ("completed", Some("cancelled"))
            | ("completed", Some("timed_out"))
            | ("completed", Some("action_required")) => summary.failure += 1,
            _ => summary.pending += 1,
        }
    }
    Ok(summary)
}

fn map_summary(raw: ApiPullSummary) -> PullRequest {
    PullRequest {
        number: raw.number,
        title: raw.title,
        author: raw.user.map(|u| u.login).unwrap_or_else(|| "?".into()),
        state: derive_state(&raw.state, raw.merged_at.as_deref()),
        draft: raw.draft,
        head_label: if raw.head.label.is_empty() {
            raw.head.ref_name.clone()
        } else {
            raw.head.label
        },
        base_ref: raw.base.ref_name,
        head_sha: raw.head.sha,
    }
}

fn map_file(raw: ApiPullFile) -> PullFile {
    PullFile {
        filename: raw.filename,
        status: match raw.status.as_str() {
            "added" => FileStatus::Added,
            "modified" => FileStatus::Modified,
            "removed" => FileStatus::Removed,
            "renamed" => FileStatus::Renamed,
            _ => FileStatus::Other,
        },
        additions: raw.additions,
        deletions: raw.deletions,
    }
}

fn derive_state(state: &str, merged_at: Option<&str>) -> PullState {
    match state {
        "open" => PullState::Open,
        _ if merged_at.is_some() => PullState::Merged,
        _ => PullState::Closed,
    }
}
