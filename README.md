<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./assets/brand/mixed-nobg.svg">
    <img src="./assets/brand/mixed-nobg-black.svg" alt="gitoui" width="420">
  </picture>
</p>

<p align="center">

[![Crate](https://img.shields.io/crates/v/gitoui.svg)](https://crates.io/crates/gitoui)
[![Built with Ratatui](https://img.shields.io/badge/Built_With-Ratatui-000?logo=ratatui&logoColor=fff&labelColor=000&color=fff)](https://ratatui.rs)
[![MIT License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

</p>

<p align="center"><em>Say <strong>oui</strong> to the most complete git terminal youser interface.</em></p>

<p align="center">
  <!-- ▶ demo GIF — full app walkthrough -->
  <img src="./assets/readme/demo.gif" alt="gitoui demo" width="900">
</p>

**gitoui** (pronounced `/ʒi.tu.i/` — *gi-tou-i*) is a friendly little TUI that brings the depth of a desktop git client right into your terminal: rendered commit graphs, GitHub PRs and issues, hunk staging, blame, stash, and more — all keyboard-driven, all themable.

## What's inside

- 🌳 **Commit graph** rendered inline as real images (Kitty / iTerm2 / Ghostty)
- 🔍 **Detail, diff, refs, file history, blame** — drill down anywhere
- ✂️ **Hunk-level staging**, interactive rebase, 3-way conflict editor
- 🐙 **GitHub PRs + issues** with reviews, reactions, mentions, labels, assignees
- 🎨 Built-in themes: Tokyo Night, Dracula, Catppuccin, Gruvbox, Nord, Solarized, One Dark, Monokai Pro
- ⚙️ TOML config, custom keybindings, user shell commands

## Install

One-liner for Linux + macOS (auto-detects OS/arch, drops `gitoui` into `~/.local/bin`):

```sh
curl -fsSL https://raw.githubusercontent.com/NayJi7/gitoui/master/install.sh | sh
```

For pinned versions, manual downloads, or building from source, see the [installation guide](https://nayji7.github.io/gitoui/getting-started/installation.html).

## Quick start

```sh
cd <your git repo>
gitoui
```

Press `?` for the keymap, `q` to quit. Full docs at [nayji7.github.io/gitoui](https://nayji7.github.io/gitoui/).

## Contributing

Open source, open arms — PRs are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for the dev setup and workflow.

## License

[MIT](LICENSE)

---

Forked from [serie](https://github.com/lusingander/serie). Inspired by [GitKraken](https://www.gitkraken.com/) and the [Git Graph](https://marketplace.visualstudio.com/items?itemName=mhutchie.git-graph) VS Code extension.
