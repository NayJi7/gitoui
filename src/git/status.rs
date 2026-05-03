use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use chrono::{DateTime, Local};

#[derive(Debug, Clone, Default)]
pub struct UncommittedChanges {
    pub staged: Vec<FileStatus>,
    pub unstaged: Vec<FileStatus>,
    pub untracked: Vec<FileStatus>,
    /// Date de dernière modification parmi tous les fichiers uncommitted
    pub last_modified: Option<DateTime<Local>>,
}

#[derive(Debug, Clone)]
pub struct FileStatus {
    pub status: StatusType,
    pub path: String,
    pub old_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StatusType {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
}

impl UncommittedChanges {
    pub fn load(repo_path: &Path) -> Result<Self, String> {
        let output = Command::new("git")
            .args(["status", "--porcelain=v2", "--branch"])
            .current_dir(repo_path)
            .output()
            .map_err(|e| format!("Failed to run git status: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "git status failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let mut changes = UncommittedChanges::default();
        let stdout = String::from_utf8_lossy(&output.stdout);

        for line in stdout.lines() {
            if let Some(rest) = line.strip_prefix("1 ") {
                let parts: Vec<&str> = rest.splitn(8, ' ').collect();
                if parts.len() >= 8 {
                    let xy = parts[0];
                    let path = parts[7].to_string();
                    let bytes = xy.as_bytes();
                    if bytes.len() >= 2 {
                        let (x, y) = (bytes[0], bytes[1]);
                        if x != b'.' && x != b'?' {
                            changes.staged.push(FileStatus {
                                status: StatusType::from_index_char(x),
                                path: path.clone(),
                                old_path: None,
                            });
                        }
                        if y != b'.' && y != b'?' {
                            changes.unstaged.push(FileStatus {
                                status: StatusType::from_worktree_char(y),
                                path,
                                old_path: None,
                            });
                        }
                    }
                }
            } else if let Some(rest) = line.strip_prefix("2 ") {
                let parts: Vec<&str> = rest.splitn(9, ' ').collect();
                if parts.len() >= 9 {
                    let xy = parts[0];
                    let orig_path = parts[7].to_string();
                    let new_path = parts[8].to_string();
                    let bytes = xy.as_bytes();
                    if bytes.len() >= 2 {
                        let (x, y) = (bytes[0], bytes[1]);
                        if x != b'.' {
                            changes.staged.push(FileStatus {
                                status: StatusType::from_index_char(x),
                                path: new_path.clone(),
                                old_path: Some(orig_path.clone()),
                            });
                        }
                        if y != b'.' {
                            changes.unstaged.push(FileStatus {
                                status: StatusType::from_worktree_char(y),
                                path: new_path,
                                old_path: Some(orig_path),
                            });
                        }
                    }
                }
            } else if let Some(rest) = line.strip_prefix("u ") {
                let parts: Vec<&str> = rest.splitn(10, ' ').collect();
                if parts.len() >= 10 {
                    let xy = parts[0];
                    let status = if xy.starts_with('D') || xy.ends_with('D') {
                        StatusType::Deleted
                    } else if xy.contains('A') {
                        StatusType::Added
                    } else {
                        StatusType::Unmerged
                    };
                    changes.unstaged.push(FileStatus {
                        status,
                        path: parts[9].to_string(),
                        old_path: None,
                    });
                }
            } else if let Some(path) = line.strip_prefix("? ") {
                if !path.is_empty() {
                    changes.untracked.push(FileStatus {
                        status: StatusType::Untracked,
                        path: path.to_string(),
                        old_path: None,
                    });
                }
            }
        }

        // Calculer la date de dernière modification parmi tous les fichiers
        let mut max_mtime: Option<SystemTime> = None;
        for file_status in changes.staged.iter().chain(&changes.unstaged).chain(&changes.untracked) {
            // Ignorer les fichiers supprimés (n'existent plus sur le disque)
            if matches!(file_status.status, StatusType::Deleted) {
                continue;
            }
            let file_path = repo_path.join(&file_status.path);
            if let Ok(metadata) = std::fs::metadata(&file_path) {
                if let Ok(mtime) = metadata.modified() {
                    max_mtime = Some(match max_mtime {
                        Some(prev) if prev > mtime => prev,
                        _ => mtime,
                    });
                }
            }
        }
        changes.last_modified = max_mtime.map(|mtime| {
            let duration = mtime.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
            DateTime::from_timestamp(duration.as_secs() as i64, 0)
                .map(|dt| dt.with_timezone(&Local))
                .unwrap_or_else(|| Local::now())
        });

        Ok(changes)
    }

    pub fn total_files(&self) -> usize {
        self.staged.len() + self.unstaged.len() + self.untracked.len()
    }

    pub fn is_dirty(&self) -> bool {
        self.total_files() > 0
    }
}

impl StatusType {
    fn from_index_char(c: u8) -> Self {
        match c {
            b'A' => StatusType::Added,
            b'M' => StatusType::Modified,
            b'D' => StatusType::Deleted,
            b'R' => StatusType::Renamed,
            b'C' => StatusType::Copied,
            _ => StatusType::Modified,
        }
    }

    fn from_worktree_char(c: u8) -> Self {
        match c {
            b'M' => StatusType::Modified,
            b'D' => StatusType::Deleted,
            _ => StatusType::Modified,
        }
    }
}
