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

use serde::{Deserialize, Serialize};

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
    /// Labels attached to the PR, in the order GitHub returned them.
    /// Surfaced in the list as short coloured chips next to the title.
    pub labels: Vec<Label>,
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
    /// GitHub's GraphQL node ID — needed for the `convertPullRequest
    /// ToDraft` / `markPullRequestReadyForReview` mutations, which
    /// the REST API doesn't expose.
    pub node_id: String,
    pub title: String,
    pub author: String,
    pub state: PullState,
    pub draft: bool,
    pub head_label: String,
    pub base_ref: String,
    /// Head commit SHA — anchors the on-demand `git show` for files
    /// and commits when drilling into the existing diff / detail views.
    pub head_sha: String,
    pub body: String,
    pub additions: u64,
    pub deletions: u64,
    pub commits: u64,
    pub changed_files: u64,
    pub reviewers: Vec<String>,
    pub reviews: ReviewsSummary,
    pub ci: CiSummary,
    pub files: Vec<PullFile>,
    /// Labels currently attached to the PR.
    pub labels: Vec<Label>,
    /// Aggregate merge readiness — drives the badge shown alongside
    /// the PR title (Ready to merge / Conflicts / Blocked / etc).
    pub mergeability: Mergeability,
    /// Relative timestamp of when the PR was opened, e.g. "13h ago".
    pub opened_when: String,
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

/// Mergeability summary derived from GitHub's `mergeable` and
/// `mergeable_state` fields. Order roughly matches the priority we
/// surface in the badge (worst state wins when ambiguous).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mergeability {
    /// Still being computed by GitHub.
    Unknown,
    /// PR is in draft mode.
    Draft,
    /// Already merged.
    Merged,
    /// Closed without merging.
    Closed,
    /// Conflicts with the base branch — needs manual resolution.
    Conflicts,
    /// CI checks failing.
    ChecksFailing,
    /// Required reviews missing (branch protection).
    Blocked,
    /// Out of date with base branch but mergeable in principle.
    Behind,
    /// All checks green, no conflicts, ready to merge.
    Ready,
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
    /// Raw unified diff for the file as returned by GitHub. `None`
    /// for binary files (where GitHub omits the field) or when the
    /// diff exceeds GitHub's per-file size limit.
    pub patch: Option<String>,
}

/// Repo-level label (colour as a `#rrggbb` hex string when present).
#[derive(Debug, Clone)]
pub struct Label {
    pub name: String,
    pub color: Option<String>,
    pub description: Option<String>,
}

/// Full commit detail with files + patches, returned by `GET
/// /repos/{owner}/{repo}/commits/{sha}`. Used when the user drills
/// into a commit from the Commits tab.
#[derive(Debug, Clone)]
pub struct CommitDetail {
    pub sha: String,
    pub author: String,
    pub date: String,
    pub message: String,
    pub additions: u64,
    pub deletions: u64,
    pub files: Vec<PullFile>,
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
    #[serde(default)]
    labels: Vec<ApiLabel>,
}

#[derive(Deserialize)]
struct ApiPullDetail {
    number: u64,
    /// GraphQL node ID — required for draft toggle mutations (REST
    /// can't flip the draft flag).
    #[serde(default)]
    node_id: String,
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
    #[serde(default)]
    labels: Vec<ApiLabel>,
    /// `true` / `false` / `null` (still computing). GitHub takes a few
    /// seconds after a push to compute this.
    #[serde(default)]
    mergeable: Option<bool>,
    /// `clean` / `dirty` / `unstable` / `blocked` / `behind` / `draft`
    /// / `unknown` — see GitHub docs. We map this to our richer
    /// `Mergeability` enum below.
    #[serde(default)]
    mergeable_state: String,
    /// ISO-8601 timestamp of when the PR was opened.
    #[serde(default)]
    created_at: String,
}

#[derive(Deserialize)]
struct ApiLabel {
    #[serde(default)]
    name: String,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

impl From<ApiLabel> for Label {
    fn from(l: ApiLabel) -> Self {
        Label {
            name: l.name,
            color: l.color,
            description: l.description,
        }
    }
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
    /// Unified-diff text. Missing on binary files and on diffs
    /// exceeding GitHub's per-file size limit.
    #[serde(default)]
    patch: Option<String>,
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
    // Fetch every state so the view can filter client-side between
    // Open / Merged / Closed / All tabs without re-querying GitHub.
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls?state=all&per_page=50&sort=updated&direction=desc",
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

