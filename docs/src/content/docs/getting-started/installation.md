---
title: Installation
description: Three ways to install gitoui — curl-pipe-sh, cargo install, or build from source.
---

import { Tabs, TabItem, Aside } from "@astrojs/starlight/components";

<Tabs>
  <TabItem label="curl | sh">

The fastest install on Linux and macOS — downloads the latest prebuilt
binary from GitHub Releases and drops it into `~/.local/bin`.

```sh
curl -fsSL https://raw.githubusercontent.com/NayJi7/gitoui/master/install.sh | sh
```

The installer:

- Picks the correct binary for your `(os, arch)` from the latest GitHub
  Release.
- Verifies the tarball checksum.
- Installs to `$HOME/.local/bin/gitoui` by default.
- Adds the install directory to your shell `PATH` if it isn't already
  there (prints the line to add if it can't edit your rc file).

### Tweaks

| Variable / flag       | Effect                                                |
|-----------------------|-------------------------------------------------------|
| `INSTALL_DIR=...`     | Override the install directory.                       |
| `GITOUI_VERSION=vX.Y` | Pin to a specific tag instead of `latest`.            |
| `NO_COLOR=1`          | Disable ANSI colors in the installer output.          |
| `--ascii` (or `GITOUI_ASCII=1`) | Force ASCII banner when piping the output. |

  </TabItem>

  <TabItem label="cargo install">

If you have a Rust toolchain available, `cargo install` works from
crates.io once <span class="gitoui-wordmark">gitoui</span> is published:

```sh
cargo install gitoui
```

<Aside type="caution">
The crates.io publication lands at v1.0. Until then, install via the
script above or `cargo install --git`:

```sh
cargo install --git https://github.com/NayJi7/gitoui
```
</Aside>

  </TabItem>

  <TabItem label="From source">

```sh
git clone https://github.com/NayJi7/gitoui
cd gitoui
cargo build --release
install -m 0755 target/release/gitoui ~/.local/bin/
```

The release binary is ~16 MB. A debug build is ~250 MB and slower at
startup — only use it for development.

  </TabItem>
</Tabs>

## Verify

```sh
gitoui --version
```

Then `cd` into any git repo and run `gitoui`. If your terminal supports a
graph image protocol you should see the commit graph painted inline.

## Updating

The `curl | sh` script is idempotent — re-run it to update to the latest
release. `cargo install --force` does the same when installed via cargo.

## Uninstalling

<span class="gitoui-wordmark">gitoui</span> doesn't write anywhere except your config dir:

```sh
rm ~/.local/bin/gitoui              # binary
rm -rf ~/.config/gitoui             # config + cached avatars + themes
```
