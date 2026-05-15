---
title: Introduction
description: What gitoui is, what it isn't, and why it forked from serie.
---

**gitoui** ([`/ʒi.tu.i/`](/faq/#how-do-i-pronounce-gitoui)) is a terminal git
client. It started as a fork of [serie](https://github.com/lusingander/serie) —
a beautifully-rendered `git log --graph` viewer — and grew into a daily-driver
TUI that covers diffs, staging, blame, history, interactive rebase, conflict
resolution, and GitHub Pull Requests / Issues.

![demo](https://raw.githubusercontent.com/NayJi7/gitoui/master/assets/readme/demo.gif)

## Why a fork?

Serie does one thing exceptionally well: it pre-renders the commit graph as
SVG/PNG and displays it inline through the terminal's image protocol
(Kitty, iTerm2, Sixel). That graph is the prettiest in any terminal client.

gitoui keeps that. What it adds:

- **Side-by-side and inline diffs** with hunk-level staging in the uncommitted
  view.
- **Blame** and **file history** (`git log --follow`).
- **2-commit comparison** — mark two commits with `Space`, get the cumulative
  diff.
- **Interactive rebase** — visual plan editor with grab-and-drop reordering,
  reword in place, plus a resume mode for paused rebases.
- **3-way conflict editor** — pick OURS / THEIRS / both per hunk, save back.
- **GitHub PRs and Issues** — full conversation threads, review actions,
  labels/assignees/milestones, mention autocomplete, reactions, comment CRUD,
  open in browser. Behind OAuth device-flow auth.
- **GitHub avatars** rendered inline next to commits, comments, and review
  events — same image protocols as the graph.
- **Two-layer keybinds** — global `[keybind]` UserEvents plus per-view
  `[keybind.scope.<path>]` overrides, with a max-one-modifier policy and
  diagnostic-styled config errors at boot.
- **User-defined themes** — drop a `.toml` in `~/.config/gitoui/themes/`,
  inherit from any built-in, override individual tokens.

## What gitoui is not

- A full-featured git client — there's no remote management UI, no submodule
  workflow, no LFS handling. The focus stays on **reading + reviewing
  history** with the operations you do daily on top of it.
- A pure ASCII renderer — the inline image protocol is the point. On terminals
  that can't speak Kitty / iTerm2 / Sixel, gitoui falls back to Unicode
  glyphs but you lose the polished graph.

## Project status

gitoui is in pre-public development. It works end-to-end and is being daily-
driven on real repos, but the API and config schema may still shift before
the v1.0 tag.

If you came here from serie and want the bare graph viewer with nothing
else: that's still the upstream project — go give it a star. gitoui is the
"with batteries" cousin.

---

_Built with Rust and [ratatui](https://github.com/ratatui/ratatui). Released
under the MIT license — see the
[GitHub repo](https://github.com/NayJi7/gitoui) for sources._
