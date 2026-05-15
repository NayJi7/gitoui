---
title: Commit graph
description: The home screen — pre-rendered git log graph with inline image protocol.
---

The home screen is `git log --graph --all` rendered as pre-computed SVG/PNG
that ships inline through your terminal's image protocol (Kitty / iTerm2 /
Sixel). It auto-detects the protocol at startup; force one with `--protocol`.

## What's shown

Each row carries:

- **Graph cell** — branches, merges, the commit dot (orange for HEAD).
- **Commit message** with inline ref glyphs (branch / remote / tag / stash).
- **Author name** — coloured per the theme.
- **Short hash**.
- **Date** — format set by `core.option.date_time_format`.

The top row is a synthetic **Uncommitted Changes** entry. Drilling into it
opens the [Uncommitted view](/features/uncommitted/) for staging + commit.

## Search

`f` opens the search input. By default search is incremental, fuzzy off,
case-insensitive off. Inline toggles while the search is active:

- `s` — toggle case sensitivity.
- `z` — toggle fuzzy matching.
- `x` — toggle regex (after a search is applied).
- `Enter` — apply the query (status line shows "Match N of M").
- `n` / `N` — next / previous match.
- `Esc` — clear the query and exit search mode.

The search bar's toggle state persists per-session via `[core.search]` in
the config.

## 2-commit comparison

`Space` on a commit marks it (highlighted with a brand-coloured chip). Move
to another commit, press `Space` again → gitoui opens the **Compare** view
with the cumulative diff between the two endpoints (`git diff <a>..<b>`).
`Ctrl+click` is the mouse equivalent. `Esc` clears the mark.

## Lazy load

The initial run loads `core.option.initial_load_count` commits (1000 by
default). `]` (`load_more`) appends another `core.option.load_more_count`
to the list — repeat as many times as you need to walk back in history.

`-n N` on the CLI overrides the initial count for one run.

## Auto-refresh

A filesystem watcher on `.git/` keeps the list in sync with external git
operations (commits from another shell, fetch, branch switches…). The
graph re-renders when needed without you having to press `r`.
