//! GitHub Issues data layer — mirrors `github::pr` for read +
//! write endpoints. Issues and PRs share the same `/issues/*`
//! comments + reactions surface, so the conversation rendering on
//! the view side reuses the PR `ConversationEntry` shape.
//!
//! Endpoints used (all REST v3):
//! - `GET /repos/{o}/{r}/issues?state=…` — list summaries (filters PRs out)
//! - `GET /repos/{o}/{r}/issues/{n}` — full detail
//! - `GET /repos/{o}/{r}/issues/{n}/comments` — conversation thread

use serde::{Deserialize, Serialize};

use super::pr::{ConversationEntry, ConversationKind, Label, ReactionCounts};
use super::{http_client, RepoCoords};

/// Compact issue summary — populates the list panel.
#[derive(Debug, Clone)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: IssueState,
    /// `Some(reason)` when the issue is closed — `completed` vs
    /// `not_planned`. Drives the closed-state colour (purple for
    /// completed, grey for not-planned).
    pub state_reason: Option<IssueStateReason>,
    pub labels: Vec<Label>,
    pub assignees: Vec<String>,
    pub comments_count: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueState {
    Open,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueStateReason {
    Completed,
    NotPlanned,
    Reopened,
}

/// Full issue payload — populates the detail view's Conversation
/// tab. We don't need a parallel-fetch architecture like PRs do:
/// issues only have a single secondary endpoint (`/comments`), so
/// the body + comments are pulled in two sequential calls.
#[derive(Debug, Clone)]
pub struct IssueDetail {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: IssueState,
    pub state_reason: Option<IssueStateReason>,
    pub body: String,
    pub labels: Vec<Label>,
    pub assignees: Vec<String>,
    pub milestone: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    /// Relative timestamp ("3h ago") for the opened-this-issue card.
    pub opened_when: String,
    /// Reactions on the issue body itself (the description card).
    /// Distinct from per-comment reactions in `conversation` — this
    /// is what the `/issues/{n}` payload's inline `reactions` field
    /// carries.
    pub reactions: ReactionCounts,
    /// Mix of issue comments — chronological. Empty when the
    /// `/comments` endpoint returns nothing.
    pub conversation: Vec<ConversationEntry>,
}

/// List every issue on the repo, filtering out pull requests
/// (GitHub's `/issues` endpoint mixes them in by default). Sorted
/// by `updated` desc to match the web UI's default.
pub fn list_issues(token: &str, coords: &RepoCoords) -> Result<Vec<Issue>, String> {
    let client = http_client()?;
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues?state=all&per_page=50&sort=updated&direction=desc",
        coords.owner, coords.repo
    );
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("GitHub /issues: {}", e))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "GitHub /issues HTTP {}: {}",
            status,
            body.lines().next().unwrap_or("")
        ));
    }
    let body = resp.text().map_err(|e| format!("/issues read: {}", e))?;
    let raw: Vec<ApiIssue> =
        serde_json::from_str(&body).map_err(|e| format!("/issues JSON: {}", e))?;
    Ok(raw
        .into_iter()
        .filter(|r| r.pull_request.is_none())
        .map(map_issue)
        .collect())
}

