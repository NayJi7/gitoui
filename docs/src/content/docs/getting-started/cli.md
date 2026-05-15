---
title: CLI options
description: Command-line flags accepted by the gitoui binary.
---

```text
gitoui [OPTIONS]
```

<span class="gitoui-wordmark">gitoui</span> reads everything it needs from the current working directory and the
config file. The flags here are the rare cases where you want to override
defaults for one specific run.

| Flag                          | Default       | Effect                                              |
|-------------------------------|---------------|-----------------------------------------------------|
| `-n`, `--max-count <NUMBER>`  | `1000`        | Initial number of commits to load. Lazy-load grows from there. |
| `-p`, `--protocol <TYPE>`     | `auto`        | Force an image protocol — one of `kitty`, `iterm2`, `sixel`, `unicode`. |
| `-o`, `--order <TYPE>`        | `chrono`      | Commit ordering algorithm. `chrono` (date-based) or `topo` (`git log --topo-order`). |
| `-g`, `--graph-width <TYPE>`  | `auto`        | Graph image cell width. `auto` / `single` / `double`. |
| `-s`, `--graph-style <TYPE>`  | `rounded`     | Graph edge style. `rounded` / `angular` / `smooth`. |
| `-i`, `--initial-selection <TYPE>` | `latest` | Cursor position on open. `latest`, `head`, or `top`. |
| `-h`, `--help`                |               | Print help and exit.                                |
| `-V`, `--version`             |               | Print version and exit.                             |

## Examples

Load fewer commits up front for huge repos:

```sh
gitoui -n 200
```

Force Sixel (e.g. when `auto` picks the wrong protocol on a custom terminal):

```sh
gitoui --protocol sixel
```

Open with the cursor on HEAD rather than the newest commit:

```sh
gitoui -i head
```

## Environment

| Variable                 | Effect                                                  |
|--------------------------|---------------------------------------------------------|
| `GITOUI_CONFIG_FILE`     | Override the config file path (skips the XDG lookup).   |
| `XDG_CONFIG_HOME`        | Base directory for the config + themes + token.         |
| `NO_COLOR`               | Honored by the `--help` output and by the no-repo error splash. |
