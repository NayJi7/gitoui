# Installation

### [Cargo](https://crates.io/crates/gitoui)

```
$ cargo install --locked gitoui
```

### [Arch Linux](https://archlinux.org/packages/extra/x86_64/gitoui/)

```
$ pacman -S gitoui
```

### [Homebrew](https://formulae.brew.sh/formula/gitoui)

```
$ brew install gitoui
```

or from [tap](https://github.com/lusingander/homebrew-tap/blob/master/gitoui.rb):

```
$ brew install lusingander/tap/gitoui
```

### [NetBSD](https://pkgsrc.se/devel/gitoui)

```
$ pkgin install gitoui
```

### Downloading binary

You can download pre-compiled binaries from [releases](https://github.com/lusingander/gitoui/releases).

### Build from source

If you want to check the latest development version, build from source:

```
$ git clone https://github.com/lusingander/gitoui.git
$ cd gitoui
$ cargo build --release # Unless it's a release build, it's very slow.
$ ./target/release/gitoui
```
