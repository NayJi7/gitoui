use regex::Regex;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct DiffEntry {
    pub old_path: Option<String>,
    pub new_path: Option<String>,
    pub hunks: Vec<Hunk>,
}

#[derive(Debug, Clone)]
pub struct Hunk {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    pub lines: Vec<DiffLine>,
    /// Where this hunk comes from. Used by the uncommitted combined diff view
    /// to know whether the hunk is staged or not, so it can show the right
    /// indicator and apply the right toggle action.
    pub origin: HunkOrigin,
}

/// Origin of a hunk — used to drive the staged/unstaged indicator and
/// stage/unstage toggle in the Uncommitted view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HunkOrigin {
    /// `git diff --cached` — already in the index.
    Staged,
    /// `git diff` — modified but not yet in the index.
    Unstaged,
    /// Untracked file rendered as a synthetic diff (all additions).
    Untracked,
    /// Hunk from a committed diff or any context where stage/unstage doesn't apply.
    #[default]
    Other,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub line_type: DiffLineType,
    pub old_line_no: Option<u32>,
    pub new_line_no: Option<u32>,
    pub content: String,
    /// Byte ranges within `content` that changed (intra-line / word diff).
    /// Empty means no word-diff available (line is not part of a matched pair).
    pub highlight_ranges: Vec<std::ops::Range<usize>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiffLineType {
    Context,
    Addition,
    Deletion,
    HunkHeader,
    FileHeader,
    BinaryNote,
}

impl DiffEntry {
    pub fn load_for_commit(repo_path: &Path, hash: &str) -> Result<Vec<Self>, String> {
        Self::load_for_commit_with_context(repo_path, hash, 3)
    }

    pub fn load_for_commit_with_context(
        repo_path: &Path,
        hash: &str,
        context_lines: u32,
    ) -> Result<Vec<Self>, String> {
        let output = Command::new("git")
            .args([
                "diff",
                &format!("--unified={}", context_lines),
                &format!("{}^", hash),
                hash,
            ])
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git diff: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_diff(&stdout)
    }

    /// Cumulative diff between two arbitrary commits — used by the 2-commit
    /// comparison mode (Space / Ctrl+click on two rows).
    /// Runs `git diff <from>..<to>` so changes appear as if going from the
    /// older endpoint to the newer one. Order detection is the caller's
    /// responsibility.
    pub fn load_for_commit_range(
        repo_path: &Path,
        from_hash: &str,
        to_hash: &str,
        context_lines: u32,
    ) -> Result<Vec<Self>, String> {
        let output = Command::new("git")
            .args([
                "diff",
                &format!("--unified={}", context_lines),
                &format!("{}..{}", from_hash, to_hash),
            ])
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git diff: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_diff(&stdout)
    }

    pub fn load_for_file(repo_path: &Path, hash: &str, file_path: &str) -> Result<Self, String> {
        Self::load_for_file_with_context(repo_path, hash, file_path, 3)
    }

    pub fn load_for_file_with_context(
        repo_path: &Path,
        hash: &str,
        file_path: &str,
        context_lines: u32,
    ) -> Result<Self, String> {
        let output = Command::new("git")
            .args([
                "diff",
                &format!("--unified={}", context_lines),
                &format!("{}^", hash),
                hash,
                "--",
                file_path,
            ])
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git diff: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let entries = parse_diff(&stdout)?;
        entries
            .into_iter()
            .next()
            .ok_or_else(|| "No diff output".to_string())
    }

    pub fn load_unstaged_for_file(repo_path: &Path, file_path: &str) -> Result<Self, String> {
        Self::load_unstaged_for_file_with_context(repo_path, file_path, 3)
    }

    pub fn load_unstaged_for_file_with_context(
        repo_path: &Path,
        file_path: &str,
        context_lines: u32,
    ) -> Result<Self, String> {
        let output = Command::new("git")
            .args([
                "diff",
                &format!("--unified={}", context_lines),
                "--",
                file_path,
            ])
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git diff: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let entries = parse_diff(&stdout)?;
        let mut entry = entries
            .into_iter()
            .next()
            .ok_or_else(|| "No diff output".to_string())?;
        for h in &mut entry.hunks {
            h.origin = HunkOrigin::Unstaged;
        }
        Ok(entry)
    }

    pub fn load_staged_for_file(repo_path: &Path, file_path: &str) -> Result<Self, String> {
        Self::load_staged_for_file_with_context(repo_path, file_path, 3)
    }

