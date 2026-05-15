---
title: Custom keybindings
description: Full per-scope reference + worked examples for overriding gitoui shortcuts.
---

import { Aside } from "@astrojs/starlight/components";

This page lists every rebindable action — global UserEvents first, then
each per-view scope.

## Key format

Each key is a string. Format: `[modifier-]base`. Examples: `q`, `ctrl-s`,
`alt-shift`, `shift-h`, `F5`, `enter`, `esc`, `space`, `pageup`, `tab`.

| Base key      | Accepted spellings                              |
|---------------|-------------------------------------------------|
| Letters       | `a`, `b`, … `z` (case-insensitive — `shift-a` and `A` are the same event). |
| Digits        | `0`–`9`                                         |
| Function      | `f1` … `f12`                                    |
| Arrows        | `up`, `down`, `left`, `right`                   |
| Whitespace    | `space`, `tab`, `backtab`                       |
| Editing       | `enter`, `esc`, `backspace`, `delete`, `insert` |
| Page nav      | `home`, `end`, `pageup`, `pagedown`             |
| Punctuation   | The character itself: `]`, `+`, `#`, `=`, `-`   |

Modifiers: `ctrl-`, `alt-`, `shift-`. **Max one** modifier prefix per key
(see [Overview — Modifier cap](/keybindings/#modifier-cap)).

## Global section — `[keybind]`

The full list of UserEvents you can bind. Defaults shown to the right.

### App-wide

| Action          | Default      |
|-----------------|--------------|
| `force_quit`    | `ctrl-c`     |
| `quit`          | `q`          |
| `help_toggle`   | `?`, `f1`    |
| `cancel`        | `esc`        |
| `close`         | (unbound)    |
| `config`        | `p`          |
| `pull_requests` | `shift-r`    |
| `issues`        | `shift-i`    |

### Navigation

| Action            | Default                |
|-------------------|------------------------|
| `navigate_up`     | `k`, `up`              |
| `navigate_down`   | `j`, `down`            |
| `navigate_left`   | `h`, `left`            |
| `navigate_right`  | `l`, `right`           |
| `go_to_parent`    | `alt-j`, `alt-down`    |
| `select_up`       | `shift-k`              |
| `select_down`     | `shift-j`              |
| `go_to_top`       | `g`                    |
| `go_to_bottom`    | `shift-g`              |
| `scroll_up`       | `ctrl-y`               |
| `scroll_down`     | `ctrl-e`               |
| `page_up`         | `ctrl-b`, `pageup`     |
| `page_down`       | `ctrl-f`, `pagedown`   |
| `half_page_up`    | `ctrl-u`               |
| `half_page_down`  | `ctrl-d`               |
| `select_middle`   | `shift-m`              |
| `select_bottom`   | `shift-l`              |
| `confirm`         | `enter`                |
| `go_to_next`      | `n`                    |
| `go_to_previous`  | `shift-n`              |

### List / search

| Action               | Default      |
|----------------------|--------------|
| `search`             | `f`          |
| `ref_list`           | `tab`        |
| `ignore_case_toggle` | `s`          |
| `fuzzy_toggle`       | `z`          |
| `refresh`            | `r`          |
| `load_more`          | `]`          |
| `mark_compare`       | `space`      |
| `push`               | `shift-p`    |
| `pull`               | `shift-u`    |
| `short_copy`         | `shift-c`    |
| `full_copy`          | `c`          |

### Commit / detail actions

| Action          | Default     |
|-----------------|-------------|
| `add_tag`       | `t`         |
| `create_branch` | `shift-b`   |
| `blame`         | `b`         |
| `checkout`      | `o`         |
| `cherry_pick`   | `shift-o`   |
| `revert`        | `v`         |
| `drop`          | `d`         |
| `merge`         | `m`         |
| `rebase`        | `e`         |
| `reset`         | `shift-s`   |
| `squash`        | `ctrl-s`    |
| `amend_commit`  | `ctrl-m`    |
| `file_history`  | `shift-h`   |
| `abort_operation` | `ctrl-a`  |

### Branch + tag actions

| Action            | Default     |
|-------------------|-------------|
| `rename_branch`   | `ctrl-r`    |
| `delete_branch`   | `shift-d`   |
| `push_branch`     | `shift-q`   |
| `set_upstream`    | `ctrl-t`    |
| `create_archive`  | `shift-e`   |
| `unselect_branch` | `shift-t`   |
| `copy_branch_name`| `shift-v`   |
| `delete_tag`      | `shift-f`   |
| `push_tag`        | `shift-w`   |
| `copy_tag_name`   | `shift-y`   |

### Uncommitted (staging)

| Action            | Default     |
|-------------------|-------------|
| `stage`           | `a`         |
| `stage_all`       | `shift-a`   |
| `unstage`         | `u`         |
| `discard`         | `x`         |
| `discard_all`     | `shift-x`   |
| `stash`           | `i`         |
| `commit`          | `w`         |
| `clean_untracked` | `v`         |

### Stash actions

| Action                       | Default     |
|------------------------------|-------------|
| `apply_stash`                | `y`         |
| `pop_stash`                  | `ctrl-p`    |
| `drop_stash`                 | `ctrl-x`    |
| `create_branch_from_stash`   | `ctrl-n`    |
| `copy_stash_name`            | `ctrl-i`    |
| `copy_stash_hash`            | `ctrl-o`    |

### Diff

| Action            | Default |
|-------------------|---------|
| `cycle_file_next` | `+`     |
| `cycle_file_prev` | `-`     |

## Scoped sections

### `[keybind.scope.list]` — Commit list

| Action               | Default | Effect                                                   |
|----------------------|---------|----------------------------------------------------------|
| `select_head_commit` | `h`     | Scroll to and center the HEAD commit in the viewport.    |

`h` is `navigate_left` globally; in the list view it has no left/right
semantics, so the scope repurposes it for "jump to HEAD". The Help page
documents both behaviours.

### `[keybind.scope.pr]` — Pull Request (any tab)

| Action              | Default     |
|---------------------|-------------|
| `reload`            | `r`         |
| `approve`           | `a`         |
| `request_changes`   | `x`         |
| `merge`             | `m`         |
| `open_in_browser`   | `o`         |
| `labels_picker`     | `l`         |
| `reviewers_picker`  | `v`         |
| `close_pr`          | `ctrl-x`    |
| `reopen_pr`         | `ctrl-o`    |
| `toggle_draft`      | `ctrl-d`    |

### `[keybind.scope.pr.list]`

| Action   | Default |
|----------|---------|
| `new_pr` | `n`     |

### `[keybind.scope.pr.conversation]`

| Action        | Default     |
|---------------|-------------|
| `new_comment` | `c`         |
| `quote_reply` | `shift-r`   |
| `edit_own`    | `e`         |
| `delete_own`  | `d`         |
| `react`       | `+`         |

### `[keybind.scope.issues]` — Issue (any tab)

| Action             | Default   |
|--------------------|-----------|
| `reload`           | `r`       |
| `labels_picker`    | `l`       |
| `assignees_picker` | `a`       |
| `milestone_picker` | `m`       |
| `close_or_reopen`  | `x`       |
| `open_in_browser`  | `o`       |

### `[keybind.scope.issues.list]`

| Action      | Default |
|-------------|---------|
| `new_issue` | `n`     |

### `[keybind.scope.issues.detail]`

| Action                   | Default     |
|--------------------------|-------------|
| `new_comment`            | `c`         |
| `quote_reply`            | `shift-r`   |
| `reference_in_new_issue` | `shift-n`   |
| `edit_own`               | `e`         |
| `delete_own`             | `d`         |
| `react`                  | `+`         |

### `[keybind.scope.rebase]` — Interactive rebase plan editor

| Action           | Default     |
|------------------|-------------|
| `pick`           | `p`         |
| `reword`         | `r`         |
| `edit`           | `e`         |
| `squash`         | `s`         |
| `fixup`          | `f`         |
| `drop`           | `d`         |
| `grab`           | `space`     |
| `abort_previous` | `shift-a`   |

### `[keybind.scope.rebase.resume]`

| Action            | Default              |
|-------------------|----------------------|
| `continue_rebase` | `c`, `shift-c`       |
| `skip_commit`     | `s`, `shift-s`       |
| `abort`           | `a`, `shift-a`       |

### `[keybind.scope.rebase.reword_editor]`

Word-jump shortcuts inside the inline subject editor.

| Action              | Default                                |
|---------------------|----------------------------------------|
| `delete_word_left`  | `ctrl-h`, `ctrl-w`, `ctrl-backspace`   |
| `delete_word_right` | `ctrl-delete`                          |
| `word_left`         | `ctrl-left`                            |
| `word_right`        | `ctrl-right`                           |

### `[keybind.scope.conflict]` — 3-way conflict editor

| Action                   | Default   |
|--------------------------|-----------|
| `pick_ours`              | `o`       |
| `pick_theirs`            | `t`       |
| `pick_both_ours_first`   | `b`       |
| `pick_both_theirs_first` | `shift-b` |
| `next_hunk`              | `n`       |
| `prev_hunk`              | `p`       |

### `[keybind.scope.compose]` — Compose forms

Shared by new PR, new Issue, new comment, edit / reply editors.

| Action   | Default  |
|----------|----------|
| `submit` | `ctrl-s` |

The mention popup (`#`), word-jump editing keys, and Esc to cancel stay
hardcoded — they're shared text-input semantics, not view actions.

## Worked examples

### Vim-style: keep h/j/k/l, free up letters for actions

```toml
[keybind]
# Move scrolling off the home row so h/j/k/l stay pure nav.
half_page_up   = ["ctrl-u"]
half_page_down = ["ctrl-d"]

[keybind.scope.list]
# Free `h` if you don't want the HEAD-jump action.
# Just don't bind anything — the global `navigate_left` (still `h`) wins.
select_head_commit = []
```

### Function-key power user

Map daily actions to F-keys for IDE-like muscle memory.

```toml
[keybind]
refresh      = ["F5"]
search       = ["F4"]
help_toggle  = ["F1"]
config       = ["F2"]

[keybind.scope.pr]
approve         = ["F6"]
request_changes = ["F7"]
merge           = ["F8"]
reload          = ["F5"]
```

### Match GitHub web shortcuts

```toml
[keybind.scope.pr.conversation]
new_comment   = ["c"]      # same as web
react         = ["r"]      # web uses `:` but we keep r
quote_reply   = ["shift-r"]
edit_own      = ["ctrl-e"]
delete_own    = ["ctrl-d"]
```

### Unbind defaults

Pass an empty array to drop a default key entirely:

```toml
[keybind]
go_to_parent = []  # no key fires GoToParent
```

<Aside type="caution">
Unbinding a navigation event like `cancel` (default `Esc`) will break exit
paths from many views. Test rebinds in a safe repo first.
</Aside>
