//! Background watcher of the CI status of the current branch.
//!
//! It feeds the little dot in the header of the commit list: orange and
//! animated while a workflow runs, then green or red for a few seconds when
//! it ends. The monitor belongs to `run()`, so it survives refreshes of the
//! app; the app only tells it what to watch.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rustc_hash::FxHashSet;

use crate::event::{AppEvent, Sender};
use crate::github::actions::{list_runs_if_changed, Listed, Run, RunConclusion};
use crate::github::RepoCoords;

/// How long the green / red dot stays after a run ended.
pub const FINISHED_VISIBLE: Duration = Duration::from_secs(5);

const IDLE_DELAY: Duration = Duration::from_secs(15);
const RUNNING_DELAY: Duration = Duration::from_secs(8);
const POKED_DELAY: Duration = Duration::from_secs(3);
const POKE_WINDOW: Duration = Duration::from_secs(30);
const ERROR_DELAY: Duration = Duration::from_secs(60);
/// Runs fetched per poll: the newest ones are enough to see what runs.
const POLL_PAGE: u32 = 10;

#[derive(Debug, Clone, PartialEq)]
pub enum CiState {
    None,
    /// The newest run that is still going.
    Running(Run),
    /// A run that was going just ended. `ok` is false when any failed.
    Finished {
        run: Run,
        ok: bool,
        until: Instant,
    },
}

/// What the runs of the branch say, given which runs were going last time.
/// Returns the new state and the ids going now.
pub fn classify(
    prev_running: &FxHashSet<u64>,
    runs: &[Run],
    now: Instant,
) -> (CiState, FxHashSet<u64>) {
    let wall_clock = chrono::Utc::now();
    let running: Vec<&Run> = runs.iter().filter(|r| r.is_live(wall_clock)).collect();
    if let Some(newest) = running.first() {
        let ids = running.iter().map(|r| r.id).collect();
        return (CiState::Running((*newest).clone()), ids);
    }
    let ended: Vec<&Run> = runs
        .iter()
        .filter(|r| prev_running.contains(&r.id))
        .collect();
    let state = if let Some(failed) = ended
        .iter()
        .find(|r| r.conclusion.is_some_and(RunConclusion::is_failure))
    {
        CiState::Finished {
            run: (*failed).clone(),
            ok: false,
            until: now + FINISHED_VISIBLE,
        }
    } else if let Some(ok) = ended
        .iter()
        .find(|r| r.conclusion == Some(RunConclusion::Success))
    {
        CiState::Finished {
            run: (*ok).clone(),
            ok: true,
            until: now + FINISHED_VISIBLE,
        }
    } else {
        // Cancelled, skipped, or no longer in the list: no flash.
        CiState::None
    };
    (state, FxHashSet::default())
}

/// A green / red flash is not cut short by the next poll: it stays until its
/// time is over, unless a run starts.
pub fn merge_state(old: &CiState, new: CiState, now: Instant) -> CiState {
    match (old, &new) {
        (CiState::Finished { until, .. }, CiState::None) if now < *until => old.clone(),
        _ => new,
    }
}

/// How long to wait before the next poll.
pub fn next_delay(state: &CiState, poked: bool) -> Duration {
    match state {
        CiState::Running(_) => RUNNING_DELAY,
        _ if poked => POKED_DELAY,
        _ => IDLE_DELAY,
    }
}

/// The rotating half circle of the animated dot.
///
/// Without a Nerd Font (the config flag, there is no way to detect glyph
/// support) it is the plain `| / - \` that every font has.
pub fn spinner_frame(tick: usize, nerd_font: bool) -> &'static str {
    const FANCY: [&str; 4] = ["◐", "◓", "◑", "◒"];
    const PLAIN: [&str; 4] = ["|", "/", "-", "\\"];
    let frames = if nerd_font { FANCY } else { PLAIN };
    frames[(tick / 3) % frames.len()]
}

#[derive(Clone)]
pub struct CiTarget {
    pub token: String,
    pub coords: RepoCoords,
    pub branch: String,
}

impl CiTarget {
    fn same_as(&self, other: &CiTarget) -> bool {
        self.token == other.token
            && self.coords.owner == other.coords.owner
            && self.coords.repo == other.coords.repo
            && self.branch == other.branch
    }
}

struct Shared {
    target: Option<CiTarget>,
    /// Bumped when the target changes: results of an older poll are dropped.
    generation: u64,
    state: CiState,
    prev_running: FxHashSet<u64>,
    /// The tag GitHub gave with the last list, and that list: asking again
    /// with the tag costs no quota when nothing changed.
    etag: Option<String>,
    last_runs: Vec<Run>,
    poke_until: Option<Instant>,
    next_poll: Instant,
    stop: bool,
}