    pub fn load_staged_for_file_with_context(
        repo_path: &Path,
        file_path: &str,
        context_lines: u32,
    ) -> Result<Self, String> {
        let output = Command::new("git")
            .args([
                "diff",
                &format!("--unified={}", context_lines),
                "--cached",
                "--",
                file_path,
            ])
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git diff: {}", e))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let entries = parse_diff(&stdout)?;
        let mut entry = entries
            .into_iter()
            .next()
            .ok_or_else(|| "No diff output".to_string())?;
        for h in &mut entry.hunks {
            h.origin = HunkOrigin::Staged;
        }
        Ok(entry)
    }

    /// Combined uncommitted diff: loads both staged (`git diff --cached`) and
    /// unstaged (`git diff`) hunks for a single file and merges them into one
    /// `DiffEntry`, with each hunk tagged via its `origin` field. Hunks are
    /// emitted in linear order by `new_start` so the Uncommitted view can show
    /// them as one stream with per-hunk staged/unstaged indicators.
    ///
    /// Returns `Ok(None)` if neither side has any changes for this file.
    pub fn load_combined_uncommitted_for_file(
        repo_path: &Path,
        file_path: &str,
        context_lines: u32,
    ) -> Result<Option<Self>, String> {
        let staged = Self::load_staged_for_file_with_context(repo_path, file_path, context_lines)
            .ok()
            .filter(|e| !e.hunks.is_empty());
        let unstaged =
            Self::load_unstaged_for_file_with_context(repo_path, file_path, context_lines)
                .ok()
                .filter(|e| !e.hunks.is_empty());

        match (staged, unstaged) {
            (None, None) => Ok(None),
            (Some(s), None) => Ok(Some(s)),
            (None, Some(u)) => Ok(Some(u)),
            (Some(mut s), Some(u)) => {
                s.hunks.extend(u.hunks);
                s.hunks.sort_by_key(|h| h.new_start);
                Ok(Some(s))
            }
        }
    }

