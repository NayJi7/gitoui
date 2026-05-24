pub mod actions;
pub mod blame;
pub mod cache;
pub mod conflict;
pub mod diff;
pub mod rebase;
pub mod status;

use std::{
    hash::Hash,
    io::{BufRead, BufReader},
    num::NonZeroUsize,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
};

use chrono::{DateTime, FixedOffset};
use lru::LruCache;
use rustc_hash::FxHashMap;

use crate::{Error, Result};

#[derive(
    Debug,
    Default,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct CommitHash(String);

impl CommitHash {
    pub fn as_short_hash(&self) -> &str {
        &self.0[0..7]
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CommitHash {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CommitType {
    #[default]
    Commit,
    Stash,
    Uncommitted,
}

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Commit {
    pub commit_hash: CommitHash,
    pub author_name: String,
    pub author_email: String,
    pub author_date: DateTime<FixedOffset>,
    pub committer_name: String,
    pub committer_email: String,
    pub committer_date: DateTime<FixedOffset>,
    pub commit_message: String,
    pub body: String,
    pub parent_commit_hashes: Vec<CommitHash>,
    pub commit_type: CommitType,
}

/// Lightweight commit metadata: everything the LIST view needs (hash,
/// author, date, subject, parents, kind, email for avatars) and
/// nothing else. ~150 bytes per record vs ~500-2000 bytes for the full
/// `Commit` because we drop `%b` (body) and the committer trio.
///
/// This is the in-memory representation the streaming loader / disk
/// cache / `Repository` keep around per commit. The full `Commit` (with
/// body + committer fields) is fetched on demand via
/// `Repository::fetch_full_commit` when the user opens the Detail view.
///
/// Note: `author_email` is kept here because the commit-list avatars
/// are looked up per-email at every render, paying a `git show` per
/// row would be catastrophic on huge repos.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CommitSummary {
    pub commit_hash: CommitHash,
    pub author_name: String,
    pub author_email: String,
    pub author_date: DateTime<FixedOffset>,
    pub commit_message: String,
    pub parent_commit_hashes: Vec<CommitHash>,
    pub commit_type: CommitType,
}

impl CommitSummary {
    /// Project a full `Commit` down to its list-level subset. Used by
    /// tests + any future ingest path that still produces full
    /// `Commit`s but needs to feed the summary-shaped list storage.
    pub fn from_commit(c: &Commit) -> Self {
        Self {
            commit_hash: c.commit_hash.clone(),
            author_name: c.author_name.clone(),
            author_email: c.author_email.clone(),
            author_date: c.author_date,
            commit_message: c.commit_message.clone(),
            parent_commit_hashes: c.parent_commit_hashes.clone(),
            commit_type: c.commit_type.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ref {
    Tag {
        name: String,
        target: CommitHash,
    },
    Branch {
        name: String,
        target: CommitHash,
    },
    RemoteBranch {
        name: String,
        target: CommitHash,
    },
    Stash {
        name: String,
        message: String,
        target: CommitHash,
    },
}

impl Ref {
    pub fn name(&self) -> &str {
        match self {
            Ref::Tag { name, .. } => name,
            Ref::Branch { name, .. } => name,
            Ref::RemoteBranch { name, .. } => name,
            Ref::Stash { name, .. } => name,
        }
    }

    pub fn target(&self) -> &CommitHash {
        match self {
            Ref::Tag { target, .. } => target,
            Ref::Branch { target, .. } => target,
            Ref::RemoteBranch { target, .. } => target,
            Ref::Stash { target, .. } => target,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Head {
    Branch { name: String },
    Detached { target: CommitHash },
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortCommit {
    Chronological,
    Topological,
}

// `Arc<CommitSummary>` so each summary lives at a stable, cheaply-shared
// heap address. Same two reasons as before (HashMap rehash safety +
// cross-thread Graph ownership) but with summaries instead of full
// commits, so the per-entry RAM cost dropped roughly 5x. The full
// `Commit` (body + committer fields) is fetched on demand via
// `Repository::fetch_full_commit` when the Detail view opens.
type CommitMap = FxHashMap<CommitHash, std::sync::Arc<CommitSummary>>;
type CommitsMap = FxHashMap<CommitHash, Vec<CommitHash>>;

pub(crate) type RefMap = FxHashMap<CommitHash, Vec<Ref>>;

/// Bounded LRU cache for fetched full `Commit`s. Sized to cover the
/// "user is bouncing between a handful of detail views" pattern
/// without ever growing unbounded. Backed by the `lru` crate (true
/// O(1) get/put on a hashmap + linked list).
const FULL_COMMIT_CACHE_CAPACITY: usize = 50;

#[derive(Debug)]
pub struct Repository {
    path: PathBuf,
    commit_map: CommitMap,

    parents_map: CommitsMap,
    children_map: CommitsMap,

    ref_map: RefMap,
    head: Head,
    // to preserve order of the original commits from `git log`, we store the commit hashes
    commit_hashes: Vec<CommitHash>,
    uncommitted_changes: Option<status::UncommittedChanges>,
    /// On-demand cache of full `Commit` records (body + committer
    /// fields), populated lazily by `fetch_full_commit`. Bounded by
    /// `FULL_COMMIT_CACHE_CAPACITY` so the user can bounce between a
    /// handful of recently-opened detail views with no extra
    /// `git show` round-trips.
    full_commit_cache: Mutex<LruCache<CommitHash, std::sync::Arc<Commit>>>,
}

impl Repository {
    pub fn load(path: &Path, sort: SortCommit, max_count: Option<usize>) -> Result<Self> {
        check_git_repository(path)?;

        let (mut ref_map, head) = load_refs(path);

        let stashes = load_all_stash_summaries(path);
        let summaries = load_all_summaries(path, sort, &head, &stashes, max_count);
        if summaries.is_empty() {
            return Err(Error::Git("no commits in the repository".into()));
        }

        let mut summaries = merge_stashes_to_summaries(summaries, stashes);

        let uncommitted_changes = status::UncommittedChanges::load(path).ok();
        if let Some(changes) = &uncommitted_changes {
            if changes.is_dirty() {
                let fake_hash = CommitHash("0000000000000000000000000000000000000000".to_string());
                let parent_hash = match &head {
                    Head::Detached { target } => Some(target.clone()),
                    Head::Branch { name } => ref_map.values().flatten().find_map(|r| match r {
                        Ref::Branch {
                            name: branch_name,
                            target,
                        } if branch_name == name => Some(target.clone()),
                        _ => None,
                    }),
                    Head::None => None,
                };
                let parent_commit_hashes = if let Some(h) = parent_hash {
                    vec![h]
                } else {
                    vec![]
                };

                // Tests pin `GITOUI_FAKE_NOW` to a fixed RFC3339 instant
                // so snapshot baselines containing the Uncommitted node
                // (which prints today's date next to the synthetic commit
                // hash) stay byte-for-byte reproducible across runs. In
                // any normal launch the env var is unset and we fall
                // back to real wall-clock time.
                let now = std::env::var("GITOUI_FAKE_NOW")
                    .ok()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                    .unwrap_or_else(|| chrono::Local::now().fixed_offset());
                let fake_summary = CommitSummary {
                    commit_hash: fake_hash.clone(),
                    parent_commit_hashes,
                    author_name: "".to_string(),
                    author_email: "".to_string(),
                    author_date: now,
                    commit_message: "Uncommitted changes".to_string(),
                    commit_type: CommitType::Uncommitted,
                };

                summaries.insert(0, fake_summary);
            }
        }

        let commit_hashes = summaries.iter().map(|c| c.commit_hash.clone()).collect();

        let (parents_map, children_map) = build_commits_maps_from_summaries(&summaries);
        let commit_map = to_commit_map(summaries);

        let stash_ref_map = load_stashes_as_refs(path);
        merge_ref_maps(&mut ref_map, stash_ref_map);

        Ok(Self::new(
            path.to_path_buf(),
            commit_map,
            parents_map,
            children_map,
            ref_map,
            head,
            commit_hashes,
            uncommitted_changes,
        ))
    }

    /// Reconstruct a `Repository` from a previously-cached commit
    /// summary list. Skips the `git log` walk - that's the whole
    /// point of the cache - but still reads stashes, refs, head and
    /// uncommitted changes fresh (they're cheap and may have moved
    /// since the cache was written). The caller is responsible for
    /// gating this path on a HEAD-equality check (see `cache::load_for`).
    pub fn from_cached_summaries(path: &Path, mut summaries: Vec<CommitSummary>) -> Result<Self> {
        check_git_repository(path)?;
        if summaries.is_empty() {
            return Err(Error::Git("cached commit list is empty".into()));
        }

        let (mut ref_map, head) = load_refs(path);
        let stashes = load_all_stash_summaries(path);
        // Merge stashes the same way the fresh load does so the
        // stash markers show up at their parent commits.
        summaries = merge_stashes_to_summaries(summaries, stashes);

        let uncommitted_changes = status::UncommittedChanges::load(path).ok();
        if let Some(changes) = &uncommitted_changes {
            if changes.is_dirty() {
                let fake_hash = CommitHash("0000000000000000000000000000000000000000".to_string());
                let parent_hash = match &head {
                    Head::Detached { target } => Some(target.clone()),
                    Head::Branch { name } => ref_map.values().flatten().find_map(|r| match r {
                        Ref::Branch {
                            name: branch_name,
                            target,
                        } if branch_name == name => Some(target.clone()),
                        _ => None,
                    }),
                    Head::None => None,
                };
                let parent_commit_hashes = if let Some(h) = parent_hash {
                    vec![h]
                } else {
                    vec![]
                };
                let now = std::env::var("GITOUI_FAKE_NOW")
                    .ok()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                    .unwrap_or_else(|| chrono::Local::now().fixed_offset());
                let fake_summary = CommitSummary {
                    commit_hash: fake_hash.clone(),
                    parent_commit_hashes,
                    author_name: "".to_string(),
                    author_email: "".to_string(),
                    author_date: now,
                    commit_message: "Uncommitted changes".to_string(),
                    commit_type: CommitType::Uncommitted,
                };
                summaries.insert(0, fake_summary);
            }
        }

        let commit_hashes = summaries.iter().map(|c| c.commit_hash.clone()).collect();
        let (parents_map, children_map) = build_commits_maps_from_summaries(&summaries);
        let commit_map = to_commit_map(summaries);
        let stash_ref_map = load_stashes_as_refs(path);
        merge_ref_maps(&mut ref_map, stash_ref_map);

        Ok(Self::new(
            path.to_path_buf(),
            commit_map,
            parents_map,
            children_map,
            ref_map,
            head,
            commit_hashes,
            uncommitted_changes,
        ))
    }

    pub fn new(
        path: PathBuf,
        commit_map: CommitMap,
        parents_map: CommitsMap,
        children_map: CommitsMap,
        ref_map: RefMap,
        head: Head,
        commit_hashes: Vec<CommitHash>,
        uncommitted_changes: Option<status::UncommittedChanges>,
    ) -> Self {
        Self {
            path,
            commit_map,
            parents_map,
            children_map,
            ref_map,
            head,
            commit_hashes,
            uncommitted_changes,
            full_commit_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(FULL_COMMIT_CACHE_CAPACITY).unwrap(),
            )),
        }
    }

    pub fn commit(&self, commit_hash: &CommitHash) -> Option<&CommitSummary> {
        self.commit_map.get(commit_hash).map(|arc| arc.as_ref())
    }

    /// Clone the `Arc<CommitSummary>` from the in-memory map. Cheap
    /// (refcount bump only) so callers can hand the summary around to
    /// long-lived owners (e.g. the owned `Graph` built by `calc_graph`,
    /// or the bg streamer shipping summaries across the thread
    /// boundary) without paying for a deep clone, and without holding
    /// a `&Repository` borrow.
    pub fn commit_arc(&self, commit_hash: &CommitHash) -> Option<std::sync::Arc<CommitSummary>> {
        self.commit_map.get(commit_hash).cloned()
    }

    pub fn all_commits(&self) -> Vec<&CommitSummary> {
        self.commit_hashes
            .iter()
            .filter_map(|hash| self.commit(hash))
            .collect()
    }

    /// Iterate the full commit list in render order as cheaply-shareable `Arc`s.
    /// Used by `calc_graph` to hand ownership of the summary metadata to the
    /// resulting `Graph`, which then outlives the foreground `Repository` view
    /// (e.g. when the background streamer rebuilds the topology).
    pub fn all_commits_arc(&self) -> Vec<std::sync::Arc<CommitSummary>> {
        self.commit_hashes
            .iter()
            .filter_map(|hash| self.commit_arc(hash))
            .collect()
    }

    pub fn stashes(&self) -> Vec<CommitSummary> {
        load_all_stash_summaries(&self.path)
    }

    pub fn commit_count(&self) -> usize {
        self.commit_hashes.len()
    }

    /// Append a freshly-streamed batch of summaries from the bg
    /// loader. Dedupes against `commit_map` so the loader can safely
    /// overshoot the initial foreground slice without producing
    /// duplicates. Rebuilds parent/child indices incrementally - we
    /// add the new summaries' edges without touching the existing ones.
    pub fn append_commits(&mut self, new_summaries: Vec<CommitSummary>) -> usize {
        let mut appended = 0usize;
        for summary in new_summaries {
            if self.commit_map.contains_key(&summary.commit_hash) {
                continue;
            }
            // Update parent/child indices incrementally.
            for parent in &summary.parent_commit_hashes {
                self.parents_map
                    .entry(summary.commit_hash.clone())
                    .or_default()
                    .push(parent.clone());
                self.children_map
                    .entry(parent.clone())
                    .or_default()
                    .push(summary.commit_hash.clone());
            }
            self.commit_hashes.push(summary.commit_hash.clone());
            self.commit_map
                .insert(summary.commit_hash.clone(), std::sync::Arc::new(summary));
            appended += 1;
        }
        appended
    }

    /// Resolve a full `Commit` (body + committer trio) by hash. Returns
    /// a refcounted handle backed by an internal LRU so re-opening the
    /// same detail view doesn't pay a `git show` round-trip every time.
    ///
    /// Path:
    /// 1. LRU cache hit -> return immediately.
    /// 2. Spawn `git show --no-patch --pretty=<full>` for the SHA.
    /// 3. Parse, insert into LRU, return.
    ///
    /// Returns `None` when the commit isn't accessible from the local
    /// git tree (e.g. fully orphaned PR commit that was never fetched).
    pub fn fetch_full_commit(&self, hash: &CommitHash) -> Option<std::sync::Arc<Commit>> {
        if let Ok(mut cache) = self.full_commit_cache.lock() {
            if let Some(cached) = cache.get(hash) {
                return Some(cached.clone());
            }
        }
        let loaded = load_commit_by_hash(&self.path, hash.as_str())?;
        let arc = std::sync::Arc::new(loaded);
        if let Ok(mut cache) = self.full_commit_cache.lock() {
            cache.put(hash.clone(), arc.clone());
        }
        Some(arc)
    }

    pub fn parents_hash(&self, commit_hash: &CommitHash) -> Vec<&CommitHash> {
        self.parents_map
            .get(commit_hash)
            .map(|hs| hs.iter().collect::<Vec<&CommitHash>>())
            .unwrap_or_default()
    }

    pub fn children_hash(&self, commit_hash: &CommitHash) -> Vec<&CommitHash> {
        self.children_map
            .get(commit_hash)
            .map(|hs| hs.iter().collect::<Vec<&CommitHash>>())
            .unwrap_or_default()
    }

    pub fn refs(&self, commit_hash: &CommitHash) -> Vec<&Ref> {
        self.ref_map
            .get(commit_hash)
            .map(|refs| refs.iter().collect::<Vec<&Ref>>())
            .unwrap_or_default()
    }

    pub fn all_refs(&self) -> Vec<&Ref> {
        self.ref_map.values().flatten().collect()
    }

    pub fn head(&self) -> &Head {
        &self.head
    }

    pub fn uncommitted_changes(&self) -> Option<&status::UncommittedChanges> {
        self.uncommitted_changes.as_ref()
    }

    pub fn commit_detail(&self, commit_hash: &CommitHash) -> (Commit, Vec<FileChange>) {
        // PR commits fetched on demand (via `refs/pull/<n>/head`) aren't
        // in the in-memory map; `fetch_full_commit` falls back to a
        // one-off `git show` for them. When even that misses (totally
        // orphaned commit), default to an empty Commit so the existing
        // CommitDetail / DiffView still render gracefully.
        let commit = self
            .fetch_full_commit(commit_hash)
            .map(|arc| arc.as_ref().clone())
            .unwrap_or_default();
        let changes = if commit.parent_commit_hashes.is_empty() {
            get_initial_commit_additions(&self.path, commit_hash)
        } else {
            get_diff_summary(&self.path, commit_hash)
        };
        (commit, changes)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn check_git_repository(path: &Path) -> Result<()> {
    if !is_inside_work_tree(path) && !is_bare_repository(path) {
        return Err(Error::Git(
            "not a git repository (or any of the parent directories)".into(),
        ));
    }
    Ok(())
}

/// Resolve the root of the git work tree (or bare repo) that contains
/// `path`. Returns `Some(root)` when `path` is *anywhere* inside a git
/// tree, the root itself OR any sub-directory, and `None` otherwise.
///
/// This is the entry point gitoui uses to anchor its whole UI at the
/// repo root: running `gitoui` from `~/proj/src/sub/` resolves to
/// `~/proj/` and we `cd` there before loading. The dir-input overlay
/// uses the same resolver so users can type any path inside a repo and
/// gitoui rebases to the root.
///
/// Combines `--is-bare-repository` and `--show-toplevel` into a single
/// `git rev-parse` invocation, `git` prints one result per flag on its
/// own line, so we get both answers for the cost of one fork+exec.
pub fn find_repo_root(path: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--is-bare-repository")
        .arg("--show-toplevel")
        .current_dir(path)
        .output()
        .ok()?;
    // stdout layout (git processes flags left-to-right and prints each
    // result on its own line):
    //   work-tree root:  "false\n<root>\n"     exit 0
    //   subfolder:       "false\n<root>\n"     exit 0, same as root,
    //                                                   `<root>` always points
    //                                                   at the toplevel
    //   bare repo:       "true\n"              exit 128, `--show-toplevel`
    //                                                     fails ("must be run
    //                                                     in a work tree") but
    //                                                     `--is-bare-repository`
    //                                                     already wrote "true"
    //   non-repo:        ""                    exit 128
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    if lines.next() == Some("true") {
        // Bare repo, the given path *is* the repo (or `--git-dir`).
        // Canonicalise so the caller sees a stable absolute path.
        return std::fs::canonicalize(path).ok();
    }
    if !output.status.success() {
        return None;
    }
    let toplevel = lines.next().unwrap_or("").trim();
    if toplevel.is_empty() {
        return None;
    }
    // Canonicalise so symlinks / trailing slashes don't surface in the
    // displayed pwd or in equality checks against other PathBuf values.
    std::fs::canonicalize(toplevel).ok()
}

/// Back-compat shim, `find_repo_root(path).is_some()`. Used in spots
/// where callers only need a yes/no answer (e.g. early validation).
pub fn is_git_path(path: &Path) -> bool {
    find_repo_root(path).is_some()
}

fn is_inside_work_tree(path: &Path) -> bool {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .current_dir(path)
        .output()
        .unwrap();
    output.status.success() && output.stdout == b"true\n"
}

fn is_bare_repository(path: &Path) -> bool {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--is-bare-repository")
        .current_dir(path)
        .output()
        .unwrap();
    output.status.success() && output.stdout == b"true\n"
}

fn load_all_summaries(
    path: &Path,
    sort: SortCommit,
    head: &Head,
    stashes: &[CommitSummary],
    max_count: Option<usize>,
) -> Vec<CommitSummary> {
    let mut cmd = Command::new("git");
    cmd.arg("log");

    cmd.arg(match sort {
        SortCommit::Chronological => "--date-order",
        SortCommit::Topological => "--topo-order",
    })
    .arg(format!("--pretty={LOAD_SUMMARIES_FORMAT}"))
    .arg("--date=iso-strict")
    .arg("-z"); // use NUL as a delimiter

    // exclude stashes and other refs
    cmd.arg("--branches").arg("--remotes").arg("--tags");

    // commits that are reachable from the stashes
    stashes.iter().for_each(|stash| {
        cmd.arg(stash.parent_commit_hashes[0].as_str());
    });

    if !matches!(head, Head::None) {
        cmd.arg("HEAD");
    }

    if let Some(n) = max_count {
        cmd.arg("--max-count").arg(n.to_string());
    }

    cmd.current_dir(path).stdout(Stdio::piped());

    // Graceful degrade: if `git` is missing from PATH or we lack
    // execution permission, fall back to an empty Vec with a logged
    // warning instead of panicking. The user sees an empty commit
    // list (caller treats this as "empty repo") with a clear
    // diagnostic in `~/.config/gitoui/logs/`.
    let mut process = match cmd.spawn() {
        Ok(p) => p,
        Err(e) => {
            crate::glog_warn!("git log spawn failed: {} - is git installed?", e);
            return Vec::new();
        }
    };

    let Some(stdout) = process.stdout.take() else {
        crate::glog_warn!("git log process has no stdout pipe");
        let _ = process.wait();
        return Vec::new();
    };

    let reader = BufReader::new(stdout);

    let mut summaries = Vec::new();

    for bytes in reader.split(b'\0') {
        let Ok(bytes) = bytes else {
            // I/O error mid-read: bail and return whatever we
            // already parsed instead of panicking.
            crate::glog_warn!("git log stdout read failed mid-stream");
            break;
        };
        if bytes.is_empty() {
            continue;
        }
        let s = String::from_utf8_lossy(&bytes);

        match parse_commit_summary(&s, CommitType::Commit) {
            Some(summary) => summaries.push(summary),
            None => {
                eprintln!(
                    "gitoui: skipping malformed commit record (expected 6 fields, got {})",
                    s.split('\x1f').count()
                );
            }
        }
    }

    if let Err(e) = process.wait() {
        crate::glog_warn!("git log wait failed: {}", e);
    }

    summaries
}

/// Stream commits past an initial-load offset via a `git log --skip=N`
/// child process. Calls `on_batch` every `batch_size` commits with the
/// fresh batch. Returns when stdout EOFs (git log finished) or when
/// `should_stop` returns true. Used by the background loader to feed
/// the main thread without ever blocking the foreground render.
///
/// We re-issue `git log` rather than re-reading the foreground load's
/// process so the bg loader is independent: a fast quit on the
/// foreground side doesn't strand a half-read stdout, and the offset
/// is exact (no race against the foreground's lazy line buffering).
pub fn stream_commits_after(
    path: &Path,
    sort: SortCommit,
    head: &Head,
    stashes: &[CommitSummary],
    skip: usize,
    batch_size: usize,
    mut on_batch: impl FnMut(Vec<CommitSummary>) -> bool,
) {
    let mut cmd = Command::new("git");
    cmd.arg("log")
        .arg(match sort {
            SortCommit::Chronological => "--date-order",
            SortCommit::Topological => "--topo-order",
        })
        .arg(format!("--pretty={LOAD_SUMMARIES_FORMAT}"))
        .arg("--date=iso-strict")
        .arg("-z");
    cmd.arg("--branches").arg("--remotes").arg("--tags");
    stashes.iter().for_each(|stash| {
        cmd.arg(stash.parent_commit_hashes[0].as_str());
    });
    if !matches!(head, Head::None) {
        cmd.arg("HEAD");
    }
    if skip > 0 {
        cmd.arg(format!("--skip={skip}"));
    }
    cmd.current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut process = match cmd.spawn() {
        Ok(p) => p,
        Err(_) => return,
    };
    let Some(stdout) = process.stdout.take() else {
        return;
    };
    let reader = BufReader::new(stdout);

    let mut batch: Vec<CommitSummary> = Vec::with_capacity(batch_size);
    for bytes in reader.split(b'\0') {
        let Ok(bytes) = bytes else { break };
        if bytes.is_empty() {
            continue;
        }
        let s = String::from_utf8_lossy(&bytes);
        let Some(summary) = parse_commit_summary(&s, CommitType::Commit) else {
            continue;
        };
        batch.push(summary);
        if batch.len() >= batch_size {
            let stop = on_batch(std::mem::replace(
                &mut batch,
                Vec::with_capacity(batch_size),
            ));
            if stop {
                let _ = process.kill();
                return;
            }
        }
    }
    if !batch.is_empty() {
        on_batch(batch);
    }
    let _ = process.wait();
}

/// Spawn `git log` and invoke `on_commit` once per parsed commit. Used
/// by the bg streamer's cache-miss fallback to feed commits to the UI
/// AND to the StreamWriter one record at a time, so peak memory stays
/// bounded even on huge repos (no intermediate Vec<Commit>).
///
/// Returns when stdout EOFs (git log finished) or when `on_commit`
/// returns `false` (caller wants to stop). Safe to call from a bg
/// thread - all I/O is best-effort.
pub fn stream_commits_one_by_one(
    path: &Path,
    sort: SortCommit,
    head: &Head,
    stashes: &[CommitSummary],
    mut on_commit: impl FnMut(CommitSummary) -> bool,
) {
    let mut cmd = Command::new("git");
    cmd.arg("log")
        .arg(match sort {
            SortCommit::Chronological => "--date-order",
            SortCommit::Topological => "--topo-order",
        })
        .arg(format!("--pretty={LOAD_SUMMARIES_FORMAT}"))
        .arg("--date=iso-strict")
        .arg("-z");
    cmd.arg("--branches").arg("--remotes").arg("--tags");
    stashes.iter().for_each(|stash| {
        cmd.arg(stash.parent_commit_hashes[0].as_str());
    });
    if !matches!(head, Head::None) {
        cmd.arg("HEAD");
    }
    cmd.current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut process = match cmd.spawn() {
        Ok(p) => p,
        Err(_) => return,
    };
    let Some(stdout) = process.stdout.take() else {
        return;
    };
    let reader = BufReader::new(stdout);
    for bytes in reader.split(b'\0') {
        let Ok(bytes) = bytes else { break };
        if bytes.is_empty() {
            continue;
        }
        let s = String::from_utf8_lossy(&bytes);
        let Some(summary) = parse_commit_summary(&s, CommitType::Commit) else {
            continue;
        };
        if !on_commit(summary) {
            let _ = process.kill();
            break;
        }
    }
    let _ = process.wait();
}

/// Walk just the commits in the symmetric range `<cached_head>..HEAD`,
/// i.e. only commits reachable from current HEAD but NOT from the
/// stored cached head. Used by the cache's surgical-refresh path on
/// fast-forward HEAD movement: instead of re-walking the entire
/// history when the user adds one commit, walk only that one and
/// prepend to the cached vector.
///
/// Returns the new commits in the same chronological-newest-first
/// order produced by `stream_commits_after`. Returns `None` on any
/// failure (cached_head unreachable, git rev-list failure, etc.)
/// so the caller falls back to a full re-walk.
pub fn walk_commits_since(
    path: &Path,
    sort: SortCommit,
    cached_head: &str,
) -> Option<Vec<CommitSummary>> {
    let range = format!("{}..HEAD", cached_head);
    let mut cmd = Command::new("git");
    cmd.arg("log")
        .arg(match sort {
            SortCommit::Chronological => "--date-order",
            SortCommit::Topological => "--topo-order",
        })
        .arg(format!("--pretty={LOAD_SUMMARIES_FORMAT}"))
        .arg("--date=iso-strict")
        .arg("-z")
        .arg(&range)
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut process = cmd.spawn().ok()?;
    let stdout = process.stdout.take()?;
    let reader = BufReader::new(stdout);
    let mut out: Vec<CommitSummary> = Vec::new();
    for bytes in reader.split(b'\0') {
        let Ok(bytes) = bytes else { break };
        if bytes.is_empty() {
            continue;
        }
        let s = String::from_utf8_lossy(&bytes);
        if let Some(summary) = parse_commit_summary(&s, CommitType::Commit) {
            out.push(summary);
        }
    }
    let status = process.wait().ok()?;
    if !status.success() {
        return None;
    }
    Some(out)
}

fn load_all_stash_summaries(path: &Path) -> Vec<CommitSummary> {
    let mut cmd = match Command::new("git")
        .arg("stash")
        .arg("list")
        .arg(format!("--pretty={LOAD_SUMMARIES_FORMAT}"))
        .arg("--date=iso-strict")
        .arg("-z")
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            crate::glog_warn!("git stash list spawn failed: {}", e);
            return Vec::new();
        }
    };

    let Some(stdout) = cmd.stdout.take() else {
        crate::glog_warn!("git stash list has no stdout");
        let _ = cmd.wait();
        return Vec::new();
    };

    let reader = BufReader::new(stdout);

    let mut summaries = Vec::new();

    for bytes in reader.split(b'\0') {
        let Ok(bytes) = bytes else {
            crate::glog_warn!("git stash list stdout read failed");
            break;
        };
        if bytes.is_empty() {
            continue;
        }
        let s = String::from_utf8_lossy(&bytes);

        match parse_commit_summary(&s, CommitType::Stash) {
            Some(summary) => summaries.push(summary),
            None => {
                eprintln!(
                    "gitoui: skipping malformed stash record (expected 6 fields, got {})",
                    s.split('\x1f').count()
                );
            }
        }
    }

    cmd.wait().unwrap();

    summaries
}

/// Resolve a single commit by hash, even if it isn't reachable from
/// any local ref (e.g. just fetched from `refs/pull/{n}/head`). Used
/// by the PR view to surface the existing CommitDetail / DiffView
/// for commits that haven't been added to the in-memory commit map.
pub fn load_commit_by_hash(path: &Path, hash: &str) -> Option<Commit> {
    let output = Command::new("git")
        .arg("log")
        .arg("-1")
        .arg("-z") // NUL-terminate records so multi-line %b doesn't break parsing
        .arg(format!("--pretty={}", load_commits_format()))
        .arg("--date=iso-strict")
        .arg(hash)
        .current_dir(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    // Split on the single NUL git appends after every record. Without
    // `-z`, the previous `s.lines().next()` truncated at the first
    // newline in `%b` (commit body) - any commit with a multi-line
    // body (most non-trivial ones) parsed as only the first line of
    // output, so parts.len() < 10 -> None -> caller fell back to
    // Commit::default() (epoch 1970-01-01 date, empty author / SHA).
    // The user saw this as "some commits open with empty metadata".
    let record = s.split('\x00').next()?.trim_end_matches('\n');
    let parts: Vec<&str> = record.split('\x1f').collect();
    if parts.len() < 10 {
        return None;
    }
    Some(Commit {
        commit_hash: parts[0].into(),
        author_name: parts[1].into(),
        author_email: parts[2].into(),
        author_date: parse_iso_date(parts[3]),
        committer_name: parts[4].into(),
        committer_email: parts[5].into(),
        committer_date: parse_iso_date(parts[6]),
        commit_message: parts[7].into(),
        body: parts[8].into(),
        parent_commit_hashes: parse_parent_commit_hashes(parts[9]),
        commit_type: CommitType::Commit,
    })
}

/// Pull a single PR commit into the local repo so `git show` can
/// render it. Two passes:
///
/// 1. `+refs/pull/<n>/head:refs/pull/<n>/head`, the canonical PR
///    head refspec. Brings the whole PR branch in one shot for live
///    PRs and is also faster than per-SHA fetches when the user
///    drills into several commits on the same PR.
/// 2. Direct SHA fetch (`git fetch <url> <sha>`), fallback for
///    squash-merged PRs whose branch was deleted: the head ref no
///    longer points at the original commit, but GitHub still keeps
///    the commit reachable for the PR's diff view and serves it
///    when the client asks for a reachable-via-pull-but-orphan-from-
///    branches SHA (`uploadpack.allowReachableSHA1InWant` = true on
///    GitHub).
///
/// Returns `Ok(())` only when the commit object is actually present
/// locally after the fetch. Returns `Err` with git stderr (or a
/// generic hint) when both passes fail.
pub fn fetch_pull_request_commit(
    path: &Path,
    owner: &str,
    repo: &str,
    pr_number: u64,
    sha: &str,
) -> std::result::Result<(), String> {
    let url = format!("https://github.com/{}/{}", owner, repo);
    // Pass 1, pull/<n>/head refspec. Best-effort: ignore errors and
    // re-check whether the commit landed.
    let refspec = format!("+refs/pull/{}/head:refs/pull/{}/head", pr_number, pr_number);
    let _ = Command::new("git")
        .args(["fetch", "--quiet", &url, &refspec])
        .current_dir(path)
        .output();
    if load_commit_by_hash(path, sha).is_some() {
        return Ok(());
    }
    // Pass 2, direct SHA fetch. `--depth=1` avoids dragging the
    // commit's full ancestry along just to render a single diff.
    let output = Command::new("git")
        .args(["fetch", "--quiet", "--depth=1", &url, sha])
        .current_dir(path)
        .output()
        .map_err(|e| format!("git fetch failed: {}", e))?;
    if load_commit_by_hash(path, sha).is_some() {
        return Ok(());
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr
            .lines()
            .next()
            .unwrap_or("git fetch failed")
            .to_string());
    }
    Err(format!(
        "git fetch succeeded but commit {} is still missing locally",
        &sha[..7.min(sha.len())]
    ))
}

fn load_commits_format() -> String {
    [
        "%H", "%an", "%ae", "%ad", "%cn", "%ce", "%cd", "%s", "%b", "%P",
    ]
    .join("%x1f") // use Unit Separator as a delimiter
}

/// Lightweight `git log` format for the list-level `CommitSummary`
/// records. Strips `%cn` / `%ce` / `%cd` (committer trio) and `%b`
/// (body), the latter is the dominant byte cost in `git log` output
/// and is unused outside the Detail view. Saves ~5x bytes per record
/// on average for large repos.
///
/// Field order MUST match `parse_commit_summary`:
///   0: %H   commit hash
///   1: %an  author name
///   2: %ae  author email
///   3: %ad  author date (iso-strict)
///   4: %s   subject (commit message first line)
///   5: %P   space-separated parent hashes
const LOAD_SUMMARIES_FORMAT: &str = "%H%x1f%an%x1f%ae%x1f%ad%x1f%s%x1f%P";

/// Parse a single `LOAD_SUMMARIES_FORMAT`-shaped record into a
/// `CommitSummary` with the given `commit_type` (Commit vs Stash).
/// Returns `None` when the record is malformed (wrong field count),
/// so the caller can skip and keep streaming.
fn parse_commit_summary(s: &str, commit_type: CommitType) -> Option<CommitSummary> {
    let parts: Vec<&str> = s.split('\x1f').collect();
    if parts.len() != 6 {
        return None;
    }
    Some(CommitSummary {
        commit_hash: parts[0].into(),
        author_name: parts[1].into(),
        author_email: parts[2].into(),
        author_date: parse_iso_date(parts[3]),
        commit_message: parts[4].into(),
        parent_commit_hashes: parse_parent_commit_hashes(parts[5]),
        commit_type,
    })
}

fn parse_iso_date(s: &str) -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339(s).unwrap()
}

fn parse_parent_commit_hashes(s: &str) -> Vec<CommitHash> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(' ').map(|s| s.into()).collect()
}

fn build_commits_maps_from_summaries(summaries: &[CommitSummary]) -> (CommitsMap, CommitsMap) {
    let mut parents_map: CommitsMap = FxHashMap::default();
    let mut children_map: CommitsMap = FxHashMap::default();
    for summary in summaries {
        let hash = &summary.commit_hash;
        for parent_hash in &summary.parent_commit_hashes {
            parents_map
                .entry(hash.clone())
                .or_default()
                .push(parent_hash.clone());
            children_map
                .entry(parent_hash.clone())
                .or_default()
                .push(hash.clone());
        }
    }

    (parents_map, children_map)
}

fn to_commit_map(summaries: Vec<CommitSummary>) -> CommitMap {
    summaries
        .into_iter()
        .map(|s| (s.commit_hash.clone(), std::sync::Arc::new(s)))
        .collect()
}

fn merge_stashes_to_summaries(
    summaries: Vec<CommitSummary>,
    stashes: Vec<CommitSummary>,
) -> Vec<CommitSummary> {
    // Stash summary has multiple parent commits, but the first parent commit is the commit that the stash was created from.
    // If the first parent commit is not found, the stash commit is ignored.
    let mut ret = Vec::new();
    let mut stash_map: FxHashMap<CommitHash, Vec<CommitSummary>> =
        stashes
            .into_iter()
            .fold(FxHashMap::default(), |mut acc, s| {
                let parent = s.parent_commit_hashes[0].clone();
                acc.entry(parent).or_default().push(s);
                acc
            });
    for summary in summaries {
        if let Some(stashes) = stash_map.remove(&summary.commit_hash) {
            for stash in stashes {
                ret.push(stash);
            }
        }
        ret.push(summary);
    }
    ret
}

pub(crate) fn load_refs(path: &Path) -> (RefMap, Head) {
    let mut cmd = Command::new("git")
        .arg("show-ref")
        .arg("--head")
        .arg("--dereference")
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdout = cmd.stdout.take().expect("failed to open stdout");

    let reader = BufReader::new(stdout);

    let mut ref_map = RefMap::default();
    let mut tag_map: FxHashMap<String, Ref> = FxHashMap::default();
    let mut head: Head = Head::None;

    for line in reader.lines() {
        let line = line.unwrap();

        let parts: Vec<&str> = line.split(' ').collect();
        if parts.len() != 2 {
            eprintln!(
                "gitoui: skipping malformed ref line (expected 2 fields, got {})",
                parts.len()
            );
            continue;
        }

        let hash = parts[0];
        let refs = parts[1];

        if refs == "HEAD" {
            head = if let Some(branch) = get_current_branch(path) {
                Head::Branch { name: branch }
            } else {
                Head::Detached {
                    target: hash.into(),
                }
            };
        } else if let Some(r) = parse_branch_refs(hash, refs) {
            ref_map.entry(hash.into()).or_default().push(r);
        } else if let Some(r) = parse_tag_refs(hash, refs) {
            // if annotated tag exists, it will be overwritten by the following line of the same tag
            // this will make the tag point to the commit that the annotated tag points to
            tag_map.insert(r.name().into(), r);
        }
    }

    for tag in tag_map.into_values() {
        ref_map.entry(tag.target().clone()).or_default().push(tag);
    }

    ref_map.values_mut().for_each(|refs| refs.sort());

    cmd.wait().unwrap();

    (ref_map, head)
}

pub(crate) fn load_stashes_as_refs(path: &Path) -> RefMap {
    let format = ["%gd", "%H", "%s"].join("%x1f"); // use Unit Separator as a delimiter
    let mut cmd = Command::new("git")
        .arg("stash")
        .arg("list")
        .arg(format!("--format={format}"))
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdout = cmd.stdout.take().expect("failed to open stdout");

    let reader = BufReader::new(stdout);

    let mut ref_map = RefMap::default();

    for line in reader.lines() {
        let line = line.unwrap();

        let parts: Vec<&str> = line.split('\x1f').collect();
        if parts.len() != 3 {
            eprintln!(
                "gitoui: skipping malformed stash ref line (expected 3 fields, got {})",
                parts.len()
            );
            continue;
        }

        let name = parts[0];
        let hash = parts[1];
        let commit_message = parts[2];

        let r = Ref::Stash {
            name: name.into(),
            message: commit_message.into(),
            target: hash.into(),
        };

        ref_map.entry(hash.into()).or_default().push(r);
    }

    cmd.wait().unwrap();

    ref_map
}

pub(crate) fn merge_ref_maps(m1: &mut RefMap, m2: RefMap) {
    for (k, v) in m2 {
        m1.entry(k).or_default().extend(v);
    }
}

fn parse_branch_refs(hash: &str, refs: &str) -> Option<Ref> {
    if refs.starts_with("refs/heads/") {
        let name = refs.trim_start_matches("refs/heads/");
        Some(Ref::Branch {
            name: name.into(),
            target: hash.into(),
        })
    } else if refs.starts_with("refs/remotes/") {
        let name = refs.trim_start_matches("refs/remotes/");
        Some(Ref::RemoteBranch {
            name: name.into(),
            target: hash.into(),
        })
    } else {
        None
    }
}

fn parse_tag_refs(hash: &str, refs: &str) -> Option<Ref> {
    if refs.starts_with("refs/tags/") {
        let name = refs.trim_start_matches("refs/tags/");
        let name = name.trim_end_matches("^{}");
        Some(Ref::Tag {
            name: name.into(),
            target: hash.into(),
        })
    } else {
        None
    }
}

fn get_current_branch(path: &Path) -> Option<String> {
    let mut cmd = Command::new("git")
        .arg("branch")
        .arg("--show-current")
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdout = cmd.stdout.take().expect("failed to open stdout");

    let reader = BufReader::new(stdout);

    let branch = if let Some(line) = reader.lines().next() {
        line.ok()
    } else {
        None
    };

    cmd.wait().unwrap();

    branch
}

#[derive(Debug)]
pub enum FileChange {
    Add {
        path: String,
        additions: usize,
    },
    Modify {
        path: String,
        additions: usize,
        deletions: usize,
    },
    Delete {
        path: String,
        deletions: usize,
    },
    Move {
        from: String,
        to: String,
        additions: usize,
        deletions: usize,
    },
}

pub fn get_diff_summary(path: &Path, commit_hash: &CommitHash) -> Vec<FileChange> {
    // Combine the previous two `git diff` invocations (`--name-status` for
    // file kinds + `--numstat` for line counts) into a single `git log`
    // call. `git diff` honours only the last of `--name-status`/`--numstat`,
    // but `git log --raw --numstat` prints BOTH sections one after the
    // other in a single fork+exec, halving the cost of opening any commit
    // in the Detail view (was 2 forks per click).
    //
    // Output layout (--format= empty drops the commit header):
    //   :100644 100644 <h1> <h2> M\tpath
    //   :000000 100644 <h1> <h2> A\tnewpath
    //   :100644 100644 <h1> <h2> R100\toldname\tnewname
    //   0\t1\tpath
    //   3\t0\tnewpath
    //   2\t1\toldname => newname
    let output = Command::new("git")
        .arg("log")
        .arg("--format=")
        .arg("--raw")
        .arg("--numstat")
        .arg("-M")
        // `git log` hides diffs for merge commits by default. Force a diff
        // against the first parent, same view the previous `git diff
        // <hash>^ <hash>` call gave us. `-m` would also work but emits one
        // diff per parent, which we'd then have to dedupe.
        .arg("--first-parent")
        .arg("-1")
        .arg(&commit_hash.0)
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .unwrap();

    let mut status_map: FxHashMap<String, (char, Option<String>)> = FxHashMap::default();
    let mut stats_map: FxHashMap<String, (usize, usize)> = FxHashMap::default();
    for line in output.stdout.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(line);
        if let Some(rest) = line.strip_prefix(':') {
            // Raw row: `<m1> <m2> <h1> <h2> <STATUS>\t<path>[\t<newpath>]`
            // The metadata block is space-separated; the status + paths are
            // tab-separated. Splitting on the first tab gives us a clean
            // boundary between the two halves.
            let Some((meta, paths)) = rest.split_once('\t') else {
                continue;
            };
            let status_token = meta.split_whitespace().last().unwrap_or("");
            let status = status_token.chars().next().unwrap_or('?');
            let mut parts = paths.split('\t');
            let path_name = parts.next().unwrap_or("").to_string();
            let rename_to = parts.next().map(|s| s.to_string());
            // For renames, key by `newname` so the numstat lookup matches
            // the `oldname => newname` parse below.
            let key = rename_to.clone().unwrap_or_else(|| path_name.clone());
            status_map.insert(
                key,
                (status, Some(path_name).filter(|_| rename_to.is_some())),
            );
            if let Some(to) = rename_to {
                // Keep an extra entry under `oldname` so a `D`/`R` row that
                // appeared earlier in the stream isn't shadowed by a stale
                // status, defensive, but cheap.
                let _ = to;
            }
        } else {
            // Numstat row: `<add>\t<del>\t<path>` (path may be
            // `oldname => newname` for renames; binary files have `-` for
            // both counts, which `parse::<usize>` will fail on → 0/0).
            let mut parts = line.split('\t');
            let add: usize = parts.next().unwrap_or("").parse().unwrap_or(0);
            let del: usize = parts.next().unwrap_or("").parse().unwrap_or(0);
            let path_field = parts.next().unwrap_or("").to_string();
            let key = if let Some((_, new)) = path_field.split_once(" => ") {
                new.trim_matches(|c| c == '{' || c == '}').to_string()
            } else {
                path_field
            };
            stats_map.insert(key, (add, del));
        }
    }

    let mut changes = Vec::new();
    for (key, (status, rename_from)) in status_map {
        let (additions, deletions) = stats_map.get(&key).copied().unwrap_or((0, 0));
        match status {
            'A' => changes.push(FileChange::Add {
                path: key,
                additions,
            }),
            'M' => changes.push(FileChange::Modify {
                path: key,
                additions,
                deletions,
            }),
            'D' => changes.push(FileChange::Delete {
                path: key,
                deletions,
            }),
            'R' => {
                let from = rename_from.unwrap_or_else(|| key.clone());
                changes.push(FileChange::Move {
                    from,
                    to: key,
                    additions,
                    deletions,
                });
            }
            _ => {}
        }
    }

    changes
}

pub fn get_initial_commit_additions(path: &Path, commit_hash: &CommitHash) -> Vec<FileChange> {
    let mut cmd = Command::new("git")
        .arg("diff")
        .arg("--numstat")
        .arg("4b825dc642cb6eb9a060e54bf8d69288fbee4904")
        .arg(&commit_hash.0)
        .current_dir(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let stdout = cmd.stdout.take().expect("failed to open stdout");
    let reader = BufReader::new(stdout);

    let mut changes = Vec::new();

    for line in reader.lines() {
        let line = line.unwrap();
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            let additions = parts[0].parse().unwrap_or(0);
            let path = parts[2].to_string();
            changes.push(FileChange::Add { path, additions });
        }
    }

    cmd.wait().unwrap();

    changes
}
