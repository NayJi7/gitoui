//! Data layer for the interactive-rebase editor.
//!
//! Public API:
//! - [`RebaseAction`] — Pick / Reword / Edit / Squash / Fixup / Drop
//! - [`RebaseItem`] — one row in the rebase todo (action + commit + metadata)
//! - [`load_rebase_items`] — `git log --reverse base..HEAD` parser
//! - [`serialise_todo`] — render a todo file in git's syntax
//! - [`apply_rebase`] — orchestrate `git rebase -i base` with our prepared
//!   todo via `GIT_SEQUENCE_EDITOR` + handle reword messages via `GIT_EDITOR`
//!
//! Rebase order convention: `load_rebase_items` returns commits in
//! chronological order (oldest first) — that's the order git's rebase-todo
//! uses. The view shows them in the same order.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebaseAction {
    /// Keep the commit as-is.
    Pick,
    /// Keep the commit but stop to let the user rewrite the message.
    Reword,
    /// Pause after applying the commit so the user can `git commit --amend`.
    Edit,
    /// Combine with the previous commit, prompting for a merged message.
    Squash,
    /// Combine with the previous commit, keeping the previous message.
    Fixup,
    /// Remove the commit from the history.
    Drop,
}

impl RebaseAction {
    /// The keyword git expects in the rebase-todo file.
    pub fn keyword(&self) -> &'static str {
        match self {
            RebaseAction::Pick => "pick",
            RebaseAction::Reword => "reword",
            RebaseAction::Edit => "edit",
            RebaseAction::Squash => "squash",
            RebaseAction::Fixup => "fixup",
            RebaseAction::Drop => "drop",
        }
    }

    /// Title-case label displayed in the editor UI.
    pub fn label(&self) -> &'static str {
        match self {
            RebaseAction::Pick => "Pick",
            RebaseAction::Reword => "Reword",
            RebaseAction::Edit => "Edit",
            RebaseAction::Squash => "Squash",
            RebaseAction::Fixup => "Fixup",
            RebaseAction::Drop => "Drop",
        }
    }

    /// Human-readable explanation of what this action will do — shown under
    /// the commit row in the Inline layout and in the Result preview.
    pub fn description(&self) -> &'static str {
        match self {
            RebaseAction::Pick => "keeps this commit as-is",
            RebaseAction::Reword => "keeps the commit, lets you rewrite the message",
            RebaseAction::Edit => "pauses so you can amend the commit",
            RebaseAction::Squash => "merges into the previous commit (combined message)",
            RebaseAction::Fixup => "merges into the previous commit (keeps prev message)",
            RebaseAction::Drop => "removes from the history",
        }
    }

    /// Cycle forwards through the action list (used by ←→ in the picker).
    pub fn cycle_next(self) -> Self {
        use RebaseAction::*;
        match self {
            Pick => Reword,
            Reword => Edit,
            Edit => Squash,
            Squash => Fixup,
            Fixup => Drop,
            Drop => Pick,
        }
    }

    pub fn cycle_prev(self) -> Self {
        use RebaseAction::*;
        match self {
            Pick => Drop,
            Reword => Pick,
            Edit => Reword,
            Squash => Edit,
            Fixup => Squash,
            Drop => Fixup,
        }
    }

    /// Map a single keypress to the corresponding action. None when the
    /// key is not a valid action shortcut.
    pub fn from_key(c: char) -> Option<Self> {
        match c {
            'p' => Some(RebaseAction::Pick),
            'r' => Some(RebaseAction::Reword),
            'e' => Some(RebaseAction::Edit),
            's' => Some(RebaseAction::Squash),
            'f' => Some(RebaseAction::Fixup),
            'd' => Some(RebaseAction::Drop),
            _ => None,
        }
    }

    /// Map a scoped action name (as resolved from `[scope.rebase]`) to
    /// the corresponding enum variant. Stricter than `from_key` because
    /// it's only used in the keybind dispatch path — typos in the user's
    /// TOML can fail at config-load time later if we plumb validation
    /// through, but for now an unknown name silently returns None and
    /// the dispatcher falls through.
    pub fn from_action_name(name: &str) -> Option<Self> {
        match name {
            "pick" => Some(RebaseAction::Pick),
            "reword" => Some(RebaseAction::Reword),
            "edit" => Some(RebaseAction::Edit),
            "squash" => Some(RebaseAction::Squash),
            "fixup" => Some(RebaseAction::Fixup),
            "drop" => Some(RebaseAction::Drop),
            _ => None,
        }
    }
}

