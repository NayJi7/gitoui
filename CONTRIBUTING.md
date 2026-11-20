# Contribution Guide

Thank you for considering contributing. Please review the guidelines below before making a contribution.

To ensure that your contributions are considered, please follow this guidelines. Contributions that do not adhere to these guidelines may not be accepted.

## Reporting Issues

Before reporting, please check if an issue with the same content already exists.

Also, please refer to [FAQ](https://gitoui.nayji7.dev/faq/index.html).

### Reporting Bugs

When reporting a bug, please include the following information:

- Application version
  - `gitoui --version`
- Version of the terminal emulator and the OS it's running on
- Information about the git repository to reproduce the issue
  - If possible, provide the smallest possible repository (debugging a repository with 100,000 commits is difficult)

On Windows, also say which terminal you ran it in (Windows Terminal, the classic console, the VS Code terminal…) and its version. gitoui reads the keyboard through the Windows console, so it does not run in Git Bash's mintty (it stops with a message).

### Suggesting Features

Before proposing a new feature, please review the [Goals](https://gitoui.nayji7.dev/introduction/index.html#goals) and [Non-Goals](https://gitoui.nayji7.dev/introduction/index.html#non-goals).

### Terminal Emulator Compatibility

If the application does not work with your terminal emulator, please first check whether the terminal emulator supports the target image display protocol.

For information on tested terminal emulators, refer to [Compatibility](https://gitoui.nayji7.dev/getting-started/compatibility.html).

## Pull Requests

We welcome pull requests, but please note that they are not guaranteed to be accepted. Following this guideline will increase the likelihood of your pull request being approved.

### Creating pull requests

- When creating a pull request, please ensure you follow the same guidelines as [mentioned for issues](#reporting-issues).
- An issue is not required for every pull request. For small or straightforward changes (such as documentation fixes or obvious bug fixes), feel free to open a pull request directly.
- For more complex changes or behavior-altering fixes, opening an issue first is strongly recommended to discuss the approach and avoid unnecessary rework.
- Do not include fixes that are not directly related to the pull request topic.

### Continuous Integration

We use [GitHub Actions](https://github.com/NayJi7/gitoui/blob/master/.github/workflows/build.yml) to perform basic checks:

- Run stable Rust.
- Run build, test, format, and lint.

### Building on Windows

Windows support is experimental. To build and test it:

- Install the **Visual Studio Build Tools** with the "Desktop development with C++" workload (Rust's MSVC toolchain needs its linker).
- Build from **PowerShell or cmd**, not from Git Bash: Git's `link` shadows the MSVC linker there and the build fails.
- Run `cargo test`. The smoke test (`tests/smoke.rs`) runs the real binary in a pseudo-terminal, which goes through ConPTY on Windows.
- To look at it, run the binary in **Windows Terminal**. A debug build is enough (`cargo build`). If `NO_COLOR` is set in your environment the colors are off by design.

### Adding a setting

A setting the Configuration view (`p`) can change needs a decision besides its code: **when does it take effect?** The page keeps its own copy of the config and saves every change at once, but the running app only reads it back when the page is left, except for the few settings applied immediately. Each row says which in `ROW_APPLIES` (`src/view/config.rs`, next to the rule for choosing); the table's size is tied to the number of rows, so a new row does not compile until it has an answer. In short: `Live` for what the page itself uses or what only makes sense to try right there (the theme, the mouse, the select mode), `OnClose` for anything that rebuilds or reloads (the default), `NextLaunch` for what is read once at startup. The page tells the user which one applies under each setting's description.

### Improving the Commit Graph

Improvements to the commit graph are welcome.

Tests for the commit graph are conducted in [./tests/graph.rs](./tests/graph.rs).

Running the tests will output images and the test repository to `./out/graph`.
If you add new test cases, please add these images under `./tests/graph/`.
If existing graphs are modified, overwrite the images and ensure no unexpected changes have occurred.

## License

This project is licensed under the [MIT License](LICENSE). By contributing, contributors agree to abide by the terms of the applicable license.
