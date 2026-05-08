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
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub line_type: DiffLineType,
    pub old_line_no: Option<u32>,
    pub new_line_no: Option<u32>,
    pub content: String,
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
        entries
            .into_iter()
            .next()
            .ok_or_else(|| "No diff output".to_string())
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
        entries
            .into_iter()
            .next()
            .ok_or_else(|| "No diff output".to_string())
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
                }],
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
            })
            .collect();

        let total = lines.len() as u32;
        let hunk = Hunk {
            old_start: 0,
            old_count: 0,
            new_start: 1,
            new_count: total,
            lines,
        };

        Ok(DiffEntry {
            old_path: Some("/dev/null".to_string()),
            new_path: Some(file_path.to_string()),
            hunks: vec![hunk],
        })
    }
}

fn is_binary_extension(file_path: &str) -> bool {
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
                    }],
                });
            }
        } else if let Some(hunk) = current_hunk.as_mut() {
            if let Some(content) = line.strip_prefix('+') {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::Addition,
                    old_line_no: None,
                    new_line_no: Some(new_line),
                    content: content.to_string(),
                });
                new_line += 1;
            } else if let Some(content) = line.strip_prefix('-') {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::Deletion,
                    old_line_no: Some(old_line),
                    new_line_no: None,
                    content: content.to_string(),
                });
                old_line += 1;
            } else if let Some(content) = line.strip_prefix(' ') {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::Context,
                    old_line_no: Some(old_line),
                    new_line_no: Some(new_line),
                    content: content.to_string(),
                });
                old_line += 1;
                new_line += 1;
            } else if line.starts_with("Binary files") {
                hunk.lines.push(DiffLine {
                    line_type: DiffLineType::BinaryNote,
                    old_line_no: None,
                    new_line_no: None,
                    content: line.to_string(),
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

    Ok(entries)
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
