# Comprehensive Git Graph (mhutchie) Extension Feature Report

## Overview

**Git Graph** by Michael Hutchison (mhutchie) is one of the most popular Git visualization extensions for VS Code. It provides a rich graphical interface for viewing commit history and performing Git operations. The extension is currently unmaintained but remains widely used. A successor fork called **Git Graph Plus** exists but has not yet achieved significant adoption.

---

## 1. GIT ACTIONS AVAILABLE

### Commit Actions (right-click on commit)

| Action | Description | Options/Dialogs |
|--------|-------------|-----------------|
| **Checkout** | Checkout a specific commit | Detached HEAD warning dialog |
| **Cherry Pick** | Apply commit changes to current branch | - **Record Origin** checkbox (`-x` flag)<br>- **No Commit** checkbox (`-n` flag) |
| **Revert** | Create a new commit that undoes changes | Standard revert dialog |
| **Drop** | Permanently remove a commit | Confirmation dialog |
| **Merge** | Merge commit into current branch | - **No Commit** checkbox<br>- **No Fast Forward** checkbox (`--no-ff`)<br>- **Squash Commits** checkbox<br>- Squash message format options |
| **Rebase** | Rebase current branch onto commit | - **Ignore Date** checkbox (non-interactive only)<br>- **Launch Interactive Rebase in Terminal** checkbox |
| **Reset** | Reset current branch to this commit | Mode selection: **Soft** / **Mixed** / **Hard** |
| **Add Tag** | Create a new tag on this commit | - Tag type: **Annotated** or **Lightweight**<br>- **Push to Remote** checkbox<br>- Tag name validation against existing tags |
| **Create Branch** | Create new branch from this commit | - **Check out** checkbox<br>- Branch name validation |
| **Copy Hash** | Copy commit SHA to clipboard | Instant action |
| **Copy Subject** | Copy commit message subject to clipboard | Instant action |
| **View Details** | Open Commit Details View | Inline or docked panel |

### Branch Actions (right-click on branch)

| Action | Description | Options/Dialogs |
|--------|-------------|-----------------|
| **Checkout** | Switch to this branch | - If uncommitted changes exist, stash/reset dialog |
| **Create Branch** | Create new branch from this branch | - **Check out** checkbox |
| **Delete** | Delete the branch | - **Force Delete** checkbox for unmerged branches<br>- Confirmation for unmerged branches with one-click force delete |
| **Merge** | Merge this branch into current branch | - **No Commit** checkbox<br>- **No Fast Forward** checkbox (`--no-ff`)<br>- **Squash Commits** checkbox<br>- Squash message format options |
| **Rebase** | Rebase current branch onto this branch | - **Ignore Date** checkbox<br>- **Launch Interactive Rebase in Terminal** checkbox |
| **Push** | Push branch to remote | - Remote selection (if multiple remotes)<br>- Consumes `remote.pushDefault`, `branch.<name>.remote`, `branch.<name>.pushRemote` configs |
| **Pull** | Pull changes for this branch | - **No Fast Forward** checkbox<br>- **Squash Commits** checkbox<br>- Squash message format options |
| **Fetch** | Fetch latest changes | - **Prune** checkbox<br>- **Prune Tags** checkbox |
| **Rename** | Rename the branch | New name input with validation |
| **Create Pull Request** | Open PR creation page | Built-in support for GitHub, GitLab, Bitbucket<br>+ Custom PR providers configurable |
| **Create Archive** | Create archive of branch | Archive options |
| **Select/Unselect in Branches Dropdown** | Filter branch visibility | Instant toggle |
| **Copy Name** | Copy branch name to clipboard | Instant action |

### Remote Branch Actions (right-click on remote branch)

| Action | Description | Options/Dialogs |
|--------|-------------|-----------------|
| **Checkout** | Checkout as new local branch | - Creates tracking branch automatically |
| **Delete** | Delete remote branch | Confirmation dialog |
| **Fetch into Local Branch** | Update local branch from remote | - **Force Fetch** checkbox (reset local to remote, not for checked-out branch) |
| **Merge** | Merge remote branch into current | Same options as regular merge |
| **Pull** | Pull from remote branch | Same options as regular pull |
| **Create Pull Request** | Open PR creation | Same as local branch |

