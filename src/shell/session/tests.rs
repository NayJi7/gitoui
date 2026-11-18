//! Integration tests that start real shells (bash, zsh, fish when installed).
//! Set `GITOUI_TEST_FISH` to a fish binary to cover fish.

use super::*;
use crate::shell::launch::{EnvVars, ShellKind};
use crate::shell::screen::SCROLLBACK_LINES;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const BASHRC: &str = "\
echo BANNER
HISTFILE=$HOME/.bash_history_test; shopt -s histappend
PS1='PROMPT> '
alias hi='echo hi-alias'
";

const ZSHRC: &str = "\
echo BANNER
HISTFILE=$HOME/.zsh_history_test; SAVEHIST=100; HISTSIZE=100
setopt INC_APPEND_HISTORY
PS1='PROMPT> '
alias hi='echo hi-alias'
exec {__p10k_fd}>&1 1>/dev/null
__fake_p10k_restore() { exec 1>&$__p10k_fd; precmd_functions=(${precmd_functions:#__fake_p10k_restore}) }
precmd_functions=(__fake_p10k_restore $precmd_functions)
";

const FISHRC: &str = "\
echo BANNER
function fish_prompt; echo -n 'PROMPT> '; end
alias hi \"echo hi-alias\"
";

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

fn shells() -> Vec<(ShellKind, PathBuf)> {
    let mut out = Vec::new();
    for (kind, name) in [(ShellKind::Bash, "bash"), (ShellKind::Zsh, "zsh")] {
        match find_in_path(name) {
            Some(p) => out.push((kind, p)),
            None => eprintln!("skipping {name}: not installed"),
        }
    }
    let fish = std::env::var_os("GITOUI_TEST_FISH")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| find_in_path("fish"));
    match fish {
        Some(p) => out.push((ShellKind::Fish, p)),
        None => eprintln!("skipping fish: not installed (set GITOUI_TEST_FISH)"),
    }
    out
}

fn history_file(kind: ShellKind) -> &'static str {
    match kind {
        ShellKind::Bash => ".bash_history_test",
        ShellKind::Zsh => ".zsh_history_test",
        ShellKind::Fish => ".local/share/fish/fish_history",
        ShellKind::Posix => "",
    }
}

fn test_home(kind: ShellKind) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let p = home.path();
    match kind {
        ShellKind::Bash => {
            std::fs::write(p.join(".bashrc"), BASHRC).unwrap();
            // Login shells (macOS) read this one, as real setups do.
            std::fs::write(
                p.join(".bash_profile"),
                "[[ -r ~/.bashrc ]] && source ~/.bashrc\n",
            )
            .unwrap();
        }
        ShellKind::Zsh => std::fs::write(p.join(".zshrc"), ZSHRC).unwrap(),
        ShellKind::Fish => {
            std::fs::create_dir_all(p.join(".config/fish")).unwrap();
            std::fs::write(p.join(".config/fish/config.fish"), FISHRC).unwrap();
        }
        ShellKind::Posix => {}
    }
    home
}

fn base_env(home: &Path) -> EnvVars {
    vec![
        ("HOME".into(), home.as_os_str().to_owned()),
        ("PATH".into(), std::env::var_os("PATH").unwrap_or_default()),
        ("USER".into(), "tester".into()),
        ("LANG".into(), "C.UTF-8".into()),
    ]
}

fn start(shell: &Path, home: &Path, repo: &Path) -> ShellSession {
    ShellSession::start(shell, base_env(home), repo, 24, 80, Arc::new(|| {})).unwrap()
}

