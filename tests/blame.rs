//! Integration tests for `git blame` wrapping. Drives the porcelain loader
//! against real temp repos and asserts the BlameLine structure carries the
//! expected (hash, author, summary, line_no, content) tuple for known commits.

use std::fs;
use std::path::Path;
use std::process::Command;

use gitoui::git::blame::{load_blame, parse_porcelain, BlameLine};
use tempfile::TempDir;

fn init_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    run_git(dir.path(), &["init", "-b", "main"]);
    run_git(dir.path(), &["config", "user.email", "alice@example.com"]);
    run_git(dir.path(), &["config", "user.name", "Alice"]);
    run_git(dir.path(), &["config", "commit.gpgsign", "false"]);
    dir
}

fn run_git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_and_commit(repo: &Path, file: &str, content: &str, message: &str) {
    fs::write(repo.join(file), content).unwrap();
    run_git(repo, &["add", file]);
    run_git(repo, &["commit", "-m", message]);
}

fn head_hash(repo: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn single_commit_file_attributes_every_line_to_that_commit() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "line1\nline2\nline3\n", "init");
    let head = head_hash(dir.path());

    let lines = load_blame(dir.path(), "a.txt").expect("load_blame");
    assert_eq!(lines.len(), 3);
    for (i, l) in lines.iter().enumerate() {
        assert_eq!(l.hash, head, "every line should be attributed to HEAD");
        assert_eq!(l.author, "Alice");
        assert_eq!(l.line_no, (i + 1) as u32);
    }
    assert_eq!(lines[0].content, "line1");
    assert_eq!(lines[1].content, "line2");
    assert_eq!(lines[2].content, "line3");
}

#[test]
fn multiple_commits_attribute_per_block() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "alpha\nbeta\ngamma\n", "first");
    let first = head_hash(dir.path());

    // Modify only the middle line.
    fs::write(dir.path().join("a.txt"), "alpha\nBETA\ngamma\n").unwrap();
    run_git(dir.path(), &["add", "a.txt"]);
    run_git(dir.path(), &["commit", "-m", "second"]);
    let second = head_hash(dir.path());

    let lines = load_blame(dir.path(), "a.txt").expect("load_blame");
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].hash, first, "alpha is still from the first commit");
    assert_eq!(lines[1].hash, second, "BETA came from the second commit");
    assert_eq!(lines[2].hash, first, "gamma is still from the first commit");
}

#[test]
fn empty_file_yields_no_blame_lines() {
    let dir = init_repo();
    write_and_commit(dir.path(), "empty.txt", "", "init");
    let lines = load_blame(dir.path(), "empty.txt").expect("load_blame");
    assert!(lines.is_empty());
}

#[test]
fn nonexistent_file_returns_err() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "x\n", "init");
    let result = load_blame(dir.path(), "missing.txt");
    assert!(result.is_err(), "blame on nonexistent file should fail");
}

#[test]
fn author_time_is_populated() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "x\n", "init");
    let lines = load_blame(dir.path(), "a.txt").expect("load_blame");
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].author_time.is_some(),
        "author_time must be set for a normal commit"
    );
}

#[test]
fn short_hash_matches_full_hash_prefix() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "x\n", "init");
    let lines = load_blame(dir.path(), "a.txt").expect("load_blame");
    let l: &BlameLine = &lines[0];
    assert_eq!(l.short_hash.len(), 7);
    assert!(l.hash.starts_with(&l.short_hash));
}

#[test]
fn parser_handles_trailing_newline_in_porcelain_output() {
    // git appends a final newline; make sure that doesn't trip the parser.
    let sample = "\
0123456789abcdef0123456789abcdef01234567 1 1 1
author Alice
author-time 1700000000
summary one
filename a.txt
\thello

";
    let lines = parse_porcelain(sample);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].content, "hello");
}
