---
title: Pull Requests
description: Review and act on PRs from inside gitoui — list, detail, conversation, compose.
---

import { Aside } from "@astrojs/starlight/components";

`Shift+R` (anywhere in the app) opens the **PR list**. Needs the repo to
have a GitHub remote and you to be logged in via the Configuration view's
GitHub Auth section.

## PR list

| Column   | What                                            |
|----------|-------------------------------------------------|
| #N       | PR number (orange).                             |
| Title    | With label chips and head→base branch arrow.    |
| Author   | Login + avatar (when avatars are enabled).      |
| Status   | Open / Merged / Closed + draft indicator.       |

| Press        | Action                                              |
|--------------|-----------------------------------------------------|
| `↑↓ / j / k` | Move cursor.                                        |
| `Enter`      | Open PR detail.                                     |
| `← / →`      | Cycle filter: Open / Merged / Closed / All.         |
| `n`          | Compose a new PR (`new_pr` in scope.pr.list).       |
| `r`          | Reload (`reload` in scope.pr).                      |
| `Esc`        | Close PR view.                                      |

## PR detail

Four tabs across the top: **Conversation**, **Files**, **Commits**,
**References**. Switch with `← / →`. The right edge of the tab bar shows
state-action shortcuts (`Ctrl+X close`, `Ctrl+D to draft`, etc.) that
follow the current PR state.

### PR-wide actions (any tab)

| Press            | Action               |
|------------------|----------------------|
| `a`              | Approve review.      |
| `x`              | Request changes.     |
| `m`              | Merge.               |
| `o`              | Open in browser.     |
| `l`              | Labels picker.       |
| `v`              | Reviewers picker.    |
| `Ctrl+X`         | Close PR.            |
| `Ctrl+O`         | Reopen.              |
| `Ctrl+D`         | Toggle draft.        |
| `r`              | Reload from API.     |

Confirmation dialogs open for destructive actions (close, merge,
toggle-draft, delete-comment).

### Conversation tab

Shows the PR description card, then every comment, review, review-comment,
and timeline event in chronological order. The selected card has a
**top-border ribbon** with card-local actions:

| Press     | Action                                                |
|-----------|-------------------------------------------------------|
| `c`       | New comment (opens the comment editor overlay).       |
| `Shift+R` | Quote-reply to the focused comment.                   |
| `e`       | Edit your own comment.                                |
| `d`       | Delete your own comment (confirmation dialog).        |
| `+`       | React (emoji picker — toggle on/off, your reactions highlighted). |
| `↵`       | Open the first `#N` reference inside the focused card. |

`#N` mentions are styled inline and clickable. The picker is mouse-aware.

### Files tab

Lists every file touched by the PR with +/− line counts. `Enter` drills
into the file's diff (same view as commit-detail diffs).

### Commits tab

Shows the commits in the PR. `Enter` drills into a commit's detail view.

### References tab

Lists `#N` references found in the PR body and in comments, plus reverse
references (other issues/PRs that mention this one).

## Compose

`n` from the PR list opens the **compose form** with fields for Title,
Body, Base branch, Head branch, Labels, Reviewers, Draft. Submit with
`Ctrl+S` (rebindable via `[keybind.scope.compose] submit`).

The body field accepts markdown and supports:

- `#` opens the **mention popup** — typing `123` filters to issues/PRs
  with that number; `↑↓` to pick; `Enter` to insert as `#123`.
- `Ctrl+H` / `Ctrl+W` / `Ctrl+Backspace` — delete word left.
- `Ctrl+Left` / `Ctrl+Right` — jump by word.

The right column shows a live preview of the rendered body.

<Aside type="note">
The OAuth token used by these features is stored at
`~/.config/gitoui/github_token.toml` with `0600` permissions. Never commit
this file — it's user-only and reissued via the Configuration view if you
need to rotate.
</Aside>
