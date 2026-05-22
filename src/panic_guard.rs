//! Panic isolation helpers for background threads.
//!
//! `std::thread::spawn` lets a panic kill the worker silently:
//! the main thread keeps running but the background job is gone
//! and the user has no way to know. Pages of gitoui rely on
//! background threads (GitHub API fetches, avatar downloads,
//! config token polling, ...) - a single panic in any of them
//! used to leave the UI permanently stale with no diagnostic.
//!
//! `spawn_protected` wraps `std::thread::spawn` with
//! `std::panic::catch_unwind` and logs the panic message to the
//! daily log file (`~/.config/gitoui/logs/`). Use this everywhere
//! we spawn a thread that touches the network, IO, or third-party
//! libraries that might panic on bad input.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::thread::{self, JoinHandle};

/// Spawn a background thread whose body is wrapped in
/// `catch_unwind`. A panic in the body is caught, formatted into
/// a `WARN` log line tagged with the supplied `label` and the
/// payload (`String` / `&str`), and discarded - the JoinHandle
/// returns `Ok(())` either way so callers don't have to special-case.
///
/// Use a meaningful `label` (e.g. `"avatar-fetch"`, `"pr-detail-load"`,
/// `"github-token-poll"`) so the log line points to the subsystem
/// that crashed without needing a backtrace.
pub fn spawn_protected<F>(label: &'static str, body: F) -> JoinHandle<()>
where
    F: FnOnce() + Send + 'static,
{
    thread::spawn(move || {
        let result = catch_unwind(AssertUnwindSafe(body));
        if let Err(payload) = result {
            let msg = if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else if let Some(s) = payload.downcast_ref::<&str>() {
                s.to_string()
            } else {
                "<non-string panic payload>".to_string()
            };
            crate::glog_warn!(
                "background thread '{}' panicked: {} (silently caught; UI may be stale)",
                label,
                msg
            );
        }
    })
}
