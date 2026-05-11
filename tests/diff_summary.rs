//! Baseline tests for `get_diff_summary` and `get_initial_commit_additions`.
//! The upcoming optimisation merges the two underlying `git diff` invocations
//! (--name-status + --numstat) into a single call; these tests freeze the
//! current output shape so the refactor can be validated end-to-end.

use std::process::Command;
use tempfile::TempDir;

use gitoui::git::{CommitHash, FileChange};

fn create_test_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    dir
}

fn commit(dir: &TempDir, msg: &str) -> CommitHash {
    Command::new("git")
        .args(["commit", "-m", msg])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let hash_output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let hash = String::from_utf8(hash_output.stdout)
        .unwrap()
        .trim()
        .to_string();
    CommitHash::from(hash.as_str())
}

fn add(dir: &TempDir, path: &str) {
    Command::new("git")
        .args(["add", path])
        .current_dir(dir.path())
        .output()
        .unwrap();
}

#[test]
fn initial_commit_uses_get_initial_commit_additions() {
    let dir = create_test_repo();
    std::fs::write(dir.path().join("a.txt"), "line1\nline2\n").unwrap();
    add(&dir, "a.txt");
    let hash = commit(&dir, "initial");

    let changes = gitoui::git::get_initial_commit_additions(dir.path(), &hash);
    assert_eq!(changes.len(), 1);
    match &changes[0] {
        FileChange::Add { path, additions } => {
            assert_eq!(path, "a.txt");
            assert_eq!(*additions, 2);
        }
        other => panic!("expected Add, got {:?}", other),
    }
}

#[test]
fn second_commit_adds_a_new_file() {
    let dir = create_test_repo();
    std::fs::write(dir.path().join("a.txt"), "x\n").unwrap();
    add(&dir, "a.txt");
    let _ = commit(&dir, "first");

    std::fs::write(dir.path().join("b.txt"), "one\ntwo\nthree\n").unwrap();
    add(&dir, "b.txt");
    let hash = commit(&dir, "add b");

    let changes = gitoui::git::get_diff_summary(dir.path(), &hash);
    assert_eq!(changes.len(), 1);
    match &changes[0] {
        FileChange::Add { path, additions } => {
            assert_eq!(path, "b.txt");
            assert_eq!(*additions, 3);
        }
        other => panic!("expected Add, got {:?}", other),
    }
}

#[test]
fn modify_file_records_additions_and_deletions() {
    let dir = create_test_repo();
    std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
    add(&dir, "a.txt");
    let _ = commit(&dir, "first");

    // Replace `two` with two new lines; net +1.
    std::fs::write(dir.path().join("a.txt"), "one\nTWO-A\nTWO-B\nthree\n").unwrap();
    add(&dir, "a.txt");
    let hash = commit(&dir, "modify");

    let changes = gitoui::git::get_diff_summary(dir.path(), &hash);
    assert_eq!(changes.len(), 1);
    match &changes[0] {
        FileChange::Modify {
            path,
            additions,
            deletions,
        } => {
            assert_eq!(path, "a.txt");
            assert_eq!(*additions, 2);
            assert_eq!(*deletions, 1);
        }
        other => panic!("expected Modify, got {:?}", other),
    }
}

#[test]
fn delete_file_records_deletions() {
    let dir = create_test_repo();
    std::fs::write(dir.path().join("a.txt"), "x\ny\n").unwrap();
    add(&dir, "a.txt");
    let _ = commit(&dir, "first");

    std::fs::remove_file(dir.path().join("a.txt")).unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let hash = commit(&dir, "delete a");

    let changes = gitoui::git::get_diff_summary(dir.path(), &hash);
    assert_eq!(changes.len(), 1);
    match &changes[0] {
        FileChange::Delete { path, deletions } => {
            assert_eq!(path, "a.txt");
            assert_eq!(*deletions, 2);
        }
        other => panic!("expected Delete, got {:?}", other),
    }
}

#[test]
fn rename_records_move_with_from_and_to() {
    let dir = create_test_repo();
    let content = "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\ntheta\niota\nkappa\n";
    std::fs::write(dir.path().join("old.txt"), content).unwrap();
    add(&dir, "old.txt");
    let _ = commit(&dir, "first");

    // git rename detection needs >50% similarity — copy content verbatim.
    std::fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
    Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let hash = commit(&dir, "rename");

    let changes = gitoui::git::get_diff_summary(dir.path(), &hash);
    assert_eq!(changes.len(), 1);
    match &changes[0] {
        FileChange::Move { from, to, .. } => {
            assert_eq!(from, "old.txt");
            assert_eq!(to, "new.txt");
        }
        other => panic!("expected Move, got {:?}", other),
    }
}

#[test]
fn multiple_files_returned_in_one_call() {
    let dir = create_test_repo();
    std::fs::write(dir.path().join("a.txt"), "1\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "2\n").unwrap();
    add(&dir, ".");
    let _ = commit(&dir, "first");

    std::fs::write(dir.path().join("a.txt"), "1\n2\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "x\n").unwrap();
    std::fs::write(dir.path().join("c.txt"), "new\nfile\n").unwrap();
    add(&dir, ".");
    let hash = commit(&dir, "second");

    let changes = gitoui::git::get_diff_summary(dir.path(), &hash);
    assert_eq!(changes.len(), 3);
    // Order is git's natural output; sort by path for assertion stability.
    let mut paths: Vec<&str> = changes
        .iter()
        .map(|c| match c {
            FileChange::Add { path, .. }
            | FileChange::Modify { path, .. }
            | FileChange::Delete { path, .. } => path.as_str(),
            FileChange::Move { to, .. } => to.as_str(),
        })
        .collect();
    paths.sort();
    assert_eq!(paths, vec!["a.txt", "b.txt", "c.txt"]);
}