/// Fetch a single issue's full detail. Two sequential REST calls —
/// the issue endpoint then the comments endpoint. Body + comments
/// land on the returned `IssueDetail`.
pub fn fetch_issue_detail(
    token: &str,
    coords: &RepoCoords,
    number: u64,
) -> Result<IssueDetail, String> {
    let client = http_client()?;
    let issue_url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}",
        coords.owner, coords.repo, number
    );
    let issue_resp = client
        .get(&issue_url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("GitHub /issues/{}: {}", number, e))?;
    if !issue_resp.status().is_success() {
        let status = issue_resp.status();
        let body = issue_resp.text().unwrap_or_default();
        return Err(format!(
            "Issue HTTP {}: {}",
            status,
            body.lines().next().unwrap_or("")
        ));
    }
    let issue_body = issue_resp
        .text()
        .map_err(|e| format!("Issue read: {}", e))?;
    let issue: ApiIssue =
        serde_json::from_str(&issue_body).map_err(|e| format!("Issue JSON: {}", e))?;

    // Comments — pulled even when comment count is 0 because the
    // /issues endpoint doesn't inline them. One call covers a
    // typical small / mid issue; >50 comments would need paging
    // (revisit when we see one in the wild).
    let comments_url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}/comments?per_page=100",
        coords.owner, coords.repo, number
    );
    let comments_resp = client
        .get(&comments_url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("Issue comments: {}", e))?;
    let api_comments: Vec<ApiIssueComment> = if comments_resp.status().is_success() {
        let s = comments_resp
            .text()
            .map_err(|e| format!("Comments read: {}", e))?;
        serde_json::from_str(&s).unwrap_or_default()
    } else {
        Vec::new()
    };

    let conversation: Vec<ConversationEntry> = api_comments
        .into_iter()
        .map(|c| ConversationEntry {
            id: Some(c.id),
            parent_id: None,
            author: c.user.map(|u| u.login).unwrap_or_else(|| "?".into()),
            when: short_relative(&c.created_at),
            body: c.body.unwrap_or_default(),
            kind: ConversationKind::Comment,
            reactions: c.reactions.unwrap_or_default().into(),
        })
        .collect();

    let opened_when = short_relative(&issue.created_at);
    let body_reactions: ReactionCounts = issue.reactions.clone().unwrap_or_default().into();
    Ok(IssueDetail {
        number: issue.number,
        title: issue.title.clone(),
        author: issue
            .user
            .as_ref()
            .map(|u| u.login.clone())
            .unwrap_or_else(|| "?".into()),
        state: derive_state(&issue.state),
        state_reason: issue.state_reason.as_deref().and_then(parse_state_reason),
        body: issue.body.clone().unwrap_or_default(),
        labels: issue.labels.iter().cloned().map(Into::into).collect(),
        assignees: issue.assignees.iter().map(|u| u.login.clone()).collect(),
        milestone: issue.milestone.as_ref().map(|m| m.title.clone()),
        created_at: issue.created_at.clone(),
        updated_at: issue.updated_at.clone(),
        closed_at: issue.closed_at.clone(),
        opened_when,
        reactions: body_reactions,
        conversation,
    })
}

fn map_issue(raw: ApiIssue) -> Issue {
    Issue {
        number: raw.number,
        title: raw.title,
        author: raw.user.map(|u| u.login).unwrap_or_else(|| "?".into()),
        state: derive_state(&raw.state),
        state_reason: raw.state_reason.as_deref().and_then(parse_state_reason),
        labels: raw.labels.into_iter().map(Into::into).collect(),
        assignees: raw.assignees.into_iter().map(|u| u.login).collect(),
        comments_count: raw.comments,
        created_at: raw.created_at,
        updated_at: raw.updated_at,
    }
}

fn derive_state(s: &str) -> IssueState {
    match s {
        "closed" => IssueState::Closed,
        _ => IssueState::Open,
    }
}

fn parse_state_reason(s: &str) -> Option<IssueStateReason> {
    match s {
        "completed" => Some(IssueStateReason::Completed),
        "not_planned" => Some(IssueStateReason::NotPlanned),
        "reopened" => Some(IssueStateReason::Reopened),
        _ => None,
    }
}

