//! Thin helpers that shell out to `git` — no libgit2 dependency.
//!
//! Keeps everything in this module so a future native implementation can
//! swap in without touching command sites.

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// One hunk from `git diff --unified=0`.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub file_path: String,
    /// Starting line number in the *new* file, 1-based.
    pub new_start: u32,
    /// Ending line number (inclusive) in the new file.
    pub new_end: u32,
}

/// Run `git diff --unified=0 <base>..HEAD` and parse the hunks.
pub fn diff_hunks(repo_root: &Path, base: &str) -> Result<Vec<DiffHunk>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("diff")
        .arg("--unified=0")
        .arg("--no-color")
        .arg(format!("{base}..HEAD"))
        .output()
        .map_err(|e| anyhow!("failed to invoke git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git diff failed: {}", stderr.trim()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_hunks(&text))
}

/// Run `git diff --unified=0` against the unstaged working tree (no commit
/// range — shows uncommitted changes relative to HEAD).
pub fn diff_hunks_worktree(repo_root: &Path) -> Result<Vec<DiffHunk>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("diff")
        .arg("--unified=0")
        .arg("--no-color")
        .arg("HEAD")
        .output()
        .map_err(|e| anyhow!("failed to invoke git: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git diff failed: {}", stderr.trim()));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_hunks(&text))
}

/// Parse the text output of `git diff --unified=0`. Only the "new file"
/// side of each hunk is captured — that's the range the user actually sees
/// at HEAD, which is what we need to match against node line ranges.
fn parse_hunks(text: &str) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    let mut current_file: Option<String> = None;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            // "+++ b/path/to/file"  — drop the "b/" prefix if present.
            // Special cases: "+++ /dev/null" means the file was deleted; skip.
            let path = rest.strip_prefix("b/").unwrap_or(rest);
            if path == "/dev/null" {
                current_file = None;
            } else {
                current_file = Some(path.to_string());
            }
            continue;
        }

        if line.starts_with("@@") {
            // Format: @@ -old_start,old_count +new_start,new_count @@ ...
            // With --unified=0 counts can be 0 (pure deletion) or omitted (=1).
            let Some(file) = current_file.as_ref() else {
                continue;
            };
            if let Some((new_start, new_count)) = parse_new_range(line) {
                if new_count == 0 {
                    // Pure deletion — no lines remain on the new side.
                    continue;
                }
                let new_end = new_start + new_count.saturating_sub(1);
                hunks.push(DiffHunk {
                    file_path: file.clone(),
                    new_start,
                    new_end,
                });
            }
        }
    }

    hunks
}

fn parse_new_range(hunk_header: &str) -> Option<(u32, u32)> {
    // Find the "+" token: e.g. "@@ -10,0 +42,3 @@ fn foo"
    let plus_pos = hunk_header.find('+')?;
    let after_plus = &hunk_header[plus_pos + 1..];
    let token_end = after_plus
        .find(|c: char| c.is_whitespace())
        .unwrap_or(after_plus.len());
    let token = &after_plus[..token_end];

    let (start_s, count_s) = match token.split_once(',') {
        Some((s, c)) => (s, c),
        None => (token, "1"),
    };
    let start: u32 = start_s.parse().ok()?;
    let count: u32 = count_s.parse().ok()?;
    Some((start, count))
}

/// Per-file recency: commit count in the window and seconds-since-last-commit.
#[derive(Debug, Clone, Default)]
pub struct FileChurn {
    pub commits_in_window: u32,
    /// Unix timestamp of the most recent commit touching the file.
    pub last_commit_ts: i64,
}

/// For each `file_paths` entry, compute churn stats in the last `days_window`.
/// Missing files (no git history) map to zero churn.
pub fn file_churn(
    repo_root: &Path,
    file_paths: &[String],
    days_window: u32,
) -> Result<HashMap<String, FileChurn>> {
    let mut out = HashMap::new();
    if file_paths.is_empty() {
        return Ok(out);
    }
    let since = format!("{}.days.ago", days_window);

    for file in file_paths {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .arg("log")
            .arg("--format=%ct")
            .arg("--since")
            .arg(&since)
            .arg("--")
            .arg(file)
            .output();

        let Ok(output) = output else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let timestamps: Vec<i64> = text
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect();
        let churn = FileChurn {
            commits_in_window: timestamps.len() as u32,
            last_commit_ts: timestamps.first().copied().unwrap_or(0),
        };
        out.insert(file.clone(), churn);
    }
    Ok(out)
}

/// Convert a unix timestamp to "N days/hours ago".
pub fn humanize_ago(unix_ts: i64) -> String {
    if unix_ts <= 0 {
        return "never".into();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let delta = (now - unix_ts).max(0);
    if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else if delta < 2_592_000 {
        format!("{}d ago", delta / 86_400)
    } else {
        format!("{}mo ago", delta / 2_592_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unified0_hunk_header() {
        assert_eq!(parse_new_range("@@ -10,0 +42,3 @@ fn foo"), Some((42, 3)));
        assert_eq!(parse_new_range("@@ -1 +1 @@"), Some((1, 1)));
        assert_eq!(parse_new_range("@@ -5,2 +8 @@"), Some((8, 1)));
    }

    #[test]
    fn parses_diff_text() {
        let text = "diff --git a/foo.rs b/foo.rs\n\
                    index 111..222 100644\n\
                    --- a/foo.rs\n\
                    +++ b/foo.rs\n\
                    @@ -10,0 +11,3 @@ fn hi\n\
                    +a\n+b\n+c\n\
                    @@ -30 +32 @@\n\
                    +x\n";
        let hunks = parse_hunks(text);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].file_path, "foo.rs");
        assert_eq!((hunks[0].new_start, hunks[0].new_end), (11, 13));
        assert_eq!((hunks[1].new_start, hunks[1].new_end), (32, 32));
    }

    #[test]
    fn skips_deleted_files() {
        let text = "diff --git a/foo.rs b/foo.rs\n\
                    --- a/foo.rs\n\
                    +++ /dev/null\n\
                    @@ -1,5 +0,0 @@\n";
        assert!(parse_hunks(text).is_empty());
    }
}