fn screen_text(view: &SessionView) -> String {
    let area = Rect::new(0, 0, 80, 24);
    let mut buf = Buffer::empty(area);
    let mut scroll = Scroll::new();
    view.render(area, &mut buf, &mut scroll);
    let mut rows: Vec<String> = (0..24)
        .map(|y| {
            (0..80)
                .map(|x| buf.cell((x, y)).unwrap().symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    while rows.last().is_some_and(|r| r.is_empty()) {
        rows.pop();
    }
    rows.join("\n")
}

fn wait_for_text(view: &SessionView, needle: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let text = screen_text(view);
        if text.contains(needle) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {needle:?}; screen:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Types a line and waits until `expect` shows up on a line of its own,
/// meaning the shell printed it (not just echoed the typed command).
fn type_line(s: &ShellSession, line: &str, expect: &str) -> String {
    s.send_input(format!("{line}\r").as_bytes());
    let view = s.view();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let text = screen_text(&view);
        if text.lines().any(|l| l.trim() == expect) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for a line {expect:?}; screen:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_cwd(view: &SessionView, want: &Path) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if view
            .cwd()
            .is_some_and(|c| c.canonicalize().ok() == want.canonicalize().ok())
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "cwd never became {want:?}, got {:?}",
            view.cwd()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn scroll_moves_accumulate_and_apply_to_the_screen() {
    let mut s = crate::shell::screen::TermScreen::new(3, 10);
    let lines: String = (0..30).map(|i| format!("{i}\r\n")).collect();
    s.process(lines.as_bytes());
    let mut scroll = Scroll::new();
    scroll.up(5);
    scroll.up(3);
    scroll.down(1);
    scroll.apply(s.screen_mut());
    assert_eq!(s.screen().scrollback(), 7);
    scroll.apply(s.screen_mut());
    assert_eq!(
        s.screen().scrollback(),
        7,
        "applying twice must not move again"
    );
    scroll.top();
    scroll.apply(s.screen_mut());
    assert_eq!(s.screen().scrollback(), 28);
    scroll.bottom();
    scroll.apply(s.screen_mut());
    assert_eq!(s.screen().scrollback(), 0);
    scroll.down(4);
    scroll.apply(s.screen_mut());
    assert_eq!(
        s.screen().scrollback(),
        0,
        "cannot go below the live screen"
    );
}

#[test]
fn the_shell_is_a_real_terminal_with_its_banner_and_prompt() {
    for (kind, shell) in shells() {
        let (home, repo) = (test_home(kind), tempfile::tempdir().unwrap());
        let s = start(&shell, home.path(), repo.path());
        let text = wait_for_text(&s.view(), "PROMPT>");
        assert!(text.contains("BANNER"), "{kind:?}: {text}");
        type_line(&s, "echo hello", "hello");
        type_line(&s, "hi", "hi-alias");
    }
}

#[test]
fn several_commands_share_one_session() {
    for (kind, shell) in shells() {
        let (home, repo) = (test_home(kind), tempfile::tempdir().unwrap());
        let s = start(&shell, home.path(), repo.path());
        let export = if kind == ShellKind::Fish {
            "set -gx FOO bar"
        } else {
            "export FOO=bar"
        };
        wait_for_text(&s.view(), "PROMPT>");
        s.send_input(format!("{export}\r").as_bytes());
        type_line(&s, "echo $FOO", "bar");
    }
}

#[test]
fn the_shell_starts_in_the_given_directory() {
    for (kind, shell) in shells() {
        let (home, repo) = (test_home(kind), tempfile::tempdir().unwrap());
        let s = start(&shell, home.path(), repo.path());
        wait_for_cwd(&s.view(), repo.path());
    }
}

#[test]
fn cd_is_reported_to_gitoui() {
    for (kind, shell) in shells() {
        let (home, repo) = (test_home(kind), tempfile::tempdir().unwrap());
        let other = tempfile::tempdir().unwrap();
        let s = start(&shell, home.path(), repo.path());
        let view = s.view();
        wait_for_cwd(&view, repo.path());
        s.send_input(format!("cd '{}'\r", other.path().display()).as_bytes());
        wait_for_cwd(&view, other.path());
    }
}

#[test]
fn ctrl_c_interrupts_and_the_prompt_returns() {
    for (kind, shell) in shells() {
        let (home, repo) = (test_home(kind), tempfile::tempdir().unwrap());
        let s = start(&shell, home.path(), repo.path());
        wait_for_text(&s.view(), "PROMPT>");
        s.send_input(b"sleep 30\r");
        std::thread::sleep(Duration::from_millis(400));
        s.send_input(b"\x03");
        type_line(&s, "echo back", "back");
    }
}

#[test]
fn history_is_written_like_in_any_terminal() {
    for (kind, shell) in shells() {
        let (home, repo) = (test_home(kind), tempfile::tempdir().unwrap());
        let mut s = start(&shell, home.path(), repo.path());
        let view = s.view();
        wait_for_text(&view, "PROMPT>");
        type_line(&s, "echo remembered-command", "remembered-command");
        s.send_input(b"exit\r");
        let deadline = Instant::now() + Duration::from_secs(10);
        while view.status() == Status::Running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        s.kill();
        let file = home.path().join(history_file(kind));
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        assert!(
            text.contains("echo remembered-command"),
            "{kind:?}: {text:?}"
        );
    }
}

#[test]
fn exit_ends_the_session() {
    for (kind, shell) in shells() {
        let (home, repo) = (test_home(kind), tempfile::tempdir().unwrap());
        let s = start(&shell, home.path(), repo.path());
        let view = s.view();
        wait_for_text(&view, "PROMPT>");
        s.send_input(b"exit 3\r");
        let deadline = Instant::now() + Duration::from_secs(10);
        while view.status() == Status::Running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(view.status(), Status::Exited(3), "{kind:?}");
        assert!(!s.is_alive(), "{kind:?}");
    }
}

#[test]
fn resize_reaches_the_shell() {
    for (kind, shell) in shells() {
        let (home, repo) = (test_home(kind), tempfile::tempdir().unwrap());
        let mut s = start(&shell, home.path(), repo.path());
        wait_for_text(&s.view(), "PROMPT>");
        s.resize(30, 100);
        type_line(&s, "stty size", "30 100");
    }
}

#[test]
fn dropping_the_session_kills_the_shell() {
    for (kind, shell) in shells() {
        let (home, repo) = (test_home(kind), tempfile::tempdir().unwrap());
        let s = start(&shell, home.path(), repo.path());
        wait_for_text(&s.view(), "PROMPT>");
        let pid = s.shell_pid().unwrap();
        assert!(platform::process_exists(pid));
        drop(s);
        let deadline = Instant::now() + Duration::from_secs(2);
        while platform::process_exists(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !platform::process_exists(pid),
            "{kind:?}: shell {pid} survived"
        );
    }
}

#[test]
fn huge_output_stays_bounded() {
    for (kind, shell) in shells() {
        let (home, repo) = (test_home(kind), tempfile::tempdir().unwrap());
        let s = start(&shell, home.path(), repo.path());
        wait_for_text(&s.view(), "PROMPT>");
        // The quotes keep the typed command from matching the awaited text.
        s.send_input(b"seq 1 200000; echo fin''ished-seq\r");
        let view = s.view();
        wait_for_text(&view, "finished-seq");
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let mut scroll = Scroll::new();
        scroll.top();
        view.render(area, &mut buf, &mut scroll);
        let kept = view.scrollback_len();
        assert!(
            (SCROLLBACK_LINES - 100..=SCROLLBACK_LINES).contains(&kept),
            "{kind:?}: {kept}"
        );
    }
}

#[test]
fn killing_a_reaped_shell_does_nothing() {
    for (kind, shell) in shells() {
        let (home, repo) = (test_home(kind), tempfile::tempdir().unwrap());
        let mut s = start(&shell, home.path(), repo.path());
        wait_for_text(&s.view(), "PROMPT>");
        s.send_input(b"exit\r");
        let reaped = s.pty.reaped.clone();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !reaped.load(std::sync::atomic::Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(reaped.load(std::sync::atomic::Ordering::SeqCst), "{kind:?}");
        assert!(!s.pty.kill(), "{kind:?}: signalled a reaped pid");
    }
}

#[test]
fn zsh_history_file_never_points_into_the_integration_dir() {
    let Some((_, zsh)) = shells().into_iter().find(|(k, _)| *k == ShellKind::Zsh) else {
        return;
    };
    let (home, repo) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    // What a system /etc/zshrc may do while ZDOTDIR still points at our dir.
    std::fs::write(
        home.path().join(".zshenv"),
        "HISTFILE=$__gitoui_inject_dir/.zsh_history\n",
    )
    .unwrap();
    std::fs::write(home.path().join(".zshrc"), "PS1='PROMPT> '\n").unwrap();
    let s = start(&zsh, home.path(), repo.path());
    wait_for_text(&s.view(), "PROMPT>");
    let text = type_line(
        &s,
        "print -r -- HF=$HISTFILE",
        &format!("HF={}/.zsh_history", home.path().display()),
    );
    assert!(!text.contains("gitoui-shell"), "{text}");
}

#[test]
fn an_unknown_shell_runs_without_directory_tracking() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let s = start(Path::new("/bin/sh"), home.path(), repo.path());
    assert_eq!(shell_kind(Path::new("/bin/sh")), ShellKind::Posix);
    s.send_input(b"echo plain-shell\r");
    wait_for_text(&s.view(), "plain-shell");
    assert_eq!(s.view().cwd(), None);
}

struct PanickingReader;

impl std::io::Read for PanickingReader {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        panic!("boom");
    }
}

#[test]
fn a_panicking_reader_ends_the_session() {
    let shared = Arc::new(Mutex::new(Shared::new(24, 80, Instant::now())));
    let woken = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = woken.clone();
    let ctx = ReaderCtx {
        shared: shared.clone(),
        writer_tx: std::sync::mpsc::channel().0,
        waker: Arc::new(move || flag.store(true, std::sync::atomic::Ordering::SeqCst)),
        reaped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    run_reader(ctx, PanickingReader, None, || 0);
    let st = lock(&shared);
    assert_eq!(st.status, Status::Exited(-1));
    assert!(woken.load(std::sync::atomic::Ordering::SeqCst));
}