    pub fn load_for_stash(repo_path: &Path, stash_ref: &str) -> Result<Vec<Self>, String> {
        let parent = format!("{}^1", stash_ref);
        let output = Command::new("git")
            .args(["diff", &parent, stash_ref])
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git diff for stash: {}", e))?;
        if !output.status.success() {
            return Err(format!(
                "git diff failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        parse_diff(&String::from_utf8_lossy(&output.stdout))
    }

    /// Returns a placeholder DiffEntry with a "Cannot open" binary note.
    pub fn binary_placeholder(file_path: &str) -> Self {
        DiffEntry {
            old_path: None,
            new_path: Some(file_path.to_string()),
            hunks: vec![Hunk {
                old_start: 0,
                old_count: 0,
                new_start: 0,
                new_count: 0,
                lines: vec![DiffLine {
                    line_type: DiffLineType::BinaryNote,
                    old_line_no: None,
                    new_line_no: None,
                    content: "Cannot open this type of file".to_string(),
                    highlight_ranges: Vec::new(),
                }],
                origin: HunkOrigin::Other,
            }],
        }
    }

    /// Load an untracked (new) file as a diff-like view with all lines as additions.
    /// Returns Err("binary") for binary/non-text files, Err(msg) for other errors.
    pub fn load_untracked_file(repo_path: &Path, file_path: &str) -> Result<Self, String> {
        let full_path = repo_path.join(file_path);

        // Binary detection: check file extension first (fast path)
        let is_binary_ext = is_binary_extension(file_path);
        if is_binary_ext {
            return Err("binary".to_string());
        }

        // Read the file — detect binary by scanning for null bytes in the first 8KB
        let content = std::fs::read(&full_path)
            .map_err(|e| format!("Cannot read file: {}", e))?;

        if content.contains(&0u8) {
            return Err("binary".to_string());
        }

        let text = String::from_utf8_lossy(&content);
        let lines: Vec<DiffLine> = text
            .lines()
            .enumerate()
            .map(|(i, line)| DiffLine {
                line_type: DiffLineType::Addition,
                old_line_no: None,
                new_line_no: Some(i as u32 + 1),
                content: line.to_string(),
                highlight_ranges: Vec::new(),
            })
            .collect();

        let total = lines.len() as u32;
        let hunk = Hunk {
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: total,
            lines,
            origin: HunkOrigin::Untracked,
        };

        Ok(DiffEntry {
            old_path: Some("/dev/null".to_string()),
            new_path: Some(file_path.to_string()),
            hunks: vec![hunk],
        })
    }
}

pub fn is_binary_extension(file_path: &str) -> bool {
    let ext = std::path::Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "webp" | "tiff" | "svg"
            | "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx"
            | "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar"
            | "exe" | "dll" | "so" | "dylib" | "bin" | "obj" | "o" | "a"
            | "mp3" | "mp4" | "wav" | "ogg" | "flac" | "avi" | "mkv" | "mov"
            | "ttf" | "otf" | "woff" | "woff2"
            | "db" | "sqlite" | "pyc" | "class"
    )
}

fn parse_diff(input: &str) -> Result<Vec<DiffEntry>, String> {
    let mut entries = Vec::new();
    let mut current_entry: Option<DiffEntry> = None;
    let mut current_hunk: Option<Hunk> = None;
    let mut old_line = 0u32;
    let mut new_line = 0u32;

    let hunk_re = Regex::new(r"@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap();

    for line in input.lines() {
        if line.starts_with("diff --git") {
            if let Some(hunk) = current_hunk.take() {
                if let Some(entry) = current_entry.as_mut() {
                    entry.hunks.push(hunk);
                }
            }
            if let Some(entry) = current_entry.take() {
                entries.push(entry);
            }
            current_entry = Some(DiffEntry {
                old_path: None,
                new_path: None,
                hunks: Vec::new(),
            });
        } else if let Some(rest) = line.strip_prefix("--- ") {
            if let Some(entry) = current_entry.as_mut() {
                let path = rest.strip_prefix("a/").unwrap_or(rest);
                if path != "/dev/null" {
                    entry.old_path = Some(path.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            if let Some(entry) = current_entry.as_mut() {
                let path = rest.strip_prefix("b/").unwrap_or(rest);
                if path != "/dev/null" {
                    entry.new_path = Some(path.to_string());
                }
            }
        } else if line.starts_with("@@") {
            if let Some(hunk) = current_hunk.take() {
                if let Some(entry) = current_entry.as_mut() {
                    entry.hunks.push(hunk);
                }
            }
            if let Some(caps) = hunk_re.captures(line) {
                old_line = caps[1].parse().unwrap_or(1);
                new_line = caps[3].parse().unwrap_or(1);
                let oc = caps
                    .get(2)
                    .map(|m| m.as_str().parse().unwrap_or(1))
                    .unwrap_or(1);
                let nc = caps
                    .get(4)
                    .map(|m| m.as_str().parse().unwrap_or(1))
                    .unwrap_or(1);
                current_hunk = Some(Hunk {
                    old_start: old_line,
                    old_count: oc,
                    new_start: new_line,
                    new_count: nc,
                    lines: vec![DiffLine {
                        line_type: DiffLineType::HunkHeader,
                        old_line_no: None,
                        new_line_no: None,
                        content: line.to_string(),
                        highlight_ranges: Vec::new(),
                    }],
                    origin: HunkOrigin::Other,
                });
            }
        } else if let Some(hunk) = current_hunk.as_mut() {
            if let Some(content) = line.strip_prefix('+') {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::Addition,
                    old_line_no: None,
                    new_line_no: Some(new_line),
                    content: content.to_string(),
                    highlight_ranges: Vec::new(),
                });
                new_line += 1;
            } else if let Some(content) = line.strip_prefix('-') {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::Deletion,
                    old_line_no: Some(old_line),
                    new_line_no: None,
                    content: content.to_string(),
                    highlight_ranges: Vec::new(),
                });
                old_line += 1;
            } else if let Some(content) = line.strip_prefix(' ') {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::Context,
                    old_line_no: Some(old_line),
                    new_line_no: Some(new_line),
                    content: content.to_string(),
                    highlight_ranges: Vec::new(),
                });
                old_line += 1;
                new_line += 1;
            } else if line.starts_with("Binary files") {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::BinaryNote,
                    old_line_no: None,
                    new_line_no: None,
                    content: line.to_string(),
                    highlight_ranges: Vec::new(),
                });
            }
        }
    }

    if let Some(hunk) = current_hunk {
        if let Some(entry) = current_entry.as_mut() {
            entry.hunks.push(hunk);
        }
    }
    if let Some(entry) = current_entry {
        entries.push(entry);
    }

    // Annotate matched deletion/addition pairs with intra-line changed ranges.
    for entry in &mut entries {
        for hunk in &mut entry.hunks {
            annotate_intra_line_diff(hunk);
        }
    }

    Ok(entries)
}

