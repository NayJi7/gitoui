//! Runs the real binary in a pseudo-terminal and reads the screen back as text.
//! On Windows this goes through ConPTY, so it shows what gitoui draws there.
//! The screen is always printed (`cargo test --test smoke -- --nocapture`).

use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

#[test]
fn the_commit_list_is_drawn_and_q_quits() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    for i in 0..3 {
        std::fs::write(repo.join(format!("f{i}")), i.to_string()).unwrap();
        git(&repo, &["add", "."]);
        git(
            &repo,
            &["commit", "-q", "-m", &format!("commit number {i}")],
        );
    }
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("pty");
    let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_gitoui"));
    cmd.args(["--graph-renderer", "ascii", "--no-update-check"]);
    cmd.cwd(&repo);
    // Nothing of the real user's config or cache is read or written.
    for var in ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"] {
        cmd.env(var, &home);
    }
    cmd.env("XDG_CONFIG_HOME", home.join("config"));
    cmd.env("XDG_CACHE_HOME", home.join("cache"));
    cmd.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(cmd).expect("spawn gitoui");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let writer = Arc::new(Mutex::new(pair.master.take_writer().expect("writer")));
    let parser = Arc::new(Mutex::new(vt100::Parser::new(
        std::num::NonZeroU16::new(30).unwrap(),
        std::num::NonZeroU16::new(100).unwrap(),
        0,
    )));
    {
        let (parser, writer) = (parser.clone(), writer.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                // ConPTY asks where the cursor is and waits for the answer.
                if buf[..n].windows(4).any(|w| w == b"\x1b[6n") {
                    let _ = writer.lock().unwrap().write_all(b"\x1b[1;1R");
                }
                parser.lock().unwrap().process(&buf[..n]);
            }
        });
    }
    let screen = || parser.lock().unwrap().screen().contents();

    let deadline = Instant::now() + Duration::from_secs(40);
    while !screen().contains("commit number 2") && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    let shown = screen();
    println!("---- screen ----");
    for (i, row) in shown.lines().enumerate() {
        println!("{i:02}|{row}");
    }
    println!("---- end ({} rows) ----", shown.lines().count());
    assert!(shown.contains("commit number 2"), "the list was not drawn");
    assert!(
        shown.contains("commit number 0"),
        "the oldest commit is missing"
    );

    writer.lock().unwrap().write_all(b"q").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(child.try_wait().unwrap().is_some(), "q did not quit");
}
