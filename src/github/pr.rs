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

/// Full PR data — populates the detail view. Fetched in one parallel
/// roundtrip when the user opens a PR; everything for the 4 tabs is
/// rolled in so subsequent tab switches are instant.
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
    // ── Per-tab payloads (lazy-loaded together with the summary above).
    /// Commits on the PR, oldest first — matches what GitHub's Commits
    /// tab shows.
    pub commit_list: Vec<PullCommit>,
    /// Chronological mix of issue comments + review bodies — populates
    /// the Conversation tab.
    pub conversation: Vec<ConversationEntry>,
    /// Detailed check runs — populates the Checks tab. The `ci`
    /// `CiSummary` above keeps roll-up counts for headline rendering.
    pub check_runs: Vec<CheckRunDetail>,
}

#[derive(Debug, Clone)]
pub struct PullCommit {
    pub sha: String,
    pub short_sha: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

#[derive(Debug, Clone)]
pub struct ConversationEntry {
    /// Stable comment id from GitHub. `None` for entries that don't have
    /// an id (e.g. reviews — they have their own id but we don't use it).
    pub id: Option<u64>,
    /// `Some(parent_id)` for review-comment replies (threaded under the
    /// parent in the Conversation tab). `None` for top-level entries.
    pub parent_id: Option<u64>,
    pub author: String,
    pub when: String,
    pub body: String,
    pub kind: ConversationKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationKind {
    /// Plain top-level comment (`POST /issues/{n}/comments`).
    Comment,
    /// PR-level review with an aggregate state.
    Review {
        state: ReviewState,
    },
    /// Inline review comment on a specific file/line. Top-level review
    /// comments live in the feed; replies (set via `in_reply_to_id`) are
    /// rendered as children of their parent.
    ReviewComment {
        file: String,
        line: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Other,
}

#[derive(Debug, Clone)]
pub struct CheckRunDetail {
    pub name: String,
    pub status: CheckStatus,
    pub conclusion: Option<CheckConclusion>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Queued,
    InProgress,
    Completed,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckConclusion {
    Success,
    Failure,
    Neutral,
    Cancelled,
    TimedOut,
    ActionRequired,
    Skipped,
    Other,
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

// Raw shapes for the additional Phase-2A endpoints.
#[derive(Deserialize)]
struct ApiCommitItem {
    #[serde(default)]
    sha: String,
    commit: ApiCommitInner,
}

#[derive(Deserialize)]
struct ApiCommitInner {
    #[serde(default)]
    message: String,
    author: Option<ApiCommitAuthor>,
}

#[derive(Deserialize)]
struct ApiCommitAuthor {
    #[serde(default)]
    name: String,
    #[serde(default)]
    date: String,
}

#[derive(Deserialize)]
struct ApiIssueComment {
    #[serde(default)]
    id: u64,
    user: Option<ApiUser>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    created_at: String,
}

#[derive(Deserialize)]
struct ApiReviewComment {
    #[serde(default)]
    id: u64,
    /// Set on replies — points at the comment this one is replying to.
    #[serde(default, rename = "in_reply_to_id")]
    in_reply_to_id: Option<u64>,
    user: Option<ApiUser>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    path: String,
    /// `line` is GitHub's preferred field for new comments; `position` is
    /// the legacy diff-offset field. We prefer `line` and fall back to
    /// nothing for now (don't show a line if absent).
    #[serde(default)]
    line: Option<u64>,
}

#[derive(Deserialize)]
struct ApiReviewWithBody {
    user: Option<ApiUser>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    submitted_at: String,
}

#[derive(Deserialize)]
struct ApiCheckRunDetail {
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
}

#[derive(Deserialize)]
struct ApiWorkflowRunsResponse {
    #[serde(default)]
    workflow_runs: Vec<ApiWorkflowRun>,
}

#[derive(Deserialize)]
struct ApiWorkflowRun {
    #[serde(default)]
    id: u64,
    /// "push" / "pull_request" / "schedule" / "dynamic" / ...
    /// GitHub uses `event=dynamic` for internal auto-pipelines (the
    /// Copilot code-review workflow falls in this bucket). User CI
    /// uses standard events, so filtering on `event != dynamic` is a
    /// reliable proxy for "what the GitHub web UI counts".
    #[serde(default)]
    event: String,
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

    // Spawn the 6 secondary fetches concurrently — total latency ≈
    // max(call) rather than sum(call). Each helper degrades to an
    // empty value on error so a single failing endpoint never breaks
    // the whole detail view.
    let reviews_handle = {
        let client = Arc::clone(&client);
        let token = token.to_string();
        let url = format!("{}/reviews?per_page=30", base);
        std::thread::spawn(move || fetch_reviews(&client, &token, &url))
    };
    let review_entries_handle = {
        let client = Arc::clone(&client);
        let token = token.to_string();
        let url = format!("{}/reviews?per_page=30", base);
        std::thread::spawn(move || fetch_review_entries(&client, &token, &url))
    };
    let files_handle = {
        let client = Arc::clone(&client);
        let token = token.to_string();
        let url = format!("{}/files?per_page=30", base);
        std::thread::spawn(move || fetch_files(&client, &token, &url))
    };
    let check_runs_handle = {
        let client = Arc::clone(&client);
        let token = token.to_string();
        let coords = coords.clone();
        let sha = pr.head.sha.clone();
        std::thread::spawn(move || fetch_check_runs_detail(&client, &token, &coords, &sha))
    };
    let commits_handle = {
        let client = Arc::clone(&client);
        let token = token.to_string();
        let url = format!("{}/commits?per_page=100", base);
        std::thread::spawn(move || fetch_pull_commits(&client, &token, &url))
    };
    let comments_handle = {
        let client = Arc::clone(&client);
        let token = token.to_string();
        let url = format!(
            "https://api.github.com/repos/{}/{}/issues/{}/comments?per_page=100",
            coords.owner, coords.repo, number
        );
        std::thread::spawn(move || fetch_issue_comments(&client, &token, &url))
    };
    let review_comments_handle = {
        let client = Arc::clone(&client);
        let token = token.to_string();
        let url = format!("{}/comments?per_page=100", base);
        std::thread::spawn(move || fetch_review_comments(&client, &token, &url))
    };

    let reviews = reviews_handle.join().unwrap_or_default();
    let review_entries = review_entries_handle.join().unwrap_or_default();
    let files = files_handle.join().unwrap_or_default();
    let check_runs = check_runs_handle.join().unwrap_or_default();
    // Derive the rollup summary from the already-filtered check runs so
    // the headline numbers agree with the Checks tab list.
    let ci = ci_from_filtered_runs(&check_runs);
    let commit_list = commits_handle.join().unwrap_or_default();
    let issue_comments = comments_handle.join().unwrap_or_default();
    let review_comments = review_comments_handle.join().unwrap_or_default();

    // Merge review bodies + issue comments into a chronological feed
    // for the Conversation tab. We collect both into entries with
    // timestamps then sort on string compare (ISO-8601 sorts
    // lexicographically).
    let mut conversation: Vec<(String, ConversationEntry)> = Vec::new();
    for c in issue_comments {
        let when = c.created_at.clone();
        conversation.push((
            when,
            ConversationEntry {
                id: Some(c.id),
                parent_id: None,
                author: c.user.map(|u| u.login).unwrap_or_else(|| "?".into()),
                when: short_relative(&c.created_at),
                body: c.body.unwrap_or_default(),
                kind: ConversationKind::Comment,
            },
        ));
    }
    for r in review_entries {
        let body = r.body.unwrap_or_default();
        // Reviews with no body and state "COMMENTED" are usually
        // "Reviewed N files" markers — filter to reduce noise.
        if body.is_empty() && r.state == "COMMENTED" {
            continue;
        }
        let state = match r.state.as_str() {
            "APPROVED" => ReviewState::Approved,
            "CHANGES_REQUESTED" => ReviewState::ChangesRequested,
            "COMMENTED" => ReviewState::Commented,
            _ => ReviewState::Other,
        };
        let when = r.submitted_at.clone();
        conversation.push((
            when,
            ConversationEntry {
                id: None,
                parent_id: None,
                author: r.user.map(|u| u.login).unwrap_or_else(|| "?".into()),
                when: short_relative(&r.submitted_at),
                body,
                kind: ConversationKind::Review { state },
            },
        ));
    }
    for rc in review_comments {
        let when = rc.created_at.clone();
        conversation.push((
            when,
            ConversationEntry {
                id: Some(rc.id),
                parent_id: rc.in_reply_to_id,
                author: rc.user.map(|u| u.login).unwrap_or_else(|| "?".into()),
                when: short_relative(&rc.created_at),
                body: rc.body.unwrap_or_default(),
                kind: ConversationKind::ReviewComment {
                    file: rc.path,
                    line: rc.line,
                },
            },
        ));
    }
    conversation.sort_by(|(a, _), (b, _)| a.cmp(b));
    let conversation: Vec<ConversationEntry> = conversation.into_iter().map(|(_, e)| e).collect();

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
        commit_list,
        conversation,
        check_runs,
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

fn fetch_review_entries(
    client: &reqwest::blocking::Client,
    token: &str,
    url: &str,
) -> Vec<ApiReviewWithBody> {
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
    serde_json::from_str::<Vec<ApiReviewWithBody>>(&body).unwrap_or_default()
}

fn fetch_pull_commits(
    client: &reqwest::blocking::Client,
    token: &str,
    url: &str,
) -> Vec<PullCommit> {
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
    let raw: Vec<ApiCommitItem> = serde_json::from_str(&body).unwrap_or_default();
    raw.into_iter()
        .map(|c| {
            let subject = c
                .commit
                .message
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            let (author, date) = match c.commit.author {
                Some(a) => (a.name, short_relative(&a.date)),
                None => ("?".to_string(), String::new()),
            };
            PullCommit {
                short_sha: c.sha.chars().take(7).collect(),
                sha: c.sha,
                author,
                date,
                subject,
            }
        })
        .collect()
}

fn fetch_issue_comments(
    client: &reqwest::blocking::Client,
    token: &str,
    url: &str,
) -> Vec<ApiIssueComment> {
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
    serde_json::from_str::<Vec<ApiIssueComment>>(&body).unwrap_or_default()
}

fn fetch_review_comments(
    client: &reqwest::blocking::Client,
    token: &str,
    url: &str,
) -> Vec<ApiReviewComment> {
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
    serde_json::from_str::<Vec<ApiReviewComment>>(&body).unwrap_or_default()
}

fn fetch_check_runs_detail(
    client: &reqwest::blocking::Client,
    token: &str,
    coords: &RepoCoords,
    sha: &str,
) -> Vec<CheckRunDetail> {
    if sha.is_empty() {
        return Vec::new();
    }
    // 1. Fetch the visible (= user CI) workflow run IDs in parallel
    //    with the raw check-runs list, then keep only check-runs whose
    //    parent workflow is user-visible. This matches what GitHub's
    //    web UI counts as "Checks N" on the PR detail page.
    let visible_ids = fetch_visible_workflow_run_ids(client, token, coords, sha);

    let url = format!(
        "https://api.github.com/repos/{}/{}/commits/{}/check-runs?per_page=50",
        coords.owner, coords.repo, sha
    );
    let resp = match client
        .get(&url)
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
    #[derive(Deserialize)]
    struct Wrapper {
        #[serde(default)]
        check_runs: Vec<ApiCheckRunDetail>,
    }
    let w: Wrapper = serde_json::from_str(&body).unwrap_or(Wrapper { check_runs: vec![] });
    w.check_runs
        .into_iter()
        .filter(|r| {
            // Drop runs whose parent workflow isn't in the user-visible
            // set. Empty set (no user CI configured) → drop everything,
            // which is also what GitHub UI does.
            match r.html_url.as_deref().and_then(extract_workflow_run_id) {
                Some(id) => visible_ids.contains(&id),
                None => false,
            }
        })
        .map(|r| CheckRunDetail {
            name: r.name,
            status: match r.status.as_str() {
                "queued" => CheckStatus::Queued,
                "in_progress" => CheckStatus::InProgress,
                "completed" => CheckStatus::Completed,
                _ => CheckStatus::Other,
            },
            conclusion: r.conclusion.as_deref().map(|c| match c {
                "success" => CheckConclusion::Success,
                "failure" => CheckConclusion::Failure,
                "neutral" => CheckConclusion::Neutral,
                "cancelled" => CheckConclusion::Cancelled,
                "timed_out" => CheckConclusion::TimedOut,
                "action_required" => CheckConclusion::ActionRequired,
                "skipped" => CheckConclusion::Skipped,
                _ => CheckConclusion::Other,
            }),
            url: r.html_url,
        })
        .collect()
}

/// Fetch the workflow runs attached to a commit SHA and return the IDs
/// of those that count as "user-visible" CI runs. The `event=dynamic`
/// type covers GitHub's internal auto-pipelines (notably Copilot's PR
/// reviewer), which GitHub itself hides from the PR detail's "Checks N"
/// badge — we mirror that filter here so our counts agree with the web UI.
fn fetch_visible_workflow_run_ids(
    client: &reqwest::blocking::Client,
    token: &str,
    coords: &RepoCoords,
    sha: &str,
) -> std::collections::HashSet<u64> {
    use std::collections::HashSet;
    if sha.is_empty() {
        return HashSet::new();
    }
    let url = format!(
        "https://api.github.com/repos/{}/{}/actions/runs?head_sha={}&per_page=100",
        coords.owner, coords.repo, sha
    );
    let resp = match client
        .get(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
    {
        Ok(r) => r,
        Err(_) => return HashSet::new(),
    };
    if !resp.status().is_success() {
        return HashSet::new();
    }
    let body = resp.text().unwrap_or_default();
    let parsed: ApiWorkflowRunsResponse = serde_json::from_str(&body)
        .unwrap_or(ApiWorkflowRunsResponse { workflow_runs: vec![] });
    parsed
        .workflow_runs
        .into_iter()
        .filter(|r| r.event != "dynamic")
        .map(|r| r.id)
        .collect()
}

/// Extract the workflow run id from a check_run.html_url like
/// `https://github.com/<owner>/<repo>/actions/runs/<id>/job/<jobid>`.
fn extract_workflow_run_id(url: &str) -> Option<u64> {
    let after = url.split("/actions/runs/").nth(1)?;
    after.split('/').next()?.parse().ok()
}

/// Best-effort "5m ago" / "2d ago" formatter for ISO-8601 GitHub
/// timestamps. Falls back to the raw timestamp on parse failures.
fn short_relative(iso: &str) -> String {
    use chrono::{DateTime, Utc};
    let parsed: Option<DateTime<Utc>> = DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|d| d.with_timezone(&Utc));
    let Some(then) = parsed else {
        return iso.to_string();
    };
    let now = Utc::now();
    let secs = (now - then).num_seconds().max(0);
    if secs < 60 {
        return format!("{}s ago", secs);
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m ago", mins);
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h ago", hours);
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{}d ago", days);
    }
    let months = days / 30;
    if months < 12 {
        return format!("{}mo ago", months);
    }
    format!("{}y ago", months / 12)
}

/// Roll up a list of (already-filtered) check runs into the small
/// pass/fail/pending counters used by the sub-header summary span.
fn ci_from_filtered_runs(runs: &[CheckRunDetail]) -> CiSummary {
    let mut summary = CiSummary::default();
    for run in runs {
        summary.total += 1;
        match (run.status, run.conclusion) {
            (CheckStatus::Completed, Some(CheckConclusion::Success))
            | (CheckStatus::Completed, Some(CheckConclusion::Neutral))
            | (CheckStatus::Completed, Some(CheckConclusion::Skipped)) => {
                summary.success += 1
            }
            (CheckStatus::Completed, Some(CheckConclusion::Failure))
            | (CheckStatus::Completed, Some(CheckConclusion::Cancelled))
            | (CheckStatus::Completed, Some(CheckConclusion::TimedOut))
            | (CheckStatus::Completed, Some(CheckConclusion::ActionRequired)) => {
                summary.failure += 1
            }
            _ => summary.pending += 1,
        }
    }
    summary
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
