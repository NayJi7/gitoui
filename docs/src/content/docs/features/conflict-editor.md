---
title: Conflict editor
description: 3-way merge conflict resolution — pick OURS / THEIRS / BOTH per hunk.
---

When a merge / rebase / cherry-pick leaves a file in a conflicted state,
gitoui detects the unmerged status and offers the **conflict editor** as
the `a` action on that file in the [Uncommitted view](/features/uncommitted/).

## What you see

The editor splits the file into:

- **Hunks** — each contiguous `<<<<<<<` / `=======` / `>>>>>>>` block from
  `git`'s conflict markers.
- **Surrounding context** — non-conflicting lines stay shown so you can see
  the resolution in place.

Each hunk shows OURS (your side) and THEIRS (incoming side) side by side
(or stacked, depending on `ui.common.conflict_view`).

## Per-hunk actions

| Press     | Action                                                              |
|-----------|---------------------------------------------------------------------|
| `o`       | Pick **OURS** for the focused hunk.                                 |
| `t`       | Pick **THEIRS**.                                                    |
| `b`       | Keep **BOTH** — OURS first, then THEIRS.                            |
| `Shift+B` | Keep **BOTH** — THEIRS first, then OURS.                            |
| `n` / `→` | Next hunk.                                                          |
| `p` / `←` | Previous hunk.                                                      |
| `↑↓`      | Scroll the file viewport without moving between hunks.              |

## Save

| Press     | Action                                                              |
|-----------|---------------------------------------------------------------------|
| `Enter`   | Write the resolution back to the file and `git add` it.             |
| `Esc`     | Cancel — file stays in its conflicted state, no writes.             |

`Enter` runs `git add <file>` on success so the file leaves the Unmerged
list — you can immediately continue the rebase / commit the merge from the
Uncommitted view.

## Layout

`ui.common.conflict_view`:

- `"two-pane"` (default) — OURS on the left, THEIRS on the right, the
  picker chip floats between.
- `"inline"` — OURS over THEIRS, scrolling vertically. Better on narrow
  terminals.

Both layouts use the same key bindings.

## When a conflict needs more than picking

The editor handles the common case (pick a side). For edits that don't fit
the OURS/THEIRS/BOTH model — manual blending, refactor-during-merge — drop
to your terminal editor (`Esc` exits without writing), edit the file
externally, save, then `git add <file>` from the Uncommitted view.
