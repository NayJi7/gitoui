//! `git blame --porcelain` wrapper.
//!
//! Returns a flat `Vec<BlameLine>` covering every line of the file at HEAD,
//! each annotated with the commit that last touched it. Built for the
//! dedicated `BlameView` — see `src/view/blame.rs`.
//!
//! Porcelain format (per `git-blame(1)`):
//! ```text
//! <40-char SHA> <orig-line> <final-line> [<num-lines>]
//! author <name>
//! author-mail <email>
//! author-time <timestamp>
//! author-tz <tz>
//! committer <name>
//! ...
//! summary <subject>
//! previous <sha> <filename>          # optional
//! filename <name>
//! <TAB><content>
//! ```
//! Subsequent lines from the same commit reuse the header — only the
//! `<sha> <orig> <final>` line and the `\t<content>` line are emitted.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use chrono::{DateTime, Local};

#[derive(Debug, Clone)]
pub struct BlameLine {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    /// Lowercase e-mail address stripped of angle brackets, e.g.
    /// `alice@example.com`. Empty string when git didn't provide one.
    pub author_mail: String,
    /// Author time in the local timezone. None if porcelain didn't provide
    /// it (defensive — shouldn't happen with vanilla git).
    pub author_time: Option<DateTime<Local>>,
    pub summary: String,
    /// 1-based line number in the file at HEAD.
    pub line_no: u32,
    pub content: String,
    /// True when this line falls on a boundary commit (rare — usually means
    /// the line was last touched at a commit outside the visible history).
    #[allow(dead_code)]
    pub boundary: bool,
}

/// Aggregated header for a single commit — built up as we parse the first
/// occurrence's full porcelain record and reused for all subsequent lines
/// from the same SHA.
#[derive(Debug, Clone, Default)]
struct CommitHeader {
    author: String,
    author_mail: String,
    author_time: Option<i64>,
    summary: String,
    boundary: bool,
}

pub fn load_blame(repo_path: &Path, file_path: &str) -> Result<Vec<BlameLine>, String> {
    let output = Command::new("git")
        .args(["blame", "--porcelain", "-w", "--", file_path])
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to run git blame: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "git blame failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(parse_porcelain(&String::from_utf8_lossy(&output.stdout)))
}

pub fn parse_porcelain(input: &str) -> Vec<BlameLine> {
    let mut out: Vec<BlameLine> = Vec::new();
    let mut headers: HashMap<String, CommitHeader> = HashMap::new();

    let mut iter = input.lines().peekable();
    while let Some(line) = iter.next() {
        // Header line: "<sha> <orig> <final> [<num>]"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 || parts[0].len() != 40 {
            // Not a hash header — skip stray lines (defensive).
            continue;
        }
        let sha = parts[0].to_string();
        let final_line: u32 = parts[2].parse().unwrap_or(0);

        let mut header = headers.get(&sha).cloned().unwrap_or_default();
        // Walk meta lines until we hit the `\t<content>` line that closes the
        // record. Meta lines are key/value separated by a space.
        let mut content = String::new();
        while let Some(&next) = iter.peek() {
            if let Some(rest) = next.strip_prefix('\t') {
                content = rest.to_string();
                iter.next();
                break;
            }
            iter.next(); // consume the meta line
            if let Some((key, value)) = next.split_once(' ') {
                match key {
                    "author" => header.author = value.to_string(),
                    "author-mail" => {
                        // git outputs `<email@domain>` — strip angle brackets.
                        header.author_mail = value
                            .trim_start_matches('<')
                            .trim_end_matches('>')
                            .to_lowercase();
                    }
                    "author-time" => header.author_time = value.parse().ok(),
                    "summary" => header.summary = value.to_string(),
                    _ => {}
                }
            } else if next == "boundary" {
                header.boundary = true;
            }
        }

        // Cache the header — subsequent occurrences of this SHA reuse it.
        headers.insert(sha.clone(), header.clone());

        let short_hash: String = sha.chars().take(7).collect();
        let author_time = header
            .author_time
            .and_then(|t| DateTime::from_timestamp(t, 0).map(|dt| dt.with_timezone(&Local)));

        out.push(BlameLine {
            hash: sha,
            short_hash,
            author: header.author.clone(),
            author_mail: header.author_mail.clone(),
            author_time,
            summary: header.summary.clone(),
            line_no: final_line,
            content,
            boundary: header.boundary,
        });
    }

    out
}

