//! Parser and resolver for merge-conflict files.
//!
//! When git fails to merge a hunk, it inserts conflict markers into the working
//! tree file. We support both the default 2-way style and the diff3 style:
//!
//! ```text
//! <<<<<<< HEAD
//! ours
//! =======          (or ||||||| base /// base lines /// ======= for diff3)
//! theirs
//! >>>>>>> branch
//! ```
//!
//! The parser walks the file once, alternating between "context" segments
//! (non-conflicted lines) and "conflict hunks". The resolver then renders the
//! file with each hunk replaced by the user's chosen resolution.

use std::path::Path;

/// Which side of a conflict the user wants to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HunkResolution {
    /// No decision yet — file is not safe to save.
    #[default]
    Unresolved,
    /// Keep only `ours` (HEAD side).
    Ours,
    /// Keep only `theirs` (incoming side).
    Theirs,
    /// Keep both, ours first then theirs.
    BothOursFirst,
    /// Keep both, theirs first then ours.
    BothTheirsFirst,
}

/// A single `<<<<<<<` … `>>>>>>>` conflict region in a file.
#[derive(Debug, Clone)]
pub struct ConflictHunk {
    /// Lines on the HEAD/ours side (without the `<<<<<<<` and `=======` markers).
    pub ours: Vec<String>,
    /// Lines from the common ancestor when the file uses diff3 conflict style,
    /// otherwise `None`. Always trim-only — never used for resolution output.
    pub base: Option<Vec<String>>,
    /// Lines on the incoming/theirs side (without the `=======` and `>>>>>>>` markers).
    pub theirs: Vec<String>,
    /// Free-form label that follows `<<<<<<<` (typically `HEAD` or a branch name).
    pub ours_label: String,
    /// Free-form label that follows `>>>>>>>` (typically the incoming branch name).
    pub theirs_label: String,
    /// User-chosen resolution. Defaults to [`HunkResolution::Unresolved`].
    pub resolution: HunkResolution,
}

/// One segment of the parsed file: either plain non-conflicted lines or a hunk.
#[derive(Debug, Clone)]
pub enum FileSegment {
    /// Untouched lines copied verbatim into the resolved output.
    Context(Vec<String>),
    /// Conflicted region resolved according to `hunk.resolution`.
    Hunk(ConflictHunk),
}

/// A parsed conflict file split into context and hunk segments.
#[derive(Debug, Clone, Default)]
pub struct ConflictFile {
    pub path: String,
    pub segments: Vec<FileSegment>,
}

impl ConflictFile {
    /// Returns indices of the segments that are hunks, in document order.
    pub fn hunk_indices(&self) -> Vec<usize> {
        self.segments
            .iter()
            .enumerate()
            .filter_map(|(i, s)| matches!(s, FileSegment::Hunk(_)).then_some(i))
            .collect()
    }

    /// Total number of hunks in the file.
    pub fn hunk_count(&self) -> usize {
        self.segments
            .iter()
            .filter(|s| matches!(s, FileSegment::Hunk(_)))
            .count()
    }

    /// Returns the n-th hunk (0-indexed) or `None`.
    pub fn hunk(&self, n: usize) -> Option<&ConflictHunk> {
        self.segments
            .iter()
            .filter_map(|s| match s {
                FileSegment::Hunk(h) => Some(h),
                _ => None,
            })
            .nth(n)
    }

    /// Mutable access to the n-th hunk.
    pub fn hunk_mut(&mut self, n: usize) -> Option<&mut ConflictHunk> {
        self.segments
            .iter_mut()
            .filter_map(|s| match s {
                FileSegment::Hunk(h) => Some(h),
                _ => None,
            })
            .nth(n)
    }

    /// True when every hunk has a non-`Unresolved` resolution.
    pub fn is_fully_resolved(&self) -> bool {
        self.segments.iter().all(|s| match s {
            FileSegment::Hunk(h) => h.resolution != HunkResolution::Unresolved,
            _ => true,
        })
    }

    /// Number of hunks still waiting for a decision.
    pub fn unresolved_count(&self) -> usize {
        self.segments
            .iter()
            .filter(|s| match s {
                FileSegment::Hunk(h) => h.resolution == HunkResolution::Unresolved,
                _ => false,
            })
            .count()
    }

