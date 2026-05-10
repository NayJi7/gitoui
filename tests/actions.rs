//! Integration tests for the destructive Git actions exposed by
//! `gitoui::git::actions`. Each test creates an isolated tempfile-based
//! repository, drives the action, then inspects the on-disk state via
//! plain `git` shell-outs to verify the expected effect occurred.
//!
//! These cover the actions that mutate history or refs (branch, tag, reset,
//! stash, cherry-pick, revert, checkout) — the ones where a regression would
//! silently corrupt a user's repository. Network actions (push, pull, fetch)
//! are intentionally not exercised here; they require remote infrastructure
//! and are better suited to manual / staging tests.

use std::fs;
use std::path::Path;
use std::process::Command;

use gitoui::git::actions;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Create an empty git repo with a deterministic identity so commit hashes
/// don't depend on the host's git config.
fn init_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    run_git(dir.path(), &["init", "-b", "main"]);
    run_git(dir.path(), &["config", "user.email", "test@test.com"]);
    run_git(dir.path(), &["config", "user.name", "Test"]);
    run_git(dir.path(), &["config", "commit.gpgsign", "false"]);
    dir
}

/// Add `path` to the work tree (relative to the repo) with the given content,
/// stage it, then commit with the given message. Returns the new HEAD hash.
fn write_and_commit(repo: &Path, file: &str, content: &str, message: &str) -> String {
    fs::write(repo.join(file), content).unwrap();
    run_git(repo, &["add", file]);
    run_git(repo, &["commit", "-m", message]);
    head_hash(repo)
}

/// Run a git subcommand, asserting non-zero exit status produces clear output.
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

/// Capture stdout of a git subcommand (trimmed). Asserts success.
fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn head_hash(repo: &Path) -> String {
    git_stdout(repo, &["rev-parse", "HEAD"])
}

fn branch_exists(repo: &Path, name: &str) -> bool {
    Command::new("git")
        .args(["show-ref", "--verify", "--quiet", &format!("refs/heads/{name}")])
        .current_dir(repo)
        .status()
        .expect("spawn git")
        .success()
}

fn tag_exists(repo: &Path, name: &str) -> bool {
    Command::new("git")
        .args(["show-ref", "--verify", "--quiet", &format!("refs/tags/{name}")])
        .current_dir(repo)
        .status()
        .expect("spawn git")
        .success()
}

fn current_branch(repo: &Path) -> String {
    git_stdout(repo, &["rev-parse", "--abbrev-ref", "HEAD"])
}

// ---------------------------------------------------------------------------
// Branch lifecycle
// ---------------------------------------------------------------------------

#[test]
fn create_branch_at_head_without_checkout() {
    let dir = init_repo();
    let head = write_and_commit(dir.path(), "a.txt", "hello\n", "initial");

    actions::create_branch_at(dir.path(), "feature", &head, false).unwrap();

    assert!(branch_exists(dir.path(), "feature"));
    // Without checkout, HEAD must stay on the original branch.
    assert_eq!(current_branch(dir.path()), "main");
}

#[test]
fn create_branch_at_head_with_checkout_switches() {
    let dir = init_repo();
    let head = write_and_commit(dir.path(), "a.txt", "hello\n", "initial");

    actions::create_branch_at(dir.path(), "feature", &head, true).unwrap();

    assert!(branch_exists(dir.path(), "feature"));
    assert_eq!(current_branch(dir.path()), "feature");
}

#[test]
fn delete_branch_removes_ref() {
    let dir = init_repo();
    let head = write_and_commit(dir.path(), "a.txt", "hello\n", "initial");
    run_git(dir.path(), &["branch", "throwaway", &head]);
    assert!(branch_exists(dir.path(), "throwaway"));

    actions::delete_branch(dir.path(), "throwaway", false).unwrap();

    assert!(!branch_exists(dir.path(), "throwaway"));
}

#[test]
fn delete_branch_unmerged_requires_force() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "hello\n", "initial");
    // Diverging branch with its own commit not on main.
    run_git(dir.path(), &["checkout", "-b", "diverged"]);
    write_and_commit(dir.path(), "b.txt", "world\n", "diverged");
    run_git(dir.path(), &["checkout", "main"]);

    // Without force, git refuses to delete an unmerged branch → action returns Err.
    assert!(actions::delete_branch(dir.path(), "diverged", false).is_err());
    assert!(branch_exists(dir.path(), "diverged"));

    // With force, deletion succeeds.
    actions::delete_branch(dir.path(), "diverged", true).unwrap();
    assert!(!branch_exists(dir.path(), "diverged"));
}

