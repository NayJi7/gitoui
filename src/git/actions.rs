use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::git::diff::{DiffLineType, Hunk};

pub type GitResult = Result<String, String>;

fn run_git(path: &Path, args: &[&str]) -> GitResult {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to run git: {}", e))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

// --- In-Progress Operation Detection ---

#[derive(Debug, Clone, PartialEq)]
pub enum InProgressOperation {
    Rebase,
    Merge,
    CherryPick,
}

/// Detect whether a rebase, merge, or cherry-pick is in progress.
///
/// `git_dir` must be the `.git/` directory path (e.g. `repository.path()`).
pub fn detect_in_progress(git_dir: &Path) -> Option<InProgressOperation> {
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        Some(InProgressOperation::Rebase)
    } else if git_dir.join("MERGE_HEAD").exists() {
        Some(InProgressOperation::Merge)
    } else if git_dir.join("CHERRY_PICK_HEAD").exists() {
        Some(InProgressOperation::CherryPick)
    } else {
        None
    }
}

pub fn abort_rebase(path: &Path) -> GitResult {
    run_git(path, &["rebase", "--abort"])
}

pub fn abort_merge(path: &Path) -> GitResult {
    run_git(path, &["merge", "--abort"])
}

pub fn abort_cherry_pick(path: &Path) -> GitResult {
    run_git(path, &["cherry-pick", "--abort"])
}

// --- Commit Actions ---

pub fn checkout_commit(path: &Path, commit_hash: &str) -> GitResult {
    run_git(path, &["checkout", commit_hash])
}

pub fn checkout_branch(path: &Path, branch: &str) -> GitResult {
    run_git(path, &["checkout", branch])
}

pub fn create_branch_at(path: &Path, name: &str, start_point: &str, checkout: bool) -> GitResult {
    run_git(path, &["branch", name, start_point])?;
    if checkout {
        run_git(path, &["checkout", name])
    } else {
        Ok(format!("Created branch '{}' at {}", name, start_point))
    }
}

pub fn delete_branch(path: &Path, name: &str, force: bool) -> GitResult {
    let flag = if force { "-D" } else { "-d" };
    run_git(path, &["branch", flag, name])
}

pub fn rename_branch(path: &Path, old: &str, new: &str) -> GitResult {
    run_git(path, &["branch", "-m", old, new])
}

pub fn create_tag(path: &Path, name: &str, target: &str, message: Option<&str>) -> GitResult {
    match message {
        Some(msg) => run_git(path, &["tag", "-a", name, "-m", msg, target]),
        None => run_git(path, &["tag", name, target]),
    }
}

pub fn delete_tag(path: &Path, name: &str) -> GitResult {
    run_git(path, &["tag", "-d", name])
}

pub fn cherry_pick(
    path: &Path,
    commit_hash: &str,
    no_commit: bool,
    record_origin: bool,
) -> GitResult {
    let mut args = vec!["cherry-pick"];
    if no_commit {
        args.push("-n");
    }
    if record_origin {
        args.push("-x");
    }
    args.push(commit_hash);
    run_git(path, &args)
}

pub fn revert_commit(path: &Path, commit_hash: &str) -> GitResult {
    run_git(path, &["revert", "--no-edit", commit_hash])
}

pub fn drop_commit(path: &Path, commit_hash: &str) -> GitResult {
    run_git(
        path,
        &[
            "rebase",
            "--onto",
            &format!("{}~1", commit_hash),
            commit_hash,
        ],
    )
}

pub fn merge_commit(
    path: &Path,
    commit_hash: &str,
    no_ff: bool,
    squash: bool,
    no_commit: bool,
) -> GitResult {
    let mut args = vec!["merge"];
    if no_ff {
        args.push("--no-ff");
    }
    if squash {
        args.push("--squash");
    }
    if no_commit {
        args.push("--no-commit");
    }
    args.push(commit_hash);
    run_git(path, &args)
}

pub fn merge_branch(
    path: &Path,
    branch: &str,
    no_ff: bool,
    squash: bool,
    no_commit: bool,
) -> GitResult {
    let mut args = vec!["merge"];
    if no_ff {
        args.push("--no-ff");
    }
    if squash {
        args.push("--squash");
    }
    if no_commit {
        args.push("--no-commit");
    }
    args.push(branch);
    run_git(path, &args)
}