/// Compact relative time formatter — reused from the PR view's
/// `short_relative` so PR and Issue cards format the same.
fn short_relative(iso: &str) -> String {
    use chrono::DateTime;
    let Ok(dt) = DateTime::parse_from_rfc3339(iso) else {
        return iso.to_string();
    };
    let now = chrono::Utc::now();
    let secs = (now.timestamp() - dt.timestamp()).max(0);
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

// ─── Raw API shapes ──────────────────────────────────────────────

#[derive(Deserialize)]
struct ApiIssue {
    number: u64,
    #[serde(default)]
    title: String,
    user: Option<ApiUser>,
    #[serde(default)]
    state: String,
    /// `completed` / `not_planned` / `reopened` — set when the
    /// issue is closed (or when it was reopened post-close).
    #[serde(default)]
    state_reason: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    labels: Vec<ApiLabel>,
    #[serde(default)]
    assignees: Vec<ApiUser>,
    #[serde(default)]
    milestone: Option<ApiMilestone>,
    #[serde(default)]
    comments: u64,
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    closed_at: Option<String>,
    /// Present on /issues endpoint when the entry is a pull request
    /// — used to filter PRs out of the list.
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
    /// Inline reactions object on the issue body. `None` when the
    /// list endpoint omitted it; `fetch_issue_detail` always gets it.
    #[serde(default)]
    reactions: Option<ApiReactions>,
}

#[derive(Deserialize, Clone)]
struct ApiUser {
    #[serde(default)]
    login: String,
}

#[derive(Deserialize, Clone)]
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
struct ApiMilestone {
    #[serde(default)]
    title: String,
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
    #[serde(default)]
    reactions: Option<ApiReactions>,
}

#[derive(Deserialize, Default, Clone)]
struct ApiReactions {
    #[serde(default, rename = "+1")]
    plus_one: u64,
    #[serde(default, rename = "-1")]
    minus_one: u64,
    #[serde(default)]
    laugh: u64,
    #[serde(default)]
    confused: u64,
    #[serde(default)]
    heart: u64,
    #[serde(default)]
    hooray: u64,
    #[serde(default)]
    rocket: u64,
    #[serde(default)]
    eyes: u64,
}

impl From<ApiReactions> for ReactionCounts {
    fn from(r: ApiReactions) -> Self {
        ReactionCounts {
            plus_one: r.plus_one,
            minus_one: r.minus_one,
            laugh: r.laugh,
            confused: r.confused,
            heart: r.heart,
            hooray: r.hooray,
            rocket: r.rocket,
            eyes: r.eyes,
        }
    }
}

// ─── Write API ───────────────────────────────────────────────────

/// Internal helper: PUT/PATCH/POST a JSON body. Returns the raw
/// response so the caller can inspect status / read the body for
/// the "Create issue" case (which needs the returned `number`).
fn request_json<T: Serialize>(
    token: &str,
    method: reqwest::Method,
    url: &str,
    body: &T,
    label: &str,
) -> Result<reqwest::blocking::Response, String> {
    let client = http_client()?;
    let payload = serde_json::to_string(body).map_err(|e| format!("{} encode: {}", label, e))?;
    client
        .request(method, url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("Content-Type", "application/json")
        .body(payload)
        .send()
        .map_err(|e| format!("{}: {}", label, e))
}

pub fn post_issue_comment(
    token: &str,
    coords: &RepoCoords,
    number: u64,
    body: &str,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}/comments",
        coords.owner, coords.repo, number
    );
    let resp = request_json(
        token,
        reqwest::Method::POST,
        &url,
        &BodyPayload { body },
        "Post comment",
    )?;
    expect_success(resp, "Post comment")
}

pub fn edit_issue_comment(
    token: &str,
    coords: &RepoCoords,
    comment_id: u64,
    body: &str,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/comments/{}",
        coords.owner, coords.repo, comment_id
    );
    let resp = request_json(
        token,
        reqwest::Method::PATCH,
        &url,
        &BodyPayload { body },
        "Edit comment",
    )?;
    expect_success(resp, "Edit comment")
}

pub fn delete_issue_comment(
    token: &str,
    coords: &RepoCoords,
    comment_id: u64,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/comments/{}",
        coords.owner, coords.repo, comment_id
    );
    let client = http_client()?;
    let resp = client
        .delete(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("Delete comment: {}", e))?;
    expect_success(resp, "Delete comment")
}

pub fn close_issue(
    token: &str,
    coords: &RepoCoords,
    number: u64,
    reason: IssueStateReason,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}",
        coords.owner, coords.repo, number
    );
    let reason_str = match reason {
        IssueStateReason::Completed => "completed",
        IssueStateReason::NotPlanned => "not_planned",
        IssueStateReason::Reopened => "completed",
    };
    let resp = request_json(
        token,
        reqwest::Method::PATCH,
        &url,
        &StateChangePayload {
            state: "closed",
            state_reason: Some(reason_str),
        },
        "Close issue",
    )?;
    expect_success(resp, "Close issue")
}

