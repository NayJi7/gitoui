//! On-disk commit-history cache.
//!
//! Walks `git log` once per session (in the background) and persists
//! the full Vec<Commit> + the HEAD it was sampled at to
//! `$XDG_CACHE_HOME/gitoui/<repo-hash>/commits.bin`. The next session
//! checks that file: if HEAD hasn't moved we load it straight off
//! disk and skip the (slow) `git log` walk entirely. If HEAD HAS
//! moved we delete the cache and fall back to a fresh walk - tracking
//! delta-since-last-cache would be more efficient but adds replay
//! logic for tag/branch deletions that nobody asked for yet.
//!
//! Graph topology is NOT cached. The user wants the cache to cover
//! `git log` only; calc_graph re-runs each session against the loaded
//! commit list.

use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use super::{Commit, SortCommit};

/// Bincode-serialised payload on disk.
#[derive(serde::Serialize, serde::Deserialize)]
struct CachePayload {
    /// Schema version. Bump when `Commit` fields change so stale
    /// caches get rejected instead of producing decode errors.
    version: u32,
    /// HEAD hash the cache was sampled at. We only trust the cache
    /// on load if the live HEAD still matches.
    head: String,
    /// Total reachable commit count (`git rev-list --count
    /// --branches --remotes --tags HEAD`) at cache time. Reload
    /// invalidates the cache when this number changes - that
    /// covers `git fetch` adding remote-only commits (HEAD didn't
    /// move so the head check wouldn't trip), a force-push
    /// dropping commits, `git gc` / packed-refs reorgs, etc.
    /// Loaded as 0 from legacy caches (will trigger one extra
    /// re-walk after the upgrade, then stabilise).
    #[serde(default)]
    reachable_count: u64,
    /// Full git-log walk. Order matters: render uses it as-is.
    commits: Vec<Commit>,
}

/// Bump when CachePayload structurally changes. v2 added
/// `reachable_count` (default-deserialises to 0 for v1 caches but
/// we treat any 0 as "unknown, fall through and re-walk").
const CACHE_VERSION: u32 = 2;

fn cache_root() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("gitoui"))
}

