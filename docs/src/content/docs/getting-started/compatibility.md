---
title: Terminal compatibility
description: Which terminals render the gitoui commit graph and avatars correctly.
---

The commit graph and GitHub avatars are inline images. The terminal needs to
speak one of three image protocols.

## Image protocols

| Terminal               | Kitty | iTerm2 | Sixel | Notes                              |
|------------------------|-------|--------|-------|------------------------------------|
| **Kitty**              | ✅    | —      | —     | Reference implementation.          |
| **Ghostty**            | ✅    | —      | —     | Kitty-compatible.                  |
| **WezTerm**            | ✅    | ✅     | ✅    | Auto-detects; picks Kitty first.   |
| **iTerm2** (macOS)     | —     | ✅     | —     |                                    |
| **Konsole**            | ✅    | —      | ✅    | Kitty since 22.04, Sixel optional. |
| **foot** (Wayland)     | —     | —      | ✅    | Sixel native; very fast.           |
| **xterm**              | —     | —      | 🟡    | Needs `xterm -ti vt340`.           |
| **Alacritty**          | —     | —      | —     | No image protocol — graph falls back to Unicode. |
| **Terminal.app** (macOS) | —   | —      | —     | Same fallback as Alacritty.        |
| **gnome-terminal / Ptyxis** | 🟡 | —    | —     | Recent VTE (≥ 0.78) supports the Kitty protocol; older VTE falls back. |
| **Windows Terminal**   | —     | —      | —     | Unicode fallback only (as of Windows Terminal 1.x). |

✅ = first-class, 🟡 = supported with config, — = not supported.

When in doubt, run `gitoui --protocol sixel` (or `kitty`, `iterm2`) to force a
specific protocol and bypass auto-detect.

## Keyboard protocol

gitoui pushes the Kitty
[keyboard protocol's `DISAMBIGUATE_ESCAPE_CODES`](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)
flag at startup. This makes `Alt+letter` shortcuts arrive as a single
KeyEvent on supporting terminals instead of `Esc` + `letter` (which would
otherwise close the active view before the letter arrived).

Terminals that don't speak the protocol silently ignore the push — Alt-key
bindings work where the terminal sends them as proper meta-encoded events
(most modern terminals do).

If `Alt+...` shortcuts don't fire on your setup, switch to `Ctrl+...` or
override the bindings — see [Custom keybindings](/keybindings/custom/).

## Mouse + clipboard

- **Mouse** is enabled by default (`ui.common.mouse_enabled = true`). Click on
  rows / tabs / chips to interact. Wheel scrolls.
- **Copy/paste** uses the system clipboard via the
  [`arboard`](https://crates.io/crates/arboard) crate. On Wayland, `wl-copy` is
  not required — `arboard`'s `wayland-data-control` feature talks the protocol
  directly.