pub fn reopen_issue(token: &str, coords: &RepoCoords, number: u64) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}",
        coords.owner, coords.repo, number
    );
    let resp = request_json(
        token,
        reqwest::Method::PATCH,
        &url,
        &StateChangePayload {
            state: "open",
            state_reason: None,
        },
        "Reopen issue",
    )?;
    expect_success(resp, "Reopen issue")
}

pub fn set_issue_labels(
    token: &str,
    coords: &RepoCoords,
    number: u64,
    labels: &[String],
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}/labels",
        coords.owner, coords.repo, number
    );
    let resp = request_json(
        token,
        reqwest::Method::PUT,
        &url,
        &LabelsPayload { labels },
        "Set labels",
    )?;
    expect_success(resp, "Set labels")
}

pub fn set_issue_assignees(
    token: &str,
    coords: &RepoCoords,
    number: u64,
    assignees: &[String],
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}",
        coords.owner, coords.repo, number
    );
    let resp = request_json(
        token,
        reqwest::Method::PATCH,
        &url,
        &AssigneesPayload { assignees },
        "Set assignees",
    )?;
    expect_success(resp, "Set assignees")
}

pub fn set_issue_milestone(
    token: &str,
    coords: &RepoCoords,
    number: u64,
    milestone_number: Option<u64>,
) -> Result<(), String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}",
        coords.owner, coords.repo, number
    );
    let resp = request_json(
        token,
        reqwest::Method::PATCH,
        &url,
        &MilestonePayload {
            milestone: milestone_number,
        },
        "Set milestone",
    )?;
    expect_success(resp, "Set milestone")
}

pub fn add_issue_reaction(
    token: &str,
    coords: &RepoCoords,
    number: u64,
    kind: crate::github::pr::ReactionKind,
) -> Result<u64, String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}/reactions",
        coords.owner, coords.repo, number
    );
    let resp = request_json(
        token,
        reqwest::Method::POST,
        &url,
        &ReactionPayload {
            content: kind.api_content(),
        },
        "React to issue",
    )?;
    let status = resp.status();
    let body = resp
        .text()
        .map_err(|e| format!("React to issue read: {}", e))?;
    if !status.is_success() {
        return Err(format!(
            "React to issue HTTP {}: {}",
            status,
            body.lines().next().unwrap_or("")
        ));
    }
    #[derive(serde::Deserialize)]
    struct R {
        id: u64,
    }
    let parsed: R = serde_json::from_str(&body).map_err(|e| format!("decode reaction: {}", e))?;
    Ok(parsed.id)
}

pub fn create_issue(
    token: &str,
    coords: &RepoCoords,
    title: &str,
    body: &str,
    labels: &[String],
    assignees: &[String],
    milestone: Option<u64>,
) -> Result<u64, String> {
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues",
        coords.owner, coords.repo
    );
    let resp = request_json(
        token,
        reqwest::Method::POST,
        &url,
        &CreateIssuePayload {
            title,
            body,
            labels,
            assignees,
            milestone,
        },
        "Create issue",
    )?;
    let status = resp.status();
    let body_text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!(
            "Create issue HTTP {}: {}",
            status,
            body_text.lines().next().unwrap_or("")
        ));
    }
    let created: ApiCreated =
        serde_json::from_str(&body_text).map_err(|e| format!("Create issue JSON: {}", e))?;
    Ok(created.number)
}

