//! Integration tests for hunk-level staging — the combined uncommitted diff
//! loader, the per-hunk origin tagging, the patch-builder + git apply path,
//! and round-trip stage / unstage behaviour. Each test spins up an isolated
//! git repo in a TempDir, mutates it the way a user would, then asserts both
//! the gitoui-level state (DiffEntry hunks, origins) and the on-disk state
//! (via plain `git diff` shell-outs).

use std::fs;
use std::path::Path;
use std::process::Command;

use gitoui::git::actions;
use gitoui::git::diff::{DiffEntry, DiffLineType, HunkOrigin};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn init_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    run_git(dir.path(), &["init", "-b", "main"]);
    run_git(dir.path(), &["config", "user.email", "test@test.com"]);
    run_git(dir.path(), &["config", "user.name", "Test"]);
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

fn git_stdout(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("spawn git");
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

fn write_and_commit(repo: &Path, file: &str, content: &str, message: &str) {
    fs::write(repo.join(file), content).unwrap();
    run_git(repo, &["add", file]);
    run_git(repo, &["commit", "-m", message]);
}

/// Build a 30-line file so we can craft well-separated hunks.
fn baseline_file_30_lines() -> String {
    (1..=30).map(|i| format!("line{}\n", i)).collect()
}

// ---------------------------------------------------------------------------
// 1. HunkOrigin tagging
// ---------------------------------------------------------------------------

#[test]
fn load_staged_tags_hunks_as_staged() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    // Stage a change.
    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    lines[5] = "STAGED_CHANGE\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();
    run_git(dir.path(), &["add", "a.txt"]);

    let entry = DiffEntry::load_staged_for_file(dir.path(), "a.txt").unwrap();
    assert!(!entry.hunks.is_empty(), "expected staged hunks");
    for h in &entry.hunks {
        assert_eq!(
            h.origin,
            HunkOrigin::Staged,
            "every loaded hunk must be tagged Staged"
        );
    }
}

#[test]
fn load_unstaged_tags_hunks_as_unstaged() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    // Make an unstaged change.
    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    lines[10] = "UNSTAGED_CHANGE\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();

    let entry = DiffEntry::load_unstaged_for_file(dir.path(), "a.txt").unwrap();
    assert!(!entry.hunks.is_empty(), "expected unstaged hunks");
    for h in &entry.hunks {
        assert_eq!(h.origin, HunkOrigin::Unstaged);
    }
}

#[test]
fn load_for_commit_keeps_origin_other() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "v1\n", "v1");
    fs::write(dir.path().join("a.txt"), "v2\n").unwrap();
    run_git(dir.path(), &["add", "a.txt"]);
    run_git(dir.path(), &["commit", "-m", "v2"]);
    let head = git_stdout(dir.path(), &["rev-parse", "HEAD"])
        .trim()
        .to_string();

    let entries = DiffEntry::load_for_commit(dir.path(), &head).unwrap();
    assert!(!entries.is_empty());
    for e in &entries {
        for h in &e.hunks {
            assert_eq!(
                h.origin,
                HunkOrigin::Other,
                "committed diff hunks must default to Other"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. Combined loader
// ---------------------------------------------------------------------------

#[test]
fn combined_loader_returns_none_for_unchanged_file() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "hello\n", "init");

    let combined = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3).unwrap();
    assert!(
        combined.is_none(),
        "unchanged file should yield no combined diff"
    );
}

#[test]
fn combined_loader_handles_staged_only() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    // Stage one change, no unstaged.
    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    lines[2] = "STAGED\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();
    run_git(dir.path(), &["add", "a.txt"]);

    let combined = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
        .unwrap()
        .expect("combined diff expected");
    assert!(!combined.hunks.is_empty());
    for h in &combined.hunks {
        assert_eq!(h.origin, HunkOrigin::Staged);
    }
}

#[test]
fn combined_loader_handles_unstaged_only() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    lines[2] = "UNSTAGED\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();
    // Do NOT git add.

    let combined = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
        .unwrap()
        .expect("combined diff expected");
    assert!(!combined.hunks.is_empty());
    for h in &combined.hunks {
        assert_eq!(h.origin, HunkOrigin::Unstaged);
    }
}