    let state = derive_state(&pr.state, pr.merged_at.as_deref());
    let mergeability = derive_mergeability(
        state,
        pr.draft,
        pr.mergeable,
        &pr.mergeable_state,
        &reviews,
        &ci,
    );

    let labels: Vec<Label> = pr.labels.into_iter().map(Into::into).collect();

    Ok(PullRequestDetail {
        number: pr.number,
        node_id: pr.node_id,
        title: pr.title,
        author: pr.user.map(|u| u.login).unwrap_or_else(|| "?".into()),
        state,
        draft: pr.draft,
        head_sha: pr.head.sha.clone(),
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
        labels,
        commit_list,
        conversation,
        check_runs,
        mergeability,
        opened_when: short_relative(&pr.created_at),
    })
}

/// Combine GitHub's signals into a single readiness flag for the badge.
/// Worst state wins when they conflict — e.g. a PR that's mergeable but
/// with failing CI surfaces as `ChecksFailing`, not `Ready`.
fn derive_mergeability(
    state: PullState,
    draft: bool,
    mergeable: Option<bool>,
    mergeable_state: &str,
    reviews: &ReviewsSummary,
    ci: &CiSummary,
) -> Mergeability {
    if matches!(state, PullState::Merged) {
        return Mergeability::Merged;
    }
    if matches!(state, PullState::Closed) {
        return Mergeability::Closed;
    }
    if draft {
        return Mergeability::Draft;
    }
    // `mergeable_state` is more specific than `mergeable` when GitHub
    // has computed it. Prefer the string when present and recognised.
    match mergeable_state {
        "dirty" => return Mergeability::Conflicts,
        "unstable" => return Mergeability::ChecksFailing,
        "blocked" => return Mergeability::Blocked,
        "behind" => return Mergeability::Behind,
        "draft" => return Mergeability::Draft,
        "clean" => {
            // Clean per GitHub — double-check our local roll-ups too,
            // since GitHub sometimes marks clean while a check is
            // still pending.
            if ci.failure > 0 {
                return Mergeability::ChecksFailing;
            }
            if reviews.changes_requested > 0 {
                return Mergeability::Blocked;
            }
            return Mergeability::Ready;
        }
        _ => {}
    }
    // Fall back to the boolean flag.
    match mergeable {
        Some(true) => {
            if ci.failure > 0 {
                Mergeability::ChecksFailing
            } else if reviews.changes_requested > 0 {
                Mergeability::Blocked
            } else {
                Mergeability::Ready
            }
        }
        Some(false) => Mergeability::Conflicts,
        None => Mergeability::Unknown,
    }
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
        labels: raw.labels.into_iter().map(Into::into).collect(),
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
        patch: raw.patch,
    }
}

fn derive_state(state: &str, merged_at: Option<&str>) -> PullState {
    match state {
        "open" => PullState::Open,
        _ if merged_at.is_some() => PullState::Merged,
        _ => PullState::Closed,
    }
}

// ---------- Write actions on PR comments ----------
//
// The GitHub API splits comment endpoints by where the comment lives:
//
// - Top-level "conversation" comments → `/repos/{o}/{r}/issues/{n}/comments`
//   (PRs are also issues internally, so they share this endpoint)
// - Inline review comments on files   → `/repos/{o}/{r}/pulls/{n}/comments`
//
// Edit + delete use the same split but go through:
// `/repos/{o}/{r}/issues/comments/{id}` and
// `/repos/{o}/{r}/pulls/comments/{id}` respectively.

#[derive(Serialize)]
struct CommentBody<'a> {
    body: &'a str,
}

#[derive(Serialize)]
struct ReplyBody<'a> {
    body: &'a str,
    in_reply_to: u64,
    // Optional, but GitHub recommends including the commit_id+path the
    // parent is anchored on. We let the API fall back when these are
    // omitted — replies to existing threads inherit the parent's anchor.
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
}

/// Post a new top-level conversation comment on a PR.
pub fn post_issue_comment(
    token: &str,
    coords: &RepoCoords,
    pr_number: u64,
    body: &str,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}/comments",
        coords.owner, coords.repo, pr_number
    );
    write_json(token, &url, reqwest::Method::POST, &CommentBody { body })
}

