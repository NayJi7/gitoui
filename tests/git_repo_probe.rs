//! Baseline tests for `git::is_git_path` — the dir-input overlay's gate
//! that decides whether a target path can be cd'd into. The upcoming
//! optimisation combines the two `git rev-parse` calls into one; these
//! tests pin down the current observable behaviour so the refactor can't
//! silently regress any case.

use std::process::Command;
use tempfile::TempDir;

fn init_repo(dir: &TempDir) {
    Command::new("git")
        .args(["init"])
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
}

fn init_bare(dir: &TempDir) {
    Command::new("git")
        .args(["init", "--bare"])
        .current_dir(dir.path())
        .output()
        .unwrap();
}

#[test]
fn accepts_work_tree_root() {
    let dir = TempDir::new().unwrap();
    init_repo(&dir);
    assert!(gitoui::git::is_git_path(dir.path()));
}

#[test]
fn accepts_bare_repository() {
    let dir = TempDir::new().unwrap();
    init_bare(&dir);
    assert!(gitoui::git::is_git_path(dir.path()));
}

#[test]
fn rejects_subfolder_of_work_tree() {
    let dir = TempDir::new().unwrap();
    init_repo(&dir);
    let sub = dir.path().join("src");
    std::fs::create_dir(&sub).unwrap();
    // Subdirectory IS inside the work tree per git, but gitoui's UI is
    // anchored at the repo root — so we reject it.
    assert!(!gitoui::git::is_git_path(&sub));
}

#[test]
fn rejects_nested_subfolder_of_work_tree() {
    let dir = TempDir::new().unwrap();
    init_repo(&dir);
    let nested = dir.path().join("src").join("inner");
    std::fs::create_dir_all(&nested).unwrap();
    assert!(!gitoui::git::is_git_path(&nested));
}

#[test]
fn rejects_non_git_directory() {
    let dir = TempDir::new().unwrap();
    // No `git init` — just a plain directory.
    assert!(!gitoui::git::is_git_path(dir.path()));
}

#[test]
fn rejects_missing_path() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("does-not-exist");
    assert!(!gitoui::git::is_git_path(&missing));
}

#[test]
fn accepts_work_tree_root_via_symlink() {
    let dir = TempDir::new().unwrap();
    init_repo(&dir);
    // Create a symlink that points at the repo root — canonicalisation
    // should resolve it back to the toplevel so the check passes.
    let link = dir
        .path()
        .parent()
        .unwrap()
        .join(format!("gitoui-test-link-{}", std::process::id()));
    let _ = std::fs::remove_file(&link);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path(), &link).unwrap();
        let result = gitoui::git::is_git_path(&link);
        let _ = std::fs::remove_file(&link);
        assert!(result, "symlink to work tree root should be accepted");
    }
}
