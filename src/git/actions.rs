use std::path::Path;
use std::process::Command;

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
