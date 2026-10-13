//! On-disk commit-history cache (streamable layout, version 6).
//!
//! The background thread walks `git log` once and persists the full
//! commit history as a sequence of independently bincode-encoded
//! `CommitSummary` records, prefixed by a header. The streaming format
//! lets readers consume one summary at a time without materialising the
//! whole `Vec<CommitSummary>` in memory.
//!
//! Cache file layout:
//!
//! ```text
//!   [CacheHeader]                                      magic + version + walk key
//!   [CommitSummary] [CommitSummary] ... [CommitSummary] one record per commit, git log order
//! ```
//!
//! # When the cache is trusted
//!
//! The cache is the exact output of one `git log` walk, in its order:
//! the list shows the first slice straight from git and the rest from
//! the cache, and the graph is computed over the cache, so any
//! difference in content OR order misaligns rows. It is therefore
//! reused only when the walk would produce the same output, i.e. when
//! its inputs are unchanged: HEAD, every branch, remote-tracking branch
//! and tag tip, the stash parents the walk includes, and the sort
//! order. `CacheKey` fingerprints those inputs; any difference means a
//! full re-walk, which runs in the background and rewrites the cache.
//!
//! Version 6 replaced the "same HEAD + same reachable count" check and
//! the surgical extend that prepended `old_head..HEAD`. Both were
//! approximations: the extend missed commits of other refs that moved
//! (fetch), kept commits of deleted refs, put prepended commits out of
//! date order, then stamped the live count into the header so the drift
//! could never be detected again.
//!
//! Older caches fail the magic + version check on load and are deleted.
//!
//! Graph topology is NOT cached; calc_graph re-runs each session.

use std::{
    fs,
    hash::{Hash, Hasher},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use bincode::Options;

use super::{CommitHash, CommitSummary, SortCommit};

/// Magic bytes at the top of every cache file, so the reader rejects
/// anything that isn't ours before decoding the header.
const CACHE_MAGIC: [u8; 4] = *b"GTOI";

/// Schema version. Bump on any structural change of the header or of
/// `CommitSummary`'s encoding, or to purge caches written by a buggy
/// writer. v6: `CacheKey` fingerprint instead of the reachable count.
const CACHE_VERSION: u32 = 6;

/// Largest single record (header or commit) the codec accepts. A commit
/// summary is a few hundred bytes; the bound only exists so a damaged
/// length prefix fails to decode instead of asking the allocator for
/// gigabytes, which aborts the whole process.
const MAX_RECORD_BYTES: u64 = 4 << 20;

/// Wire codec for every record: little-endian, fixed-width integers
/// (the legacy `bincode::serialize` encoding), bounded per record.
fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .allow_trailing_bytes()
        .with_limit(MAX_RECORD_BYTES)
}

fn sort_byte(sort: SortCommit) -> u8 {
    match sort {
        SortCommit::Chronological => 0,
        SortCommit::Topological => 1,
    }
}

/// Everything the history walk depends on, condensed. Two walks with
/// equal keys produce the same commits in the same order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    /// HEAD at key time. Part of the fingerprint too; kept separately
    /// for log lines.
    pub head_hash: String,
    /// Hash of HEAD, every `refs/heads`, `refs/remotes` and `refs/tags`
    /// tip, the stash parents and the sort order.
    pub fingerprint: u64,
    pub sort_order: u8,
}

impl CacheKey {
    /// Compute the key for the walk `stream_commits_one_by_one` runs:
    /// `--branches --remotes --tags <stash parents> HEAD`. Returns None
    /// when there is no HEAD yet or git fails; the caller then neither
    /// reads nor writes a cache.
    ///
    /// Compute it BEFORE starting a walk: if a ref moves while the walk
    /// runs, the stored key is the older one and the next launch simply
    /// re-walks, instead of trusting a key newer than the content.
    pub fn compute(
        repo_path: &Path,
        sort: SortCommit,
        stash_parents: &[CommitHash],
    ) -> Option<Self> {
        let head_hash = current_head_hash(repo_path)?;
        let out = std::process::Command::new("git")
            .args([
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                "refs/heads",
                "refs/remotes",
                "refs/tags",
            ])
            .current_dir(repo_path)
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(Self::from_parts(
            &head_hash,
            &out.stdout,
            stash_parents,
            sort,
        ))
    }

