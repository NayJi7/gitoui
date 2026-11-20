//! Owner of the shell session for the whole life of gitoui.
//!
//! The app is rebuilt on every refresh, so the session cannot live in it.
//! `run()` creates one host before the refresh loop and lends it to each app.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::event::{AppEvent, Sender};

use super::launch::EnvVars;
use super::platform;
use super::session::ShellSession;

pub(crate) struct ShellHost {
    session: Option<ShellSession>,
    sender: Sender,
    /// Set while a `ShellUpdated` event is queued, so a flood of output
    /// produces one wake-up at a time.
    wake_pending: Arc<AtomicBool>,
}

impl std::fmt::Debug for ShellHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellHost")
            .field("running", &self.session.is_some())
            .finish()
    }
}

impl ShellHost {
    pub(crate) fn new(sender: Sender) -> Self {
        Self {
            session: None,
            sender,
            wake_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    fn waker(&self) -> super::session::Waker {
        let sender = self.sender.clone();
        let pending = self.wake_pending.clone();
        Arc::new(move || {
            if !pending.swap(true, Ordering::AcqRel) {
                sender.send(AppEvent::ShellUpdated);
            }
        })
    }

    /// The session, started if there is none or if the shell ended. When the
    /// user's shell cannot be started, `/bin/sh` is used instead.
    pub(crate) fn ensure_started(&mut self, rows: u16, cols: u16) -> io::Result<&mut ShellSession> {
        if !self.session.as_ref().is_some_and(ShellSession::is_alive) {
            let env: EnvVars = std::env::vars_os().collect();
            // The shell starts in `C:\dir`, not in the verbatim `\\?\C:\dir`
            // form: PowerShell would print its prompt as
            // `Microsoft.PowerShell.Core\FileSystem::\\?\C:\dir>`.
            let cwd = crate::paths::simplify(
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            );
            let shell = platform::user_shell(&env);
            let session = ShellSession::start(&shell, env.clone(), &cwd, rows, cols, self.waker())
                .or_else(|_| {
                    ShellSession::start(
                        Path::new(platform::FALLBACK_SHELL),
                        env,
                        &cwd,
                        rows,
                        cols,
                        self.waker(),
                    )
                })?;
            self.session = Some(session);
        }
        self.session
            .as_mut()
            .ok_or_else(|| io::Error::other("no shell session"))
    }

    pub(crate) fn session(&mut self) -> Option<&mut ShellSession> {
        self.session.as_mut()
    }

    /// Lets the next burst of output wake the app again.
    pub(crate) fn acknowledge_wake(&self) {
        self.wake_pending.store(false, Ordering::Release);
    }
}
