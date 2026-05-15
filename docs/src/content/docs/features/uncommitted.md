---
title: Uncommitted & staging
description: Stage hunks, stash, commit — without leaving gitoui.
---

`Enter` on the top row of the commit list opens the **Uncommitted** view.
Three side-by-side panels:

| Panel        | What                                              |
|--------------|---------------------------------------------------|
| **Unstaged** | Modified / deleted files in the working tree.     |
| **Staged**   | Files in the index ready to be committed.         |
| **Untracked**| New files not yet tracked.                        |

`← / →` switch between panels, `↑ / ↓` move the cursor within a panel.

## File-level operations

| Press     | Action                                                                 |
|-----------|------------------------------------------------------------------------|
| `a`       | Stage the focused file (or open the conflict editor for unmerged files). |
| `Shift+A` | Stage every unstaged + untracked file.                                 |
| `u`       | Unstage the focused file (Staged panel).                               |
| `Shift+U` | Unstage every staged file.                                             |
| `x`       | Discard an unstaged change (with confirmation).                        |
| `Shift+X` | Discard every unstaged + staged change.                                |
| `v`       | Clean (delete) an untracked file.                                      |

## Open the diff with hunk-level staging

`Enter` on a file opens its diff. From there:

- The diff is **interactive**: pick a hunk, press `Enter` to stage or
  unstage only that hunk (modes the way `git add -p` does).
- Hit `r` to refresh the panel state.

## Commit, stash, branch

| Press   | Action                                                                                 |
|---------|----------------------------------------------------------------------------------------|
| `w`     | Open the **commit message editor** — multi-line, supports `Ctrl+H` / `Ctrl+W` word-delete and `Ctrl+Enter` / `Ctrl+S` submit. |
| `i`     | Stash the working tree (with optional message + include-untracked flag dialog).        |

## Conflict editor

Unmerged files (the result of a paused merge, rebase or cherry-pick) show a
`⚠ N conflict(s)` badge on the row. `a` on such a file opens the
[3-way conflict editor](/features/conflict-editor/) — pick OURS / THEIRS /
BOTH per hunk and save.

## Footer hints

The footer dynamically shows only the shortcuts that apply to the current
state — `stage-all` only appears when something is unstaged,
`unstage-all` only when something is staged, etc. Same goes for the
`H:history` chip (off when the focused file row is empty).
