//! Sandbox profile generation and host sandbox detection for macOS (Seatbelt) and Linux (Landlock).
//!
//! Restricts local shell command execution to prevent writes outside of the workspace,
//! git common directory, temporary folders, and standard developer tool caches.
//!
//! # Platform differences and scope limits
//!
//! - On macOS, Seatbelt (SBPL) restricts writes and denies reading credential stores and
//!   environment secret files unless explicitly permitted via allow-lists.
//! - On Linux, Landlock restricts writes and restricts reads via an allowlist: commands can read
//!   only system directories, the workspace, tool caches under `$HOME`, and `--allow-read` grants;
//!   credential stores and other files under `$HOME` are not granted and therefore unreadable.
//!   The harness environment filter applies on both platforms.
//!
//! # Network isolation (`--network=off`)
//!
//! - On macOS, Seatbelt denies outbound network connections while allowing loopback (`localhost:*`)
//!   and Unix domain sockets, then explicitly denies known Docker daemon sockets.
//! - On Linux, Landlock (ABI >= 4, Linux 6.7+) handles TCP connection restriction (`AccessNet::ConnectTcp`),
//!   denying all outbound TCP connections (including localhost). UDP and Unix sockets are not restricted by Landlock.

use std::path::{Path, PathBuf};

/// Sandbox profile restricting command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Profile {
    /// An SBPL profile string for `sandbox-exec` on macOS.
    Seatbelt(String),
    /// A list of write-allowed directory paths for Landlock on Linux.
    Landlock(Vec<PathBuf>),
    /// A list of write-allowed directory paths for Landlock on Linux, blocking outbound TCP connections.
    LandlockOffline(Vec<PathBuf>),
    /// Write-allowed and read-allowed directory paths for Landlock on Linux, optionally blocking outbound TCP connections.
    LandlockConfined {
        write: Vec<PathBuf>,
        read: Vec<PathBuf>,
        block_tcp: bool,
    },
}

impl From<String> for Profile {
    fn from(s: String) -> Self {
        Profile::Seatbelt(s)
    }
}

impl From<&str> for Profile {
    fn from(s: &str) -> Self {
        Profile::Seatbelt(s.to_string())
    }
}

/// Returns the Landlock ABI version supported by the running kernel, or 0 if unsupported or on other platforms.
#[cfg(target_os = "linux")]
pub fn landlock_abi() -> i64 {
    unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<libc::c_void>(),
            0usize,
            1u32,
        ) as i64
    }
}

/// Returns the Landlock ABI version supported by the running kernel, or 0 if unsupported or on other platforms.
#[cfg(not(target_os = "linux"))]
pub fn landlock_abi() -> i64 {
    0
}