fn lock(m: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The state, and the condition the thread sleeps on until the next poll is
/// due (woken early by a new target, a poke or the end of the app).
type Handle = Arc<(Mutex<Shared>, Condvar)>;

pub struct CiMonitor {
    shared: Handle,
}

impl std::fmt::Debug for CiMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CiMonitor").finish()
    }
}

impl CiMonitor {
    pub fn new(sender: Sender) -> Self {
        let shared: Handle = Arc::new((
            Mutex::new(Shared {
                target: None,
                generation: 0,
                state: CiState::None,
                prev_running: FxHashSet::default(),
                etag: None,
                last_runs: Vec::new(),
                poke_until: None,
                next_poll: Instant::now(),
                stop: false,
            }),
            Condvar::new(),
        ));
        let worker = shared.clone();
        let _ = std::thread::Builder::new()
            .name("gitoui-ci-monitor".into())
            .spawn(move || poll_loop(worker, sender));
        Self { shared }
    }

    /// What to watch: `None` when GitHub is unavailable or HEAD is detached.
    pub fn configure(&self, target: Option<CiTarget>) {
        let mut st = lock(&self.shared.0);
        let unchanged = match (&st.target, &target) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_as(b),
            _ => false,
        };
        if unchanged {
            return;
        }
        st.target = target;
        st.generation += 1;
        st.state = CiState::None;
        st.prev_running.clear();
        st.etag = None;
        st.last_runs.clear();
        st.next_poll = Instant::now();
        self.shared.1.notify_all();
    }

    /// Poll soon and often for a while: a push just started some runs.
    pub fn poke(&self) {
        let mut st = lock(&self.shared.0);
        let now = Instant::now();
        st.poke_until = Some(now + POKE_WINDOW);
        st.next_poll = now;
        self.shared.1.notify_all();
    }

    /// The state to show now (a finished dot expires by itself).
    pub fn state(&self, now: Instant) -> CiState {
        match lock(&self.shared.0).state.clone() {
            CiState::Finished { until, .. } if now >= until => CiState::None,
            other => other,
        }
    }
}

impl Drop for CiMonitor {
    fn drop(&mut self) {
        lock(&self.shared.0).stop = true;
        self.shared.1.notify_all();
    }
}

