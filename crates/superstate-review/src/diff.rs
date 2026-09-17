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