/// Reply to an existing review comment thread (file-level inline thread).
/// `parent_id` is the comment we're replying to (GitHub flattens the
/// thread so this can be any comment in the chain — the API normalises
/// `in_reply_to` to the thread root anyway).
pub fn post_review_comment_reply(
    token: &str,
    coords: &RepoCoords,
    pr_number: u64,
    body: &str,
    parent_id: u64,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}/comments",
        coords.owner, coords.repo, pr_number
    );
    write_json(
        token,
        &url,
        reqwest::Method::POST,
        &ReplyBody {
            body,
            in_reply_to: parent_id,
            commit_id: None,
            path: None,
        },
    )
}

/// Edit an existing top-level conversation comment.
pub fn patch_issue_comment(
    token: &str,
    coords: &RepoCoords,
    comment_id: u64,
    body: &str,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/comments/{}",
        coords.owner, coords.repo, comment_id
    );
    write_json(token, &url, reqwest::Method::PATCH, &CommentBody { body })
}

/// Edit an existing inline review comment.
pub fn patch_review_comment(
    token: &str,
    coords: &RepoCoords,
    comment_id: u64,
    body: &str,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/comments/{}",
        coords.owner, coords.repo, comment_id
    );
    write_json(token, &url, reqwest::Method::PATCH, &CommentBody { body })
}

/// Delete a top-level conversation comment.
pub fn delete_issue_comment(
    token: &str,
    coords: &RepoCoords,
    comment_id: u64,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/comments/{}",
        coords.owner, coords.repo, comment_id
    );
    delete_request(token, &url)
}

/// Delete an inline review comment.
pub fn delete_review_comment(
    token: &str,
    coords: &RepoCoords,
    comment_id: u64,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/comments/{}",
        coords.owner, coords.repo, comment_id
    );
    delete_request(token, &url)
}

/// Possible verdicts for a PR-level review.
#[derive(Debug, Clone, Copy)]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
    Comment,
}

impl ReviewVerdict {
    fn event_str(self) -> &'static str {
        match self {
            ReviewVerdict::Approve => "APPROVE",
            ReviewVerdict::RequestChanges => "REQUEST_CHANGES",
            ReviewVerdict::Comment => "COMMENT",
        }
    }
}

#[derive(Serialize)]
struct ReviewSubmission<'a> {
    event: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<&'a str>,
}

/// Submit a PR review (Approve / Request changes / Comment). `body` is
/// optional for Approve but required by GitHub for RequestChanges.
pub fn submit_review(
    token: &str,
    coords: &RepoCoords,
    pr_number: u64,
    verdict: ReviewVerdict,
    body: Option<&str>,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}/reviews",
        coords.owner, coords.repo, pr_number
    );
    let body = body.filter(|b| !b.is_empty());
    write_json(
        token,
        &url,
        reqwest::Method::POST,
        &ReviewSubmission {
            event: verdict.event_str(),
            body,
        },
    )
}

#[derive(Debug, Clone, Copy)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    fn as_str(self) -> &'static str {
        match self {
            MergeMethod::Merge => "merge",
            MergeMethod::Squash => "squash",
            MergeMethod::Rebase => "rebase",
        }
    }
}

#[derive(Serialize)]
struct MergeBody<'a> {
    merge_method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_title: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_message: Option<&'a str>,
}

/// Merge a PR. `commit_title` / `commit_message` are optional — GitHub
/// uses sensible defaults if omitted (PR title / description).
pub fn merge_pull_request(
    token: &str,
    coords: &RepoCoords,
    pr_number: u64,
    method: MergeMethod,
    commit_title: Option<&str>,
    commit_message: Option<&str>,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}/merge",
        coords.owner, coords.repo, pr_number
    );
    write_json(
        token,
        &url,
        reqwest::Method::PUT,
        &MergeBody {
            merge_method: method.as_str(),
            commit_title: commit_title.filter(|s| !s.is_empty()),
            commit_message: commit_message.filter(|s| !s.is_empty()),
        },
    )
}

