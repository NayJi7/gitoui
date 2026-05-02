use std::process::Command;
use tempfile::TempDir;

fn create_test_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
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
    dir
}

fn commit(dir: &TempDir, msg: &str) -> String {
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
    String::from_utf8(hash_output.stdout)
        .unwrap()
        .trim()
        .to_string()
}

#[test]
fn test_uncommitted_changes_untracked() {
    let dir = create_test_repo();
    std::fs::write(dir.path().join("new.txt"), "hello").unwrap();

    let changes =
        gitbranch::git::status::UncommittedChanges::load(dir.path()).unwrap();
    assert!(changes.is_dirty());
    assert_eq!(changes.untracked.len(), 1);
    assert_eq!(changes.untracked[0].path, "new.txt");
    assert_eq!(changes.staged.len(), 0);
    assert_eq!(changes.unstaged.len(), 0);
}

#[test]
fn test_uncommitted_changes_staged() {
    let dir = create_test_repo();
    std::fs::write(dir.path().join("a.txt"), "content").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let changes =
        gitbranch::git::status::UncommittedChanges::load(dir.path()).unwrap();
    assert!(changes.is_dirty());
    assert_eq!(changes.staged.len(), 1);
    assert_eq!(changes.untracked.len(), 0);
}

#[test]
fn test_uncommitted_changes_modified() {
    let dir = create_test_repo();
    std::fs::write(dir.path().join("a.txt"), "initial").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    commit(&dir, "initial");

    std::fs::write(dir.path().join("a.txt"), "modified").unwrap();

    let changes =
        gitbranch::git::status::UncommittedChanges::load(dir.path()).unwrap();
    assert!(changes.is_dirty());
    assert_eq!(changes.unstaged.len(), 1);
}

#[test]
fn test_uncommitted_changes_clean() {
    let dir = create_test_repo();
    std::fs::write(dir.path().join("a.txt"), "initial").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    commit(&dir, "initial");

    let changes =
        gitbranch::git::status::UncommittedChanges::load(dir.path()).unwrap();
    assert!(!changes.is_dirty());
    assert_eq!(changes.total_files(), 0);
}

#[test]
fn test_diff_for_commit() {
    let dir = create_test_repo();
    std::fs::write(dir.path().join("a.txt"), "line1\nline2\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let _hash1 = commit(&dir, "initial");

    std::fs::write(dir.path().join("a.txt"), "line1\nmodified\nline3\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let hash2 = commit(&dir, "modify a.txt");

    let entries =
        gitbranch::git::diff::DiffEntry::load_for_commit(dir.path(), &hash2).unwrap();
    assert!(!entries.is_empty());

    let has_modified = entries.iter().any(|e| {
        e.hunks.iter().any(|h| {
            h.lines
                .iter()
                .any(|l| l.content.contains("modified"))
        })
    });
    assert!(has_modified);
}

#[test]
fn test_diff_for_file() {
    let dir = create_test_repo();
    std::fs::write(dir.path().join("a.txt"), "original\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "other\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    commit(&dir, "initial");

    std::fs::write(dir.path().join("a.txt"), "changed\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let hash = commit(&dir, "modify a");

    let entry = gitbranch::git::diff::DiffEntry::load_for_file(
        dir.path(),
        &hash,
        "a.txt",
    )
    .unwrap();
    assert!(entry.new_path.is_some());
    let has_change = entry.hunks.iter().any(|h| {
        h.lines.iter().any(|l| l.content.contains("changed"))
    });
    assert!(has_change);
}
