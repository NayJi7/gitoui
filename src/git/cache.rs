//! On-disk commit-history cache (streamable v4).
//!
//! Walks `git log` once per session (in the background) and persists
//! the full commit history to disk as a sequence of independently
//! bincode-encoded `CommitSummary` records, prefixed by a single
//! header describing the schema version + the HEAD + reachable-count
//! at write time. The streaming format lets readers consume one
//! summary at a time without ever materialising the whole
//! `Vec<CommitSummary>` in memory.
//!
//! v4 cuts the per-record payload from full `Commit` (with body +
//! committer trio) down to `CommitSummary` (hash + author + subject +
//! parents only). The body and committer fields are unused outside
//! the Detail view, which now fetches them on demand via
//! `Repository::fetch_full_commit`. On large repos this cuts the
//! cache footprint and load time roughly 5x.
//!
//! Cache file layout:
//!
//! ```text
//!   [CacheHeader]                                      bincode-encoded; magic + version + meta
//!   [CommitSummary] [CommitSummary] ... [CommitSummary] sequence; one bincode record per commit
//! ```
//!
//! Reader uses `bincode::deserialize_from(&mut reader)` in a loop; EOF
//! returns an error variant which we map to "stream done". Writer
//! uses `bincode::serialize_into(&mut writer, &summary)` per record
//! and flushes at the end before atomic rename.
//!
//! Old v1/v2/v3 caches fail the magic + version check on load and are
//! silently deleted so the bg thread re-walks and re-saves in the v4
//! format on next run.
//!
//! Graph topology is NOT cached. The user wants the cache to cover
//! `git log` only; calc_graph re-runs each session against the loaded
//! commit list.

use std::{
    fs,
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use super::{CommitSummary, SortCommit};

/// Magic bytes at the top of every v3 cache file. Lets the reader
/// reject anything that isn't ours (v1/v2 caches, truncated files,
/// random garbage from disk corruption) before we even try to decode
/// the header.
const CACHE_MAGIC: [u8; 4] = *b"GTOI";

/// Schema version. v4 = streamable: header + sequence of independent
/// `CommitSummary` records (was `Commit` in v3). Bump when the
/// on-disk format structurally changes (NOT when `CommitSummary`
/// itself gains a serde-optional field).
const CACHE_VERSION: u32 = 4;

/// Header at the top of every cache file. Bincode-encoded as a single
/// record so the reader can land on the first commit record by
/// `deserialize_from`'ing this struct, then looping on commits.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheHeader {
    /// `CACHE_MAGIC`. Reader rejects any value other than this so we
    /// can't accidentally try to decode a v1/v2 payload as a v3
    /// header (their first 4 bytes are bincode's u32-LE
    /// length-prefix on the wrapping `CachePayload` struct - not the
    /// ASCII `"GTOI"` we expect here).
    pub magic: [u8; 4],
    /// `CACHE_VERSION`. Bumped on any structural change.
    pub version: u32,
    /// HEAD hash the cache was sampled at. We only trust the cache
    /// on load if the live HEAD still matches.
    pub head_hash: String,
    /// Total reachable commit count (`git rev-list --count
    /// --branches --remotes --tags HEAD`) at cache time. Used to
    /// detect drift when HEAD itself hasn't moved (fetch, gc,
    /// force-push). 0 means "unknown" and triggers a defensive
    /// re-walk on next load.
    pub reachable_count: u64,
    /// 0 = chronological, 1 = topological. Recorded so readers can
    /// verify the cached order matches the runtime sort preference,
    /// surgical-extend writes the same order as the existing cache.
    pub sort_order: u8,
}

impl CacheHeader {
    fn new(head_hash: String, reachable_count: u64, sort: SortCommit) -> Self {
        Self {
            magic: CACHE_MAGIC,
            version: CACHE_VERSION,
            head_hash,
            reachable_count,
            sort_order: match sort {
                SortCommit::Chronological => 0,
                SortCommit::Topological => 1,
            },
        }
    }

    fn is_valid(&self) -> bool {
        self.magic == CACHE_MAGIC && self.version == CACHE_VERSION
    }
}

/// Streamable reader. Opens the cache file, reads the header into
/// memory, and exposes `next_commit()` which decodes ONE commit at a
/// time from the underlying file - no Vec<Commit> ever materialises
/// in process memory at the reader layer.
pub struct StreamReader {
    reader: BufReader<fs::File>,
    header: CacheHeader,
}