fn write_json<T: Serialize>(
    token: &str,
    url: &str,
    method: reqwest::Method,
    body: &T,
) -> Result<(), String> {
    let client = http_client()?;
    let json = serde_json::to_string(body).map_err(|e| format!("encode body: {}", e))?;
    let resp = client
        .request(method, url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("Content-Type", "application/json")
        .body(json)
        .send()
        .map_err(|e| format!("GitHub write: {}", e))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        Err(humanize_github_error(status, &body))
    }
}

fn delete_request(token: &str, url: &str) -> Result<(), String> {
    let client = http_client()?;
    let resp = client
        .delete(url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("GitHub delete: {}", e))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        Err(humanize_github_error(status, &body))
    }
}

/// Fetch one commit's full data (message body, author, files +
/// patches). Used by the Commits-tab drill-down — gives us per-file
/// diffs without going through the PR-files endpoint.
pub fn fetch_commit_detail(
    token: &str,
    coords: &RepoCoords,
    sha: &str,
) -> Result<CommitDetail, String> {
    let client = http_client()?;
    let url = format!(
        "https://api.github.com/repos/{}/{}/commits/{}",
        coords.owner, coords.repo, sha
    );
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("GitHub /commits: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(humanize_github_error(status, &body));
    }
    let body = resp.text().map_err(|e| format!("/commits read: {}", e))?;
    let raw: ApiCommitFull =
        serde_json::from_str(&body).map_err(|e| format!("/commits JSON: {}", e))?;
    Ok(CommitDetail {
        sha: raw.sha,
        author: raw
            .commit
            .author
            .as_ref()
            .map(|a| a.name.clone())
            .unwrap_or_default(),
        date: raw
            .commit
            .author
            .as_ref()
            .map(|a| short_relative(&a.date))
            .unwrap_or_default(),
        message: raw.commit.message,
        additions: raw.stats.as_ref().map(|s| s.additions).unwrap_or(0),
        deletions: raw.stats.as_ref().map(|s| s.deletions).unwrap_or(0),
        files: raw.files.into_iter().map(map_file).collect(),
    })
}

#[derive(Deserialize)]
struct ApiCommitFull {
    #[serde(default)]
    sha: String,
    commit: ApiCommitInner,
    #[serde(default)]
    stats: Option<ApiCommitStats>,
    #[serde(default)]
    files: Vec<ApiPullFile>,
}

#[derive(Deserialize)]
struct ApiCommitStats {
    #[serde(default)]
    additions: u64,
    #[serde(default)]
    deletions: u64,
}

// ─── PR creation ─────────────────────────────────────────────────

#[derive(Serialize)]
struct CreatePrBody<'a> {
    title: &'a str,
    head: &'a str,
    base: &'a str,
    #[serde(skip_serializing_if = "str::is_empty")]
    body: &'a str,
    draft: bool,
}

/// `POST /repos/{owner}/{repo}/pulls`. Returns the new PR's number
/// on success — the caller can then open its detail view.
pub fn create_pull_request(
    token: &str,
    coords: &RepoCoords,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<u64, String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls",
        coords.owner, coords.repo
    );
    let client = http_client()?;
    let json = serde_json::to_string(&CreatePrBody {
        title,
        head,
        base,
        body,
        draft,
    })
    .map_err(|e| format!("encode body: {}", e))?;
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("Content-Type", "application/json")
        .body(json)
        .send()
        .map_err(|e| format!("GitHub create PR: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let raw = resp.text().unwrap_or_default();
        return Err(humanize_github_error(status, &raw));
    }
    let raw = resp.text().map_err(|e| format!("create PR read: {}", e))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("create PR JSON: {}", e))?;
    let number = parsed
        .get("number")
        .and_then(|n| n.as_u64())
        .ok_or_else(|| "GitHub response missing PR number".to_string())?;
    Ok(number)
}

// ─── State-changing actions (close / reopen / draft toggle / labels / reviewers) ───

#[derive(Serialize)]
struct StatePatch<'a> {
    state: &'a str,
}

/// Close (without merge) or reopen a PR via `PATCH /pulls/{n}`.
/// `state` must be `"open"` or `"closed"`.
pub fn set_pull_request_state(
    token: &str,
    coords: &RepoCoords,
    pr_number: u64,
    state: &str,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}",
        coords.owner, coords.repo, pr_number
    );
    write_json(token, &url, reqwest::Method::PATCH, &StatePatch { state })
}