    fn from_parts(
        head_hash: &str,
        refs_listing: &[u8],
        stash_parents: &[CommitHash],
        sort: SortCommit,
    ) -> Self {
        let mut hasher = rustc_hash::FxHasher::default();
        head_hash.hash(&mut hasher);
        refs_listing.hash(&mut hasher);
        for parent in stash_parents {
            parent.as_str().hash(&mut hasher);
        }
        let sort_order = sort_byte(sort);
        sort_order.hash(&mut hasher);
        Self {
            head_hash: head_hash.to_string(),
            fingerprint: hasher.finish(),
            sort_order,
        }
    }

    fn matches(&self, header: &CacheHeader) -> bool {
        header.fingerprint == self.fingerprint
            && header.sort_order == self.sort_order
            && header.head_hash == self.head_hash
    }
}

/// Header at the top of every cache file, decoded on its own so the
/// reader lands on the first commit record right after it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheHeader {
    /// `CACHE_MAGIC`.
    pub magic: [u8; 4],
    /// `CACHE_VERSION`.
    pub version: u32,
    /// HEAD the walk ran from (`CacheKey::head_hash`).
    pub head_hash: String,
    /// `CacheKey::fingerprint` of the walk that wrote this file.
    pub fingerprint: u64,
    /// 0 = chronological, 1 = topological.
    pub sort_order: u8,
}

impl CacheHeader {
    /// Header for a cache holding the walk described by `key`. Always go
    /// through here so `magic` and `version` are stamped consistently.
    pub fn new(key: &CacheKey) -> Self {
        Self {
            magic: CACHE_MAGIC,
            version: CACHE_VERSION,
            head_hash: key.head_hash.clone(),
            fingerprint: key.fingerprint,
            sort_order: key.sort_order,
        }
    }

    fn is_valid(&self) -> bool {
        self.magic == CACHE_MAGIC && self.version == CACHE_VERSION
    }
}

/// Streamable reader: decodes the header on open, then ONE commit per
/// `next_commit()` call.
pub struct StreamReader {
    reader: BufReader<fs::File>,
    header: CacheHeader,
}

impl StreamReader {
    /// Open the cache file at `path` and decode its header. Returns
    /// `Err` if the file is missing, the header doesn't decode, or the
    /// magic / version don't match.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = fs::File::open(path)?;
        let mut reader = BufReader::new(file);
        let header: CacheHeader = codec().deserialize_from(&mut reader).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("cache header decode: {e}"),
            )
        })?;
        if !header.is_valid() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "cache magic/version mismatch (got magic={:?} version={})",
                    header.magic, header.version
                ),
            ));
        }
        Ok(Self { reader, header })
    }

    pub fn header(&self) -> &CacheHeader {
        &self.header
    }

    /// Decode the next commit-summary record. Returns:
    ///   - `Some(Ok(summary))` on a successful decode,
    ///   - `Some(Err(_))` on a partial / corrupt record,
    ///   - `None` on clean EOF (no more bytes).
    pub fn next_commit(&mut self) -> Option<io::Result<CommitSummary>> {
        // Peek one byte to tell a clean EOF from a decode error.
        let mut probe = [0u8; 1];
        match self.reader.read(&mut probe) {
            Ok(0) => return None,
            Ok(_) => {}
            Err(e) => return Some(Err(e)),
        }
        // BufReader keeps the byte in its buffer: rewinding is free.
        if let Err(e) = self.reader.seek_relative(-1) {
            return Some(Err(e));
        }
        match codec().deserialize_from::<_, CommitSummary>(&mut self.reader) {
            Ok(c) => Some(Ok(c)),
            Err(e) => Some(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("commit decode: {e}"),
            ))),
        }
    }
}

/// Distinguishes temp files of writers living in the same process.
static WRITER_SEQ: AtomicU64 = AtomicU64::new(0);

/// Streamable writer. Writes into a temp file private to this writer,
/// then `finish()` flushes and atomically renames it over the cache.
/// Concurrent writers (two gitoui windows, a bg respawn while the
/// previous walk still writes) therefore never share a file: each
/// produces a complete cache and the last rename wins. A writer dropped
/// without `finish()` deletes its temp file.
pub struct StreamWriter {
    writer: Option<BufWriter<fs::File>>,
    final_path: PathBuf,
    tmp_path: PathBuf,
    written: u64,
}

