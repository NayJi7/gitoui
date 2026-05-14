//! GitHub API client — used by views that need data beyond what `git`
//! alone exposes (pull requests, reviews, CI status). All calls go
//! through the user's OAuth token from `github_auth_state`.
//!
//! Scope is intentionally narrow: read-only listing + detail today;
//! mutating actions (approve / merge / comment) live in dedicated
//! follow-up modules.

pub mod issue;
pub mod pr;

use std::time::Duration;

/// Build a `reqwest::blocking::Client` configured the way GitHub's API
/// expects: short timeout (we surface a notification on failure rather
/// than block the UI for minutes), a real User-Agent (GitHub rejects
/// requests without one), and HTTP/2 enabled.
pub(crate) fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("gitoui")
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("http client init: {}", e))
}

/// Owner+repo extracted from the active remote, e.g. `("NayJi7", "gitoui")`.
#[derive(Debug, Clone)]
pub struct RepoCoords {
    pub owner: String,
    pub repo: String,
}

impl RepoCoords {
    /// Parse a remote URL into `RepoCoords`. Mirrors `lib.rs::parse_github_repo`
    /// but returns the parts split so callers can build URL paths.
    pub fn parse_remote(url: &str) -> Option<Self> {
        let path = if let Some(rest) = url.strip_prefix("git@github.com:") {
            rest
        } else if let Some(rest) = url.strip_prefix("https://github.com/") {
            rest
        } else {
            url.strip_prefix("ssh://git@github.com/")?
        };
        let path = path.trim_end_matches(".git");
        let (owner, repo) = path.split_once('/')?;
        if owner.is_empty() || repo.is_empty() {
            None
        } else {
            Some(RepoCoords {
                owner: owner.to_string(),
                repo: repo.to_string(),
            })
        }
    }

    /// Inspect the repo's `origin` (or first GitHub-hosted) remote and
    /// extract owner+repo. Returns `None` when the repo has no GitHub
    /// remote configured.
    pub fn from_repo(repo_path: &std::path::Path) -> Option<Self> {
        let out = std::process::Command::new("git")
            .current_dir(repo_path)
            .args(["remote", "-v"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        // Prefer `origin`; fall back to the first GitHub-hosted remote.
        let mut fallback: Option<RepoCoords> = None;
        for line in text.lines() {
            let mut parts = line.split_whitespace();
            let name = parts.next()?;
            let url = parts.next()?;
            if let Some(coords) = Self::parse_remote(url) {
                if name == "origin" {
                    return Some(coords);
                }
                if fallback.is_none() {
                    fallback = Some(coords);
                }
            }
        }
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_handles_all_common_formats() {
        assert!(RepoCoords::parse_remote("git@github.com:NayJi7/gitoui.git").is_some());
        assert!(RepoCoords::parse_remote("https://github.com/NayJi7/gitoui").is_some());
        assert!(RepoCoords::parse_remote("ssh://git@github.com/NayJi7/gitoui.git").is_some());
        assert!(RepoCoords::parse_remote("https://gitlab.com/NayJi7/gitoui").is_none());
    }

    #[test]
    fn parse_remote_strips_trailing_dot_git() {
        let c = RepoCoords::parse_remote("git@github.com:owner/repo.git").unwrap();
        assert_eq!(c.owner, "owner");
        assert_eq!(c.repo, "repo");
    }
}