#[test]
fn combined_loader_merges_staged_and_unstaged_sorted() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    // Stage a change near the top of the file.
    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    lines[2] = "STAGED_TOP\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();
    run_git(dir.path(), &["add", "a.txt"]);

    // Make another change near the bottom, unstaged.
    let mut lines: Vec<String> = lines.clone();
    lines[25] = "UNSTAGED_BOTTOM\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();

    let combined = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
        .unwrap()
        .expect("combined diff expected");
    assert!(
        combined.hunks.len() >= 2,
        "expected at least two hunks, got {}",
        combined.hunks.len()
    );

    // Sorted by new_start ascending.
    let starts: Vec<u32> = combined.hunks.iter().map(|h| h.new_start).collect();
    let mut sorted = starts.clone();
    sorted.sort();
    assert_eq!(starts, sorted, "combined hunks must be sorted by new_start");

    // Mix of origins present.
    let origins: Vec<HunkOrigin> = combined.hunks.iter().map(|h| h.origin).collect();
    assert!(
        origins.contains(&HunkOrigin::Staged),
        "missing staged hunk: {origins:?}"
    );
    assert!(
        origins.contains(&HunkOrigin::Unstaged),
        "missing unstaged hunk: {origins:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. stage_hunk / unstage_hunk via git apply
// ---------------------------------------------------------------------------

#[test]
fn stage_hunk_moves_change_to_index() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    // Two well-separated unstaged hunks.
    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    lines[2] = "CHANGE_TOP\n".to_string();
    lines[20] = "CHANGE_BOTTOM\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();

    let combined = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
        .unwrap()
        .unwrap();
    assert_eq!(combined.hunks.len(), 2);

    // Stage the first hunk.
    actions::stage_hunk(dir.path(), "a.txt", &combined.hunks[0]).expect("stage_hunk failed");

    // The first change is now in the index.
    let cached = git_stdout(dir.path(), &["diff", "--cached", "a.txt"]);
    assert!(
        cached.contains("CHANGE_TOP"),
        "staged diff missing CHANGE_TOP: {cached}"
    );
    assert!(
        !cached.contains("CHANGE_BOTTOM"),
        "staged diff should not include CHANGE_BOTTOM yet"
    );

    // The second change is still in the working tree only.
    let unstaged = git_stdout(dir.path(), &["diff", "a.txt"]);
    assert!(unstaged.contains("CHANGE_BOTTOM"));
}

#[test]
fn unstage_hunk_moves_change_out_of_index() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    // Stage everything first.
    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    lines[2] = "STAGED_A\n".to_string();
    lines[20] = "STAGED_B\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();
    run_git(dir.path(), &["add", "a.txt"]);

    let combined = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
        .unwrap()
        .unwrap();
    let staged_hunks: Vec<_> = combined
        .hunks
        .iter()
        .filter(|h| h.origin == HunkOrigin::Staged)
        .cloned()
        .collect();
    assert_eq!(staged_hunks.len(), 2);

    // Unstage just the first one.
    actions::unstage_hunk(dir.path(), "a.txt", &staged_hunks[0]).expect("unstage_hunk failed");

    let cached = git_stdout(dir.path(), &["diff", "--cached", "a.txt"]);
    assert!(
        !cached.contains("STAGED_A"),
        "first hunk should have been unstaged"
    );
    assert!(
        cached.contains("STAGED_B"),
        "second hunk should remain staged"
    );

    let unstaged = git_stdout(dir.path(), &["diff", "a.txt"]);
    assert!(
        unstaged.contains("STAGED_A"),
        "first hunk should be in working tree diff"
    );
}

#[test]
fn stage_then_unstage_round_trip_restores_state() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    lines[10] = "ROUND_TRIP\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();

    // Capture initial state.
    let initial_unstaged = git_stdout(dir.path(), &["diff", "a.txt"]);
    let initial_staged = git_stdout(dir.path(), &["diff", "--cached", "a.txt"]);
    assert!(initial_staged.is_empty());

    // Stage → unstage.
    let combined = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
        .unwrap()
        .unwrap();
    actions::stage_hunk(dir.path(), "a.txt", &combined.hunks[0]).expect("stage_hunk failed");

    let after_stage_combined =
        DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
            .unwrap()
            .unwrap();
    actions::unstage_hunk(dir.path(), "a.txt", &after_stage_combined.hunks[0])
        .expect("unstage_hunk failed");

    // Back to the initial state.
    let final_unstaged = git_stdout(dir.path(), &["diff", "a.txt"]);
    let final_staged = git_stdout(dir.path(), &["diff", "--cached", "a.txt"]);
    assert_eq!(
        final_unstaged, initial_unstaged,
        "unstaged diff should be restored"
    );
    assert_eq!(
        final_staged, initial_staged,
        "staged diff should be restored"
    );
}

// ---------------------------------------------------------------------------
// 4. Multi-hunk surgical staging
// ---------------------------------------------------------------------------