#[test]
fn rename_branch_relabels_ref() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "hello\n", "initial");
    run_git(dir.path(), &["branch", "old-name"]);

    actions::rename_branch(dir.path(), "old-name", "new-name").unwrap();

    assert!(!branch_exists(dir.path(), "old-name"));
    assert!(branch_exists(dir.path(), "new-name"));
}

// ---------------------------------------------------------------------------
// Tag lifecycle
// ---------------------------------------------------------------------------

#[test]
fn create_tag_lightweight() {
    let dir = init_repo();
    let head = write_and_commit(dir.path(), "a.txt", "hello\n", "initial");

    actions::create_tag(dir.path(), "v1.0", &head, None).unwrap();

    assert!(tag_exists(dir.path(), "v1.0"));
    // Lightweight tags resolve directly to the commit.
    assert_eq!(git_stdout(dir.path(), &["rev-parse", "v1.0"]), head);
}

#[test]
fn create_tag_annotated_carries_message() {
    let dir = init_repo();
    let head = write_and_commit(dir.path(), "a.txt", "hello\n", "initial");

    actions::create_tag(dir.path(), "v2.0", &head, Some("first stable release")).unwrap();

    assert!(tag_exists(dir.path(), "v2.0"));
    let body = git_stdout(dir.path(), &["tag", "-l", "v2.0", "--format=%(contents)"]);
    assert!(body.contains("first stable release"), "tag body: {body}");
}

#[test]
fn delete_tag_removes_ref() {
    let dir = init_repo();
    let head = write_and_commit(dir.path(), "a.txt", "hello\n", "initial");
    run_git(dir.path(), &["tag", "doomed", &head]);
    assert!(tag_exists(dir.path(), "doomed"));

    actions::delete_tag(dir.path(), "doomed").unwrap();

    assert!(!tag_exists(dir.path(), "doomed"));
}

// ---------------------------------------------------------------------------
// Reset (mixed / hard / soft)
// ---------------------------------------------------------------------------

#[test]
fn reset_hard_discards_working_tree_and_moves_head() {
    let dir = init_repo();
    let first = write_and_commit(dir.path(), "a.txt", "v1\n", "first");
    let _second = write_and_commit(dir.path(), "a.txt", "v2\n", "second");
    // Dirty the work tree so we can verify --hard wipes it.
    fs::write(dir.path().join("a.txt"), "uncommitted\n").unwrap();

    actions::reset(dir.path(), &first, "--hard").unwrap();

    assert_eq!(head_hash(dir.path()), first);
    let content = fs::read_to_string(dir.path().join("a.txt")).unwrap();
    assert_eq!(content, "v1\n", "--hard should restore v1, not the dirty content");
}

#[test]
fn reset_soft_keeps_changes_staged() {
    let dir = init_repo();
    let first = write_and_commit(dir.path(), "a.txt", "v1\n", "first");
    let _second = write_and_commit(dir.path(), "a.txt", "v2\n", "second");

    actions::reset(dir.path(), &first, "--soft").unwrap();

    assert_eq!(head_hash(dir.path()), first);
    // After --soft to the previous commit, the diff between the index and HEAD
    // should be the changes from "second" — i.e. a.txt should be staged with v2.
    let staged = git_stdout(dir.path(), &["diff", "--cached", "--name-only"]);
    assert!(staged.contains("a.txt"), "expected a.txt in index, got: {staged}");
}

#[test]
fn reset_mixed_unstages_but_keeps_working_tree() {
    let dir = init_repo();
    let first = write_and_commit(dir.path(), "a.txt", "v1\n", "first");
    let _second = write_and_commit(dir.path(), "a.txt", "v2\n", "second");

    actions::reset(dir.path(), &first, "--mixed").unwrap();

    assert_eq!(head_hash(dir.path()), first);
    // --mixed: nothing staged, but a.txt on disk still has v2.
    let staged = git_stdout(dir.path(), &["diff", "--cached", "--name-only"]);
    assert!(staged.is_empty(), "expected empty index, got: {staged}");
    let content = fs::read_to_string(dir.path().join("a.txt")).unwrap();
    assert_eq!(content, "v2\n");
}

// ---------------------------------------------------------------------------
// Stash lifecycle
// ---------------------------------------------------------------------------

