use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

fn run_git(root: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("failed to run git {}: {e}", args.join(" ")))
}

fn output_detail(out: &Output) -> String {
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }
    format!("exit status {}", out.status)
}

fn validate_branch_name(branch: &str) -> Result<(), String> {
    if branch.trim().is_empty() {
        return Err("branch name must not be empty".to_string());
    }
    if branch != branch.trim()
        || branch.starts_with('-')
        || branch.starts_with('+')
        || branch.contains(':')
    {
        return Err(format!("invalid branch name '{branch}'"));
    }
    Ok(())
}

fn ensure_origin_remote(root: &Path) -> Result<(), String> {
    let out = run_git(root, &["remote", "get-url", "origin"])?;
    if out.status.success() {
        let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !url.is_empty() {
            return Ok(());
        }
    }
    let detail = output_detail(&out);
    if detail.to_ascii_lowercase().contains("not a git repository") {
        return Err(format!("{}: not a git repository", root.display()));
    }
    Err(format!("repository has no 'origin' remote: {detail}"))
}

fn default_branch_from_origin_head(root: &Path) -> Result<String, String> {
    let out = run_git(root, &["symbolic-ref", "refs/remotes/origin/HEAD"])?;
    if !out.status.success() {
        let detail = output_detail(&out);
        return Err(format!(
            "could not read default branch from origin/HEAD: {detail}"
        ));
    }
    let full_ref = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match full_ref.strip_prefix("refs/remotes/origin/") {
        Some(name) if !name.is_empty() => Ok(name.to_string()),
        _ => Err(format!("origin/HEAD points to unexpected ref '{full_ref}'")),
    }
}

fn ensure_local_branch(root: &Path, branch: &str) -> Result<(), String> {
    let full_ref = format!("refs/heads/{branch}");
    let out = run_git(root, &["show-ref", "--verify", "--quiet", &full_ref])?;
    if out.status.success() {
        return Ok(());
    }
    Err(format!("branch '{branch}' does not exist"))
}

fn is_gh_auth_error(out: &Output) -> bool {
    if out.status.code() == Some(4) {
        return true;
    }
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    )
    .to_ascii_lowercase();
    [
        "gh auth login",
        "not logged in",
        "not logged into",
        "not signed in",
        "authentication",
        "unauthorized",
        "http 401",
    ]
    .iter()
    .any(|needle| combined.contains(needle))
}

fn extract_pr_url(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("https://") || line.starts_with("http://"))
        .map(String::from)
}

pub fn push_branch(root: &Path, branch: &str) -> Result<(), String> {
    validate_branch_name(branch)?;
    ensure_origin_remote(root)?;
    let default_branch = default_branch_from_origin_head(root)?;
    if branch == default_branch {
        return Err(format!(
            "refusing to push default branch '{default_branch}'"
        ));
    }
    ensure_local_branch(root, branch)?;

    let out = run_git(root, &["push", "-u", "origin", branch])?;
    if !out.status.success() {
        let detail = output_detail(&out);
        return Err(format!("git push -u origin {branch} failed: {detail}"));
    }
    Ok(())
}

pub fn open_pull_request(
    root: &Path,
    branch: &str,
    title: &str,
    body: &str,
) -> Result<String, String> {
    open_pull_request_with_gh(root, branch, title, body, OsStr::new("gh"))
}

pub fn open_pull_request_with_gh(
    root: &Path,
    branch: &str,
    title: &str,
    body: &str,
    gh_bin: &OsStr,
) -> Result<String, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("pull request title must not be empty".to_string());
    }
    validate_branch_name(branch)?;
    ensure_origin_remote(root)?;
    let default_branch = default_branch_from_origin_head(root)?;
    if branch == default_branch {
        return Err(format!(
            "refusing to open a pull request from default branch '{default_branch}'"
        ));
    }
    ensure_local_branch(root, branch)?;

    let out = Command::new(gh_bin)
        .args([
            "pr",
            "create",
            "--base",
            &default_branch,
            "--head",
            branch,
            "--title",
            title,
            "--body",
            body,
        ])
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "gh CLI is not installed or not found on PATH".to_string()
            } else {
                format!("failed to run gh pr create: {e}")
            }
        })?;

    if !out.status.success() {
        let detail = output_detail(&out);
        if is_gh_auth_error(&out) {
            return Err(format!(
                "gh is not signed in (run `gh auth login`): {detail}"
            ));
        }
        return Err(format!("gh pr create failed: {detail}"));
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    extract_pr_url(&stdout).ok_or_else(|| {
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            "gh pr create succeeded but returned no pull request URL".to_string()
        } else {
            format!("gh pr create did not return a pull request URL: {trimmed}")
        }
    })
}