/// Toggle a PR's draft flag. GitHub's REST API does not expose this
/// — only the GraphQL `convertPullRequestToDraft` / `markPullRequest
/// ReadyForReview` mutations work, so we POST to `/graphql` directly.
pub fn set_pull_request_draft(
    token: &str,
    node_id: &str,
    draft: bool,
) -> Result<(), String> {
    let mutation = if draft {
        "convertPullRequestToDraft"
    } else {
        "markPullRequestReadyForReview"
    };
    // GraphQL escaping — node ids are opaque base64-ish strings, no
    // double quotes or backslashes, so plain interpolation is safe.
    let query = format!(
        r#"mutation {{ {mutation}(input: {{ pullRequestId: "{node_id}" }}) {{ pullRequest {{ id }} }} }}"#,
        mutation = mutation,
        node_id = node_id,
    );
    #[derive(Serialize)]
    struct GqlBody<'a> {
        query: &'a str,
    }
    let client = http_client()?;
    let body_json = serde_json::to_string(&GqlBody { query: &query })
        .map_err(|e| format!("encode GraphQL body: {}", e))?;
    let resp = client
        .post("https://api.github.com/graphql")
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("Content-Type", "application/json")
        .body(body_json)
        .send()
        .map_err(|e| format!("GitHub GraphQL: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(humanize_github_error(status, &body));
    }
    // GraphQL returns 200 even on logical errors — they show up in
    // a top-level `errors` array. Parse and surface the first one.
    let body = resp.text().map_err(|e| format!("GraphQL read: {}", e))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    if let Some(errs) = parsed.get("errors").and_then(|v| v.as_array()) {
        if let Some(first) = errs.first() {
            let msg = first
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("GraphQL error");
            return Err(msg.to_string());
        }
    }
    Ok(())
}

/// Fetch all labels available on a repo (the picker source). Paginated
/// at 100 per page — for any sane repo size, one page is enough.
pub fn list_repo_labels(
    token: &str,
    coords: &RepoCoords,
) -> Result<Vec<Label>, String> {
    let client = http_client()?;
    let url = format!(
        "https://api.github.com/repos/{}/{}/labels?per_page=100",
        coords.owner, coords.repo
    );
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("GitHub /labels: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(humanize_github_error(status, &body));
    }
    let body = resp.text().map_err(|e| format!("/labels read: {}", e))?;
    let raw: Vec<ApiLabel> =
        serde_json::from_str(&body).map_err(|e| format!("/labels JSON: {}", e))?;
    Ok(raw.into_iter().map(Into::into).collect())
}

#[derive(Serialize)]
struct LabelsBody<'a> {
    labels: &'a [String],
}

/// Replace the full set of labels on a PR (treated as an issue by GH).
pub fn set_pull_request_labels(
    token: &str,
    coords: &RepoCoords,
    pr_number: u64,
    labels: &[String],
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}/labels",
        coords.owner, coords.repo, pr_number
    );
    write_json(token, &url, reqwest::Method::PUT, &LabelsBody { labels })
}

/// Fetch the list of users that can be assigned / requested as
/// reviewers on this repo. GitHub's `/assignees` endpoint is the
/// canonical source for the picker.
pub fn list_repo_assignees(
    token: &str,
    coords: &RepoCoords,
) -> Result<Vec<String>, String> {
    let client = http_client()?;
    let url = format!(
        "https://api.github.com/repos/{}/{}/assignees?per_page=100",
        coords.owner, coords.repo
    );
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("GitHub /assignees: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(humanize_github_error(status, &body));
    }
    let body = resp.text().map_err(|e| format!("/assignees read: {}", e))?;
    let raw: Vec<ApiUser> = serde_json::from_str(&body)
        .map_err(|e| format!("/assignees JSON: {}", e))?;
    Ok(raw.into_iter().map(|u| u.login).filter(|l| !l.is_empty()).collect())
}

#[derive(Serialize)]
struct ReviewersBody<'a> {
    reviewers: &'a [String],
}

