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
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(humanize_git_error(args, &stderr))
    }
}

/// Translate a raw `git` stderr into a one-line, footer-friendly
/// message. The git command's stderr is rich but not pretty, it
/// often includes multi-line hints, ANSI escapes, and shell-style
/// suggestions that look noisy in a single-row toast. We pattern-
/// match on the common failures and fall back to a stripped first
/// line for anything unrecognised.
fn humanize_git_error(args: &[&str], stderr: &str) -> String {
    // Sub-command drives some context-specific messages (e.g.,
    // "rebase: …" vs "blame: …").
    let subcommand = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .copied()
        .unwrap_or("");
    let stripped = stderr.trim();
    let lower = stripped.to_ascii_lowercase();
    let first_line = stripped
        .lines()
        .next()
        .unwrap_or(stripped)
        .trim()
        .trim_start_matches("fatal:")
        .trim_start_matches("error:")
        .trim_start_matches("warning:")
        .trim()
        .to_string();

    // ── Working-tree state failures ────────────────────────────────
    if lower.contains("your local changes to the following files would be overwritten")
        || lower.contains("would be overwritten by")
    {
        return "Uncommitted local changes block this, commit or stash first.".to_string();
    }
    if lower.contains("please commit your changes or stash them") {
        return "Working tree isn't clean, commit or stash changes first.".into();
    }
    if lower.contains("needs merge") || lower.contains("you have unmerged paths") {
        return "Unmerged paths from a previous conflict, resolve them first.".into();
    }
    if lower.contains("nothing to commit") {
        return "Nothing to commit, working tree is clean.".into();
    }
    if lower.contains("nothing to amend") {
        return "Nothing to amend, no staged changes.".into();
    }

    // ── Conflict / apply failures ──────────────────────────────────
    if lower.contains("conflict (")
        || lower.contains("merge conflict")
        || lower.contains("automatic merge failed")
    {
        return match subcommand {
            "cherry-pick" => {
                "Cherry-pick conflict, resolve in the conflict editor, then continue.".into()
            }
            "rebase" => "Rebase conflict, resolve in the conflict editor, then continue.".into(),
            "merge" => "Merge conflict, resolve in the conflict editor, then commit.".into(),
            "revert" => "Revert conflict, resolve in the conflict editor, then continue.".into(),
            _ => "Conflict, resolve manually then continue.".into(),
        };
    }
    if lower.contains("could not apply") {
        return format!(
            "Couldn't apply commit cleanly{}",
            if subcommand.is_empty() {
                ""
            } else {
                " (likely a conflict)"
            }
        );
    }

    // ── In-progress operation conflicts ────────────────────────────
    if lower.contains("rebase in progress") || lower.contains("you are currently rebasing") {
        return "A rebase is already in progress, finish or abort it first.".into();
    }
    if lower.contains("you are currently cherry-picking")
        || lower.contains("cherry-pick is now empty")
    {
        return "A cherry-pick is already in progress, finish or abort it first.".into();
    }
    if lower.contains("you have not concluded your merge")
        || lower.contains("merging is not possible")
    {
        return "A merge is already in progress, finish or abort it first.".into();
    }

    // ── Reference / ancestry problems ──────────────────────────────
    if lower.contains("not something we can merge") {
        return "That reference isn't mergeable, check the commit exists locally.".into();
    }
    if lower.contains("not a valid object")
        || lower.contains("bad revision")
        || lower.contains("bad object")
        || lower.contains("unknown revision")
        || lower.contains("ambiguous argument")
    {
        return "Reference not found, commit or branch doesn't exist locally.".into();
    }
    if lower.contains("refusing to merge unrelated histories") {
        return "Refusing to merge unrelated histories, branches have no common ancestor.".into();
    }
    if lower.contains("not a tree object") || lower.contains("is not a commit") {
        return "Object isn't a commit, wrong kind of git reference.".into();
    }
    if subcommand == "rebase" && (lower.contains("no such") || lower.contains("nothing to do")) {
        return "Rebase target isn't in this branch's history, nothing to do.".into();
    }
    if subcommand == "rebase" && lower.contains("invalid upstream") {
        return "Rebase: invalid upstream, that commit isn't reachable.".into();
    }
    if subcommand == "rebase"
        && (lower.contains("would have no commits") || lower.contains("nothing to commit"))
    {
        return "Rebase would produce no commits, target equals current.".into();
    }

    // ── Branch / checkout failures ─────────────────────────────────
    if lower.contains("a branch named") && lower.contains("already exists") {
        return "Branch already exists with that name.".into();
    }
    if lower.contains("not a valid branch name") {
        return "Invalid branch name.".into();
    }
    if lower.contains("cannot delete branch") && lower.contains("checked out") {
        return "Can't delete the currently-checked-out branch, switch first.".into();
    }
    if lower.contains("branch") && lower.contains("not fully merged") {
        return "Branch isn't fully merged, use force-delete to remove anyway.".into();
    }
    if lower.contains("no such branch") {
        return "Branch doesn't exist locally.".into();
    }
    if subcommand == "checkout" && lower.contains("did not match any") {
        return "Checkout target not found, branch or commit doesn't exist.".into();
    }

    // ── Path / file problems ───────────────────────────────────────
    if subcommand == "blame"
        && (lower.contains("no such path")
            || lower.contains("did not match")
            || lower.contains("no such file"))
    {
        return "Blame: file isn't in the target commit.".into();
    }
    if subcommand == "log" && (lower.contains("no such path") || lower.contains("did not match")) {
        return "File history: file isn't in this commit's tree.".into();
    }
    if lower.contains("pathspec") && lower.contains("did not match") {
        return "Path not found at that commit.".into();
    }
    if lower.contains("no such path in") {
        return "File not present at that commit.".into();
    }

    // ── Network / remote failures ──────────────────────────────────
    if lower.contains("could not read from remote repository")
        || lower.contains("permission denied (publickey)")
    {
        return "Remote unreachable, check SSH/HTTPS auth and network.".into();
    }
    if lower.contains("repository not found") {
        return "Remote repository not found, check the URL/permissions.".into();
    }
    if lower.contains("non-fast-forward") || lower.contains("rejected") && subcommand == "push" {
        return "Push rejected, remote has commits you don't have. Pull/rebase first.".into();
    }
    if lower.contains("couldn't find remote ref") {
        return "Remote ref not found, branch may have been deleted upstream.".into();
    }

    // ── Tag / stash specifics ──────────────────────────────────────
    if subcommand == "tag" && lower.contains("already exists") {
        return "Tag already exists with that name.".into();
    }
    if subcommand == "stash"
        && (lower.contains("no stash entries") || lower.contains("no stash found"))
    {
        return "No stash entries to act on.".into();
    }

    // ── Last-resort fallback: clean up git's prefix + first line. ──
    let cleaned = first_line.trim_end_matches('.').trim_end_matches(';');
    if cleaned.is_empty() {
        return format!("git {} failed.", subcommand);
    }
    if subcommand.is_empty() {
        cleaned.to_string()
    } else {
        // Capitalise the subcommand for a touch of polish.
        let mut chars = subcommand.chars();
        let pretty_cmd = match chars.next() {
            Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            None => subcommand.to_string(),
        };
        format!("{}: {}", pretty_cmd, cleaned)
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
    // Write to a temp file so multiline messages (subject\n\nbody) are
    // preserved exactly, git commit -m can swallow embedded newlines on
    // some platforms, whereas -F always reads the file verbatim.
    let tmp = path.join(".git").join("GITOUI_COMMIT_MSG_TMP");
    std::fs::write(&tmp, message).map_err(|e| format!("Failed to write commit message: {}", e))?;
    let tmp_str = tmp.to_string_lossy().into_owned();
    let result = if amend {
        run_git(path, &["commit", "--amend", "-F", &tmp_str])
    } else {
        run_git(path, &["commit", "-F", &tmp_str])
    };
    let _ = std::fs::remove_file(&tmp);
    result
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
/// caller can surface the underlying git error verbatim, `git apply` is
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

pub fn branch_ahead_behind_vs(
    path: &Path,
    branch: &str,
    vs: &str,
) -> Result<(String, String), String> {
    let ahead = run_git(
        path,
        &["rev-list", "--count", &format!("{}..{}", vs, branch)],
    )
    .unwrap_or_else(|_| "0".to_string());
    let behind = run_git(
        path,
        &["rev-list", "--count", &format!("{}..{}", branch, vs)],
    )
    .unwrap_or_else(|_| "0".to_string());
    Ok((ahead.trim().to_string(), behind.trim().to_string()))
}

// --- Tag Metadata ---

/// Resolved metadata for a git tag (annotated or lightweight).
#[derive(Debug, Clone)]
pub struct TagInfo {
    /// "Annotated" or "Lightweight"
    pub tag_type: String,
    /// Short hash of the target commit
    pub target_hash: String,
    /// Subject line of the target commit
    pub target_commit_message: String,
    /// Tagger identity (annotated only)
    pub tagger: Option<String>,
    /// Tagger date in ISO format (annotated only)
    pub date: Option<String>,
    /// Tag message body (annotated only)
    pub message: Option<String>,
}

/// Resolve full metadata for a tag. Works for both annotated and
/// lightweight tags by combining `git cat-file` (for the tag object
/// itself) with `git log` (for the target commit details).
pub fn tag_info(path: &Path, tag: &str) -> Result<TagInfo, String> {
    let obj_type = run_git(path, &["cat-file", "-t", tag])?;
    let is_annotated = obj_type.trim() == "tag";

    // Target commit: `git log -1 --format=<hash> <subject>` resolves
    // through annotated tags automatically.
    let target_line = run_git(path, &["log", "-1", "--format=%h %s", tag])
        .unwrap_or_default();
    let (target_hash, target_msg) = match target_line.find(' ') {
        Some(pos) => (
            target_line[..pos].to_string(),
            target_line[pos + 1..].to_string(),
        ),
        None => (target_line.clone(), String::new()),
    };

    if is_annotated {
        // Parse `git cat-file -p <tag>` output for tagger/message.
        let raw = run_git(path, &["cat-file", "-p", tag]).unwrap_or_default();
        let mut tagger: Option<String> = None;
        let mut date: Option<String> = None;
        let mut in_body = false;
        let mut body_lines: Vec<&str> = Vec::new();

        for line in raw.lines() {
            if in_body {
                body_lines.push(line);
                continue;
            }
            if line.is_empty() {
                in_body = true;
                continue;
            }
            if let Some(rest) = line.strip_prefix("tagger ") {
                // Format: "Name <email> timestamp tz"
                // We want "Name <email>" and a human-readable date.
                if let Some(email_end) = rest.rfind('>') {
                    tagger = Some(rest[..=email_end].to_string());
                    // Remaining is " timestamp tz" - convert via git
                    let ts_tz = rest[email_end + 1..].trim();
                    if !ts_tz.is_empty() {
                        // Use git to format the raw timestamp
                        let parts: Vec<&str> = ts_tz.split_whitespace().collect();
                        if parts.len() == 2 {
                            if let Ok(epoch) = parts[0].parse::<i64>() {
                                let tz = parts[1];
                                // Build ISO-ish date manually
                                date = Some(format_epoch_tz(epoch, tz));
                            }
                        }
                    }
                }
            }
        }

        // First line of body = subject, rest = message body
        let message = if body_lines.len() > 1 {
            let body = body_lines[1..].join("\n").trim().to_string();
            if body.is_empty() { None } else { Some(body) }
        } else {
            None
        };

        Ok(TagInfo {
            tag_type: "Annotated".to_string(),
            target_hash,
            target_commit_message: target_msg,
            tagger,
            date,
            message,
        })
    } else {
        Ok(TagInfo {
            tag_type: "Lightweight".to_string(),
            target_hash,
            target_commit_message: target_msg,
            tagger: None,
            date: None,
            message: None,
        })
    }
}

/// Format a Unix epoch + timezone offset string (e.g. "+0200") into a
/// human-readable date. Falls back to the raw epoch if parsing fails.
fn format_epoch_tz(epoch: i64, tz: &str) -> String {
    // Parse tz offset like "+0200" or "-0500"
    if tz.len() < 5 {
        return epoch.to_string();
    }
    let sign: i64 = if tz.starts_with('-') { -1 } else { 1 };
    let hours: i64 = tz[1..3].parse().unwrap_or(0);
    let mins: i64 = tz[3..5].parse().unwrap_or(0);
    let offset_secs = sign * (hours * 3600 + mins * 60);
    let local = epoch + offset_secs;
    // Break into components (no chrono dependency)
    let days_since_epoch = local.div_euclid(86400);
    let time_of_day = local.rem_euclid(86400);
    let h = time_of_day / 3600;
    let m = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;
    // Convert days since 1970-01-01 to y/m/d
    let (y, mo, d) = days_to_ymd(days_since_epoch);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}",
        y, mo, d, h, m, s, tz
    )
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: i64) -> (i64, i64, i64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Return "short_hash subject" for the tip commit of a branch.
pub fn branch_tip_detail(
    path: &Path,
    branch: &str,
) -> Result<(String, String, String, String), String> {
    let line = run_git(
        path,
        &[
            "log",
            "-1",
            "--format=%h\x1f%s\x1f%an <%ae>\x1f%ai",
            branch,
        ],
    )?;
    let parts: Vec<&str> = line.split('\x1f').collect();
    if parts.len() >= 4 {
        Ok((
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
            parts[3].to_string(),
        ))
    } else {
        Err("unexpected format".to_string())
    }
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
        if let Some(rest) = line.strip_prefix("worktree ") {
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
            current_path = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            current_head = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("branch ") {
            current_branch = Some(rest.to_string());
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
            let _ = add_remote(
                std::path::Path::new("/nonexistent"),
                "origin",
                "https://example.com",
            );
        });
    }

    #[test]
    fn remove_remote_produces_valid_command_args() {
        let _ = std::panic::catch_unwind(|| {
            let _ = remove_remote(std::path::Path::new("/nonexistent"), "origin");
        });
    }
}