impl StreamWriter {
    /// Create a writer that will land at `path` on `finish()`. The
    /// header is written immediately.
    pub fn create(path: &Path, header: CacheHeader) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "commits.bin".into());
        let tmp = path.with_file_name(format!(
            "{name}.{}-{}.tmp",
            std::process::id(),
            WRITER_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let file = fs::File::create(&tmp)?;
        let mut this = Self {
            writer: Some(BufWriter::new(file)),
            final_path: path.to_path_buf(),
            tmp_path: tmp,
            written: 0,
        };
        let w = this.writer.as_mut().expect("writer is set until finish");
        codec().serialize_into(w, &header).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("cache header encode: {e}"),
            )
        })?;
        Ok(this)
    }

    /// Encode one commit-summary record into the stream.
    pub fn write_commit(&mut self, c: &CommitSummary) -> io::Result<()> {
        let w = self.writer.as_mut().expect("writer is set until finish");
        codec().serialize_into(w, c).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("commit encode: {e}"))
        })?;
        self.written += 1;
        Ok(())
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    /// Flush, fsync, and atomically rename the temp file into place.
    pub fn finish(mut self) -> io::Result<PathBuf> {
        let mut writer = self.writer.take().expect("writer is set until finish");
        writer.flush()?;
        let file = writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all().ok();
        drop(file);
        fs::rename(&self.tmp_path, &self.final_path)?;
        Ok(self.final_path.clone())
    }
}

impl Drop for StreamWriter {
    fn drop(&mut self) {
        // Unfinished or failed write: never leave the temp file behind.
        // After a successful finish() the rename already moved it away.
        let _ = fs::remove_file(&self.tmp_path);
    }
}

fn cache_root() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("gitoui"))
}

