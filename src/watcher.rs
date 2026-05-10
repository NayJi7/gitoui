//! Filesystem watcher for .git/ that triggers auto-refresh of the current view
//! when external git operations modify the repository state.
//!
//! The watcher runs in a dedicated thread, debounces bursts of filesystem events
//! (a single `git commit` writes to many paths in quick succession), and
//! collapses each burst into a single `AppEvent::FilesystemChanged`.
//!
//! The handler in `app.rs` decides whether to actually refresh — it skips
//! refresh during input/dialog states to avoid disrupting the user.

use std::path::Path;
use std::time::Duration;

use notify_debouncer_mini::{
    new_debouncer,
    notify::{RecommendedWatcher, RecursiveMode},
    DebounceEventResult, Debouncer,
};

use crate::event::{AppEvent, Sender};

/// Debounce window: long enough to absorb a burst (a `git commit` touches
/// HEAD, refs/heads/<branch>, index, logs/, … in quick succession), short
/// enough that the UI feels live.
const DEBOUNCE_MS: u64 = 500;

/// Path components inside `.git/` we don't care about. Filtering them at the
/// debouncer level reduces wakeups but isn't strictly necessary — the
/// downstream handler is idempotent.
fn is_relevant_event(path: &Path) -> bool {
    let s = path.to_string_lossy();
    // .git/objects/ — every commit creates blob/tree/commit objects, useless noise
    if s.contains("/objects/") || s.ends_with("/objects") {
        return false;
    }
    // .git/logs/ — reflog updates fire on every operation, redundant with refs
    if s.contains("/logs/") || s.ends_with("/logs") {
        return false;
    }
    // .lock files appear and disappear during git operations, ignore them
    if s.ends_with(".lock") {
        return false;
    }
    // COMMIT_EDITMSG is just the user's commit message buffer
    if s.ends_with("COMMIT_EDITMSG") {
        return false;
    }
    true
}

/// Spawn the watcher. Returns the debouncer handle: keep it alive (drop it
/// to stop watching). Returns `None` if the path can't be watched (e.g.
/// .git/ doesn't exist yet).
pub fn start(
    git_dir: &Path,
    sender: Sender,
) -> Option<Debouncer<RecommendedWatcher>> {
    if !git_dir.exists() {
        return None;
    }

    let mut debouncer = new_debouncer(
        Duration::from_millis(DEBOUNCE_MS),
        move |res: DebounceEventResult| {
            if let Ok(events) = res {
                if events.iter().any(|e| is_relevant_event(&e.path)) {
                    // try_send so a full channel doesn't block the watcher thread
                    sender.try_send(AppEvent::FilesystemChanged);
                }
            }
        },
    )
    .ok()?;

    debouncer
        .watcher()
        .watch(git_dir, RecursiveMode::Recursive)
        .ok()?;

    Some(debouncer)
}