/// List repo labels — drives the labels picker. Same response shape
/// as the PR module's equivalent call but kept local so issue.rs can
/// be lifted out independently.
pub fn list_repo_labels(token: &str, coords: &RepoCoords) -> Result<Vec<Label>, String> {
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
        .map_err(|e| format!("List labels: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("List labels HTTP {}", resp.status()));
    }
    let body = resp.text().map_err(|e| format!("Read labels: {}", e))?;
    let raw: Vec<ApiLabel> =
        serde_json::from_str(&body).map_err(|e| format!("Labels JSON: {}", e))?;
    Ok(raw.into_iter().map(Into::into).collect())
}

/// List candidate assignees (repo collaborators with push access).
pub fn list_repo_assignees(token: &str, coords: &RepoCoords) -> Result<Vec<String>, String> {
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
        .map_err(|e| format!("List assignees: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("List assignees HTTP {}", resp.status()));
    }
    let body = resp.text().map_err(|e| format!("Read assignees: {}", e))?;
    let raw: Vec<ApiUser> =
        serde_json::from_str(&body).map_err(|e| format!("Assignees JSON: {}", e))?;
    Ok(raw.into_iter().map(|u| u.login).collect())
}

/// List open milestones — the picker shows only open ones since
/// closed milestones can't be set on a new issue.
pub fn list_repo_milestones(token: &str, coords: &RepoCoords) -> Result<Vec<Milestone>, String> {
    let client = http_client()?;
    let url = format!(
        "https://api.github.com/repos/{}/{}/milestones?state=open&per_page=100",
        coords.owner, coords.repo
    );
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("List milestones: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("List milestones HTTP {}", resp.status()));
    }
    let body = resp.text().map_err(|e| format!("Read milestones: {}", e))?;
    let raw: Vec<ApiMilestoneFull> =
        serde_json::from_str(&body).map_err(|e| format!("Milestones JSON: {}", e))?;
    Ok(raw
        .into_iter()
        .map(|m| Milestone {
            number: m.number,
            title: m.title,
        })
        .collect())
}

/// Issue timeline events — the Timeline tab pulls this. Each entry
/// is a structured event (label added/removed, assigned, mentioned,
/// closed, reopened, cross-referenced, etc.). We project the raw
/// payload into `TimelineEvent` and let the renderer pick an icon +
/// short label for each kind.
pub fn list_issue_timeline(
    token: &str,
    coords: &RepoCoords,
    number: u64,
) -> Result<Vec<TimelineEvent>, String> {
    let client = http_client()?;
    let url = format!(
        "https://api.github.com/repos/{}/{}/issues/{}/timeline?per_page=100",
        coords.owner, coords.repo, number
    );
    let resp = client
        .get(&url)
        .bearer_auth(token)
        // Timeline preview was elevated to GA but the modern accept
        // still works (and unlocks all event kinds).
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| format!("Timeline: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("Timeline HTTP {}", resp.status()));
    }
    let body = resp.text().map_err(|e| format!("Read timeline: {}", e))?;
    let raw: Vec<ApiTimelineEvent> =
        serde_json::from_str(&body).map_err(|e| format!("Timeline JSON: {}", e))?;
    Ok(raw.into_iter().filter_map(project_event).collect())
}

/// Linked PRs — runs a code-search-style query that finds open and
/// closed PRs whose body / commits / commit messages mention the
/// issue number. Surfaced in the Linked tab.
pub fn list_linked_prs(
    token: &str,
    coords: &RepoCoords,
    number: u64,
) -> Result<Vec<LinkedPr>, String> {
    let client = http_client()?;
    let q = format!("repo:{}/{} type:pr {}", coords.owner, coords.repo, number);
    let resp = client
        .get("https://api.github.com/search/issues")
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .query(&[("q", q.as_str()), ("per_page", "50")])
        .send()
        .map_err(|e| format!("Linked PRs: {}", e))?;
    if !resp.status().is_success() {
        return Err(format!("Linked PRs HTTP {}", resp.status()));
    }
    let body = resp.text().map_err(|e| format!("Read linked: {}", e))?;
    let raw: ApiSearchResults =
        serde_json::from_str(&body).map_err(|e| format!("Linked JSON: {}", e))?;
    Ok(raw
        .items
        .into_iter()
        .filter(|i| i.pull_request.is_some())
        .map(|i| LinkedPr {
            number: i.number,
            title: i.title,
            state: i.state,
            author: i.user.map(|u| u.login).unwrap_or_default(),
        })
        .collect())
}

/// Try to load the body of the first issue template found under
/// `.github/ISSUE_TEMPLATE/` (or the alternative single-file paths
/// GitHub recognises). Returns `None` when no template is
/// configured. The lookup is path-based — we don't go through the
/// API because the user already has the repo on disk. All paths are
/// resolved relative to `repo_path` so gitoui works even when run
/// from a sub-directory of the worktree.
pub fn load_first_template(repo_path: &std::path::Path) -> Option<String> {
    let candidates = [
        ".github/ISSUE_TEMPLATE",
        ".github/ISSUE_TEMPLATE.md",
        ".github/issue_template.md",
        "docs/ISSUE_TEMPLATE.md",
        "ISSUE_TEMPLATE.md",
    ];
    load_first_template_at(repo_path, &candidates)
}

/// Shared lookup for issue + PR template directories / single files.
/// Tries each candidate path relative to `repo_path`; the first hit
/// returns the (frontmatter-stripped) body. For directories the
/// first alphabetically-sorted `.md` / `.yaml` is picked, mirroring
/// the behaviour GitHub's web UI uses when no template is selected
/// in the picker.
pub(crate) fn load_first_template_at(
    repo_path: &std::path::Path,
    candidates: &[&str],
) -> Option<String> {
    use std::fs;
    for c in candidates {
        let path = repo_path.join(c);
        if path.is_file() {
            if let Ok(s) = fs::read_to_string(&path) {
                return Some(strip_template_frontmatter(&s));
            }
        }
        if path.is_dir() {
            if let Ok(entries) = fs::read_dir(&path) {
                let mut files: Vec<_> = entries
                    .flatten()
                    .filter(|e| {
                        e.path().extension().map_or(false, |x| {
                            x == "md" || x == "markdown" || x == "yml" || x == "yaml"
                        })
                    })
                    .collect();
                files.sort_by_key(|e| e.file_name());
                if let Some(first) = files.first() {
                    if let Ok(s) = fs::read_to_string(first.path()) {
                        return Some(strip_template_frontmatter(&s));
                    }
                }
            }
        }
    }
    None
}

/// Strip the leading `---\n...\n---` YAML frontmatter from a
/// template — that block is metadata for GitHub's web UI, not body
/// content we want to seed into the compose buffer.
pub(crate) fn strip_template_frontmatter(s: &str) -> String {
    let mut lines = s.lines();
    if lines.next() != Some("---") {
        return s.to_string();
    }
    let mut out = String::new();
    let mut closed = false;
    for line in lines.by_ref() {
        if line.trim() == "---" {
            closed = true;
            break;
        }
    }
    if !closed {
        return s.to_string();
    }
    let body: Vec<&str> = lines.collect();
    out.push_str(&body.join("\n"));
    out.trim_start_matches('\n').to_string()
}

// ─── Public ancillary types ──────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Milestone {
    pub number: u64,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct TimelineEvent {
    pub when: String,
    pub kind: TimelineKind,
    pub actor: Option<String>,
}

#[derive(Debug, Clone)]
pub enum TimelineKind {
    Labeled {
        name: String,
        color: Option<String>,
    },
    Unlabeled {
        name: String,
        color: Option<String>,
    },
    Assigned {
        who: String,
    },
    Unassigned {
        who: String,
    },
    Milestoned {
        title: String,
    },
    Demilestoned {
        title: String,
    },
    Closed {
        reason: Option<IssueStateReason>,
    },
    Reopened,
    Renamed {
        from: String,
        to: String,
    },
    CrossReferenced {
        issue_number: Option<u64>,
        title: Option<String>,
    },
    Mentioned,
    Subscribed,
    Pinned,
    Unpinned,
    /// Catch-all so the renderer can still emit a row for unknown
    /// kinds without losing the timestamp + actor.
    Other {
        kind: String,
    },
}

#[derive(Debug, Clone)]
pub struct LinkedPr {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub author: String,
}

// ─── Internal helpers ────────────────────────────────────────────

fn expect_success(resp: reqwest::blocking::Response, label: &str) -> Result<(), String> {
    if resp.status().is_success() {
        Ok(())
    } else {
        let code = resp.status();
        let body = resp.text().unwrap_or_default();
        Err(format!(
            "{} HTTP {}: {}",
            label,
            code,
            body.lines().next().unwrap_or("")
        ))
    }
}

fn project_event(raw: ApiTimelineEvent) -> Option<TimelineEvent> {
    let actor = raw.actor.map(|u| u.login);
    let when = short_relative(&raw.created_at);
    let kind = match raw.event.as_str() {
        "labeled" => TimelineKind::Labeled {
            name: raw.label.as_ref()?.name.clone(),
            color: raw.label.as_ref()?.color.clone(),
        },
        "unlabeled" => TimelineKind::Unlabeled {
            name: raw.label.as_ref()?.name.clone(),
            color: raw.label.as_ref()?.color.clone(),
        },
        "assigned" => TimelineKind::Assigned {
            who: raw.assignee.as_ref()?.login.clone(),
        },
        "unassigned" => TimelineKind::Unassigned {
            who: raw.assignee.as_ref()?.login.clone(),
        },
        "milestoned" => TimelineKind::Milestoned {
            title: raw.milestone.as_ref()?.title.clone(),
        },
        "demilestoned" => TimelineKind::Demilestoned {
            title: raw.milestone.as_ref()?.title.clone(),
        },
        "closed" => TimelineKind::Closed {
            reason: raw.state_reason.as_deref().and_then(parse_state_reason),
        },
        "reopened" => TimelineKind::Reopened,
        "renamed" => {
            let r = raw.rename.as_ref()?;
            TimelineKind::Renamed {
                from: r.from.clone(),
                to: r.to.clone(),
            }
        }
        "cross-referenced" => TimelineKind::CrossReferenced {
            issue_number: raw
                .source
                .as_ref()
                .and_then(|s| s.issue.as_ref())
                .map(|i| i.number),
            title: raw
                .source
                .as_ref()
                .and_then(|s| s.issue.as_ref())
                .map(|i| i.title.clone()),
        },
        "mentioned" => TimelineKind::Mentioned,
        "subscribed" => TimelineKind::Subscribed,
        "pinned" => TimelineKind::Pinned,
        "unpinned" => TimelineKind::Unpinned,
        // Comments are inlined here too but we render them in the
        // Conversation tab — skip in Timeline to avoid duplication.
        "commented" => return None,
        other => TimelineKind::Other {
            kind: other.to_string(),
        },
    };
    Some(TimelineEvent { when, kind, actor })
}

// ─── Write payload shapes ────────────────────────────────────────

#[derive(Serialize)]
struct BodyPayload<'a> {
    body: &'a str,
}

#[derive(Serialize)]
struct StateChangePayload<'a> {
    state: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    state_reason: Option<&'a str>,
}