fn poll_loop(shared: Handle, sender: Sender) {
    let (mutex, wake) = (&shared.0, &shared.1);
    loop {
        // Sleep until a poll is due: no wake-up in between, only a new
        // target, a poke or the end of the app interrupt the wait.
        let (target, generation, etag) = {
            let mut st = lock(mutex);
            loop {
                if st.stop {
                    return;
                }
                let now = Instant::now();
                let due = match &st.target {
                    Some(t) if now >= st.next_poll => {
                        Some((t.clone(), st.generation, st.etag.clone()))
                    }
                    _ => None,
                };
                if let Some(found) = due {
                    break found;
                }
                st = if st.target.is_some() {
                    let wait = st.next_poll.saturating_duration_since(now);
                    wake.wait_timeout(st, wait)
                        .unwrap_or_else(|e| e.into_inner())
                        .0
                } else {
                    wake.wait(st).unwrap_or_else(|e| e.into_inner())
                };
            }
        };
        let result = list_runs_if_changed(
            &target.token,
            &target.coords,
            Some(&target.branch),
            POLL_PAGE,
            etag.as_deref(),
        );
        let mut st = lock(mutex);
        if st.generation != generation {
            continue;
        }
        let now = Instant::now();
        match result {
            Ok(listed) => {
                // `Unchanged` is a free answer: the list is the one we have.
                let runs = match listed {
                    Listed::Fresh(runs, tag) => {
                        st.etag = tag;
                        st.last_runs = runs.clone();
                        runs
                    }
                    Listed::Unchanged => st.last_runs.clone(),
                };
                let (state, prev) = classify(&st.prev_running, &runs, now);
                let state = merge_state(&st.state, state, now);
                let poked = st.poke_until.is_some_and(|u| now < u);
                st.next_poll = now + next_delay(&state, poked);
                let changed = state != st.state
                    || matches!(
                        (&state, &st.state),
                        (CiState::Finished { .. }, CiState::None)
                    );
                st.state = state;
                st.prev_running = prev;
                if changed {
                    sender.send(AppEvent::CiStatusUpdated);
                }
            }
            Err(_) => st.next_poll = now + ERROR_DELAY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::actions::parse_runs;

    fn runs(spec: &[(u64, &str, Option<&str>)]) -> Vec<Run> {
        let items: Vec<String> = spec
            .iter()
            .map(|(id, status, conclusion)| {
                let c = conclusion.map_or("null".to_string(), |c| format!("\"{c}\""));
                format!(r#"{{"id":{id},"name":"CI","status":"{status}","conclusion":{c}}}"#)
            })
            .collect();
        parse_runs(&format!(r#"{{"workflow_runs":[{}]}}"#, items.join(","))).unwrap()
    }

    fn ids(v: &[u64]) -> FxHashSet<u64> {
        v.iter().copied().collect()
    }

    #[test]
    fn a_run_already_finished_never_flashes() {
        let now = Instant::now();
        let (state, prev) = classify(&ids(&[]), &runs(&[(1, "completed", Some("failure"))]), now);
        assert_eq!(state, CiState::None);
        assert!(prev.is_empty());
    }

    #[test]
    fn a_running_run_is_reported_with_the_newest_first() {
        let now = Instant::now();
        let (state, prev) = classify(
            &ids(&[]),
            &runs(&[
                (3, "in_progress", None),
                (2, "queued", None),
                (1, "completed", Some("success")),
            ]),
            now,
        );
        assert!(matches!(&state, CiState::Running(r) if r.id == 3));
        assert_eq!(prev, ids(&[3, 2]));
    }

    #[test]
    fn a_run_that_ends_flashes_green_or_red() {
        let now = Instant::now();
        let (state, prev) = classify(&ids(&[5]), &runs(&[(5, "completed", Some("success"))]), now);
        assert!(
            matches!(&state, CiState::Finished { run, ok: true, until } if run.id == 5 && *until == now + FINISHED_VISIBLE)
        );
        assert!(prev.is_empty());
        let (state, _) = classify(&ids(&[5]), &runs(&[(5, "completed", Some("failure"))]), now);
        assert!(matches!(state, CiState::Finished { ok: false, .. }));
        let (state, _) = classify(
            &ids(&[5]),
            &runs(&[(5, "completed", Some("timed_out"))]),
            now,
        );
        assert!(matches!(state, CiState::Finished { ok: false, .. }));
    }

    #[test]
    fn one_failure_among_successes_is_red() {
        let now = Instant::now();
        let list = runs(&[
            (7, "completed", Some("success")),
            (6, "completed", Some("failure")),
        ]);
        let (state, _) = classify(&ids(&[6, 7]), &list, now);
        assert!(matches!(&state, CiState::Finished { run, ok: false, .. } if run.id == 6));
    }

    #[test]
    fn cancelled_or_vanished_runs_show_nothing() {
        let now = Instant::now();
        let (state, _) = classify(
            &ids(&[5]),
            &runs(&[(5, "completed", Some("cancelled"))]),
            now,
        );
        assert_eq!(state, CiState::None);
        let (state, _) = classify(&ids(&[5]), &runs(&[(9, "completed", Some("success"))]), now);
        assert_eq!(state, CiState::None);
    }

    #[test]
    fn polling_speeds_up_while_something_runs_or_was_pushed() {
        let running = classify(
            &ids(&[]),
            &runs(&[(1, "in_progress", None)]),
            Instant::now(),
        )
        .0;
        assert_eq!(next_delay(&running, false), RUNNING_DELAY);
        assert_eq!(next_delay(&CiState::None, true), POKED_DELAY);
        assert_eq!(next_delay(&CiState::None, false), IDLE_DELAY);
    }

    #[test]
    fn a_run_stuck_in_the_queue_does_not_keep_the_dot_spinning() {
        let now = Instant::now();
        let stuck = parse_runs(
            r#"{"workflow_runs":[{"id":1,"status":"queued","created_at":"2020-01-01T00:00:00Z"}]}"#,
        )
        .unwrap();
        let (state, prev) = classify(&ids(&[]), &stuck, now);
        assert_eq!(state, CiState::None);
        assert!(prev.is_empty());
    }

    #[test]
    fn a_flash_is_kept_until_it_expires() {
        let now = Instant::now();
        let finished = classify(&ids(&[5]), &runs(&[(5, "completed", Some("success"))]), now).0;
        // The next poll sees nothing running and nothing new.
        assert_eq!(
            merge_state(&finished, CiState::None, now + Duration::from_secs(3)),
            finished
        );
        // Once expired, the new state wins.
        assert_eq!(
            merge_state(
                &finished,
                CiState::None,
                now + FINISHED_VISIBLE + Duration::from_secs(1)
            ),
            CiState::None
        );
        // A run starting replaces the flash at once.
        let running = classify(&ids(&[]), &runs(&[(6, "in_progress", None)]), now).0;
        assert_eq!(merge_state(&finished, running.clone(), now), running);
    }

    #[test]
    fn the_dot_turns() {
        let frames: Vec<&str> = (0..12).step_by(3).map(|t| spinner_frame(t, true)).collect();
        assert_eq!(frames, vec!["◐", "◓", "◑", "◒"]);
        assert_eq!(spinner_frame(12, true), "◐");
        assert_eq!(spinner_frame(12, false), "|");
        assert_eq!(spinner_frame(3, false), "/");
    }
}
