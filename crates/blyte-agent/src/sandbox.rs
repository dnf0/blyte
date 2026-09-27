//! macOS Seatbelt (SBPL) sandbox profile generation and host sandbox detection.
//!
//! Restricts local shell command execution to prevent writes outside of the workspace,
//! git common directory, temporary folders, and standard developer tool caches.

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
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let s = canon.to_string_lossy();
        let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
        out.push_str(&format!(r#"(allow file-write* (subpath "{escaped}"))"#));
    }

    // Permit writes to essential standard I/O pseudo-devices.
    out.push_str(
        r#"(allow file-write* (literal "/dev/null") (literal "/dev/tty") (subpath "/dev/fd"))"#,
    );
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
            ] {
                allowed.push(home_path.join(sub));
            }
        }
    }

    allowed
}
