---
title: Themes
description: Built-in themes and how to ship your own as a TOML file.
---

import { Aside } from "@astrojs/starlight/components";

gitoui ships **10 built-in themes**, and lets you drop your own as a TOML
file under `~/.config/gitoui/themes/`. Both are addressed by name through
the `core.option.theme` config key.

## Built-in themes

| Name              | Vibe              |
|-------------------|-------------------|
| `Tokyo Night`     | Default — dark blue/purple. |
| `Dracula`         | Iconic purple/pink/green dark. |
| `Catppuccin Mocha`| Pastel dark.      |
| `Catppuccin Latte`| Pastel light.     |
| `Gruvbox Dark`    | Warm earthy dark. |
| `Nord`            | Cold Scandinavian. |
| `Solarized Dark`  | Classic dark.     |
| `Solarized Light` | Classic light.    |
| `One Dark`        | Atom-derived.     |
| `Monokai Pro`     | Vivid contrast.   |

Pick one from the Configuration view (`p` from the list, then ←/→ on the
"theme" row) or set it in `config.toml`:

```toml
[core.option]
theme = "Dracula"
```

Each named theme bundles a matching **syntax theme** (the syntect theme used
for diff highlighting) — that's why setting `theme = "Dracula"` also changes
the syntax highlighter to Dracula. You can override the syntax pick on its
own with `syntax_theme = "..."`.

## Custom themes — TOML format

Drop a file at `~/.config/gitoui/themes/<name>.toml`. The name you set in
`core.option.theme` must match the **file stem** (without `.toml`).

A minimal file:

```toml
# ~/.config/gitoui/themes/Brand.toml
base = "Tokyo Night"   # inherit from a built-in (optional)

fg = "#F1ECEC"
bg = "#111111"

list_match_bg     = "#F05133"
list_ref_branch_fg = "#F05133"
help_key_fg       = "#F05133"
status_info_fg    = "#F05133"
status_error_fg   = "#8B2A12"

graph_branches = [
  "#F05133", "#8B2A12", "#F1ECEC", "#F5F4F2",
  "#4B4646", "#7d8590", "#bb9af7", "#9ece6a",
  "#e0af68", "#7dcfff", "#73daca", "#ff9e64",
]
```

Then `config.toml`:

```toml
[core.option]
theme = "Brand"
```

Custom themes show up in the Configuration view's theme cycle right after the
built-ins. Switching to one applies it live — no restart.

### `base` inheritance

`base = "Tokyo Night"` starts from the Tokyo Night palette; any token you
**don't** mention keeps its base value. Omit `base` to start from defaults.

### Auto syntax theme

If you **don't** set `syntax_theme`, gitoui auto-picks one based on the
resolved `bg` luminance:

- **Dark bg** → `base16-ocean.dark`
- **Light bg** → `InspiredGitHub`

Set `syntax_theme = "Dracula"` (or any other syntect theme name) to force a
specific one. This overrides both the base's syntax theme and the auto-pick.

## Color values

Every color token accepts the full ratatui `Color` syntax:

```toml
fg = "#abcdef"             # hex string
bg = "black"               # ANSI named
list_match_fg = "bright-white"
list_match_bg = "11"       # xterm 256 indexed
detail_label_fg = { Rgb = [100, 100, 100] }  # legacy struct form
```

Hex strings are the recommended form.

## Available tokens

The full list lives in
[`src/color.rs::ColorTheme`](https://github.com/NayJi7/gitoui/blob/master/src/color.rs).
Headline tokens:

| Token                          | What                                   |
|--------------------------------|----------------------------------------|
| `fg` / `bg`                    | Base foreground / background.          |
| `list_selected_fg` / `list_selected_bg` | Cursor row.                  |
| `list_compare_marked_fg` / `list_compare_marked_bg` | 2-commit compare endpoint. |
| `list_ref_branch_fg`, `_remote_branch_fg`, `_tag_fg`, `_stash_fg`, `_head_fg` | Ref glyph colors next to commits. |
| `list_match_fg` / `_bg`         | Search-highlighted matches.            |
| `list_commit_message_fg`, `list_name_fg`, `list_hash_fg`, `list_date_fg` | Commit list columns. |
| `detail_*`                     | Right-pane commit-detail tokens.       |
| `ref_selected_fg` / `_bg`      | Refs panel cursor.                     |
| `help_block_title_fg`, `help_key_fg` | Help page.                       |
| `status_input_fg`, `status_info_fg`, `status_success_fg`, `status_warn_fg`, `status_error_fg` | Footer / dialog tones. |
| `divider_fg`                   | Panel borders + separators.            |
| `graph_branches`               | Array of N hex strings for the graph palette. |

<Aside type="note">
When a named theme is active, individual `[color]` overrides in your config
file are **ignored** — the theme replaces the palette wholesale. To tweak a
single token while keeping a theme, fork it: copy the closest built-in into
`~/.config/gitoui/themes/<name>.toml`, then change just the tokens you want.
</Aside>

## Errors

Bad theme files surface as styled boot-time diagnostics:

- **Theme not found** — name doesn't match any built-in OR any
  `~/.config/gitoui/themes/<name>.toml` file. The error includes the
  searched path.
- **Theme file invalid** — TOML parse error inside your custom theme.
- **Unknown base** — `base = "X"` in the file but `X` isn't a built-in.
- **I/O error** — file exists but can't be read (permissions, etc.).