/// Compact "x ago" formatter — keeps the annotation column narrow. Falls back
/// to "—" when the timestamp wasn't parsed.
pub fn relative_time(dt: Option<&DateTime<Local>>) -> String {
    let Some(dt) = dt else {
        return "—".to_string();
    };
    let now = Local::now();
    let delta = now.signed_duration_since(*dt);
    let secs = delta.num_seconds();
    if secs < 60 {
        format!("{}s ago", secs.max(0))
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else if secs < 86_400 * 30 {
        format!("{}d ago", secs / 86_400)
    } else if secs < 86_400 * 365 {
        format!("{}mo ago", secs / (86_400 * 30))
    } else {
        format!("{}y ago", secs / (86_400 * 365))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_line_with_full_header() {
        let sample = "\
abc1234567890abcdef1234567890abcdef12345 1 1 1
author Alice
author-mail <alice@example.com>
author-time 1700000000
author-tz +0000
committer Alice
committer-mail <alice@example.com>
committer-time 1700000000
committer-tz +0000
summary initial commit
filename a.txt
\thello world
";
        let lines = parse_porcelain(sample);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].hash, "abc1234567890abcdef1234567890abcdef12345");
        assert_eq!(lines[0].short_hash, "abc1234");
        assert_eq!(lines[0].author, "Alice");
        assert_eq!(lines[0].summary, "initial commit");
        assert_eq!(lines[0].line_no, 1);
        assert_eq!(lines[0].content, "hello world");
    }

    #[test]
    fn reuses_header_for_subsequent_lines_of_same_commit() {
        let sample = "\
abc1234567890abcdef1234567890abcdef12345 1 1 3
author Alice
author-time 1700000000
summary first
filename a.txt
\tline one
abc1234567890abcdef1234567890abcdef12345 2 2
\tline two
abc1234567890abcdef1234567890abcdef12345 3 3
\tline three
";
        let lines = parse_porcelain(sample);
        assert_eq!(lines.len(), 3);
        for l in &lines {
            assert_eq!(l.author, "Alice", "header must be reused");
            assert_eq!(l.summary, "first");
        }
        assert_eq!(lines[0].line_no, 1);
        assert_eq!(lines[2].line_no, 3);
        assert_eq!(lines[1].content, "line two");
    }

    #[test]
    fn handles_interleaved_commits() {
        let sample = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 1 1 1
author Alice
author-time 1700000000
summary first
filename a.txt
\talpha
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb 2 2 1
author Bob
author-time 1700000100
summary second
filename a.txt
\tbeta
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa 3 3
\tgamma
";
        let lines = parse_porcelain(sample);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].author, "Alice");
        assert_eq!(lines[1].author, "Bob");
        assert_eq!(
            lines[2].author, "Alice",
            "third line reuses Alice's cached header"
        );
    }

    #[test]
    fn empty_blame_output_yields_empty_vec() {
        assert!(parse_porcelain("").is_empty());
    }

    #[test]
    fn relative_time_buckets() {
        // Use a fixed point in time to drive deterministic comparisons.
        let now = Local::now();
        let mins_ago = now - chrono::Duration::minutes(5);
        assert!(relative_time(Some(&mins_ago)).contains("m ago"));
        let hours_ago = now - chrono::Duration::hours(3);
        assert!(relative_time(Some(&hours_ago)).contains("h ago"));
        let days_ago = now - chrono::Duration::days(2);
        assert!(relative_time(Some(&days_ago)).contains("d ago"));
        assert_eq!(relative_time(None), "—");
    }
}
