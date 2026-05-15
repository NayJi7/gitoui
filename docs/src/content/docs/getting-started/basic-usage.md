---
title: Basic usage
description: Open gitoui in a git repo and find your way around the main views.
---

import { Aside } from "@astrojs/starlight/components";

Run `gitoui` from inside any git repository:

```sh
cd ~/my/repo
gitoui
```

You land on the **commit list** — your home screen. The commit graph is on the
left, then commit message, author, hash, and date. Use arrows or `j`/`k` to
move. Each shortcut shown below is reconfigurable — see the
[Keybindings page](/keybindings/).

## The five canonical moves

| Press           | Goes to                                                |
|-----------------|--------------------------------------------------------|
| `Enter`         | Commit detail (or **Uncommitted** on the top row).     |
| `Tab`           | Refs panel (branches, tags, stashes, remotes).         |
| `f`             | Search — fuzzy by default, toggle case/regex inline.   |
| `?` or `F1`     | Help — the live reference of every shortcut.           |
| `p`             | Configuration — themes, modes, GitHub auth.            |

Hit `Esc` (or `q` in the list view) to back out of any view.

## Common workflows

### Review a commit + open its diff

`Enter` on the commit → opens **Commit Detail**. The right panel lists every
action (Add Tag, Blame, Create Branch, Checkout, Cherry Pick, Revert, Drop,
Merge into current, Rebase current on, Reset, Squash, Amend). The left panel
shows author, date, hash, refs, message, and a file-change summary.

`Enter` on a file in that summary opens the **Diff** view (side-by-side mode
configurable). `+` and `-` cycle to the next/previous file in the commit.

### Stage and commit

From the commit list, `Enter` on the top row (Uncommitted Changes) opens the
**Uncommitted** view: Unstaged, Staged, and Untracked panels side by side.

| Press   | Action                                                       |
|---------|--------------------------------------------------------------|
| `a`     | Stage the focused file (or open the **3-way conflict editor** when the file is in conflict). |
| `u`     | Unstage from the Staged section.                             |
| `x`     | Discard an unstaged change.                                  |
| `w`     | Open the commit message editor.                              |
| `i`     | Stash everything.                                            |

### 2-commit comparison

`Space` on a commit marks it. Move to a second commit and press `Space`
again — gitoui opens the cumulative diff between the two endpoints.
`Esc` clears the mark.

### Interactive rebase

`e` on a non-HEAD commit opens the **rebase plan editor**. Use the per-row
shortcuts (`p` pick, `r` reword, `e` edit, `s` squash, `f` fixup, `d` drop)
to set actions, `Space` to grab and reorder rows with arrows, `Enter` to
apply. If the rebase pauses (conflict, edit), gitoui detects it on the next
open and shows a resume banner with `c` continue / `s` skip / `a` abort.

### Open a Pull Request

`Shift+R` (anywhere) opens the **PR list**. `Enter` drills into a PR — the
Conversation / Files / Commits / References tabs work like GitHub's web UI.
`a` approve, `m` merge, `c` new comment, `+` react, `Shift+R` quote-reply.

Same flow for **Issues** with `Shift+I`.

<Aside type="tip">
The Help page is the live reference: every shortcut shown there matches your
current config, so any rebind is reflected automatically.
</Aside>

## Where things live

| Path                                      | What                                          |
|-------------------------------------------|-----------------------------------------------|
| `$GITOUI_CONFIG_FILE` or `$XDG_CONFIG_HOME/gitoui/config.toml` (defaults to `~/.config/gitoui/config.toml`) | Main config. |
| `~/.config/gitoui/themes/<name>.toml`     | Custom color themes — see [Themes](/configurations/themes/). |
| `~/.config/gitoui/github_token.toml`      | OAuth token (mode `0600`, never commit).      |
| `~/.cache/gitoui/avatars/`                | Cached GitHub avatar PNGs.                    |