fn stash_count(repo: &Path) -> usize {
    git_stdout(repo, &["stash", "list"])
        .lines()
        .filter(|l| !l.is_empty())
        .count()
}

#[test]
fn pop_stash_reapplies_and_drops() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "v1\n", "initial");
    fs::write(dir.path().join("a.txt"), "v2-uncommitted\n").unwrap();
    run_git(dir.path(), &["stash"]);
    assert_eq!(stash_count(dir.path()), 1);

    actions::pop_stash(dir.path(), "stash@{0}").unwrap();

    assert_eq!(stash_count(dir.path()), 0);
    let content = fs::read_to_string(dir.path().join("a.txt")).unwrap();
    assert_eq!(content, "v2-uncommitted\n");
}

#[test]
fn apply_stash_keeps_entry() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "v1\n", "initial");
    fs::write(dir.path().join("a.txt"), "v2-uncommitted\n").unwrap();
    run_git(dir.path(), &["stash"]);
    assert_eq!(stash_count(dir.path()), 1);

    actions::apply_stash(dir.path(), "stash@{0}").unwrap();

    assert_eq!(stash_count(dir.path()), 1, "apply must NOT drop the stash");
    let content = fs::read_to_string(dir.path().join("a.txt")).unwrap();
    assert_eq!(content, "v2-uncommitted\n");
}

#[test]
fn drop_stash_removes_entry_without_changing_worktree() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "v1\n", "initial");
    fs::write(dir.path().join("a.txt"), "v2-uncommitted\n").unwrap();
    run_git(dir.path(), &["stash"]);
    assert_eq!(stash_count(dir.path()), 1);

    actions::drop_stash(dir.path(), "stash@{0}").unwrap();

    assert_eq!(stash_count(dir.path()), 0);
    // Drop should not restore the stashed content — work tree stays clean.
    let content = fs::read_to_string(dir.path().join("a.txt")).unwrap();
    assert_eq!(content, "v1\n");
}

// ---------------------------------------------------------------------------
// Cherry-pick / revert (clean cases — conflicts are a separate concern)
// ---------------------------------------------------------------------------

#[test]
fn cherry_pick_brings_commit_to_current_branch() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "base\n", "base");

    // Side branch with a unique commit.
    run_git(dir.path(), &["checkout", "-b", "side"]);
    let side = write_and_commit(dir.path(), "b.txt", "side feature\n", "add b");

    // Back to main and cherry-pick the side commit.
    run_git(dir.path(), &["checkout", "main"]);
    actions::cherry_pick(dir.path(), &side, false, false).unwrap();

    let content = fs::read_to_string(dir.path().join("b.txt")).unwrap();
    assert_eq!(content, "side feature\n");
    // HEAD should be a new commit that introduces b.txt.
    let log = git_stdout(dir.path(), &["log", "--oneline", "main"]);
    assert!(log.contains("add b"), "log: {log}");
}

#[test]
fn revert_creates_inverse_commit() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "base\n", "base");
    let target = write_and_commit(dir.path(), "a.txt", "edited\n", "edit a");
    let head_before_revert = head_hash(dir.path());
    assert_eq!(head_before_revert, target);

    actions::revert_commit(dir.path(), &target).unwrap();

    // HEAD advanced (new revert commit) and the file is back to "base".
    assert_ne!(head_hash(dir.path()), target);
    let content = fs::read_to_string(dir.path().join("a.txt")).unwrap();
    assert_eq!(content, "base\n");
}

// ---------------------------------------------------------------------------
// Checkout
// ---------------------------------------------------------------------------

#[test]
fn checkout_branch_switches_head() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "hello\n", "initial");
    run_git(dir.path(), &["branch", "side"]);
    assert_eq!(current_branch(dir.path()), "main");

    actions::checkout_branch(dir.path(), "side").unwrap();

    assert_eq!(current_branch(dir.path()), "side");
}

#[test]
fn checkout_commit_detaches_head() {
    let dir = init_repo();
    let first = write_and_commit(dir.path(), "a.txt", "v1\n", "first");
    write_and_commit(dir.path(), "a.txt", "v2\n", "second");

    actions::checkout_commit(dir.path(), &first).unwrap();

    // Detached HEAD: --abbrev-ref returns the literal "HEAD".
    assert_eq!(current_branch(dir.path()), "HEAD");
    assert_eq!(head_hash(dir.path()), first);
}