### Tag Actions (right-click on tag)

| Action | Description | Options/Dialogs |
|--------|-------------|-----------------|
| **View Details** | View annotated tag metadata | Shows: tagger name, email, date, message |
| **Delete** | Delete the tag | Confirmation dialog |
| **Push** | Push tag to remote | Remote selection |
| **Create Archive** | Create archive at tag | Archive options |
| **Copy Name** | Copy tag name to clipboard | Instant action |

### Stash Actions (right-click on stash)

| Action | Description | Options/Dialogs |
|--------|-------------|-----------------|
| **Apply** | Apply stash without removing | - **Reinstate Index** checkbox (`--index`) |
| **Pop** | Apply stash and remove it | - **Reinstate Index** checkbox (`--index`) |
| **Drop** | Delete stash permanently | Confirmation dialog |
| **Create Branch From** | Create branch from stash | Branch name input |
| **Copy Name** | Copy stash name to clipboard | Instant action |
| **Copy Hash** | Copy stash hash to clipboard | Instant action |

### Uncommitted Changes Actions

| Action | Description | Options/Dialogs |
|--------|-------------|-----------------|
| **Stash** | Stash uncommitted changes | - **Include Untracked** checkbox<br>- Custom stash message input |
| **Reset** | Reset uncommitted changes | Mode: **Mixed** or **Hard** |
| **Clean** | Remove untracked files | Confirmation dialog |
| **Open Source Control View** | Switch to VS Code's SCM panel | Instant action |

---

## 2. DIALOG OPTIONS & CHECKBOXES SUMMARY

### Cherry Pick Dialog
- [ ] **Record Origin** (`-x`) - Append "(cherry picked from commit ...)" to message
- [ ] **No Commit** (`-n`) - Apply changes without creating a commit

### Merge Dialog
- [ ] **No Commit** (`--no-commit`) - Perform merge but don't auto-commit
- [ ] **No Fast Forward** (`--no-ff`) - Create merge commit even if fast-forward possible **[DEFAULT: ON]**
- [ ] **Squash Commits** (`--squash`) - Squash all commits into one
  - Message format: Default or Git SQUASH_MSG

### Rebase Dialog
- [ ] **Ignore Date** (`--ignore-date`) - Use current timestamp for rebased commits **[DEFAULT: ON]**
- [ ] **Launch Interactive Rebase in Terminal** - Opens terminal with `git rebase -i`

### Reset Dialog
- **Mode Selection**:
  - **Soft** (`--soft`) - Move HEAD only, keep staged/unstaged changes
  - **Mixed** (`--mixed`) - Move HEAD, unstage changes, keep working tree **[DEFAULT]**
  - **Hard** (`--hard`) - Move HEAD, discard all changes

### Pull Dialog
- [ ] **No Fast Forward** (`--no-ff`)
- [ ] **Squash Commits** (`--squash`)
  - Message format options

### Create Branch Dialog
- [ ] **Check out** - Switch to new branch after creation **[DEFAULT: OFF]**
- Name validation against existing branches/tags

### Add Tag Dialog
- **Type**: **Annotated** (with message, tagger info) or **Lightweight** (just reference) **[DEFAULT: Annotated]**
- [ ] **Push to Remote** - Push tag after creation **[DEFAULT: OFF]**
- Name validation against existing branches/tags
- **Sign Tag** - GPG/X.509 signing (if enabled in settings)

### Delete Branch Dialog
- [ ] **Force Delete** (`-D`) - Delete even if not merged **[DEFAULT: OFF]**
- Smart handling: If deletion fails due to unmerged, shows one-click force delete dialog

### Apply/Pop Stash Dialog
- [ ] **Reinstate Index** (`--index`) - Try to restore staged changes **[DEFAULT: OFF]**

### Stash Uncommitted Changes Dialog
- [ ] **Include Untracked** (`-u`) - Include untracked files **[DEFAULT: ON]**
- Stash message input

### Fetch Remote Dialog
- [ ] **Prune** (`--prune`) - Remove stale remote-tracking branches **[DEFAULT: OFF]**
- [ ] **Prune Tags** (`--prune-tags`) - Remove local tags not on remote **[DEFAULT: OFF]**