/// One row of the rebase plan. Carries enough metadata to render a rich
/// editor row without re-shelling out to git.
#[derive(Debug, Clone)]
pub struct RebaseItem {
    pub action: RebaseAction,
    /// Full commit SHA-1 (40 chars). We keep the full hash so the todo
    /// file matches what git expects byte-for-byte.
    pub commit_hash: String,
    /// First line of the commit message.
    pub subject: String,
    /// Author display name (`%an`).
    pub author: String,
    /// Relative date (`%cr`) — "2 days ago".
    pub date: String,
    /// New message to use when `action == Reword`. `None` means "use the
    /// commit's existing message"; the view's inline editor populates this
    /// when the user actually edits.
    pub new_message: Option<String>,
}

/// Run `git log --reverse <base>..HEAD` and parse the output into items,
/// each defaulting to `RebaseAction::Pick`.
///
/// `base_hash` is the commit that the user is rebasing onto — its children
/// (`base..HEAD`) are the ones that will be replayed. The list is returned
/// in chronological order (oldest first), which is the order git's
/// rebase-todo uses.
pub fn load_rebase_items(repo: &Path, base_hash: &str) -> Result<Vec<RebaseItem>, String> {
    // Field separator: \x1f (Unit Separator) — same convention as the
    // commit-list loader.
    let format = "%H\x1f%s\x1f%an\x1f%cr";
    let range = format!("{}..HEAD", base_hash);
    let out = Command::new("git")
        .current_dir(repo)
        .args([
            "log",
            "--reverse",
            "--no-merges",
            &format!("--format={}", format),
            &range,
        ])
        .output()
        .map_err(|e| format!("git log failed: {}", e))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut items = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\x1f').collect();
        if parts.len() != 4 {
            continue;
        }
        items.push(RebaseItem {
            action: RebaseAction::Pick,
            commit_hash: parts[0].to_string(),
            subject: parts[1].to_string(),
            author: parts[2].to_string(),
            date: parts[3].to_string(),
            new_message: None,
        });
    }
    Ok(items)
}

/// Render the rebase-todo contents in the syntax git expects:
///   `<action> <hash> <subject>\n`
/// Drops emit a leading `# ` comment so git ignores them entirely.
/// (`drop` is actually a valid keyword too; we use the comment form so the
/// line stays visible in `.git/rebase-merge/git-rebase-todo` if the user
/// inspects it during a paused rebase.)
pub fn serialise_todo(items: &[RebaseItem]) -> String {
    let mut out = String::new();
    for it in items {
        if it.action == RebaseAction::Drop {
            // Skip — equivalent to "drop <hash>" but doesn't leave a
            // confusing dropped-line message in the rebase output.
            continue;
        }
        out.push_str(it.action.keyword());
        out.push(' ');
        out.push_str(&it.commit_hash);
        out.push(' ');
        out.push_str(&it.subject);
        out.push('\n');
    }
    out
}

/// Result of an `apply_rebase` call.
#[derive(Debug)]
pub enum RebaseOutcome {
    /// Rebase finished without leaving the repo in a conflicted state.
    Clean,
    /// Rebase stopped — either because git hit a conflict, or because one
    /// of the steps was `Edit`/`Reword` and is waiting on the user. The
    /// working tree is in a `rebase in progress` state; the user can
    /// resolve / amend, then `git rebase --continue` (or use our editors).
    Paused(String),
    /// `.git/rebase-merge` or `.git/rebase-apply` already exists from a
    /// previous unfinished rebase — we refused to start a new one to
    /// avoid clobbering state. Caller should surface a clear "abort the
    /// previous rebase first" message.
    AlreadyInProgress,
}