/// 64-bit FxHash that identifies a repo for cache purposes.
///
/// Uses `git rev-parse --git-common-dir` so that multiple worktrees of
/// the same repository share a single cache file (their walks cover the
/// same refs; a worktree with another HEAD just re-walks).
///
/// Falls back to the canonicalized repo path if the `git` call fails
/// (covers non-repo dirs called with `--clear-cache` etc.).
fn repo_key(path: &Path) -> String {
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
    let canonical =
        common_dir.unwrap_or_else(|| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
    let mut hasher = rustc_hash::FxHasher::default();
    canonical.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub fn cache_path_for(repo_path: &Path) -> Option<PathBuf> {
    cache_root().map(|root| root.join(repo_key(repo_path)).join("commits.bin"))
}

/// `git rev-parse HEAD`. None on bare repos, unborn HEAD, non-git dirs.
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

/// Open the repo's cache for streaming if it holds exactly the walk
/// described by `key`. A stale cache (any ref moved, sort changed) is
/// left in place: the caller re-walks and overwrites it atomically, and
/// another worktree sharing the file may still be able to use it. Only
/// an undecodable header gets the file deleted.
pub fn open_stream_for(repo_path: &Path, key: &CacheKey) -> Option<StreamReader> {
    open_stream_at(&cache_path_for(repo_path)?, key)
}

fn open_stream_at(path: &Path, key: &CacheKey) -> Option<StreamReader> {
    if !path.exists() {
        return None;
    }
    let reader = match StreamReader::open(path) {
        Ok(r) => r,
        Err(e) => {
            crate::glog_info!("cache header rejected ({}): treating as missing", e);
            let _ = fs::remove_file(path);
            return None;
        }
    };
    if !key.matches(reader.header()) {
        crate::glog_info!(
            "cache stale (HEAD {} -> {}, or other refs / sort changed): re-walking",
            reader.header().head_hash,
            key.head_hash
        );
        return None;
    }
    crate::glog_info!("cache hit: streaming commits from {}", path.display());
    Some(reader)
}

/// Drop the cache file for this repo: `--clear-cache`, or after a record
/// failed to decode mid-stream.
pub fn invalidate(repo_path: &Path) {
    if let Some(path) = cache_path_for(repo_path) {
        let _ = fs::remove_file(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{CommitHash, CommitType};
    use chrono::DateTime;

    fn make_commit(hash: &str, parents: &[&str]) -> CommitSummary {
        CommitSummary {
            commit_hash: CommitHash::from(hash),
            author_name: "test".into(),
            author_email: "t@e".into(),
            author_date: DateTime::parse_from_rfc3339("2024-01-01T00:00:00+00:00").unwrap(),
            commit_message: format!("msg {hash}"),
            parent_commit_hashes: parents.iter().map(|p| CommitHash::from(*p)).collect(),
            commit_type: CommitType::Commit,
        }
    }

    fn key(head: &str, fingerprint: u64) -> CacheKey {
        CacheKey {
            head_hash: head.into(),
            fingerprint,
            sort_order: 0,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gitoui-cache-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn read_all(path: &Path) -> io::Result<Vec<CommitSummary>> {
        let mut reader = StreamReader::open(path)?;
        let mut out = Vec::new();
        while let Some(item) = reader.next_commit() {
            out.push(item?);
        }
        Ok(out)
    }

    /// Write 5 commits through StreamWriter, read them back, compare.
    #[test]
    fn stream_writer_reader_roundtrip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("commits.bin");

        let mut writer =
            StreamWriter::create(&path, CacheHeader::new(&key("deadbeef", 42))).unwrap();
        let originals = vec![
            make_commit("a", &[]),
            make_commit("b", &["a"]),
            make_commit("c", &["b"]),
            make_commit("d", &["c"]),
            make_commit("e", &["d"]),
        ];
        for c in &originals {
            writer.write_commit(c).unwrap();
        }
        assert_eq!(writer.written(), 5);
        writer.finish().unwrap();

        let reader = StreamReader::open(&path).unwrap();
        assert_eq!(reader.header().magic, *b"GTOI");
        assert_eq!(reader.header().version, CACHE_VERSION);
        assert_eq!(reader.header().head_hash, "deadbeef");
        assert_eq!(reader.header().fingerprint, 42);
        assert_eq!(reader.header().sort_order, 0);

        let readback = read_all(&path).unwrap();
        assert_eq!(readback.len(), originals.len());
        for (a, b) in originals.iter().zip(readback.iter()) {
            assert_eq!(a.commit_hash, b.commit_hash);
            assert_eq!(a.commit_message, b.commit_message);
            assert_eq!(a.parent_commit_hashes, b.parent_commit_hashes);
        }
        // Nothing but the cache itself is left in the directory.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A 0-commit cache (header-only) round-trips: next_commit is None.
    #[test]
    fn stream_reader_handles_empty_body() {
        let dir = temp_dir("empty");
        let path = dir.join("commits.bin");
        StreamWriter::create(&path, CacheHeader::new(&key("h", 1)))
            .unwrap()
            .finish()
            .unwrap();
        let mut reader = StreamReader::open(&path).unwrap();
        assert!(reader.next_commit().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Two writers aimed at the same cache file (two gitoui windows on
    /// one repo, or a bg respawn while the previous walk still writes)
    /// must each produce a complete file: whichever lands last wins, and
    /// the result decodes end to end.
    #[test]
    fn concurrent_writers_never_leave_a_corrupt_cache() {
        let dir = temp_dir("race");
        let path = dir.join("commits.bin");

        let first: Vec<_> = (0..200)
            .map(|i| make_commit(&format!("a{i:03}"), &[]))
            .collect();
        let second: Vec<_> = (0..300)
            .map(|i| make_commit(&format!("b{i:03}"), &[]))
            .collect();
        let mut wa = StreamWriter::create(&path, CacheHeader::new(&key("a", 1))).unwrap();
        let mut wb = StreamWriter::create(&path, CacheHeader::new(&key("b", 2))).unwrap();
        for (i, b) in second.iter().enumerate() {
            if let Some(a) = first.get(i) {
                wa.write_commit(a).unwrap();
            }
            wb.write_commit(b).unwrap();
        }
        let ra = wa.finish();
        let rb = wb.finish();
        assert!(
            ra.is_ok() && rb.is_ok(),
            "both writers must finish: {ra:?} {rb:?}"
        );

        let got = read_all(&path).expect("cache must decode end to end");
        assert_eq!(got.len(), 300, "the last writer's 300 commits, intact");
        assert!(got.iter().all(|c| c.commit_hash.as_str().starts_with('b')));
        let _ = fs::remove_dir_all(&dir);
    }

    /// A writer dropped before `finish()` (write error, aborted walk)
    /// leaves neither a cache nor a temp file behind.
    #[test]
    fn abandoned_writer_cleans_up_its_temp_file() {
        let dir = temp_dir("abandon");
        let path = dir.join("commits.bin");
        let mut w = StreamWriter::create(&path, CacheHeader::new(&key("h", 1))).unwrap();
        w.write_commit(&make_commit("a", &[])).unwrap();
        drop(w);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A damaged record whose length prefix claims gigabytes must fail
    /// to decode cleanly instead of trying to allocate that much (which
    /// aborts the process with no panic hook and no log line).
    #[test]
    fn corrupt_length_prefix_is_rejected_without_huge_allocation() {
        let dir = temp_dir("bad");
        let path = dir.join("commits.bin");
        StreamWriter::create(&path, CacheHeader::new(&key("h", 1)))
            .unwrap()
            .finish()
            .unwrap();
        // A record starts with the commit hash String: u64 length + bytes.
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(&(u64::MAX / 4).to_le_bytes());
        bytes.extend_from_slice(b"garbage");
        fs::write(&path, bytes).unwrap();

        let mut reader = StreamReader::open(&path).unwrap();
        let item = reader.next_commit().expect("a record is present");
        assert!(item.is_err(), "garbage must not decode");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_stream_accepts_only_the_exact_walk_key() {
        let dir = temp_dir("open");
        let path = dir.join("commits.bin");
        let k = key("h1", 7);
        let mut w = StreamWriter::create(&path, CacheHeader::new(&k)).unwrap();
        w.write_commit(&make_commit("a", &[])).unwrap();
        w.finish().unwrap();

        assert!(open_stream_at(&path, &k).is_some());
        assert!(open_stream_at(&path, &key("h1", 8)).is_none(), "other refs");
        assert!(open_stream_at(&path, &key("h2", 7)).is_none(), "other HEAD");
        let topo = CacheKey {
            sort_order: 1,
            ..k.clone()
        };
        assert!(open_stream_at(&path, &topo).is_none(), "other sort order");
        // A stale cache is kept for the re-walk to overwrite.
        assert!(path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@e")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// main with two commits, plus `origin/feature` one commit ahead.
    fn sample_repo(tag: &str) -> (PathBuf, PathBuf) {
        let root = temp_dir(tag);
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "A"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "B"]);
        let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);
        let f = git(&repo, &["commit-tree", &tree, "-p", "HEAD", "-m", "F"]);
        git(&repo, &["update-ref", "refs/remotes/origin/feature", &f]);
        (root, repo)
    }

    fn key_of(repo: &Path) -> CacheKey {
        CacheKey::compute(repo, SortCommit::Chronological, &[]).expect("key")
    }

    /// The walk covers every ref, so the key must change whenever any of
    /// them moves, appears or disappears, not only when HEAD does.
    /// Each case below is a way the old "HEAD + reachable count" check
    /// (and the surgical extend) served a cache that no longer matched
    /// `git log`.
    #[test]
    fn cache_key_tracks_every_input_of_the_walk() {
        let (root, repo) = sample_repo("key");
        let base = key_of(&repo);
        assert_eq!(key_of(&repo), base, "stable while nothing changes");

        // A fetch advances a remote branch, HEAD untouched.
        let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);
        let f2 = git(
            &repo,
            &["commit-tree", &tree, "-p", "origin/feature", "-m", "F2"],
        );
        git(&repo, &["update-ref", "refs/remotes/origin/feature", &f2]);
        let after_fetch = key_of(&repo);
        assert_ne!(after_fetch, base, "remote branch moved");

        // A remote branch is pruned.
        git(&repo, &["update-ref", "-d", "refs/remotes/origin/feature"]);
        let after_prune = key_of(&repo);
        assert_ne!(after_prune, after_fetch, "remote branch deleted");

        // A tag on an existing commit (same commit set, same count).
        git(&repo, &["tag", "v1", "HEAD~1"]);
        let after_tag = key_of(&repo);
        assert_ne!(after_tag, after_prune, "tag added");

        // HEAD moves.
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "C"]);
        let after_commit = key_of(&repo);
        assert_ne!(after_commit, after_tag, "HEAD moved");

        // Stash parents and sort order are walk inputs too.
        let head = CommitHash::from(after_commit.head_hash.as_str());
        let with_stash = CacheKey::compute(&repo, SortCommit::Chronological, &[head]).unwrap();
        assert_ne!(with_stash, after_commit, "stash parent included");
        let topo = CacheKey::compute(&repo, SortCommit::Topological, &[]).unwrap();
        assert_ne!(topo, after_commit, "sort order");

        let _ = fs::remove_dir_all(&root);
    }

    /// Same refs, same key: a fresh clone-like copy in another directory
    /// (or another worktree on the same HEAD) can reuse the cache.
    #[test]
    fn cache_key_does_not_depend_on_the_checkout_path() {
        let (root, repo) = sample_repo("key-path");
        let copy = root.join("copy");
        let status = std::process::Command::new("cp")
            .args(["-a"])
            .arg(&repo)
            .arg(&copy)
            .status()
            .unwrap();
        assert!(status.success());
        assert_eq!(key_of(&repo), key_of(&copy));
        let _ = fs::remove_dir_all(&root);
    }
}