/// Request one or more reviewers on a PR. Users already requested or
/// who are the PR author cause a 422 — caller should filter beforehand.
pub fn request_pull_request_reviewers(
    token: &str,
    coords: &RepoCoords,
    pr_number: u64,
    users: &[String],
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}/requested_reviewers",
        coords.owner, coords.repo, pr_number
    );
    write_json(
        token,
        &url,
        reqwest::Method::POST,
        &ReviewersBody { reviewers: users },
    )
}

/// Withdraw a reviewer request from a PR.
pub fn remove_pull_request_reviewers(
    token: &str,
    coords: &RepoCoords,
    pr_number: u64,
    users: &[String],
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/pulls/{}/requested_reviewers",
        coords.owner, coords.repo, pr_number
    );
    let client = http_client()?;
    let body_json = serde_json::to_string(&ReviewersBody { reviewers: users })
        .map_err(|e| format!("encode reviewers body: {}", e))?;
    let resp = client
        .delete(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("Content-Type", "application/json")
        .body(body_json)
        .send()
        .map_err(|e| format!("GitHub delete reviewers: {}", e))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        Err(humanize_github_error(status, &body))
    }
}

/// Translate a GitHub REST API error response into a one-line, human-
/// friendly message suitable for the footer toast.
///
/// GitHub envelopes errors as `{ "message": "...", "errors": [...] }`,
/// where `errors[].message` (or `errors[]` as a bare string) carries
/// the actual validation reason. We pattern-match on the canonical
/// strings documented in GitHub's REST API reference and fall back to
/// status-code-specific phrasing when the message is unknown.
fn humanize_github_error(status: reqwest::StatusCode, body: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let top_message = parsed
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    // `errors` may be an array of bare strings *or* of `{message, code}`
    // objects depending on the endpoint — handle both.
    let first_error = parsed
        .get("errors")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .map(|e| match e {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Object(_) => e
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string(),
            _ => String::new(),
        })
        .unwrap_or_default();

    let scan = format!("{} {}", top_message, first_error).to_lowercase();

    // Self-review guard (422 on review submission, our most common case).
    if scan.contains("approve your own pull request") {
        return "Cannot approve your own PR (GitHub disallows self-review)".into();
    }
    if scan.contains("request changes on your own pull request") {
        return "Cannot request changes on your own PR (GitHub disallows self-review)"
            .into();
    }
    // Merge / state guards.
    if scan.contains("pull request is not mergeable")
        || scan.contains("pull request is in unstable state")
    {
        return "PR is not mergeable — resolve conflicts or wait for checks".into();
    }
    if scan.contains("head branch was modified") {
        return "Head branch changed since you started — refresh and retry".into();
    }
    if scan.contains("base branch was modified") {
        return "Base branch changed since you started — refresh and retry".into();
    }
    if scan.contains("required status check") {
        return "Required status checks haven't passed yet".into();
    }
    if scan.contains("at least 1 approving review") {
        return "PR needs an approving review before it can merge".into();
    }
    if scan.contains("review must be requested") {
        return "A reviewer must be requested before this action".into();
    }
    // Auth / rate / generic categories.
    if scan.contains("bad credentials") {
        return "GitHub token rejected — sign in again".into();
    }
    if scan.contains("api rate limit exceeded") || scan.contains("secondary rate limit")
    {
        return "GitHub rate limit reached — try again later".into();
    }
    if scan.contains("must have admin rights")
        || scan.contains("must have push access")
        || scan.contains("resource not accessible by")
    {
        return "Permission denied — your token lacks the required scope".into();
    }
    // Content validation.
    if scan.contains("body can't be blank") || scan.contains("body is too short") {
        return "Comment body cannot be empty".into();
    }
    if scan.contains("body is too long") {
        return "Comment is too long".into();
    }

    // Pick the most specific available detail string for the fallback.
    let detail = if !first_error.is_empty() {
        first_error
    } else if !top_message.is_empty() {
        top_message
    } else {
        body.lines().next().unwrap_or("").to_string()
    };
    match status.as_u16() {
        401 => format!("Authentication failed: {}", detail),
        403 => format!("Permission denied: {}", detail),
        404 => "Not found — resource may have been deleted".into(),
        409 => format!("Conflict: {}", detail),
        422 => format!("Invalid request: {}", detail),
        500..=599 => {
            format!("GitHub server error ({}): {}", status.as_u16(), detail)
        }
        _ => format!("HTTP {}: {}", status.as_u16(), detail),
    }
}
