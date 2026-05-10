//! Filesystem watcher for .git/ that triggers auto-refresh of the current view
//! when external git operations modify the repository state.
//!
//! Defense in depth, in this order:
//!   1. notify-debouncer-mini absorbs OS event bursts in a 1.5 s window
//!   2. A whitelist filter keeps only events on files we actually care about
//!      (HEAD, index, refs/*, MERGE_HEAD/CHERRY_PICK_HEAD/REBASE_HEAD,
//!      packed-refs) — everything else (objects/, logs/, hooks/, .lock,
//!      COMMIT_EDITMSG, info/, …) is ignored
//!   3. A content fingerprint is computed before each event is sent; if the
//!      repo state matches the previous snapshot the event is dropped. This
//!      eliminates the "filesystem reports change but nothing actually
//!      changed" case (atime/metadata noise, atomic-rename refresh, etc.)
//!
//! The handler in `app.rs` adds a final throttle so two refreshes can't fire
//! within a couple seconds of each other.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use notify_debouncer_mini::{
    new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
    DebounceEventResult, Debouncer,
};

use crate::event::{AppEvent, Sender};

/// How long to wait after a burst of events before sending a single
/// FilesystemChanged. A `git commit` writes to several files within a few
/// dozen milliseconds, so we want a window comfortably larger than that, but
/// short enough that a manual fetch in another shell still feels live.
const DEBOUNCE_MS: u64 = 1500;

/// Whitelist: only events touching these paths matter. The check is by suffix
/// match on the filename or by parent-directory containment for `refs/`.
fn is_relevant_event(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if matches!(
        name,
        "HEAD"
            | "MERGE_HEAD"
            | "CHERRY_PICK_HEAD"
            | "REBASE_HEAD"
            | "FETCH_HEAD"
            | "ORIG_HEAD"
            | "packed-refs"
            | "index"
    ) {
        return true;
    }
    // Anything inside refs/ (heads, tags, remotes, stash) is fair game.
    // We walk parents looking for a "refs" component.
    let mut p = path.parent();
    while let Some(parent) = p {
        if parent.file_name().and_then(|n| n.to_str()) == Some("refs") {
            return true;
        }
        p = parent.parent();
    }
    false
}

/// Snapshot of the repo state pieces the UI cares about.
/// Two equal snapshots ⇒ nothing meaningful has changed ⇒ skip refresh.
#[derive(Debug, Default, PartialEq, Eq)]
struct GitFingerprint {
    head: Option<Vec<u8>>,
    packed_refs: Option<Vec<u8>>,
    /// (relative path under refs/, file content)
    refs: BTreeMap<PathBuf, Vec<u8>>,
    /// In-progress operation markers (existence + content)
    merge_head: Option<Vec<u8>>,
    cherry_pick_head: Option<Vec<u8>>,
    rebase_head: Option<Vec<u8>>,
    /// Index file mtime (cheap proxy — reading the binary index would be
    /// wasteful, and any staging change updates the mtime)
    index_mtime: Option<SystemTime>,
}

impl GitFingerprint {
    fn capture(git_dir: &Path) -> Self {
        let mut fp = GitFingerprint::default();
        fp.head = fs::read(git_dir.join("HEAD")).ok();
        fp.packed_refs = fs::read(git_dir.join("packed-refs")).ok();
        fp.merge_head = fs::read(git_dir.join("MERGE_HEAD")).ok();
        fp.cherry_pick_head = fs::read(git_dir.join("CHERRY_PICK_HEAD")).ok();
        fp.rebase_head = fs::read(git_dir.join("REBASE_HEAD")).ok();
        fp.index_mtime = fs::metadata(git_dir.join("index"))
            .and_then(|m| m.modified())
            .ok();

        // Walk refs/ recursively. Tiny in practice (one file per branch/tag).
        let refs_root = git_dir.join("refs");
        let mut stack = vec![refs_root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = entry.metadata() else { continue };
                if meta.is_dir() {
                    stack.push(path);
                } else if let Ok(content) = fs::read(&path) {
                    if let Ok(rel) = path.strip_prefix(&refs_root) {
                        fp.refs.insert(rel.to_path_buf(), content);
                    }
                }
            }
        }
        fp
    }
}

/// Spawn the watcher. Returns the debouncer handle: keep it alive (drop it
/// to stop watching). Returns `None` if the path can't be watched (e.g.
/// `.git/` doesn't exist yet).
pub fn start(
    git_dir: &Path,
    sender: Sender,
) -> Option<Debouncer<RecommendedWatcher>> {
    if !git_dir.exists() {
        return None;
    }

    let last_fingerprint = Arc::new(Mutex::new(GitFingerprint::capture(git_dir)));
    let git_dir_owned = git_dir.to_path_buf();

    let mut debouncer = new_debouncer(
        Duration::from_millis(DEBOUNCE_MS),
        move |res: DebounceEventResult| {
            let Ok(events) = res else { return };
            // Whitelist filter first — cheap.
            if !events.iter().any(|e| is_relevant_event(&e.path)) {
                return;
            }
            // Content compare next — eliminates touch / no-op writes.
            let now = GitFingerprint::capture(&git_dir_owned);
            let mut last = last_fingerprint.lock().unwrap();
            if *last == now {
                return;
            }
            *last = now;
            sender.try_send(AppEvent::FilesystemChanged);
        },
    )
    .ok()?;

    debouncer
        .watcher()
        .watch(git_dir, RecursiveMode::Recursive)
        .ok()?;

    Some(debouncer)
}
