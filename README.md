<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="./assets/brand/mixed-nobg.svg">
    <img src="./assets/brand/mixed-nobg-black.svg" alt="gitoui" width="420">
  </picture>
</p>

<p align="center">

[![crates.io](https://img.shields.io/crates/v/gitoui?style=flat-square&label=crates.io&labelColor=2a2624&color=f05133&logo=rust&logoColor=f1ecec)](https://crates.io/crates/gitoui)
[![GitHub release](https://img.shields.io/github/v/release/NayJi7/gitoui?style=flat-square&label=release&labelColor=2a2624&color=f05133&logo=github&logoColor=f1ecec)](https://github.com/NayJi7/gitoui/releases/latest)
[![downloads](https://img.shields.io/crates/d/gitoui?style=flat-square&label=downloads&labelColor=2a2624&color=4b4646&logo=data:image/svg%2Bxml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0iY3VycmVudENvbG9yIj48cGF0aCBkPSJNMTIgMi41TDMgN3YxMGw5IDQuNSA5LTQuNVY3bC05LTQuNXptMCAyLjA0TDE4Ljk0IDcuNSAxMiAxMC45NiA1LjA2IDcuNSAxMiA0LjU0ek00LjUgOS4wNmw2Ljc1IDMuMzh2Ni45NEw0LjUgMTZWOS4wNnptMTUgNi45NGwtNi43NSAzLjM4VjEyLjQ0TDE5LjUgOS4wNlYxNnoiLz48L3N2Zz4=&logoColor=f1ecec)](https://crates.io/crates/gitoui)
[![GitHub stars](https://img.shields.io/github/stars/NayJi7/gitoui?style=flat-square&label=stars&labelColor=2a2624&color=f0b04a&logo=github&logoColor=f1ecec)](https://github.com/NayJi7/gitoui/stargazers)
[![Built with Ratatui](https://img.shields.io/badge/Built_With-Ratatui-000?logo=ratatui&logoColor=fff&labelColor=000&color=fff)](https://ratatui.rs)

</p>

<p align="center"><em>Say <strong>oui</strong> to the smoothest git terminal youser experience.</em></p>

<p align="center">
  <!-- ▶ demo GIF — full app walkthrough -->
  <img src="./assets/readme/demo.gif" alt="gitoui demo" width="900">
</p>

**gitoui** ([`/ʒi.tu.i/`](https://nayji7.github.io/gitoui/faq/#how-do-i-pronounce-gitoui)) is a terminal git client. It started as a fork of [serie](https://github.com/lusingander/serie) — a beautifully-rendered `git log --graph` viewer — and grew into a daily-driver TUI that covers diffs, staging, blame, history, interactive rebase, conflict resolution, and GitHub Pull Requests / Issues.

## What's inside

The commit graph stays serie's: pre-rendered SVGs sent inline through your terminal's image protocol (Kitty, iTerm2, Ghostty, Sixel). Around it, gitoui adds the operations you do on top of `git log`:

- Side-by-side or inline diffs with `git add -p`-style hunk staging baked into the Uncommitted view.
- `b` to blame, `Shift+H` to follow file history across renames.
- Mark two commits with `Space` for a cumulative diff between them.
- Visual interactive rebase: grab + drop rows, reword in place, pause + resume modes.
- 3-way conflict editor — pick OURS / THEIRS / BOTH per hunk, no need to drop to vimdiff.
- Full GitHub workflow: PR + Issue threads, reviews, reactions, labels, assignees, mention autocomplete, comment CRUD.
- Inline GitHub avatars rendered through the same image protocols as the graph.
- Two-layer keybinds: global `[keybind]` UserEvents plus per-view `[keybind.scope.<path>]` overrides.
- Ten built-in themes (Tokyo Night, Dracula, Catppuccin, Gruvbox, Nord, Solarized, One Dark, Monokai Pro), and a `.toml` in `~/.config/gitoui/themes/` for your own.

Full feature tour at [nayji7.github.io/gitoui/introduction/](https://nayji7.github.io/gitoui/introduction/).

## Install

One-liner for Linux + macOS (auto-detects OS/arch, drops `gitoui` into `~/.local/bin`):

```sh
curl -fsSL https://nayji7.github.io/gitoui/install.sh | sh
```

Or via Cargo from crates.io:

```sh
cargo install gitoui
```

For pinned versions, manual downloads, or building from source, see the [installation guide](https://nayji7.github.io/gitoui/getting-started/installation/).

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