/// True when a rebase-merge or rebase-apply directory exists. We surface
/// this state separately from a rebase we just ran because the user can't
/// fix it with `--continue` — they need to explicitly abort first.
pub fn rebase_in_progress(repo: &Path) -> bool {
    let git_dir = repo.join(".git");
    git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir()
}

/// Cheap stat-and-read for `.git/rebase-merge/stopped-sha` — returns the
/// full SHA git is paused on, or `None` if no rebase is paused (or the
/// file just doesn't exist, e.g. on `git rebase --skip`'s narrow window).
/// Used by the commit-list renderer to badge the paused row directly,
/// without the cost of `read_resume_state` (which parses todo + done +
/// runs `git status`).
pub fn read_stopped_sha(repo: &Path) -> Option<String> {
    let path = repo.join(".git").join("rebase-merge").join("stopped-sha");
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Resolve a revision expression like `HEAD^^` to a full SHA. Returns
/// `Err` when git can't resolve the ref (e.g. the commit has no
/// grandparent because its parent is the repo root).
fn rev_parse(repo: &Path, rev: &str) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--verify", "--end-of-options", rev])
        .output()
        .map_err(|e| format!("git rev-parse failed: {}", e))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Squash `target` into its parent — non-interactive shortcut that runs
/// the same rebase machinery as the interactive editor, but with a
/// pre-built `[pick parent, fixup target, pick …rest]` plan. Faster
/// than opening the editor when the user just wants to combine adjacent
/// commits (the 90% case).
///
/// Refuses gracefully if:
/// - target is the initial commit (no parent),
/// - target's parent IS the initial commit (rebase --root needed; not
///   wired up here yet),
/// - target is a merge commit (multiple parents — ambiguous semantics),
/// - another rebase is already in progress.
pub fn squash_with_parent(repo: &Path, target_hash: &str) -> Result<RebaseOutcome, String> {
    if rebase_in_progress(repo) {
        return Ok(RebaseOutcome::AlreadyInProgress);
    }
    // Reject merge commits — a fixup on a merge produces surprising
    // history; better to make the user use the interactive editor.
    let parents = Command::new("git")
        .current_dir(repo)
        .args(["rev-list", "--parents", "-n", "1", target_hash])
        .output()
        .map_err(|e| format!("git rev-list failed: {}", e))?;
    if !parents.status.success() {
        return Err(String::from_utf8_lossy(&parents.stderr).trim().to_string());
    }
    let parent_count = String::from_utf8_lossy(&parents.stdout)
        .split_whitespace()
        .count()
        .saturating_sub(1);
    if parent_count == 0 {
        return Err("Cannot squash the initial commit — it has no parent.".into());
    }
    if parent_count > 1 {
        return Err("Cannot squash a merge commit — use the interactive rebase instead.".into());
    }

    let grandparent = match rev_parse(repo, &format!("{}^^", target_hash)) {
        Ok(h) => h,
        Err(_) => {
            return Err("Cannot squash into the root commit yet — \
                 use the interactive rebase editor for this case."
                .into());
        }
    };
    let mut items = load_rebase_items(repo, &grandparent)?;
    let mut found = false;
    for it in &mut items {
        let hash = &it.commit_hash;
        if hash.starts_with(target_hash) || target_hash.starts_with(hash.as_str()) {
            it.action = RebaseAction::Fixup;
            found = true;
            break;
        }
    }
    if !found {
        return Err(format!(
            "Target {} not found in rebase range — refusing to squash.",
            target_hash.chars().take(7).collect::<String>()
        ));
    }
    apply_rebase(repo, &grandparent, &items)
}

/// `git rebase --abort` — caller verifies state first.
pub fn abort_rebase(repo: &Path) -> Result<(), String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["rebase", "--abort"])
        .output()
        .map_err(|e| format!("git rebase --abort failed to spawn: {}", e))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// One entry in the resume-view step list, used for both completed and
/// remaining steps. We only need to render them; no need to round-trip
/// through the full RebaseItem (which carries author/date metadata we
/// don't have during a paused rebase).
#[derive(Debug, Clone)]
pub struct ResumeStep {
    pub action: RebaseAction,
    pub short_hash: String,
    pub subject: String,
}