    /// Render the resolved file content as a single string with `\n` line endings
    /// and a trailing newline (matching git's expectation). Unresolved hunks fall
    /// back to keeping `ours` so this still produces a parseable file, but callers
    /// should check `is_fully_resolved()` before saving.
    pub fn render_resolved(&self) -> String {
        let mut out = String::new();
        for seg in &self.segments {
            match seg {
                FileSegment::Context(lines) => {
                    for line in lines {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
                FileSegment::Hunk(h) => {
                    let pick: &[String] = match h.resolution {
                        HunkResolution::Ours | HunkResolution::Unresolved => &h.ours,
                        HunkResolution::Theirs => &h.theirs,
                        HunkResolution::BothOursFirst => {
                            for l in &h.ours {
                                out.push_str(l);
                                out.push('\n');
                            }
                            for l in &h.theirs {
                                out.push_str(l);
                                out.push('\n');
                            }
                            continue;
                        }
                        HunkResolution::BothTheirsFirst => {
                            for l in &h.theirs {
                                out.push_str(l);
                                out.push('\n');
                            }
                            for l in &h.ours {
                                out.push_str(l);
                                out.push('\n');
                            }
                            continue;
                        }
                    };
                    for line in pick {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
        }
        out
    }
}

/// Parses raw text containing conflict markers into a [`ConflictFile`].
///
/// Marker prefixes are detected as the start of a line; trailing labels are
/// captured but optional. Standard markers use exactly 7 angle brackets — we
/// match `>= 7` to be forgiving but the renderer always re-emits 7.
pub fn parse_conflict_text(path: &str, text: &str) -> ConflictFile {
    let mut segments: Vec<FileSegment> = Vec::new();
    let mut context: Vec<String> = Vec::new();

    let lines: Vec<&str> = text.split('\n').collect();
    // Drop the trailing empty line that `split('\n')` produces from a `\n`-terminated string.
    let last_idx = if text.ends_with('\n') {
        lines.len().saturating_sub(1)
    } else {
        lines.len()
    };

    let mut i = 0usize;
    while i < last_idx {
        let line = lines[i];
        if let Some(label) = strip_marker_prefix(line, '<') {
            // Flush context before opening a new hunk.
            if !context.is_empty() {
                segments.push(FileSegment::Context(std::mem::take(&mut context)));
            }

            let mut ours: Vec<String> = Vec::new();
            let mut base: Option<Vec<String>> = None;
            let mut theirs: Vec<String> = Vec::new();
            let mut theirs_label = String::new();
            i += 1;

            // Phase 1: accumulate ours until ||||||| (diff3) or =======.
            while i < last_idx {
                let l = lines[i];
                if strip_marker_prefix(l, '|').is_some() {
                    base = Some(Vec::new());
                    i += 1;
                    break;
                }
                if strip_marker_prefix(l, '=').is_some() {
                    i += 1;
                    // base stays None; jump to theirs.
                    break;
                }
                if strip_marker_prefix(l, '>').is_some() {
                    // Malformed: closing without a separator. Treat ours as theirs-end.
                    theirs_label = strip_marker_prefix(l, '>').unwrap_or_default();
                    segments.push(FileSegment::Hunk(ConflictHunk {
                        ours,
                        base: None,
                        theirs: Vec::new(),
                        ours_label: label,
                        theirs_label,
                        resolution: HunkResolution::Unresolved,
                    }));
                    return ConflictFile {
                        path: path.to_string(),
                        segments,
                    };
                }
                ours.push(l.to_string());
                i += 1;
            }

            // Phase 2 (diff3 only): accumulate base lines until =======.
            if let Some(base_lines) = base.as_mut() {
                while i < last_idx {
                    let l = lines[i];
                    if strip_marker_prefix(l, '=').is_some() {
                        i += 1;
                        break;
                    }
                    base_lines.push(l.to_string());
                    i += 1;
                }
            }

            // Phase 3: accumulate theirs until >>>>>>>.
            while i < last_idx {
                let l = lines[i];
                if let Some(lbl) = strip_marker_prefix(l, '>') {
                    theirs_label = lbl;
                    i += 1;
                    break;
                }
                theirs.push(l.to_string());
                i += 1;
            }

            segments.push(FileSegment::Hunk(ConflictHunk {
                ours,
                base,
                theirs,
                ours_label: label,
                theirs_label,
                resolution: HunkResolution::Unresolved,
            }));
        } else {
            context.push(line.to_string());
            i += 1;
        }
    }

    if !context.is_empty() {
        segments.push(FileSegment::Context(context));
    }

    ConflictFile {
        path: path.to_string(),
        segments,
    }
}

/// Strip a conflict-marker prefix and return the trailing label (or empty string
/// when there is no label). Returns `None` when the line does not start with at
/// least 7 occurrences of `ch`.
fn strip_marker_prefix(line: &str, ch: char) -> Option<String> {
    let count = line.chars().take_while(|c| *c == ch).count();
    if count < 7 {
        return None;
    }
    let rest = &line[count..];
    Some(rest.trim_start().to_string())
}

/// Loads a conflict file from disk and parses it.
pub fn parse_conflict_path(path: &Path) -> std::io::Result<ConflictFile> {
    let display = path.to_string_lossy().into_owned();
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(parse_conflict_text(&display, &text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ours(h: &ConflictHunk) -> Vec<&str> {
        h.ours.iter().map(|s| s.as_str()).collect()
    }
    fn theirs(h: &ConflictHunk) -> Vec<&str> {
        h.theirs.iter().map(|s| s.as_str()).collect()
    }

    #[test]
    fn parses_two_way_conflict() {
        let text = "\
common before
<<<<<<< HEAD
my line
=======
their line
>>>>>>> feature
common after
";
        let f = parse_conflict_text("a.txt", text);
        assert_eq!(f.hunk_count(), 1);
        let h = f.hunk(0).unwrap();
        assert_eq!(ours(h), vec!["my line"]);
        assert_eq!(theirs(h), vec!["their line"]);
        assert!(h.base.is_none());
        assert_eq!(h.ours_label, "HEAD");
        assert_eq!(h.theirs_label, "feature");
    }

    #[test]
    fn parses_diff3_with_base() {
        let text = "\
<<<<<<< HEAD
mine
||||||| merged common ancestors
base text
=======
theirs text
>>>>>>> other
";
        let f = parse_conflict_text("a.txt", text);
        let h = f.hunk(0).unwrap();
        assert_eq!(ours(h), vec!["mine"]);
        assert_eq!(theirs(h), vec!["theirs text"]);
        assert!(h.base.is_some());
        assert_eq!(
            h.base
                .as_ref()
                .unwrap()
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec!["base text"]
        );
    }

    #[test]
    fn parses_multiple_hunks_with_context() {
        let text = "\
line 1
<<<<<<< HEAD
A1
A2
=======
B1
B2
>>>>>>> br
between
<<<<<<< HEAD
C
=======
D
>>>>>>> br
end
";
        let f = parse_conflict_text("x", text);
        assert_eq!(f.hunk_count(), 2);
        assert_eq!(ours(f.hunk(0).unwrap()), vec!["A1", "A2"]);
        assert_eq!(theirs(f.hunk(1).unwrap()), vec!["D"]);
        // Context segments should preserve the surrounding lines.
        let context_lines: Vec<&str> = f
            .segments
            .iter()
            .filter_map(|s| match s {
                FileSegment::Context(c) => Some(c.iter().map(|s| s.as_str()).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(context_lines, vec!["line 1", "between", "end"]);
    }

    #[test]
    fn renders_ours_resolution() {
        let text = "\
before
<<<<<<< HEAD
mine
=======
theirs
>>>>>>> b
after
";
        let mut f = parse_conflict_text("x", text);
        f.hunk_mut(0).unwrap().resolution = HunkResolution::Ours;
        assert_eq!(f.render_resolved(), "before\nmine\nafter\n");
    }

    #[test]
    fn renders_theirs_resolution() {
        let text = "\
<<<<<<< HEAD
mine
=======
theirs
>>>>>>> b
";
        let mut f = parse_conflict_text("x", text);
        f.hunk_mut(0).unwrap().resolution = HunkResolution::Theirs;
        assert_eq!(f.render_resolved(), "theirs\n");
    }

    #[test]
    fn renders_both_ours_first() {
        let text = "\
<<<<<<< HEAD
A
=======
B
>>>>>>> b
";
        let mut f = parse_conflict_text("x", text);
        f.hunk_mut(0).unwrap().resolution = HunkResolution::BothOursFirst;
        assert_eq!(f.render_resolved(), "A\nB\n");
    }

    #[test]
    fn renders_both_theirs_first() {
        let text = "\
<<<<<<< HEAD
A
=======
B
>>>>>>> b
";
        let mut f = parse_conflict_text("x", text);
        f.hunk_mut(0).unwrap().resolution = HunkResolution::BothTheirsFirst;
        assert_eq!(f.render_resolved(), "B\nA\n");
    }

    #[test]
    fn fully_resolved_detection() {
        let text = "\
<<<<<<< HEAD
A
=======
B
>>>>>>> b
context
<<<<<<< HEAD
C
=======
D
>>>>>>> b
";
        let mut f = parse_conflict_text("x", text);
        assert!(!f.is_fully_resolved());
        assert_eq!(f.unresolved_count(), 2);
        f.hunk_mut(0).unwrap().resolution = HunkResolution::Ours;
        assert!(!f.is_fully_resolved());
        f.hunk_mut(1).unwrap().resolution = HunkResolution::Theirs;
        assert!(f.is_fully_resolved());
        assert_eq!(f.unresolved_count(), 0);
    }

    #[test]
    fn handles_file_without_trailing_newline() {
        let text = "a\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> b";
        let f = parse_conflict_text("x", text);
        assert_eq!(f.hunk_count(), 1);
    }

    #[test]
    fn handles_no_conflicts() {
        let f = parse_conflict_text("x", "just\nplain\ntext\n");
        assert_eq!(f.hunk_count(), 0);
        assert_eq!(f.render_resolved(), "just\nplain\ntext\n");
    }
}
