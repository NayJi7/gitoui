---
title: Requirements
description: System and terminal prerequisites for running gitoui.
---

<span class="gitoui-wordmark">gitoui</span> targets recent Linux and macOS terminals. Windows isn't officially
supported but should work under WSL2 with a compatible terminal.

## Runtime

| Tool          | Version                | Why                                        |
|---------------|------------------------|--------------------------------------------|
| `git`         | 2.30+                  | <span class="gitoui-wordmark">gitoui</span> shells out to the CLI for every git operation. |
| A terminal    | Recent (see below)     | The commit graph and avatars are rendered inline as images. |
| `xdg-open` / `open` | platform default | Used by the "open in browser" shortcut for PRs and issues. |

## Terminal image protocols

<span class="gitoui-wordmark">gitoui</span> pre-renders the commit graph as SVG/PNG and ships it inline. To see the
graph, your terminal needs to speak one of:

| Protocol | Terminals (non-exhaustive)                            |
|----------|-------------------------------------------------------|
| **Kitty**     | Kitty, Ghostty, WezTerm (auto)                  |
| **iTerm2**    | iTerm2, WezTerm (when configured)               |
| **Sixel**     | foot, mlterm, xterm (with `-ti vt340`), WezTerm |

In a terminal that supports **none** of these, <span class="gitoui-wordmark">gitoui</span> still works — it falls
back to Unicode glyphs for the graph — but you lose the inline image quality.

The same protocol drives the GitHub avatar rendering. If your terminal can show
the commit graph, it can show avatars next to commits, comments, and review
events.

## Building from source

If you're building locally instead of using a prebuilt binary:

| Tool         | Version | Notes                                   |
|--------------|---------|-----------------------------------------|
| **Rust**     | 1.88+   | `rust-toolchain.toml` not pinned — use stable. |
| **Cargo**    | ships with Rust | |
| **`cc`** / `cargo`-compatible linker | platform default | For native deps (`sharp` isn't required, this is Rust only). |

Once Rust is installed:

```sh
git clone https://github.com/NayJi7/gitoui
cd gitoui
cargo build --release
./target/release/gitoui
```

## GitHub features

The Pull Requests and Issues views need:

- A repo with a **`github.com` remote** (any of `origin`, `upstream`, or a
  custom name — <span class="gitoui-wordmark">gitoui</span> scans `git remote -v` at startup).
- A logged-in **GitHub account** via OAuth device-flow (configure under the
  Configuration page — section "GitHub Auth"). Token is stored at
  `~/.config/gitoui/github_token.toml` with `0600` permissions.

Without auth or without a GitHub remote, the `Shift+R` (PRs) and `Shift+I`
(Issues) shortcuts are silent no-ops — the rest of <span class="gitoui-wordmark">gitoui</span> works fine.
