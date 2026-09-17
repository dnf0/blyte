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
    // Stage untracked files intent-to-add so newly created files are captured in diffs (e.g. against HEAD)
    let _ = Command::new("git")
        .args(["add", "-N", "."])
        .current_dir(repo_root)
        .output();

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

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}