/// Snapshot of `.git/rebase-merge/*` for the resume view. `None` for any
/// field means the file was missing — we still show what we can.
#[derive(Debug, Clone)]
pub struct ResumeState {
    /// Lines already executed (from `done`).
    pub done: Vec<ResumeStep>,
    /// Lines still queued (from `git-rebase-todo`).
    pub remaining: Vec<ResumeStep>,
    /// The commit git stopped on, if recorded (`stopped-sha`).
    pub stopped_sha: Option<String>,
    /// First line of the working-tree conflict / status summary, when we
    /// can extract one. Helpful context above the action hints.
    pub headline: Option<String>,
}

/// Parse `.git/rebase-merge/{done,git-rebase-todo,stopped-sha}` and
/// summarise the working-tree state. Returns `None` when no rebase is in
/// progress (caller should already have checked `rebase_in_progress`).
pub fn read_resume_state(repo: &Path) -> Option<ResumeState> {
    let rm = repo.join(".git").join("rebase-merge");
    if !rm.is_dir() {
        return None;
    }
    let parse_todo = |contents: &str| -> Vec<ResumeStep> {
        contents
            .lines()
            .filter_map(|raw| {
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }
                let mut parts = line.splitn(3, ' ');
                let keyword = parts.next()?;
                let hash = parts.next()?;
                let subject = parts.next().unwrap_or("").to_string();
                let action = match keyword {
                    "pick" | "p" => RebaseAction::Pick,
                    "reword" | "r" => RebaseAction::Reword,
                    "edit" | "e" => RebaseAction::Edit,
                    "squash" | "s" => RebaseAction::Squash,
                    "fixup" | "f" => RebaseAction::Fixup,
                    "drop" | "d" => RebaseAction::Drop,
                    _ => return None,
                };
                Some(ResumeStep {
                    action,
                    short_hash: hash.chars().take(7).collect(),
                    subject,
                })
            })
            .collect()
    };
    let done = std::fs::read_to_string(rm.join("done"))
        .map(|s| parse_todo(&s))
        .unwrap_or_default();
    let remaining = std::fs::read_to_string(rm.join("git-rebase-todo"))
        .map(|s| parse_todo(&s))
        .unwrap_or_default();
    let stopped_sha = std::fs::read_to_string(rm.join("stopped-sha"))
        .ok()
        .map(|s| s.trim().to_string());

    // Try to extract a meaningful headline from `git status --short` —
    // the rebase-stopped state always shows unmerged paths or staged
    // changes that the user needs to address.
    let headline = Command::new("git")
        .current_dir(repo)
        .args(["status", "--short"])
        .output()
        .ok()
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s.lines().next().unwrap_or("").to_string())
            }
        });

    Some(ResumeState {
        done,
        remaining,
        stopped_sha,
        headline,
    })
}

/// `git rebase --continue` with `GIT_EDITOR=true` so any prompt
/// (e.g. paused on an Edit step that's been amended) just accepts the
/// current message instead of opening a real editor.
pub fn continue_rebase(repo: &Path) -> Result<RebaseOutcome, String> {
    let out = Command::new("git")
        .current_dir(repo)
        .env("GIT_EDITOR", "true")
        .args(["rebase", "--continue"])
        .output()
        .map_err(|e| format!("git rebase --continue failed to spawn: {}", e))?;
    if out.status.success() {
        Ok(RebaseOutcome::Clean)
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Ok(RebaseOutcome::Paused(msg))
    }
}

/// `git rebase --skip` — drops the current commit and resumes the rebase.
pub fn skip_rebase(repo: &Path) -> Result<RebaseOutcome, String> {
    let out = Command::new("git")
        .current_dir(repo)
        .env("GIT_EDITOR", "true")
        .args(["rebase", "--skip"])
        .output()
        .map_err(|e| format!("git rebase --skip failed to spawn: {}", e))?;
    if out.status.success() {
        Ok(RebaseOutcome::Clean)
    } else {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Ok(RebaseOutcome::Paused(msg))
    }
}

