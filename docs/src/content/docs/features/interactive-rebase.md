---
title: Interactive rebase
description: Visual plan editor for git rebase -i, plus a resume mode for paused rebases.
---

import { Aside } from "@astrojs/starlight/components";

`e` on a non-HEAD commit (from the commit list or detail view) opens the
**Interactive rebase plan editor**. Same semantics as `git rebase -i
<commit>~1`, but visual — and with an in-place reword editor.

## The plan editor

The view lists every commit from the selected one up to HEAD, with the
default action `pick` next to each row.

| Press                | Action                                                                                       |
|----------------------|----------------------------------------------------------------------------------------------|
| `p`                  | **Pick** — keep the commit.                                                                  |
| `r`                  | **Reword** — keep the commit; open the inline subject editor.                                |
| `e`                  | **Edit** — pause the rebase at this commit so you can amend.                                 |
| `s`                  | **Squash** — combine into the previous commit, keep both messages.                           |
| `f`                  | **Fixup** — combine into the previous commit, drop this message.                             |
| `d`                  | **Drop** — remove the commit.                                                                |
| `Space`              | **Grab** the row — `↑↓` now reorders the commit instead of moving the cursor; `Space` again or `Enter` drops the grab. |
| `↑ / ↓`              | Move cursor (or move grabbed row).                                                           |
| `← / →`              | Cycle the action of the focused row.                                                         |
| `Enter`              | **Apply** the plan — gitoui writes it to `.git/rebase-merge/git-rebase-todo` and runs `git rebase -i`. |
| `Esc`                | Cancel (releases the grab first if one is active).                                           |

## Reword inline editor

Hitting `r` on a commit, or moving onto a row already set to `reword`,
opens the inline subject editor. You type the new message on the row
itself.

| Press                                    | Action                          |
|------------------------------------------|---------------------------------|
| `Enter`                                  | Commit the reword.              |
| `Esc`                                    | Cancel.                         |
| `Ctrl+H` / `Ctrl+W` / `Ctrl+Backspace`   | Delete word to the left.        |
| `Ctrl+Delete`                            | Delete word to the right.       |
| `Ctrl+Left` / `Ctrl+Right`               | Jump cursor by word.            |

## Resume mode

If your rebase pauses (conflict, `edit` action, etc.), gitoui detects the
`.git/rebase-merge/` directory on the next launch and shows a **resume
banner** at the top of the rebase view.

| Press               | Action                                                                |
|---------------------|-----------------------------------------------------------------------|
| `c` / `Shift+C`     | Continue (`git rebase --continue`).                                   |
| `s` / `Shift+S`     | Skip current commit (`git rebase --skip`).                            |
| `a` / `Shift+A`     | Abort (`git rebase --abort`).                                         |
| `Esc`               | Close the resume view without acting (rebase stays paused).           |

The banner re-opens automatically every time you launch gitoui until the
rebase is resolved.

## Layout modes

`ui.common.rebase_view` controls how dense the plan view is:

- `"inline"` (default) — actions sit next to commit messages.
- `"compact"` — single line per commit, fewer columns.
- `"split"` — actions in a separate left column, message on the right.

<Aside type="caution">
Like `git rebase -i`, the plan editor rewrites history. Make sure you have
either an unrelated branch or a saved tag if you want a recovery point —
gitoui doesn't auto-tag for you.
</Aside>