/// 64-bit FxHash that identifies a repo for cache purposes.
///
/// Uses `git rev-parse --git-common-dir` so that multiple worktrees
/// (`git worktree add`) of the same underlying repository SHARE a
/// single cache file. Without this, each worktree path hashed to a
/// different key, both wrote their own cache, and a fast-forward in
/// one worktree left the other's cache stale with no detection - the
/// caches couldn't see each other.
///
/// Falls back to the canonicalized repo path if the `git` call fails
/// (covers non-repo dirs called with `--clear-cache` etc.).
fn repo_key(path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let common_dir = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("--git-common-dir")
        .current_dir(path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .and_then(|rel| {
            // `--git-common-dir` returns a path relative to the
            // invocation cwd; canonicalize to absolute so symlinked
            // worktrees still resolve to the same key.
            let p = if std::path::Path::new(&rel).is_absolute() {
                std::path::PathBuf::from(rel)
            } else {
                path.join(rel)
            };
            fs::canonicalize(&p).ok()
        });
    let canonical = common_dir
        .unwrap_or_else(|| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
    let mut hasher = rustc_hash::FxHasher::default();
    canonical.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn cache_path_for(repo_path: &Path) -> Option<PathBuf> {
    cache_root().map(|root| root.join(repo_key(repo_path)).join("commits.bin"))
}

/// `git rev-parse HEAD` - returns the current HEAD commit hash so the
/// cache loader can detect when HEAD has moved since the cache was
/// written. Returns None on bare repos, detached-HEAD-with-no-commits,
/// non-git dirs, etc; cache loading is gated on Some.
pub fn current_head_hash(repo_path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let hash = String::from_utf8(out.stdout).ok()?;
    Some(hash.trim().to_string())
}

/// `git rev-list --count --branches --remotes --tags HEAD` - total
/// reachable commit count across every local + remote ref + tag,
/// not just HEAD's chain. Used to detect cache staleness when HEAD
/// itself didn't move (typically: `git fetch` brought new remote
/// commits, `git tag` added a new tag, force-push removed commits).
/// Returns None on failure; treated as "unknown, skip count check".
pub fn reachable_commit_count(repo_path: &Path) -> Option<u64> {
    let out = std::process::Command::new("git")
        .arg("rev-list")
        .arg("--count")
        .arg("--branches")
        .arg("--remotes")
        .arg("--tags")
        .arg("HEAD")
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    s.trim().parse::<u64>().ok()
}

/// Attempt to load a cached commit history for `repo_path`. Returns
/// `Some(commits)` only when:
///   - the cache file exists,
///   - the schema version matches,
///   - the cached HEAD equals the live HEAD.
/// All other cases - missing file, decode failure, HEAD mismatch -
/// return None and the caller falls back to a fresh `git log` walk.
pub fn load_for(repo_path: &Path) -> Option<Vec<Commit>> {
    let path = cache_path_for(repo_path)?;
    if !path.exists() {
        return None;
    }
    let live_head = current_head_hash(repo_path)?;
    let mut file = fs::File::open(&path).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let payload: CachePayload = bincode::deserialize(&bytes).ok()?;
    if payload.version != CACHE_VERSION {
        crate::glog_info!(
            "cache version mismatch ({} vs {}): ignoring stale cache",
            payload.version,
            CACHE_VERSION
        );
        let _ = fs::remove_file(&path);
        return None;
    }
    if payload.head != live_head {
        crate::glog_info!(
            "cache HEAD moved ({} -> {}): re-walking",
            payload.head,
            live_head
        );
        let _ = fs::remove_file(&path);
        return None;
    }
    // Even with HEAD unchanged, the reachable commit count may
    // differ if the user fetched new remote-only commits, gc'd,
    // added a tag, or had history rewritten upstream. When the
    // count drifts we invalidate the cache - the in-memory
    // `commits` vec would otherwise miss those new entries (or
    // contain stale ones that no longer exist).
    if payload.reachable_count > 0 {
        if let Some(live_count) = reachable_commit_count(repo_path) {
            if payload.reachable_count != live_count {
                crate::glog_info!(
                    "cache reachable-count drifted ({} -> {}): re-walking",
                    payload.reachable_count,
                    live_count
                );
                let _ = fs::remove_file(&path);
                return None;
            }
        }
    } else {
        // Legacy v1 cache (no count field): one forced re-walk to
        // capture the live count, then subsequent loads benefit.
        crate::glog_info!(
            "cache lacks reachable-count: re-walking once to upgrade payload"
        );
        let _ = fs::remove_file(&path);
        return None;
    }
    crate::glog_info!(
        "cache hit: loaded {} commits from {}",
        payload.commits.len(),
        path.display()
    );
    Some(payload.commits)
}

/// Returns true if `cached_head` is an ancestor of the live `HEAD`,
/// i.e. live HEAD is a strict descendant - the kind of move that
/// happens when the user adds a commit locally. We can then walk
/// only the delta and prepend, instead of re-walking the full
/// history.
fn cached_head_is_ancestor(repo_path: &Path, cached_head: &str) -> bool {
    std::process::Command::new("git")
        .arg("merge-base")
        .arg("--is-ancestor")
        .arg(cached_head)
        .arg("HEAD")
        .current_dir(repo_path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Surgical refresh path: load the on-disk payload (without the
/// HEAD-equal / count-equal gates), and IF live HEAD is a strict
/// descendant of the cached HEAD, walk just the delta range
/// `cached_head..HEAD`, prepend those new commits to the cached
/// vec, rewrite the cache atomically, and return the merged set.
///
/// Returns `None` when the cache isn't extensible (missing,
/// schema mismatch, HEAD diverged via force-push / rebase,
/// reachable count drifted by more than the HEAD-only delta) -
/// caller falls back to a full re-walk.
///
/// On success the on-disk cache is already updated, so the
/// returned `Vec<Commit>` is what the bg streamer should ship to
/// the UI as the authoritative new history. Avoids the ~8 s full
/// re-walk on rust-lang/rust for the typical "user committed once"
/// scenario.
pub fn try_extend(repo_path: &Path, sort: SortCommit) -> Option<Vec<Commit>> {
    let path = cache_path_for(repo_path)?;
    if !path.exists() {
        return None;
    }
    let mut file = fs::File::open(&path).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let payload: CachePayload = bincode::deserialize(&bytes).ok()?;
    if payload.version != CACHE_VERSION {
        return None;
    }
    let live_head = current_head_hash(repo_path)?;
    if payload.head == live_head {
        // Caller should have hit `load_for` first; nothing to do.
        return Some(payload.commits);
    }
    if !cached_head_is_ancestor(repo_path, &payload.head) {
        // Diverged history (force-push, rebase, branch swap, ...);
        // can't surgically extend. Full re-walk needed.
        return None;
    }
    let new_commits = super::walk_commits_since(repo_path, sort, &payload.head)?;
    if new_commits.is_empty() {
        // HEAD differs but rev-list returned nothing -> shouldn't
        // happen on a fast-forward; bail to full walk.
        return None;
    }
    // Prepend (newest-first ordering preserved).
    let added = new_commits.len();
    let mut merged = new_commits;
    merged.extend(payload.commits);
    crate::glog_info!(
        "cache surgical extend: +{} commits, total now {}",
        added,
        merged.len()
    );
    // Persist the extended set so the next session doesn't re-walk.
    let _ = save_for(repo_path, &merged);
    Some(merged)
}

/// Write the full commit history to disk. Creates the per-repo cache
/// dir on demand. Safe to call from a background thread - all I/O is
/// best-effort and silently no-ops on failure (the cache is purely
/// an optimisation).
pub fn save_for(repo_path: &Path, commits: &[Commit]) -> io::Result<()> {
    let Some(path) = cache_path_for(repo_path) else {
        return Ok(());
    };
    let Some(head) = current_head_hash(repo_path) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Snapshot the live reachable-count alongside the commits so
    // the next load can detect drift (fetch, gc, force-push) even
    // when HEAD itself hasn't moved. 0 means "unknown" and triggers
    // a defensive re-walk on next load - acceptable here.
    let reachable_count = reachable_commit_count(repo_path).unwrap_or(0);
    let payload = CachePayload {
        version: CACHE_VERSION,
        head,
        reachable_count,
        commits: commits.to_vec(),
    };
    let bytes = bincode::serialize(&payload)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    // Atomic write: serialize → temp file → rename. Avoids leaving a
    // half-written cache file if the process is killed mid-write.
    let tmp = path.with_extension("bin.tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all().ok();
    drop(file);
    fs::rename(&tmp, &path)?;
    crate::glog_info!(
        "cache write OK: {} commits ({} bytes) to {}",
        commits.len(),
        bytes.len(),
        path.display()
    );
    Ok(())
}

/// Drop the cache file for this repo. Use to recover after a corrupt
/// load (caller already deleted) or from a CLI `--clear-cache` flag.
pub fn invalidate(repo_path: &Path) {
    if let Some(path) = cache_path_for(repo_path) {
        let _ = fs::remove_file(&path);
    }
}
