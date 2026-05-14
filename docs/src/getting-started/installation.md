# Installation

### One-line installer (Linux / macOS)

```
curl -fsSL https://raw.githubusercontent.com/NayJi7/gitoui/master/install.sh | sh
```

Auto-detects your OS + architecture, downloads the matching release tarball
to `~/.local/bin/gitoui`, and verifies its checksum. Run it again any time
to update to the latest release.

Override the install directory or pin a specific version:

```
INSTALL_DIR=$HOME/bin GITOUI_VERSION=v0.1.0 \
    curl -fsSL https://raw.githubusercontent.com/NayJi7/gitoui/master/install.sh | sh
```

### Downloading the binary manually

Pre-compiled binaries for Linux + macOS (x86_64 and aarch64) are attached
to each [release](https://github.com/NayJi7/gitoui/releases). Each archive
ships a single `gitoui` executable; drop it anywhere on your `$PATH`.

### Build from source

```
$ git clone https://github.com/NayJi7/gitoui.git
$ cd gitoui
$ cargo build --release    # debug builds are noticeably slower
$ ./target/release/gitoui
```