pub fn rebase_onto(path: &Path, target: &str, ignore_date: bool, interactive: bool) -> GitResult {
    let mut args = vec!["rebase"];
    if interactive {
        args.push("-i");
    }
    if ignore_date {
        args.push("--ignore-date");
    }
    args.push(target);
    run_git(path, &args)
}

pub fn reset(path: &Path, target: &str, mode: &str) -> GitResult {
    run_git(path, &["reset", mode, target])
}

// --- Remote Actions ---

pub fn get_remotes(path: &Path) -> Result<Vec<String>, String> {
    let output = run_git(path, &["remote"])?;
    Ok(output
        .lines()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

pub fn push_set_upstream(path: &Path, remote: &str, branch: &str) -> GitResult {
    run_git(path, &["push", "--set-upstream", remote, branch])
}

pub fn add_remote(path: &Path, name: &str, url: &str) -> GitResult {
    run_git(path, &["remote", "add", name, url])
}

pub fn remove_remote(path: &Path, name: &str) -> GitResult {
    run_git(path, &["remote", "remove", name])
}

pub fn set_upstream(path: &Path, remote: &str, branch: &str) -> GitResult {
    run_git(
        path,
        &[
            "branch",
            "--set-upstream-to",
            &format!("{}/{}", remote, branch),
            branch,
        ],
    )
}

// --- Branch Actions ---

pub fn delete_remote_branch(path: &Path, remote: &str, branch: &str) -> GitResult {
    run_git(path, &["push", remote, "--delete", branch])
}

pub fn pull_branch(path: &Path, remote: &str, branch: &str) -> GitResult {
    run_git(path, &["pull", remote, branch])
}

pub fn push_branch(path: &Path, remote: &str, branch: &str, force: bool) -> GitResult {
    let mut args = vec!["push", remote, branch];
    if force {
        args.push("--force-with-lease");
    }
    run_git(path, &args)
}

pub fn push_commit(path: &Path, _commit_hash: &str) -> GitResult {
    // Push current branch to its upstream
    run_git(path, &["push"])
}

// --- Stash Actions ---

pub fn apply_stash(path: &Path, stash_ref: &str) -> GitResult {
    run_git(path, &["stash", "apply", stash_ref])
}

pub fn pop_stash(path: &Path, stash_ref: &str) -> GitResult {
    run_git(path, &["stash", "pop", stash_ref])
}

pub fn drop_stash(path: &Path, stash_ref: &str) -> GitResult {
    run_git(path, &["stash", "drop", stash_ref])
}

pub fn create_branch_from_stash(path: &Path, branch_name: &str, stash_ref: &str) -> GitResult {
    run_git(path, &["stash", "branch", branch_name, stash_ref])
}

// --- Uncommitted Actions ---

pub fn stage_file(path: &Path, file: &str) -> GitResult {
    run_git(path, &["add", file])
}

pub fn stage_all(path: &Path) -> GitResult {
    run_git(path, &["add", "."])
}

pub fn unstage_file(path: &Path, file: &str) -> GitResult {
    run_git(path, &["reset", "HEAD", file])
}

pub fn unstage_all(path: &Path) -> GitResult {
    run_git(path, &["reset", "HEAD"])
}

pub fn discard_file(path: &Path, file: &str) -> GitResult {
    run_git(path, &["checkout", "--", file])
}

pub fn discard_all(path: &Path) -> GitResult {
    run_git(path, &["checkout", "--", "."])
}

pub fn stash(path: &Path, message: Option<&str>, include_untracked: bool) -> GitResult {
    let mut args = vec!["stash", "push"];
    if include_untracked {
        args.push("-u");
    }
    if let Some(msg) = message {
        args.push("-m");
        args.push(msg);
    }
    run_git(path, &args)
}

pub fn commit(path: &Path, message: &str, amend: bool) -> GitResult {
    if amend {
        run_git(path, &["commit", "--amend", "-m", message])
    } else {
        run_git(path, &["commit", "-m", message])
    }
}

pub fn fetch(path: &Path) -> GitResult {
    run_git(path, &["fetch", "--all"])
}

pub fn clean_untracked(path: &Path) -> GitResult {
    run_git(path, &["clean", "-fd"])
}

// --- Hunk-Level Staging ---

/// Serialise a single `Hunk` into a unified-diff patch that `git apply` can
/// consume on stdin. The patch references the file via both `a/<path>` and
/// `b/<path>` (the standard git diff convention).
fn hunk_to_patch(file_path: &str, hunk: &Hunk) -> String {
    let mut s = String::new();
    s.push_str(&format!("diff --git a/{0} b/{0}\n", file_path));
    s.push_str(&format!("--- a/{}\n", file_path));
    s.push_str(&format!("+++ b/{}\n", file_path));
    for line in &hunk.lines {
        match line.line_type {
            DiffLineType::HunkHeader => {
                s.push_str(&line.content);
                s.push('\n');
            }
            DiffLineType::Context => {
                s.push(' ');
                s.push_str(&line.content);
                s.push('\n');
            }
            DiffLineType::Addition => {
                s.push('+');
                s.push_str(&line.content);
                s.push('\n');
            }
            DiffLineType::Deletion => {
                s.push('-');
                s.push_str(&line.content);
                s.push('\n');
            }
            _ => {}
        }
    }
    s
}

/// Pipe a patch to `git apply` (optionally in reverse). Captures stderr so the
/// caller can surface the underlying git error verbatim — `git apply` is
/// notoriously strict about whitespace and line endings.
fn run_git_apply(repo_path: &Path, patch: &str, reverse: bool) -> GitResult {
    let mut args = vec!["apply", "--cached", "--whitespace=nowarn"];
    if reverse {
        args.push("--reverse");
    }
    args.push("-");
    let mut child = Command::new("git")
        .args(&args)
        .current_dir(repo_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn git apply: {}", e))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "Failed to open git apply stdin".to_string())?;
        stdin
            .write_all(patch.as_bytes())
            .map_err(|e| format!("Failed to write patch: {}", e))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("Failed to wait on git apply: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Stage a single hunk by piping it as a patch into `git apply --cached`.
/// The hunk must come from the unstaged side (`git diff`).
pub fn stage_hunk(repo_path: &Path, file_path: &str, hunk: &Hunk) -> GitResult {
    let patch = hunk_to_patch(file_path, hunk);
    run_git_apply(repo_path, &patch, false)
}

/// Unstage a single hunk by reverse-applying it against the index.
/// The hunk must come from the staged side (`git diff --cached`).
pub fn unstage_hunk(repo_path: &Path, file_path: &str, hunk: &Hunk) -> GitResult {
    let patch = hunk_to_patch(file_path, hunk);
    run_git_apply(repo_path, &patch, true)
}

// --- Branch Metadata ---

pub fn branch_upstream(path: &Path, branch: &str) -> GitResult {
    run_git(
        path,
        &[
            "rev-parse",
            "--abbrev-ref",
            &format!("{}@{{upstream}}", branch),
        ],
    )
}

pub fn branch_ahead_count(path: &Path, branch: &str) -> GitResult {
    run_git(
        path,
        &["rev-list", "--count", &format!("@{{upstream}}..{}", branch)],
    )
}

pub fn branch_behind_count(path: &Path, branch: &str) -> GitResult {
    run_git(
        path,
        &["rev-list", "--count", &format!("{}..@{{upstream}}", branch)],
    )
}

pub fn branch_ahead_behind_vs(path: &Path, branch: &str, vs: &str) -> Result<(String, String), String> {
    let ahead = run_git(path, &["rev-list", "--count", &format!("{}..{}", vs, branch)])
        .unwrap_or_else(|_| "0".to_string());
    let behind = run_git(path, &["rev-list", "--count", &format!("{}..{}", branch, vs)])
        .unwrap_or_else(|_| "0".to_string());
    Ok((ahead.trim().to_string(), behind.trim().to_string()))
}

pub fn branch_tip_info(path: &Path, branch: &str) -> GitResult {
    run_git(path, &["log", "-1", "--format=%H %s", branch])
}

// --- Tag Metadata ---

pub fn tag_metadata(path: &Path, tag: &str) -> GitResult {
    run_git(
        path,
        &[
            "for-each-ref",
            "--format=%(objecttype)%x00%(objectname)%x00%(*objectname)%x00%(taggername)%x00%(taggeremail)%x00%(taggerdate:iso-strict)%x00%(contents:subject)",
            &format!("refs/tags/{}", tag),
        ],
    )
}

pub fn tag_target_info(path: &Path, tag: &str) -> GitResult {
    run_git(path, &["log", "-1", "--format=%H %s", tag])
}

// --- File History ---

#[derive(Debug, Clone)]
pub struct FileHistoryEntry {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    pub author: String,
    pub author_email: String,
    pub date: String,
}

/// Run `git log --follow` for a single file and return a list of commits.
pub fn file_history(path: &Path, file_path: &str) -> Result<Vec<FileHistoryEntry>, String> {
    let output = Command::new("git")
        .args([
            "log",
            "--follow",
            "--format=%H|%s|%an|%ae|%ar",
            "--",
            file_path,
        ])
        .current_dir(path)
        .output()
        .map_err(|e| format!("Failed to run git log: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let entries = stdout
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(5, '|').collect();
            if parts.len() < 5 {
                return None;
            }
            let hash = parts[0].to_string();
            let short_hash = hash[..7.min(hash.len())].to_string();
            Some(FileHistoryEntry {
                hash,
                short_hash,
                subject: parts[1].to_string(),
                author: parts[2].to_string(),
                author_email: parts[3].to_lowercase(),
                date: parts[4].to_string(),
            })
        })
        .collect();

    Ok(entries)
}

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: String,
    pub head: String,
    pub branch: Option<String>,
    pub is_current: bool,
    pub is_dirty: bool,
}

pub fn add_worktree(path: &Path, worktree_path: &str, branch: &str) -> GitResult {
    let output = Command::new("git")
        .args(["worktree", "add", "-b", branch, worktree_path])
        .current_dir(path)
        .output()
        .map_err(|e| format!("Failed to run git worktree add: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "git worktree add failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn delete_worktree(repo_path: &Path, worktree_path: &str, force: bool) -> GitResult {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(worktree_path);

    let output = Command::new("git")
        .args(&args)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to run git worktree remove: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "git worktree remove failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn list_worktrees(repo_path: &Path) -> Vec<WorktreeInfo> {
    let output = match Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_head = String::new();
    let mut current_branch: Option<String> = None;
    let mut is_first = true;

    for line in stdout.lines() {
        if line.starts_with("worktree ") {
            if let Some(path) = current_path.take() {
                let is_current = is_first;
                is_first = false;
                worktrees.push(WorktreeInfo {
                    path,
                    head: current_head.clone(),
                    branch: current_branch.take(),
                    is_current,
                    is_dirty: false,
                });
                current_head.clear();
            }
            current_path = Some(line["worktree ".len()..].to_string());
        } else if line.starts_with("HEAD ") {
            current_head = line["HEAD ".len()..].to_string();
        } else if line.starts_with("branch ") {
            current_branch = Some(line["branch ".len()..].to_string());
        }
    }
    if let Some(path) = current_path {
        let is_current = is_first;
        worktrees.push(WorktreeInfo {
            path,
            head: current_head,
            branch: current_branch,
            is_current,
            is_dirty: false,
        });
    }

    // Check dirty status for each worktree
    for wt in &mut worktrees {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&wt.path)
            .output();
        wt.is_dirty = matches!(output, Ok(o) if !o.stdout.is_empty());
    }

    worktrees
}

#[cfg(test)]
mod remote_tests {
    use super::*;

    #[test]
    fn add_remote_produces_valid_command_args() {
        let _ = std::panic::catch_unwind(|| {
            let _ = add_remote(std::path::Path::new("/nonexistent"), "origin", "https://example.com");
        });
    }

    #[test]
    fn remove_remote_produces_valid_command_args() {
        let _ = std::panic::catch_unwind(|| {
            let _ = remove_remote(std::path::Path::new("/nonexistent"), "origin");
        });
    }
}