/// Drive `git rebase -i <base>` with our prepared todo + reword messages.
///
/// Implementation strategy:
/// 1. Write our serialised todo to a temp file.
/// 2. For each Reword that has a `new_message`, write it to a numbered
///    file. Git invokes `GIT_EDITOR` for each reword in todo order, so a
///    counter file lets the editor script pick the right message.
/// 3. Set `GIT_SEQUENCE_EDITOR` to a tiny script that overwrites git's
///    todo with ours, and `GIT_EDITOR` to one that feeds reword messages
///    and accepts everything else.
/// 4. Run `git rebase -i <base>` and inspect the exit code.
///
/// Temp files live in the repo's `.git/` directory so they get GC'd with
/// the rebase scratch space.
pub fn apply_rebase(
    repo: &Path,
    base_hash: &str,
    items: &[RebaseItem],
) -> Result<RebaseOutcome, String> {
    // Refuse to start if a previous rebase is still in progress — git would
    // exit 128 with a wall of text and we'd surface it as a confusing
    // "Rebase paused" notification. Return a distinct variant so the view
    // can prompt the user to abort first.
    if rebase_in_progress(repo) {
        return Ok(RebaseOutcome::AlreadyInProgress);
    }

    let git_dir = repo.join(".git");
    let scratch = git_dir.join("gitoui-rebase");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| format!("mkdir scratch: {}", e))?;

    // 1. Todo file
    let todo_path = scratch.join("todo");
    std::fs::write(&todo_path, serialise_todo(items)).map_err(|e| format!("write todo: {}", e))?;

    // 2. Index reword messages by their position among "prompting" actions.
    //
    //   git invokes GIT_EDITOR for COMMIT_EDITMSG once per Reword AND once
    //   per Squash in todo order. Picking the next-reword message by a
    //   simple counter would feed it into a Squash prompt (which combines
    //   messages and also invokes the editor) and shift every following
    //   Reword by one slot. We instead key files by the prompt position
    //   so Squash invocations naturally fall through to git's default
    //   combined-message behaviour without consuming a reword slot.
    let mut prompt_idx = 0usize;
    for it in items {
        match it.action {
            RebaseAction::Reword => {
                prompt_idx += 1;
                if let Some(msg) = &it.new_message {
                    std::fs::write(scratch.join(format!("prompt_{}.txt", prompt_idx)), msg)
                        .map_err(|e| format!("write reword msg: {}", e))?;
                }
            }
            RebaseAction::Squash => {
                prompt_idx += 1;
                // No file written — git's default combined message is used.
            }
            _ => {}
        }
    }
    std::fs::write(scratch.join("prompt_counter"), "1")
        .map_err(|e| format!("write counter: {}", e))?;

    // 3. Editor script — handles both GIT_SEQUENCE_EDITOR and GIT_EDITOR
    //    invocations by inspecting the filename it's asked to edit.
    let editor_path = scratch.join("editor.sh");
    let editor_src = format!(
        r#"#!/bin/bash
# gitoui interactive-rebase editor hook.
# - git-rebase-todo: overwrite with our prepared list.
# - COMMIT_EDITMSG  : if a prompt_<N>.txt was prepared for this
#                     position, feed it; otherwise leave git's
#                     default (original message for Reword without
#                     edits, combined message for Squash).
file="$1"
name=$(basename "$file")
scratch={scratch}
case "$name" in
  git-rebase-todo)
    cp "$scratch/todo" "$file"
    ;;
  COMMIT_EDITMSG)
    idx=$(cat "$scratch/prompt_counter" 2>/dev/null || echo 1)
    msg="$scratch/prompt_${{idx}}.txt"
    if [ -f "$msg" ]; then
      cp "$msg" "$file"
    fi
    echo $((idx + 1)) > "$scratch/prompt_counter"
    ;;