/// For each group of consecutive deletions followed by consecutive additions in a hunk,
/// pair them up and compute which byte ranges changed within each matched line.
fn annotate_intra_line_diff(hunk: &mut Hunk) {
    let n = hunk.lines.len();
    let mut i = 0;
    while i < n {
        if hunk.lines[i].line_type != DiffLineType::Deletion {
            i += 1;
            continue;
        }
        let del_start = i;
        while i < n && hunk.lines[i].line_type == DiffLineType::Deletion {
            i += 1;
        }
        let del_end = i;
        let add_start = i;
        while i < n && hunk.lines[i].line_type == DiffLineType::Addition {
            i += 1;
        }
        let add_end = i;

        let n_pairs = (del_end - del_start).min(add_end - add_start);
        for k in 0..n_pairs {
            let del_content = hunk.lines[del_start + k].content.clone();
            let add_content = hunk.lines[add_start + k].content.clone();
            let (del_ranges, add_ranges) = diff_intra_line(&del_content, &add_content);
            hunk.lines[del_start + k].highlight_ranges = del_ranges;
            hunk.lines[add_start + k].highlight_ranges = add_ranges;
        }
    }
}

/// Compute the changed byte ranges within a matched old/new line pair using
/// common-prefix + common-suffix analysis. Returns `(old_ranges, new_ranges)`.
fn diff_intra_line(
    old: &str,
    new: &str,
) -> (Vec<std::ops::Range<usize>>, Vec<std::ops::Range<usize>>) {
    if old.is_empty() || new.is_empty() || old == new {
        return (Vec::new(), Vec::new());
    }

    // Count matching chars from the start.
    let prefix_chars = old.chars().zip(new.chars()).take_while(|(a, b)| a == b).count();
    let prefix_old_bytes: usize = old.chars().take(prefix_chars).map(|c| c.len_utf8()).sum();
    let prefix_new_bytes: usize = new.chars().take(prefix_chars).map(|c| c.len_utf8()).sum();

    // Count matching chars from the end (in the remainder after prefix).
    let old_rest: Vec<char> = old[prefix_old_bytes..].chars().collect();
    let new_rest: Vec<char> = new[prefix_new_bytes..].chars().collect();

    let suffix_chars = old_rest
        .iter()
        .rev()
        .zip(new_rest.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let suffix_old_bytes: usize = old_rest.iter().rev().take(suffix_chars).map(|c| c.len_utf8()).sum();
    let suffix_new_bytes: usize = new_rest.iter().rev().take(suffix_chars).map(|c| c.len_utf8()).sum();

    let old_end = old.len() - suffix_old_bytes;
    let new_end = new.len() - suffix_new_bytes;

    let old_range = prefix_old_bytes..old_end;
    let new_range = prefix_new_bytes..new_end;

    let old_ranges = if !old_range.is_empty() { vec![old_range] } else { vec![] };
    let new_ranges = if !new_range.is_empty() { vec![new_range] } else { vec![] };

    (old_ranges, new_ranges)
}

#[cfg(test)]
mod intra_line_tests {
    use super::*;

    #[test]
    fn identical_lines_produce_no_ranges() {
        let (old, new) = diff_intra_line("foo bar", "foo bar");
        assert!(old.is_empty());
        assert!(new.is_empty());
    }

    #[test]
    fn single_word_change() {
        let (old, new) = diff_intra_line("foo bar baz", "foo qux baz");
        assert_eq!(old, vec![4..7]);
        assert_eq!(new, vec![4..7]);
    }

    #[test]
    fn suffix_only_change() {
        let (old, new) = diff_intra_line("hello world", "hello rust");
        assert_eq!(old, vec![6..11]);
        assert_eq!(new, vec![6..10]);
    }

    #[test]
    fn prefix_only_change() {
        let (old, new) = diff_intra_line("old thing", "new thing");
        assert_eq!(old, vec![0..3]);
        assert_eq!(new, vec![0..3]);
    }

    #[test]
    fn entirely_different_lines() {
        let (old, new) = diff_intra_line("abc", "xyz");
        assert_eq!(old, vec![0..3]);
        assert_eq!(new, vec![0..3]);
    }

    #[test]
    fn empty_old_produces_no_ranges() {
        let (old, new) = diff_intra_line("", "new");
        assert!(old.is_empty());
        assert!(new.is_empty());
    }
}

impl DiffEntry {
    pub fn count_additions_and_deletions(&self) -> (usize, usize) {
        let mut additions = 0;
        let mut deletions = 0;

        for hunk in &self.hunks {
            for line in &hunk.lines {
                match line.line_type {
                    DiffLineType::Addition => additions += 1,
                    DiffLineType::Deletion => deletions += 1,
                    _ => {}
                }
            }
        }

        (additions, deletions)
    }
}