impl StreamReader {
    /// Open the cache file at `path` and decode its header. Returns
    /// `Err` if the file is missing, the header doesn't parse, or the
    /// magic / version don't match - caller treats those as "no
    /// usable cache" and falls back to a fresh walk.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = fs::File::open(path)?;
        let mut reader = BufReader::new(file);
        let header: CacheHeader = bincode::deserialize_from(&mut reader).map_err(|e| {
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
    ///   - `Some(Err(_))` on a partial / corrupt record (caller can
    ///     treat as EOF and continue with what it got),
    ///   - `None` on clean EOF (no more bytes).
    pub fn next_commit(&mut self) -> Option<io::Result<CommitSummary>> {
        // Peek one byte to distinguish "clean EOF" from "decode error
        // on a real attempt". bincode's `deserialize_from` returns an
        // Io error wrapped in a bincode::ErrorKind::Io on EOF, which
        // we'd otherwise have to string-match - cheaper to peek.
        let mut probe = [0u8; 1];
        match self.reader.read(&mut probe) {
            Ok(0) => return None, // clean EOF
            Ok(_) => {}
            Err(e) => return Some(Err(e)),
        }
        // Rewind the probe byte and let bincode consume the full
        // record. BufReader retains the byte in its internal buffer,
        // so the seek-back is cheap (no syscall).
        if let Err(e) = self.reader.seek_relative(-1) {
            return Some(Err(e));
        }
        match bincode::deserialize_from::<_, CommitSummary>(&mut self.reader) {
            Ok(c) => Some(Ok(c)),
            Err(e) => Some(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("commit decode: {e}"),
            ))),
        }
    }
}

/// Streamable writer. Opens a temp file, writes the header, then
/// accepts one `Commit` at a time via `write_commit`. `finish()`
/// flushes + atomically renames into place.
pub struct StreamWriter {
    writer: BufWriter<fs::File>,
    final_path: PathBuf,
    tmp_path: PathBuf,
    written: u64,
}