esac
exit 0
"#,
        scratch = scratch.display()
    );
    std::fs::write(&editor_path, editor_src).map_err(|e| format!("write editor: {}", e))?;
    let mut perms = std::fs::metadata(&editor_path)
        .map_err(|e| format!("stat editor: {}", e))?
        .permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    std::fs::set_permissions(&editor_path, perms).map_err(|e| format!("chmod editor: {}", e))?;

    // 4. Run rebase. We pipe stderr/stdout so the caller can surface them.
    let out = Command::new("git")
        .current_dir(repo)
        .env("GIT_SEQUENCE_EDITOR", &editor_path)
        .env("GIT_EDITOR", &editor_path)
        .args(["rebase", "-i", base_hash])
        .output()
        .map_err(|e| format!("git rebase failed to spawn: {}", e))?;

    // Cleanup our scratch dir — the rebase machinery has its own state
    // in .git/rebase-merge if it's still running.
    let _ = std::fs::remove_dir_all(&scratch);

    if out.status.success() {
        Ok(RebaseOutcome::Clean)
    } else {
        // A non-zero exit means git stopped — could be a conflict, an
        // edit step, or a real error. Either way, surface stderr.
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Ok(RebaseOutcome::Paused(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(hash: &str, subject: &str) -> RebaseItem {
        RebaseItem {
            action: RebaseAction::Pick,
            commit_hash: hash.to_string(),
            subject: subject.to_string(),
            author: "tester".to_string(),
            date: "1h ago".to_string(),
            new_message: None,
        }
    }

    #[test]
    fn keyword_round_trip() {
        for a in [
            RebaseAction::Pick,
            RebaseAction::Reword,
            RebaseAction::Edit,
            RebaseAction::Squash,
            RebaseAction::Fixup,
            RebaseAction::Drop,
        ] {
            // Keyword stays lowercase
            assert!(a.keyword().chars().all(|c| !c.is_uppercase()));
            // Label is title case
            assert!(a.label().chars().next().unwrap().is_uppercase());
        }
    }

    #[test]
    fn cycle_next_full_loop() {
        let mut a = RebaseAction::Pick;
        for _ in 0..6 {
            a = a.cycle_next();
        }
        assert_eq!(a, RebaseAction::Pick);
    }

    #[test]
    fn cycle_prev_is_inverse_of_next() {
        for a in [
            RebaseAction::Pick,
            RebaseAction::Reword,
            RebaseAction::Edit,
            RebaseAction::Squash,
            RebaseAction::Fixup,
            RebaseAction::Drop,
        ] {
            assert_eq!(a.cycle_next().cycle_prev(), a);
            assert_eq!(a.cycle_prev().cycle_next(), a);
        }
    }

    #[test]
    fn from_key_covers_all_actions() {
        assert_eq!(RebaseAction::from_key('p'), Some(RebaseAction::Pick));
        assert_eq!(RebaseAction::from_key('r'), Some(RebaseAction::Reword));
        assert_eq!(RebaseAction::from_key('e'), Some(RebaseAction::Edit));
        assert_eq!(RebaseAction::from_key('s'), Some(RebaseAction::Squash));
        assert_eq!(RebaseAction::from_key('f'), Some(RebaseAction::Fixup));
        assert_eq!(RebaseAction::from_key('d'), Some(RebaseAction::Drop));
        assert_eq!(RebaseAction::from_key('x'), None);
    }

    #[test]
    fn serialise_drops_skip_dropped_lines() {
        let mut items = vec![
            pick("aaa1111", "first"),
            pick("bbb2222", "second"),
            pick("ccc3333", "third"),
        ];
        items[1].action = RebaseAction::Drop;
        let out = serialise_todo(&items);
        assert!(out.contains("pick aaa1111 first"));
        assert!(!out.contains("bbb2222"));
        assert!(out.contains("pick ccc3333 third"));
    }

    #[test]
    fn serialise_renders_each_action_keyword() {
        let mut items = vec![
            pick("aaa", "a"),
            pick("bbb", "b"),
            pick("ccc", "c"),
            pick("ddd", "d"),
            pick("eee", "e"),
        ];
        items[0].action = RebaseAction::Pick;
        items[1].action = RebaseAction::Reword;
        items[2].action = RebaseAction::Edit;
        items[3].action = RebaseAction::Squash;
        items[4].action = RebaseAction::Fixup;
        let out = serialise_todo(&items);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].starts_with("pick "));
        assert!(lines[1].starts_with("reword "));
        assert!(lines[2].starts_with("edit "));
        assert!(lines[3].starts_with("squash "));
        assert!(lines[4].starts_with("fixup "));
    }

    #[test]
    fn serialise_empty_input_returns_empty_string() {
        assert_eq!(serialise_todo(&[]), "");
    }
}
