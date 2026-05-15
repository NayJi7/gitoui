---
title: User commands
description: Bind your own shell commands to in-app shortcuts.
---

<span class="gitoui-wordmark">gitoui</span> lets you wire up custom shell commands to numbered keybinds — useful
for project-specific actions (e.g. open the deploy log, run a test, open
the current commit in a code review tool).

## Define commands

In `config.toml`:

```toml
[core.user_command.1]
key = "shift-1"           # bind any key (within the modifier-cap rules)
name = "Run tests"        # label shown in the user command view
command = "cargo test --workspace"

[core.user_command.2]
key = "shift-2"
name = "Open in code-review tool"
command = "review-this ${commit_hash}"
```

The `${commit_hash}` placeholder expands to the SHA of the **currently
selected commit** when the command fires. Other placeholders:

| Token              | Value                                                                |
|--------------------|----------------------------------------------------------------------|
| `${commit_hash}`   | Full SHA of the focused commit.                                      |
| `${short_hash}`    | Short SHA (`git rev-parse --short`).                                 |
| `${branch_name}`   | Current branch name (`HEAD` if detached).                            |
| `${file_path}`     | Focused file path (Diff / Uncommitted / Blame / File History views). |
| `${repo_path}`     | Absolute path to the repo root.                                      |

## Run a command

The bound key fires the command in a subshell. <span class="gitoui-wordmark">gitoui</span> suspends the TUI
(leaves the alt-screen + raw mode), runs the command, prints any output to
the underlying terminal, then resumes once it exits — same flow as
`git commit` opening your `$EDITOR`.

For background commands that don't need output, append `&` (or `nohup`):

```toml
command = "notify-send 'deploy started' && deploy.sh &"
```

## Limits

- Commands run with the same env as the <span class="gitoui-wordmark">gitoui</span> process — no isolation.
- Output is shown verbatim; if your command produces ANSI escape codes
  they'll render through.
- Long-running commands block the UI until exit. For "fire and forget"
  flows, use `&` or background them via a wrapper.
