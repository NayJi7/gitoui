//! A persistent session of the user's shell inside a pseudo-terminal, shown
//! as an integrated terminal.
//!
//! A reader thread feeds everything the shell prints to a terminal emulator
//! screen. The only thing it takes out of the stream is the invisible marker
//! the injected scripts print at each prompt, which tells gitoui the shell's
//! current directory.

use std::io::{self, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, ChildKiller, CommandBuilder, MasterPty, PtySize};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::launch::{
    plain_spec, session_spec, shell_kind, EnvVars, Integration, LaunchSpec, ShellKind,
};
use super::markers::{Marker, MarkerScanner, Segment};
use super::platform;
use super::screen::{render_screen, TermScreen};

pub(crate) type Waker = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Running,
    /// The shell ended with this exit code (`-1` when unknown).
    Exited(i32),
}

/// User scrolling, as moves applied at render time on top of the position
/// the emulator already keeps (it holds the view still while output arrives).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Scroll {
    pending: isize,
    to_top: bool,
    to_bottom: bool,
}

impl Scroll {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Towards older lines.
    pub(crate) fn up(&mut self, n: usize) {
        self.pending = self.pending.saturating_add(n as isize);
    }

    pub(crate) fn down(&mut self, n: usize) {
        self.pending = self.pending.saturating_sub(n as isize);
    }

    pub(crate) fn top(&mut self) {
        self.to_top = true;
        self.to_bottom = false;
    }

    pub(crate) fn bottom(&mut self) {
        self.to_bottom = true;
        self.to_top = false;
    }

    pub(crate) fn apply(&mut self, screen: &mut vt100::Screen) {
        if self.to_bottom {
            screen.set_scrollback(0);
        } else if self.to_top {
            screen.set_scrollback(usize::MAX);
        } else if self.pending != 0 {
            let target = (screen.scrollback() as isize)
                .saturating_add(self.pending)
                .max(0);
            screen.set_scrollback(target as usize);
        }
        *self = Self::default();
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Shared {
    screen: TermScreen,
    rows: u16,
    cols: u16,
    status: Status,
    /// Directory reported by the shell at its last prompt.
    cwd: Option<PathBuf>,
    started_at: Instant,
    lived: Option<Duration>,
}

impl Shared {
    fn new(rows: u16, cols: u16, now: Instant) -> Self {
        Self {
            screen: TermScreen::new(rows, cols),
            rows,
            cols,
            status: Status::Running,
            cwd: None,
            started_at: now,
            lived: None,
        }
    }

    fn apply(&mut self, segment: Segment, tx: &mpsc::Sender<Vec<u8>>) {
        match segment {
            Segment::Output(bytes) => {
                self.screen.process(&bytes);
                let replies = self.screen.take_replies();
                if !replies.is_empty() {
                    let _ = tx.send(replies);
                }
            }
            Segment::Marker(Marker::Cwd(dir)) => self.cwd = Some(PathBuf::from(dir)),
        }
    }

    fn ended(&mut self, code: i32, now: Instant) {
        self.lived = Some(now.saturating_duration_since(self.started_at));
        self.status = Status::Exited(code);
    }
}

struct PtyHandle {
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    writer_tx: mpsc::Sender<Vec<u8>>,
    pid: Option<u32>,
    /// Set by the reader thread once the child was waited for: its pid may
    /// then be reused by an unrelated process and must not be signalled.
    reaped: Arc<AtomicBool>,
}

impl PtyHandle {
    /// Whether a signal was sent.
    fn kill(&mut self) -> bool {
        if self.reaped.load(Ordering::SeqCst) {
            return false;
        }
        let _ = self.killer.kill();
        if let Some(pid) = self.pid {
            platform::kill_process_group(pid as i32);
        }
        true
    }
}

pub(crate) struct ShellSession {
    shared: Arc<Mutex<Shared>>,
    pty: PtyHandle,
    /// Kept alive for the whole session: the shell reads its scripts from it.
    _integration: Option<Integration>,
}

#[derive(Clone)]
pub struct SessionView {
    shared: Arc<Mutex<Shared>>,
}

impl std::fmt::Debug for SessionView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionView")
            .field("status", &self.status())
            .finish()
    }
}

fn to_io<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::other(e.to_string())
}

type Spawned = (
    PtyHandle,
    Box<dyn Read + Send>,
    Box<dyn Child + Send + Sync>,
);

fn open_pty(rows: u16, cols: u16, spec: &LaunchSpec) -> io::Result<Spawned> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(to_io)?;
    let mut cmd = CommandBuilder::new(&spec.program);
    cmd.args(&spec.args);
    cmd.env_clear();
    for (k, v) in &spec.env {
        cmd.env(k, v);
    }
    cmd.cwd(&spec.cwd);
    let child = pair.slave.spawn_command(cmd).map_err(to_io)?;
    drop(pair.slave);
    let reader = pair.master.try_clone_reader().map_err(to_io)?;
    let mut writer = pair.master.take_writer().map_err(to_io)?;
    let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>();
    std::thread::Builder::new()
        .name("gitoui-shell-writer".into())
        .spawn(move || {
            while let Ok(bytes) = writer_rx.recv() {
                if writer
                    .write_all(&bytes)
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        })?;
    let handle = PtyHandle {
        killer: child.clone_killer(),
        pid: child.process_id(),
        master: pair.master,
        writer_tx,
        reaped: Arc::new(AtomicBool::new(false)),
    };
    Ok((handle, reader, child))
}