/// Returns whether sandbox execution is available on this system.
///
/// On macOS, returns true when `/usr/bin/sandbox-exec` is present on disk.
/// On Linux, returns true when the kernel supports Landlock.
/// On other platforms, returns false.
pub fn available() -> bool {
    #[cfg(target_os = "macos")]
    {
        Path::new("/usr/bin/sandbox-exec").exists()
    }
    #[cfg(target_os = "linux")]
    {
        landlock_abi() >= 1
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// Escapes a path for inclusion in an SBPL rule.
fn escape_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Canonicalises when possible and escapes a path for inclusion in an SBPL subpath rule.
fn escape_subpath(path: &Path) -> String {
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    escape_path(&canon)
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

/// Returns the list of standard read-allowed paths for a confined Landlock sandbox on Linux.
///
/// In order: system roots, tool directories under `home` (if given), and `allow_read` grants.
pub fn read_allowed(home: Option<&Path>, allow_read: &[PathBuf]) -> Vec<PathBuf> {
    let mut allowed = Vec::new();

    const SYSTEM_ROOTS: &[&str] = &[
        "/usr", "/lib", "/lib32", "/lib64", "/libx32", "/bin", "/sbin", "/etc", "/opt", "/proc",
        "/sys", "/dev", "/run", "/var", "/tmp", "/snap", "/nix",
    ];
    for root in SYSTEM_ROOTS {
        allowed.push(PathBuf::from(root));
    }

    if let Some(home) = home {
        const TOOL_DIRS: &[&str] = &[
            ".cargo",
            ".rustup",
            ".cache",
            ".local/bin",
            ".local/lib",
            ".local/share/uv",
            ".local/share/pnpm",
            ".local/share/mise",
            ".npm",
            ".nvm",
            ".pyenv",
            ".rbenv",
            ".asdf",
            ".bun",
            ".deno",
            ".gradle",
            ".m2",
            "go",
            ".config/git",
            ".gitconfig",
            ".gitignore_global",
            ".gitattributes",
            ".profile",
            ".bashrc",
            ".bash_profile",
            ".bash_login",
            ".inputrc",
            ".editorconfig",
            ".docker/buildx",
        ];
        for sub in TOOL_DIRS {
            allowed.push(home.join(sub));
        }
    }

    allowed.extend(allow_read.iter().cloned());
    allowed
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
    allowed.push(PathBuf::from("/tmp"));
    allowed.push(PathBuf::from("/var/tmp"));
    allowed.push(PathBuf::from("/dev"));

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

    // Allow tool caches in enclosing node_modules above the worktree (vitest's .vite-temp, etc.).
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    let home_canon = home.as_deref().and_then(|h| h.canonicalize().ok());
    let is_home = |p: &Path| -> bool {
        if let Some(h) = home.as_deref() {
            if p == h {
                return true;
            }
        }
        if let Some(hc) = home_canon.as_deref() {
            if p == hc || p.canonicalize().ok().as_deref() == Some(hc) {
                return true;
            }
        }
        false
    };

    if !is_home(root) {
        for a in root.ancestors().skip(1) {
            if is_home(a) {
                break;
            }
            let nm = a.join("node_modules");
            if nm.is_dir() {
                for name in [".vite-temp", ".vite", ".cache"] {
                    let dir = nm.join(name);
                    let _ = std::fs::create_dir_all(&dir);
                    allowed.push(dir);
                }
            }
        }
    }

    allowed
}

/// Returns the standard Unix socket paths used by Docker and container runtimes.
pub fn docker_sockets(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/var/run/docker.sock"),
        home.join(".docker/run/docker.sock"),
        home.join(".colima/default/docker.sock"),
        home.join(".orbstack/run/docker.sock"),
    ]
}

/// Augments an SBPL profile string with network-denial rules, allowing loopback and unix sockets,
/// then denying the specified Docker unix sockets (and their canonicalised paths if different).
pub fn offline(mut profile: String, docker_sockets: &[PathBuf]) -> String {
    profile.push_str(
        r#"(deny network-outbound)(allow network-outbound (remote ip "localhost:*"))(allow network-outbound (remote unix-socket))"#,
    );
    if !docker_sockets.is_empty() {
        let mut paths = Vec::new();
        for s in docker_sockets {
            if !paths.contains(s) {
                paths.push(s.clone());
            }
            if let Ok(canon) = s.canonicalize() {
                if canon != *s && !paths.contains(&canon) {
                    paths.push(canon);
                }
            }
        }
        profile.push_str("(deny network-outbound");
        for p in paths {
            let escaped = escape_path(&p);
            profile.push_str(&format!(
                r#" (remote unix-socket (path-literal "{escaped}"))"#
            ));
        }
        profile.push(')');
    }
    profile
}

/// Returns the default sandbox profile for the current platform.
///
/// On macOS, returns an SBPL profile denying writes outside `default_allowed` and reads under credential stores.
/// On other platforms, returns a confined Landlock profile with `default_allowed` and `read_allowed`.
pub fn default_profile(root: &Path, allow_read: &[PathBuf]) -> Profile {
    #[cfg(target_os = "macos")]
    {
        let deny_read = match std::env::var_os("HOME") {
            Some(home) if !home.is_empty() => read_denied(Path::new(&home)),
            _ => Vec::new(),
        };
        Profile::Seatbelt(profile_with(&default_allowed(root), &deny_read, allow_read))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var_os("HOME");
        let home_path = home.as_ref().filter(|h| !h.is_empty()).map(Path::new);
        Profile::LandlockConfined {
            write: default_allowed(root),
            read: read_allowed(home_path, allow_read),
            block_tcp: false,
        }
    }
}

/// Returns the default sandbox profile with network configuration.
///
/// When `network` is true, returns the same profile as [`default_profile`].
/// When `network` is false, returns an offline profile denying outbound network connections.
pub fn default_profile_net(root: &Path, allow_read: &[PathBuf], network: bool) -> Profile {
    if network {
        return default_profile(root, allow_read);
    }
    #[cfg(target_os = "macos")]
    {
        let base = match default_profile(root, allow_read) {
            Profile::Seatbelt(s) => s,
            _ => unreachable!(),
        };
        let socks = match std::env::var_os("HOME") {
            Some(home) if !home.is_empty() => docker_sockets(Path::new(&home)),
            _ => Vec::new(),
        };
        Profile::Seatbelt(offline(base, &socks))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var_os("HOME");
        let home_path = home.as_ref().filter(|h| !h.is_empty()).map(Path::new);
        Profile::LandlockConfined {
            write: default_allowed(root),
            read: read_allowed(home_path, allow_read),
            block_tcp: true,
        }
    }
}

/// Sets up Landlock sandbox rules in the child process before exec.
#[cfg(target_os = "linux")]
pub(crate) fn landlock_pre_exec(
    cmd: &mut std::process::Command,
    write: &[PathBuf],
    read: Option<&[PathBuf]>,
    block_tcp: bool,
) -> std::io::Result<()> {
    use landlock::{
        path_beneath_rules, Access, AccessFs, AccessNet, Ruleset, RulesetAttr, RulesetCreated,
        RulesetCreatedAttr, ABI,
    };
    use std::os::unix::process::CommandExt;
    use std::sync::Mutex;

    if block_tcp && landlock_abi() < 4 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "needs Linux 6.7 or later",
        ));
    }

    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::from_write(ABI::V5))
        .map_err(std::io::Error::other)?;

    if read.is_some() {
        ruleset = ruleset
            .handle_access(AccessFs::from_read(ABI::V5))
            .map_err(std::io::Error::other)?;
    }

    if block_tcp {
        ruleset = ruleset
            .handle_access(AccessNet::ConnectTcp)
            .map_err(std::io::Error::other)?;
    }

    let ruleset: RulesetCreated = ruleset.create().map_err(std::io::Error::other)?;

    let ruleset: RulesetCreated = match read {
        Some(read_paths) => ruleset
            .add_rules(path_beneath_rules(write, AccessFs::from_all(ABI::V5)))
            .map_err(std::io::Error::other)?
            .add_rules(path_beneath_rules(read_paths, AccessFs::from_read(ABI::V5)))
            .map_err(std::io::Error::other)?,
        None => ruleset
            .add_rules(path_beneath_rules(write, AccessFs::from_write(ABI::V5)))
            .map_err(std::io::Error::other)?,
    };

    let ruleset = Mutex::new(Some(ruleset));
    unsafe {
        cmd.pre_exec(move || {
            // SAFETY: In the child process after fork in a multi-threaded parent,
            // we must not allocate memory or use non-async-signal-safe functions.
            // On error return Err(std::io::Error::from_raw_os_error(libc::EPERM)),
            // never a formatted error.
            let mut guard = ruleset
                .lock()
                .map_err(|_| std::io::Error::from_raw_os_error(libc::EPERM))?;
            let r = guard
                .take()
                .ok_or_else(|| std::io::Error::from_raw_os_error(libc::EPERM))?;
            if r.restrict_self().is_err() {
                return Err(std::io::Error::from_raw_os_error(libc::EPERM));
            }
            Ok(())
        });
    }
    Ok(())
}
