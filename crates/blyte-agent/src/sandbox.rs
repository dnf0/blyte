//! macOS Seatbelt (SBPL) sandbox profile generation and host sandbox detection.
//!
//! Restricts local shell command execution to prevent writes outside of the workspace,
//! git common directory, temporary folders, and standard developer tool caches, and
//! denies reads of credential stores and environment secret files unless permitted.

use std::path::{Path, PathBuf};

/// Returns whether seatbelt sandbox execution is available on this system.
///
/// Returns true only on macOS when `/usr/bin/sandbox-exec` is present on disk.
pub fn available() -> bool {
    #[cfg(target_os = "macos")]
    {
        Path::new("/usr/bin/sandbox-exec").exists()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Canonicalises when possible and escapes a path for inclusion in an SBPL subpath rule.
fn escape_subpath(path: &Path) -> String {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let s = canon.to_string_lossy();
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Generates an SBPL (Seatbelt Profile Language) profile denying file writes outside allowed paths.
///
/// # Arguments
///
/// * `allowed` - Paths where file write operations should be permitted.
///
/// # Returns
///
/// An SBPL profile string allowing general operations, denying writes by default, and
/// explicitly whitelisting writes under each path in `allowed` plus `/dev/null`, `/dev/tty`, and `/dev/fd`.
pub fn profile(allowed: &[PathBuf]) -> String {
    // Start with the base policy: allow default operations, but deny all file writes.
    let mut out = String::from("(version 1)(allow default)(deny file-write*)");

    // Allow recursive writes under each specified path, resolving symlinks when possible.
    for path in allowed {
        let escaped = escape_subpath(path);
        out.push_str(&format!(r#"(allow file-write* (subpath "{escaped}"))"#));
    }

    // Permit writes to essential standard I/O pseudo-devices.
    out.push_str(
        r#"(allow file-write* (literal "/dev/null") (literal "/dev/tty") (subpath "/dev/fd"))"#,
    );
    out
}

/// Returns the list of standard credential and secret store paths under `home` that should be denied reading.
pub fn read_denied(home: &Path) -> Vec<PathBuf> {
    const DENIED_PATHS: &[&str] = &[
        ".ssh",
        ".aws",
        ".azure",
        ".config/gcloud",
        ".config/gh",
        ".kube",
        ".gnupg",
        ".docker/config.json",
        ".git-credentials",
        ".netrc",
        ".pypirc",
        ".cargo/credentials.toml",
        ".cargo/credentials",
        ".codex",
        "Library/Keychains",
        "Library/Cookies",
        "Library/Safari",
        "Library/Application Support/Google/Chrome",
        "Library/Application Support/Firefox",
        "Library/Application Support/Arc",
        "Library/Application Support/BraveSoftware",
    ];
    DENIED_PATHS.iter().map(|sub| home.join(sub)).collect()
}

/// Generates an SBPL profile denying file writes outside `write`, denying file reads under
/// `deny_read` and any `.env` file, and granting file reads back under `allow_read`.
/// Reads are denied as `file-read-data`: contents and directory listings are refused, but `stat`
/// still works (pytest stats every path while collecting, including `.env`).
pub fn profile_with(write: &[PathBuf], deny_read: &[PathBuf], allow_read: &[PathBuf]) -> String {
    let mut out = profile(write);
    for path in deny_read {
        let escaped = escape_subpath(path);
        out.push_str(&format!(r#"(deny file-read-data (subpath "{escaped}"))"#));
    }
    out.push_str(r##"(deny file-read-data (regex #"/\.env$"))"##);
    for path in allow_read {
        let escaped = escape_subpath(path);
        out.push_str(&format!(r#"(allow file-read-data (subpath "{escaped}"))"#));
    }
    out
}

/// Computes the default list of write-allowed paths for a local workspace run.
///
/// Includes the workspace root, the git common directory (if in a git repo), system temp dirs,
/// and common developer caches under `$HOME`.
///
/// # Arguments
///
/// * `root` - Workspace directory path.
///
/// # Returns
///
/// A vector of paths that should be allowed for file writes under the sandbox.
pub fn default_allowed(root: &Path) -> Vec<PathBuf> {
    let mut allowed = Vec::new();
    // Allow the workspace root where changes are made.
    allowed.push(root.to_path_buf());

    // Resolve git common directory so git commits in a worktree land in the shared repo.
    if let Ok(output) = std::process::Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
    {
        if output.status.success() {
            let s = String::from_utf8_lossy(&output.stdout);
            let path_str = s.trim();
            if !path_str.is_empty() {
                allowed.push(PathBuf::from(path_str));
            }
        }
    }

    // Allow system temporary directories used by compilers and scripts.
    allowed.push(PathBuf::from("/private/tmp"));
    allowed.push(PathBuf::from("/private/var/folders"));

    // Allow user-specified temporary directory if configured in the environment.
    if let Some(tmp) = std::env::var_os("TMPDIR") {
        if !tmp.is_empty() {
            allowed.push(PathBuf::from(tmp));
        }
    }

    // Allow common developer tool caches under $HOME so cargo, npm, etc. can function.
    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            let home_path = PathBuf::from(home);
            for sub in [
                ".cargo",
                ".rustup",
                ".cache",
                ".npm",
                ".local",
                "Library/Caches",
                ".gradle",
                ".m2",
                "go",
                ".docker/buildx",
            ] {
                allowed.push(home_path.join(sub));
            }
        }
    }

    allowed
}