#[test]
fn stage_middle_hunk_only() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    // Three changes, far enough apart to be three separate hunks (default context = 3 lines).
    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    lines[2] = "TOP\n".to_string();
    lines[14] = "MID\n".to_string();
    lines[26] = "BOT\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();

    let combined = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
        .unwrap()
        .unwrap();
    assert_eq!(combined.hunks.len(), 3, "expected three separated hunks");

    actions::stage_hunk(dir.path(), "a.txt", &combined.hunks[1]).expect("stage middle failed");

    let cached = git_stdout(dir.path(), &["diff", "--cached", "a.txt"]);
    assert!(cached.contains("MID"));
    assert!(!cached.contains("TOP"));
    assert!(!cached.contains("BOT"));
}

#[test]
fn combined_loader_hunk_count_matches_separate_loaders() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    // Stage one hunk.
    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    lines[2] = "STG\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();
    run_git(dir.path(), &["add", "a.txt"]);

    // Add another change unstaged.
    let mut lines: Vec<String> = lines.clone();
    lines[20] = "UNS\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();

    let staged = DiffEntry::load_staged_for_file(dir.path(), "a.txt").unwrap();
    let unstaged = DiffEntry::load_unstaged_for_file(dir.path(), "a.txt").unwrap();
    let combined = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
        .unwrap()
        .unwrap();
    assert_eq!(
        combined.hunks.len(),
        staged.hunks.len() + unstaged.hunks.len(),
        "combined must contain every staged + unstaged hunk"
    );
}

// ---------------------------------------------------------------------------
// 5. Patch correctness — diff-line counts preserved
// ---------------------------------------------------------------------------

#[test]
fn stage_hunk_preserves_addition_deletion_counts() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    // Mixed change: replace one line + add a new one.
    lines[10] = "REPLACED\n".to_string();
    lines.insert(11, "INSERTED\n".to_string());
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();

    let combined = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
        .unwrap()
        .unwrap();
    let (adds, dels) = combined.count_additions_and_deletions();

    actions::stage_hunk(dir.path(), "a.txt", &combined.hunks[0]).expect("stage failed");
    let staged = DiffEntry::load_staged_for_file(dir.path(), "a.txt").unwrap();
    let (stg_adds, stg_dels) = staged.count_additions_and_deletions();
    assert_eq!(
        adds, stg_adds,
        "staged addition count must equal hunk addition count"
    );
    assert_eq!(
        dels, stg_dels,
        "staged deletion count must equal hunk deletion count"
    );
}

#[test]
fn binary_placeholder_origin_defaults_to_other() {
    let placeholder = DiffEntry::binary_placeholder("img.png");
    assert_eq!(placeholder.hunks.len(), 1);
    assert_eq!(placeholder.hunks[0].origin, HunkOrigin::Other);
    assert!(placeholder.hunks[0]
        .lines
        .iter()
        .any(|l| l.line_type == DiffLineType::BinaryNote));
}

// ---------------------------------------------------------------------------
// 6. Idempotency / safety
// ---------------------------------------------------------------------------

#[test]
fn stage_hunk_twice_second_call_is_a_noop_error() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", &baseline_file_30_lines(), "init");

    let mut lines: Vec<String> = baseline_file_30_lines()
        .lines()
        .map(|s| s.to_string() + "\n")
        .collect();
    lines[5] = "DBL\n".to_string();
    fs::write(dir.path().join("a.txt"), lines.concat()).unwrap();

    let combined = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
        .unwrap()
        .unwrap();
    actions::stage_hunk(dir.path(), "a.txt", &combined.hunks[0]).expect("first stage failed");

    // Second apply against the same hunk should fail because git apply detects
    // the patch is already applied. We want a clean Err rather than a panic
    // or silent corruption.
    let second = actions::stage_hunk(dir.path(), "a.txt", &combined.hunks[0]);
    assert!(
        second.is_err(),
        "second stage of same hunk should fail cleanly"
    );
}

#[test]
fn stage_hunk_does_not_disturb_other_files() {
    let dir = init_repo();
    write_and_commit(dir.path(), "a.txt", "hello\n", "init a");
    write_and_commit(dir.path(), "b.txt", "world\n", "init b");

    // Change both files; stage only a.txt's hunk.
    fs::write(dir.path().join("a.txt"), "hello!\n").unwrap();
    fs::write(dir.path().join("b.txt"), "world!\n").unwrap();

    let combined_a = DiffEntry::load_combined_uncommitted_for_file(dir.path(), "a.txt", 3)
        .unwrap()
        .unwrap();
    actions::stage_hunk(dir.path(), "a.txt", &combined_a.hunks[0]).unwrap();

    let cached = git_stdout(dir.path(), &["diff", "--cached"]);
    assert!(cached.contains("a.txt"));
    assert!(
        !cached.contains("b.txt"),
        "b.txt must remain unstaged after staging a.txt only"
    );
}