struct ReaderCtx {
    shared: Arc<Mutex<Shared>>,
    writer_tx: mpsc::Sender<Vec<u8>>,
    waker: Waker,
    reaped: Arc<AtomicBool>,
}

/// Reads the pty until it closes, then waits for the process. A panic while
/// handling output (the emulator parses arbitrary bytes) ends the session
/// instead of leaving the terminal frozen.
fn run_reader(
    ctx: ReaderCtx,
    mut reader: impl Read,
    mut scanner: Option<MarkerScanner>,
    wait: impl FnOnce() -> i32,
) {
    let looped = catch_unwind(AssertUnwindSafe(|| {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let segments = match scanner.as_mut() {
                Some(s) => s.feed(&buf[..n]),
                None => vec![Segment::Output(buf[..n].to_vec())],
            };
            {
                let mut st = lock(&ctx.shared);
                for segment in segments {
                    st.apply(segment, &ctx.writer_tx);
                }
            }
            (ctx.waker)();
        }
    }));
    let code = if looped.is_ok() {
        let code = wait();
        ctx.reaped.store(true, Ordering::SeqCst);
        code
    } else {
        crate::glog_warn!("terminal reader panicked; ending the session");
        -1
    };
    lock(&ctx.shared).ended(code, Instant::now());
    (ctx.waker)();
}

impl ShellSession {
    pub(crate) fn start(
        shell: &Path,
        env: EnvVars,
        cwd: &Path,
        rows: u16,
        cols: u16,
        waker: Waker,
    ) -> io::Result<Self> {
        let kind = shell_kind(shell);
        let (spec, integration) = if kind == ShellKind::Posix {
            (plain_spec(shell, env, cwd), None)
        } else {
            let integration = Integration::create()?;
            let spec = session_spec(shell, kind, &integration, env, cwd, platform::login_shell());
            (spec, Some(integration))
        };
        let shared = Arc::new(Mutex::new(Shared::new(rows, cols, Instant::now())));
        let (pty, reader, child) = open_pty(rows, cols, &spec)?;
        let ctx = ReaderCtx {
            shared: shared.clone(),
            writer_tx: pty.writer_tx.clone(),
            waker,
            reaped: pty.reaped.clone(),
        };
        let scanner = integration.as_ref().map(|i| MarkerScanner::new(i.nonce()));
        let mut child = child;
        std::thread::Builder::new()
            .name("gitoui-shell-reader".into())
            .spawn(move || {
                run_reader(ctx, reader, scanner, move || {
                    child.wait().map(|s| s.exit_code() as i32).unwrap_or(-1)
                });
            })?;
        Ok(Self {
            shared,
            pty,
            _integration: integration,
        })
    }

    pub(crate) fn view(&self) -> SessionView {
        SessionView {
            shared: self.shared.clone(),
        }
    }

    /// Never blocks: bytes go through the writer thread.
    pub(crate) fn send_input(&self, bytes: &[u8]) {
        let _ = self.pty.writer_tx.send(bytes.to_vec());
    }

    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        {
            let st = lock(&self.shared);
            if st.rows == rows && st.cols == cols {
                return;
            }
        }
        let _ = self.pty.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        let mut st = lock(&self.shared);
        st.rows = rows;
        st.cols = cols;
        st.screen.resize(rows, cols);
    }

    pub(crate) fn is_alive(&self) -> bool {
        lock(&self.shared).status == Status::Running
    }

    #[cfg(test)]
    pub(crate) fn kill(&mut self) {
        self.pty.kill();
    }

    #[cfg(test)]
    pub(crate) fn shell_pid(&self) -> Option<u32> {
        self.pty.pid
    }
}

impl Drop for ShellSession {
    fn drop(&mut self) {
        self.pty.kill();
    }
}

impl SessionView {
    pub(crate) fn status(&self) -> Status {
        lock(&self.shared).status.clone()
    }

    /// The shell's directory at its last prompt, when it reports one.
    pub(crate) fn cwd(&self) -> Option<PathBuf> {
        lock(&self.shared).cwd.clone()
    }

    /// How long the shell lived, once it ended.
    pub(crate) fn lived(&self) -> Option<Duration> {
        lock(&self.shared).lived
    }

    pub(crate) fn application_cursor(&self) -> bool {
        lock(&self.shared).screen.screen().application_cursor()
    }

    /// The mouse mode the running program asked for, if any.
    pub(crate) fn mouse_protocol(
        &self,
    ) -> Option<(vt100::MouseProtocolMode, vt100::MouseProtocolEncoding)> {
        let st = lock(&self.shared);
        let screen = st.screen.screen();
        match screen.mouse_protocol_mode() {
            vt100::MouseProtocolMode::None => None,
            mode => Some((mode, screen.mouse_protocol_encoding())),
        }
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer, scroll: &mut Scroll) {
        let mut guard = lock(&self.shared);
        let st = &mut *guard;
        scroll.apply(st.screen.screen_mut());
        let show_cursor = st.status == Status::Running && st.screen.screen().scrollback() == 0;
        render_screen(st.screen.screen(), area, buf, show_cursor);
    }

    #[cfg(test)]
    fn scrollback_len(&self) -> usize {
        lock(&self.shared).screen.screen().scrollback()
    }
}

#[cfg(test)]
mod tests;
