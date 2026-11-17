//! GitHub API client, used by views that need data beyond what `git`
//! alone exposes (pull requests, reviews, CI status). All calls go
//! through the user's OAuth token from `github_auth_state`.
//!
//! Scope is intentionally narrow: read-only listing + detail today;
//! mutating actions (approve / merge / comment) live in dedicated
//! follow-up modules.

pub mod actions;
pub mod issue;
pub mod pr;

use std::time::Duration;

/// How long to wait for a connection to open before trying again: on a network
/// that drops packets, a fresh attempt succeeds where waiting does not.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Attempts to open a connection before giving up.
pub(crate) const CONNECT_ATTEMPTS: usize = 5;

/// The client of the GitHub API, shared by every call (so connections are
/// reused instead of being opened, with their DNS and TLS, for each request):
/// a real User-Agent (GitHub rejects requests without one), a short time to
/// connect, and a timeout that surfaces a notification on failure rather than
/// blocking the UI for minutes.
pub(crate) fn http_client() -> Result<reqwest::blocking::Client, String> {
    static CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let built = reqwest::blocking::Client::builder()
        .user_agent("gitoui")
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client init: {}", e))?;
    Ok(CLIENT.get_or_init(|| built).clone())
}

/// Runs `attempt` up to `times` times while the error is retryable.
pub(crate) fn retry<T, E>(
    times: usize,
    retryable: impl Fn(&E) -> bool,
    mut attempt: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    let mut last = attempt();
    for _ in 1..times {
        match &last {
            Err(e) if retryable(e) => last = attempt(),
            _ => break,
        }
    }
    last
}

/// The request never reached GitHub: safe to try again, even for a POST.
pub(crate) fn connection_failed(e: &reqwest::Error) -> bool {
    e.is_connect()
}

/// `send()` that opens the connection again when it could not be opened.
/// Only that case is retried: a request that went out is never repeated.
pub(crate) trait SendRetrying {
    fn send_retrying(self) -> reqwest::Result<reqwest::blocking::Response>;
}

impl SendRetrying for reqwest::blocking::RequestBuilder {
    fn send_retrying(self) -> reqwest::Result<reqwest::blocking::Response> {
        for _ in 1..CONNECT_ATTEMPTS {
            // A body that cannot be cloned (a stream) is sent once.
            let Some(attempt) = self.try_clone() else {
                return self.send();
            };
            match attempt.send() {
                Err(e) if connection_failed(&e) => continue,
                other => return other,
            }
        }
        self.send()
    }
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

/// A number nobody used before. A request carries it and its answer brings it
/// back, so an answer that comes late (from a page that was closed, or from an
/// older request) never replaces a newer one.
pub fn next_generation() -> u64 {
    static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
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

    #[test]
    fn a_refused_connection_is_tried_again_then_reported() {
        // Nothing listens here: the connection is refused at once, which is
        // exactly the kind of failure that is retried.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let start = std::time::Instant::now();
        let err = http_client()
            .unwrap()
            .get(format!("http://127.0.0.1:{port}/"))
            .send_retrying()
            .unwrap_err();
        assert!(connection_failed(&err), "{err}");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "refusals are fast"
        );
    }

    #[test]
    fn retry_only_repeats_what_is_retryable() {
        let mut calls = 0;
        let r: Result<u8, &str> = retry(
            5,
            |e: &&str| *e == "connect",
            || {
                calls += 1;
                Err("other")
            },
        );
        assert!(
            r.is_err() && calls == 1,
            "a request that went out is not repeated"
        );
    }
}