impl StreamWriter {
    /// Create a new streaming writer that will land at `path` on
    /// `finish()`. Writes the header immediately so the file is
    /// well-formed (header + zero commits) the moment the writer is
    /// dropped without a `finish()`.
    pub fn create(path: &Path, header: CacheHeader) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("bin.tmp");
        let file = fs::File::create(&tmp)?;
        let mut writer = BufWriter::new(file);
        bincode::serialize_into(&mut writer, &header).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("cache header encode: {e}"),
            )
        })?;
        Ok(Self {
            writer,
            final_path: path.to_path_buf(),
            tmp_path: tmp,
            written: 0,
        })
    }

    /// Encode one commit-summary record into the stream. Cheap (single
    /// bincode call into a buffered writer); the underlying file
    /// write is amortised across `BufWriter`'s 8 KB buffer.
    pub fn write_commit(&mut self, c: &CommitSummary) -> io::Result<()> {
        bincode::serialize_into(&mut self.writer, c).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("commit encode: {e}"))
        })?;
        self.written += 1;
        Ok(())
    }

    pub fn written(&self) -> u64 {
        self.written
    }

    /// Flush, fsync, and atomically rename the temp file into place.
    /// Returns the final path on success. If anything fails the temp
    /// file is left on disk for the caller to clean up - we don't
    /// silently delete it because that would mask the original error.
    pub fn finish(mut self) -> io::Result<PathBuf> {
        self.writer.flush()?;
        let file = self.writer.into_inner().map_err(|e| e.into_error())?;
        file.sync_all().ok();
        drop(file);
        fs::rename(&self.tmp_path, &self.final_path)?;
        Ok(self.final_path)
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
    let canonical =
        common_dir.unwrap_or_else(|| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()));
    let mut hasher = rustc_hash::FxHasher::default();
    canonical.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub fn cache_path_for(repo_path: &Path) -> Option<PathBuf> {
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

/// Validate a cache file against the live repo state. Returns a
/// `StreamReader` positioned just past the header (ready for the
/// first `next_commit()` call) when:
///   - the cache file exists,
///   - the header magic + version match,
///   - the cached HEAD equals the live HEAD,
///   - the cached `sort_order` equals the runtime `sort`,
///   - the cached reachable-count equals the live reachable-count
///     (defensive against `fetch` / `gc` / force-push under unchanged HEAD).
///
/// All other cases (missing file, header reject, HEAD drift, count
/// drift, sort mismatch) delete the cache file and return None -
/// caller falls back to a fresh `git log` walk.
pub fn open_stream_for(repo_path: &Path, sort: SortCommit) -> Option<StreamReader> {
    let path = cache_path_for(repo_path)?;
    if !path.exists() {
        return None;
    }
    let live_head = current_head_hash(repo_path)?;
    let reader = match StreamReader::open(&path) {
        Ok(r) => r,
        Err(e) => {
            crate::glog_info!("cache header rejected ({}): treating as missing", e);
            let _ = fs::remove_file(&path);
            return None;
        }
    };
    if reader.header.head_hash != live_head {
        crate::glog_info!(
            "cache HEAD moved ({} -> {}): re-walking",
            reader.header.head_hash,
            live_head
        );
        let _ = fs::remove_file(&path);
        return None;
    }
    let wanted_sort: u8 = match sort {
        SortCommit::Chronological => 0,
        SortCommit::Topological => 1,
    };
    if reader.header.sort_order != wanted_sort {
        crate::glog_info!(
            "cache sort order mismatch (cached={}, wanted={}): re-walking",
            reader.header.sort_order,
            wanted_sort
        );
        let _ = fs::remove_file(&path);
        return None;
    }
    if reader.header.reachable_count == 0 {
        crate::glog_info!("cache lacks reachable-count: re-walking once to upgrade payload");
        let _ = fs::remove_file(&path);
        return None;
    }
    if let Some(live_count) = reachable_commit_count(repo_path) {
        if reader.header.reachable_count != live_count {
            crate::glog_info!(
                "cache reachable-count drifted ({} -> {}): re-walking",
                reader.header.reachable_count,
                live_count
            );
            let _ = fs::remove_file(&path);
            return None;
        }
    }
    crate::glog_info!("cache hit: streaming commits from {}", path.display());
    Some(reader)
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

/// Read the cache header without holding the file open. Used by
/// `try_extend` to inspect the cached HEAD before deciding whether a
/// surgical extend is possible. Returns None on any decode / IO
/// failure.
fn read_header(path: &Path) -> Option<CacheHeader> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let h: CacheHeader = bincode::deserialize_from(&mut reader).ok()?;
    if !h.is_valid() {
        return None;
    }
    Some(h)
}

/// Surgical refresh path: read the on-disk header (without the
/// HEAD-equal / count-equal gates), and IF live HEAD is a strict
/// descendant of the cached HEAD, walk just the delta range
/// `cached_head..HEAD`, prepend those new commits, and rewrite the
/// cache atomically.
///
/// Returns the new commits in chronological-newest-first order on
/// success; the caller appends the rest by re-opening the cache.
/// When the cache isn't extensible (missing, schema mismatch, HEAD
/// diverged via force-push / rebase, reachable count drifted by more
/// than the HEAD-only delta), returns None and the caller falls
/// back to a full re-walk.
///
/// NOTE: extending the on-disk file in place would require knowing
/// each record's byte length to prepend new records ahead of the
/// existing ones. Bincode records are variable-width so that's not
/// directly doable; we rewrite the whole file in streaming fashion
/// (header + new commits + existing commits) using `StreamWriter`,
/// reading the existing commits via `StreamReader` so peak memory
/// stays bounded.
pub fn try_extend(repo_path: &Path, sort: SortCommit) -> Option<Vec<CommitSummary>> {
    let path = cache_path_for(repo_path)?;
    if !path.exists() {
        return None;
    }
    let header = read_header(&path)?;
    let live_head = current_head_hash(repo_path)?;
    // Sort mismatch: the on-disk records are in the OLD order and
    // splicing new commits walked in the NEW order at the front would
    // produce a hybrid stream the reader can't make sense of. Refuse
    // to extend; the caller's `open_stream_for(.., sort)` will then
    // reject the same cache on the sort check and force a clean
    // fresh-walk that rewrites the cache in the wanted order.
    let wanted_sort: u8 = match sort {
        SortCommit::Chronological => 0,
        SortCommit::Topological => 1,
    };
    if header.sort_order != wanted_sort {
        return None;
    }
    if header.head_hash == live_head {
        // Caller should have hit `open_stream_for` first; nothing to do
        // here - return an empty vec to signal "no extension needed".
        return Some(Vec::new());
    }
    if !cached_head_is_ancestor(repo_path, &header.head_hash) {
        return None;
    }
    let new_commits = super::walk_commits_since(repo_path, sort, &header.head_hash)?;
    if new_commits.is_empty() {
        return None;
    }
    // Rewrite the cache: header + new commits (prepended) + existing
    // commits (streamed back from the old file). Memory peak stays
    // bounded - we never hold the existing cache in memory.
    let new_head = live_head.clone();
    let new_count = reachable_commit_count(repo_path).unwrap_or(0);
    let new_header = CacheHeader::new(new_head, new_count, sort);

    // Re-read the existing file's commits via the streamer. Open
    // separately from the writer so we don't race against our own
    // atomic rename.
    let mut old_reader = StreamReader::open(&path).ok()?;
    let mut writer = StreamWriter::create(&path, new_header).ok()?;
    for c in &new_commits {
        writer.write_commit(c).ok()?;
    }
    loop {
        match old_reader.next_commit() {
            Some(Ok(c)) => {
                if writer.write_commit(&c).is_err() {
                    return None;
                }
            }
            Some(Err(e)) => {
                crate::glog_warn!("cache extend: stop reading old cache at error: {}", e);
                break;
            }
            None => break,
        }
    }
    drop(old_reader);
    writer.finish().ok()?;
    crate::glog_info!(
        "cache surgical extend: +{} commits prepended",
        new_commits.len()
    );
    Some(new_commits)
}

/// Persist the full commit-summary history to disk by streaming
/// `summaries` one record at a time through a `StreamWriter`. Safe
/// to call from a background thread - all I/O is best-effort and
/// silently no-ops on failure (the cache is purely an optimisation).
///
/// This wraps `StreamWriter`: callers that already have a
/// `Vec<CommitSummary>` pay no extra allocation. Callers that want
/// to stream their own summaries (e.g. fresh `git log` walks that
/// consume one record at a time) should use `StreamWriter::create`
/// directly to avoid building the intermediate vec.
pub fn save_for(repo_path: &Path, summaries: &[CommitSummary]) -> io::Result<()> {
    let Some(path) = cache_path_for(repo_path) else {
        return Ok(());
    };
    let Some(head) = current_head_hash(repo_path) else {
        return Ok(());
    };
    let reachable_count = reachable_commit_count(repo_path).unwrap_or(0);
    // Default to chronological in the wire format - the only existing
    // call site doesn't pass sort, and the v4 reader doesn't gate on
    // sort_order yet; it's metadata for future use.
    let header = CacheHeader::new(head, reachable_count, SortCommit::Chronological);
    let mut writer = StreamWriter::create(&path, header)?;
    for c in summaries {
        writer.write_commit(c)?;
    }
    let written = writer.written();
    writer.finish()?;
    crate::glog_info!("cache write OK: {} commits to {}", written, path.display());
    Ok(())
}

/// Drop the cache file for this repo. Use to recover after a corrupt
/// load (caller already deleted) or from a CLI `--clear-cache` flag.
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

    /// End-to-end: write 5 commits through StreamWriter, read them
    /// back through StreamReader, compare. Locks in the wire format
    /// so future bincode-encoding tweaks can't silently break v3
    /// readers in the wild.
    #[test]
    fn stream_writer_reader_roundtrip() {
        let dir = std::env::temp_dir().join(format!("gitoui-cache-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("commits.bin");
        let _ = std::fs::remove_file(&path);

        let header = CacheHeader::new("deadbeef".into(), 5, SortCommit::Chronological);
        let mut writer = StreamWriter::create(&path, header).unwrap();
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

        let mut reader = StreamReader::open(&path).unwrap();
        assert_eq!(reader.header().magic, *b"GTOI");
        assert_eq!(reader.header().version, CACHE_VERSION);
        assert_eq!(reader.header().head_hash, "deadbeef");
        assert_eq!(reader.header().reachable_count, 5);
        assert_eq!(reader.header().sort_order, 0);

        let mut readback: Vec<CommitSummary> = Vec::new();
        while let Some(item) = reader.next_commit() {
            readback.push(item.unwrap());
        }
        assert_eq!(readback.len(), originals.len());
        for (a, b) in originals.iter().zip(readback.iter()) {
            assert_eq!(a.commit_hash, b.commit_hash);
            assert_eq!(a.commit_message, b.commit_message);
            assert_eq!(a.parent_commit_hashes, b.parent_commit_hashes);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A 0-commit cache (header-only) must round-trip cleanly: open
    /// succeeds, next_commit returns None on the first call. Guards
    /// against a regression where the EOF probe in StreamReader
    /// mis-detected the end of an empty body as a corrupt record.
    #[test]
    fn stream_reader_handles_empty_body() {
        let dir = std::env::temp_dir().join(format!("gitoui-cache-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("commits.bin");
        let _ = std::fs::remove_file(&path);

        let header = CacheHeader::new("h".into(), 0, SortCommit::Chronological);
        let writer = StreamWriter::create(&path, header).unwrap();
        writer.finish().unwrap();

        let mut reader = StreamReader::open(&path).unwrap();
        assert!(reader.next_commit().is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
