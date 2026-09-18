use std::path::Path;
use std::process::Command;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DiffError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Git error: {0}")]
    Git(String),
}

pub fn get_git_diff(repo_root: &Path, commit_range: Option<&str>) -> Result<String, DiffError> {
    let mut cmd = Command::new("git");
    cmd.arg("diff");
    if let Some(range) = commit_range {
        cmd.arg(range);
    }
    cmd.current_dir(repo_root);

    let output = cmd.output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DiffError::Git(stderr.to_string()));
    }

    let mut diff = String::from_utf8_lossy(&output.stdout).to_string();

    // If diffing the working tree (no commit range specified, or range is HEAD),
    // include untracked files via read-only `git diff --no-index` without mutating the index.
    let is_working_tree_diff = commit_range.is_none() || commit_range == Some("HEAD");
    if is_working_tree_diff {
        if let Ok(ls_out) = Command::new("git")
            .args(["ls-files", "-z", "--others", "--exclude-standard"])
            .current_dir(repo_root)
            .output()
        {
            if ls_out.status.success() {
                for file_bytes in ls_out.stdout.split(|&b| b == 0) {
                    if file_bytes.is_empty() {
                        continue;
                    }
                    if let Ok(file) = std::str::from_utf8(file_bytes) {
                        let no_index_out = Command::new("git")
                            .args(["diff", "--no-index", "--", "/dev/null", file])
                            .current_dir(repo_root)
                            .output();
                        if let Ok(out) = no_index_out {
                            let file_diff = String::from_utf8_lossy(&out.stdout);
                            if !file_diff.is_empty() {
                                if !diff.is_empty() && !diff.ends_with('\n') {
                                    diff.push('\n');
                                }
                                diff.push_str(&file_diff);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(diff)
}

/// Extracts the cumulative diff for a milestone, capturing both committed commits since
/// `start_commit` and any uncommitted or untracked changes in the working tree.
///
/// # Parameters
/// - `repo_root`: The absolute or relative root path of the git repository.
/// - `start_commit`: Optional commit hash recorded at the start of the milestone.
///
/// # Returns
/// - `Ok(String)`: The combined unified diff string.
/// - `Err(DiffError)`: If git command execution encounters an I/O failure.
pub fn get_milestone_diff(
    repo_root: &Path,
    start_commit: Option<&str>,
) -> Result<String, DiffError> {
    // Determine the current repository HEAD revision hash.
    // If rev-parse fails (e.g. freshly initialized repo without commits), fallback to working tree diff.
    let head_output = match Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repo_root)
        .output()
    {
        Ok(out) => out,
        Err(e) => return Err(DiffError::Io(e)),
    };

    let head_hash = if head_output.status.success() {
        String::from_utf8_lossy(&head_output.stdout)
            .trim()
            .to_string()
    } else {
        return get_git_diff(repo_root, None);
    };

    let mut combined_diff = String::new();
    let normalized_start = start_commit.map(|s| s.trim()).filter(|s| !s.is_empty());

    // Step 2: If a starting commit was provided and differs from HEAD, capture all commits on the branch.
    if let Some(start) = normalized_start {
        if start != head_hash {
            let range = format!("{}..HEAD", start);
            let committed_diff = get_git_diff(repo_root, Some(&range))?;
            if !committed_diff.trim().is_empty() {
                combined_diff.push_str(&committed_diff);
            }
        }
    }

    // Step 3: Check for working tree modifications or untracked files and append to diff.
    let working_diff = get_git_diff(repo_root, Some("HEAD"))?;
    if !working_diff.trim().is_empty() {
        if !combined_diff.is_empty() && !combined_diff.ends_with('\n') {
            combined_diff.push('\n');
        }
        combined_diff.push_str(&working_diff);
    }

    // Step 4: If no milestone start commit was provided and the diff is still empty, fallback to HEAD~1..HEAD.
    if combined_diff.trim().is_empty() && normalized_start.is_none() {
        if let Ok(out) = Command::new("git")
            .args(["rev-parse", "--verify", "HEAD~1"])
            .current_dir(repo_root)
            .output()
        {
            if out.status.success() {
                let last_commit_diff = get_git_diff(repo_root, Some("HEAD~1..HEAD"))?;
                return Ok(last_commit_diff);
            }
        }
    }

    Ok(combined_diff)
}