#[derive(Serialize)]
struct LabelsPayload<'a> {
    labels: &'a [String],
}

#[derive(Serialize)]
struct AssigneesPayload<'a> {
    assignees: &'a [String],
}

#[derive(Serialize)]
struct MilestonePayload {
    milestone: Option<u64>,
}

#[derive(Serialize)]
struct ReactionPayload<'a> {
    content: &'a str,
}

#[derive(Serialize)]
struct CreateIssuePayload<'a> {
    title: &'a str,
    body: &'a str,
    labels: &'a [String],
    assignees: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    milestone: Option<u64>,
}

#[derive(Deserialize)]
struct ApiCreated {
    number: u64,
}

#[derive(Deserialize)]
struct ApiMilestoneFull {
    #[serde(default)]
    number: u64,
    #[serde(default)]
    title: String,
}

// ─── Timeline raw shapes ─────────────────────────────────────────

#[derive(Deserialize)]
struct ApiTimelineEvent {
    #[serde(default)]
    event: String,
    #[serde(default)]
    created_at: String,
    actor: Option<ApiUser>,
    label: Option<ApiLabel>,
    assignee: Option<ApiUser>,
    milestone: Option<ApiMilestone>,
    rename: Option<ApiRename>,
    source: Option<ApiSource>,
    #[serde(default)]
    state_reason: Option<String>,
}

#[derive(Deserialize)]
struct ApiRename {
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: String,
}

#[derive(Deserialize)]
struct ApiSource {
    issue: Option<ApiSourceIssue>,
}

#[derive(Deserialize)]
struct ApiSourceIssue {
    #[serde(default)]
    number: u64,
    #[serde(default)]
    title: String,
}

#[derive(Deserialize)]
struct ApiSearchResults {
    #[serde(default)]
    items: Vec<ApiSearchItem>,
}

#[derive(Deserialize)]
struct ApiSearchItem {
    #[serde(default)]
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    state: String,
    user: Option<ApiUser>,
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
}