### Fetch into Local Branch Dialog
- [ ] **Force Fetch** - Reset local branch to match remote (not for checked-out branch) **[DEFAULT: OFF]**

---

## 3. ADVANCED & SPECIAL FEATURES

### Code Review System
- Start code review on any commit or between any two commits
- Files needing review are **bolded** in the file list
- Viewing diff or opening file marks it as reviewed (un-bolds)
- **Mark as Reviewed** / **Mark as Not Reviewed** manual actions in context menu
- Persists across VS Code sessions
- Auto-closes after 90 days of inactivity
- Resume/end code reviews via Command Palette commands

### Commit Details View
- View commit metadata (hash, author, committer, date, message)
- **File changes list** with A|M|D|R|U status indicators
- **Visual Studio Code Diff** - Click any file to see diff
- **Open current version** of any affected file
- **View Diff with Working File** - Compare historical version with current working tree
- **Copy file path** (absolute or relative) to clipboard
- Click HTTP/HTTPS URLs in commit body to open in browser
- File view modes: **File Tree** (with compact folders option) or **File List**
- Commit signature status display (GPG/X.509)
- Hover tooltips on commits showing which branches/tags/stashes include it

### Commit Comparison View
- Compare any two commits (click one, then Ctrl/Cmd+click another)
- Same file operations as Commit Details View
- Code review support

### Uncommitted Changes View
- View all uncommitted changes inline in the graph
- Show/hide untracked files
- Compare uncommitted changes with any commit

### Repository Settings Widget
- **Remotes Management**: View, add, edit, delete, fetch, prune remotes
- **Issue Linking**: Configure regex to convert issue numbers in commits to hyperlinks
- **Pull Request Creation**: Configure automatic PR form pre-filling
  - Built-in: GitHub, GitLab, Bitbucket
  - Custom providers via regex templates
- **Export/Import Configuration**: Share Git Graph settings via committed JSON file

### Find Widget
- Search commits by: commit message, date, author, hash, branch/tag names
- Optional mode: Auto-open Commit Details View as you navigate matches
- Regex support

### Keyboard Shortcuts (configurable)
- `Ctrl/Cmd + F` - Open Find Widget
- `Ctrl/Cmd + H` - Scroll to HEAD
- `Ctrl/Cmd + R` - Refresh view
- `Ctrl/Cmd + S` - Scroll to next stash
- `Ctrl/Cmd + Shift + S` - Scroll to previous stash
- `Up/Down` - Navigate commits
- `Ctrl/Cmd + Up/Down` - Navigate parent/child commits
- `Ctrl/Cmd + Shift + Up/Down` - Navigate alternative branches at merges
- `Enter` - Submit dialog (primary action)
- `Escape` - Close dialog/menu/details view

### Visual Features
- **Graph Styles**: Rounded or angular branch lines
- **Custom Colors**: Configurable branch colors (12 default HEX colors)
- **Uncommitted Changes Display**: Open circle at uncommitted changes OR at checked-out commit
- **Reference Labels**: Align branches/tags left/right, combine local+remote labels
- **Column Visibility**: Toggle Date, Author, Commit columns
- **Commit Ordering**: date, author-date, or topological
- **First Parent Only** mode
- **Avatars**: Fetch author/committer avatars from GitHub/GitLab/Gravatar
- **Emoji Shortcodes**: Automatic gitmoji replacement in commit messages
- **Markdown Rendering**: Bold, italics, inline code in commit messages
- **Signature Status Icons**: Visual GPG verification indicators

### Repository Discovery
- Automatic workspace repository detection
- Configurable max search depth for subfolders
- Manual add/remove repositories via Command Palette
- Multi-repository dropdown with sorting options

### Signing Support
- GPG/X.509 commit signing
- GPG/X.509 tag signing
- Signature verification display

### Archive Creation
- Create archive from branch, tag, or commit

---

## 4. FEATURES POTENTIALLY MISSING FROM TUI APPS

Based on typical TUI git clients (like lazygit, tig, gitoui), Git Graph provides these advantages:

### Visual/UX Advantages
1. **True Graph Visualization** - SVG/canvas-rendered branch/merge topology with colors
2. **Rich Commit Details Panel** - Persistent side panel with file trees, diffs, metadata
3. **Code Review System** - Track reviewed files with persistence across sessions
4. **Avatar Support** - Visual author identification
5. **Emoji/Markdown Rendering** - Pretty commit message display
6. **Signature Verification UI** - Visual GPG status indicators

### Integration Features
7. **VS Code Native Diff** - Opens actual VS Code diff viewer with syntax highlighting
8. **Pull Request Integration** - One-click PR creation with pre-filled forms for GitHub/GitLab/Bitbucket
9. **Issue Linking** - Auto-hyperlink issue numbers to issue trackers
10. **Integrated Terminal Launch** - Opens VS Code terminal for interactive rebase
11. **File Opening** - Open any file version directly in VS Code editor
12. **Diff with Working File** - Compare any historical commit against current working tree

### Workflow Features
13. **Repository Settings Persistence** - Per-repo configuration exportable/importable
14. **Custom Branch Glob Patterns** - Saveable branch filtering presets
15. **Find Widget with Auto-Details** - Search with automatic commit details opening
16. **Smart Force Delete** - One-click force delete for unmerged branches
17. **Reference Name Validation** - Real-time checking against existing branches/tags
18. **Stash Navigation Shortcuts** - Dedicated keyboard shortcuts for stash jumping
19. **Context Menu Customization** - Hide/show any context menu action per type
20. **Multiple Repository Support** - Dropdown to switch between workspace repos

### Data/Display Features
21. **Author vs Committer Date Toggle** - Switch between date types
22. **RefLog Inclusion** - Show commits only mentioned by reflogs
23. **First-Parent Filtering** - Simplify merge-heavy histories
24. **Untracked Files Toggle** - Show/hide untracked in uncommitted changes
25. **Commit Mute Options** - Mute non-HEAD ancestors or merge commits
26. **Remote HEAD Display** - Show origin/HEAD symbolic references

---

## 5. NOTABLE LIMITATIONS / MISSING FEATURES

Compared to full Git clients or other extensions, Git Graph lacks:

- **Interactive Rebase UI** - Only launches terminal with `git rebase -i`
- **Submodule Support** - No specific submodule actions
- **Worktree Support** - No git worktree operations
- **Bisect Support** - No `git bisect` integration
- **Patch/Apply Patch** - No patch file operations
- **Blame/Annotate** - No line-by-line blame view
- **Reflog Browser** - No dedicated reflog visualization
- **Rebase --onto** - No explicit `--onto` support in UI
- **Rerere** - No reuse recorded resolution support
- **Bundle Operations** - No git bundle support
- **Notes** - No git notes support
- **Filter-Repo/Filter-Branch** - No history rewriting beyond rebase
- **Sparse Checkout** - No sparse checkout UI
- **Cherry-pick Range** - No range cherry-pick UI (single commits only)
- **Rebase --autosquash** - No autosquash/fixup commit handling

---

## 6. GIT GRAPH PLUS (SUCCESSOR)

**Status**: Fork exists at `Starthe0807/git-graph-plus` but is very new (as of 2025) with minimal adoption.

**Current State**:
- 11 stars on GitHub
- Updated recently (active development)
- Built with Svelte
- Claims to be "a modern Git visualization tool for VS Code"
- Marketplace presence uncertain (404 on direct link attempts)

**Recommendation**: For comprehensive feature comparison, focus on the original **mhutchie/vscode-git-graph** (v1.30.0) as it represents the mature, battle-tested feature set that most users know. Git Graph Plus has not yet differentiated itself with significant new features.

---

## Summary

The original **Git Graph** extension provides an exceptionally comprehensive set of Git actions with well-designed dialogs offering all major git command flags as checkboxes. Its standout features for TUI comparison are:

1. **Visual graph rendering** with colors and styles
2. **Rich commit details/comparison views** with file trees and VS Code integration
3. **Code review tracking system** with persistence
4. **Pull request creation integration** with major providers
5. **Highly configurable UI** (context menus, columns, colors, keyboard shortcuts)
6. **Repository-level settings** with export/import capability

The extension essentially provides a GUI wrapper around standard git commands with thoughtful defaults and extensive customization options, making it an excellent reference for what features users expect in a modern Git visualization tool.