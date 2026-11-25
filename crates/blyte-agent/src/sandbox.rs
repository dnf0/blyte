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
        ".config/blyte/env",
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

/// System temporary directories where tests may create and read `.env` files.
const TEMP_DOT_ENV_DIRS: &[&str] = &["/private/tmp", "/private/var/folders"];

/// Generates an SBPL profile denying file writes outside `write`, denying file reads under
/// `deny_read` and any `.env` file, allowing `.env` files under `/private/tmp` and
/// `/private/var/folders` outside `workspace` (so tests can read `.env` files they create in temp
/// folders while the workspace `.env` stays denied), and granting file reads back under
/// `allow_read`.
/// Reads are denied as `file-read-data`: contents and directory listings are refused, but `stat`
/// still works (pytest stats every path while collecting, including `.env`).
pub fn profile_with(
    workspace: &Path,
    write: &[PathBuf],
    deny_read: &[PathBuf],
    allow_read: &[PathBuf],
) -> String {
    let mut out = profile(write);
    for path in deny_read {
        let escaped = escape_subpath(path);
        out.push_str(&format!(r#"(deny file-read-data (subpath "{escaped}"))"#));
    }
    out.push_str(r##"(deny file-read-data (regex #"/\.env$"))"##);
    let escaped_ws = escape_subpath(workspace);
    for dir in TEMP_DOT_ENV_DIRS {
        out.push_str(&format!(
            r##"(allow file-read-data (require-all (subpath "{dir}") (require-not (subpath "{escaped_ws}")) (regex #"/\.env$")))"##
        ));
    }
    for path in allow_read {
        let escaped = escape_subpath(path);
        out.push_str(&format!(r#"(allow file-read-data (subpath "{escaped}"))"#));
    }
    out
}

fn push_home_local_grants(home_path: &Path, allowed: &mut Vec<PathBuf>) {
    #[cfg(target_os = "macos")]
    {
        allowed.push(home_path.join(".local"));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let local_dir = home_path.join(".local");
        if let Ok(entries) = std::fs::read_dir(&local_dir) {
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .filter(|entry| entry.file_name() != std::ffi::OsStr::new("state"))
                .map(|entry| entry.path())
                .collect();
            paths.sort();
            allowed.extend(paths);
        }
        let state_dir = local_dir.join("state");
        if let Ok(entries) = std::fs::read_dir(&state_dir) {
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .filter(|entry| entry.file_name() != std::ffi::OsStr::new("blyte"))
                .map(|entry| entry.path())
                .collect();
            paths.sort();
            allowed.extend(paths);
        }
    }
}

/// Returns the blyte state directory (`$XDG_STATE_HOME/blyte` when set, else `$HOME/.local/state/blyte`).
pub fn state_blyte_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("blyte"));
        }
    }
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".local").join("state").join("blyte"))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Returns the directory holding surface authentication tokens (`<state_blyte_dir>/surface`).
pub fn surface_token_dir() -> Option<PathBuf> {
    state_blyte_dir().map(|d| d.join("surface"))
}

/// Returns the path to the surface authentication token for `main_checkout`.
pub fn surface_token_path(main_checkout: &Path) -> Result<PathBuf, String> {
    let dir = surface_token_dir().ok_or_else(|| {
        "cannot determine surface token directory: neither XDG_STATE_HOME nor HOME is set"
            .to_string()
    })?;
    let canon = canonicalize_existing_prefix(main_checkout);
    let key = format!("{:016x}", fnv1a64(canon.as_os_str().as_encoded_bytes()));
    Ok(dir.join(format!("{key}.token")))
}

fn canonicalize_existing_prefix(path: &Path) -> PathBuf {
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }
    let mut tail = Vec::new();
    let mut cur = path;
    while let Some(name) = cur.file_name() {
        tail.push(name.to_os_string());
        match cur.parent() {
            Some(parent) => {
                cur = parent;
                if let Ok(mut canon) = cur.canonicalize() {
                    for part in tail.into_iter().rev() {
                        canon.push(part);
                    }
                    return canon;
                }
            }
            None => break,
        }
    }
    path.to_path_buf()
}

#[cfg(target_os = "macos")]
/// Seatbelt lets an earlier `allow file-read-data` override a later `deny file-read*`, so each read
/// deny here is written for `file-read-data` as well.
fn deny_state_blyte_write(mut profile: String) -> String {
    if let Some(dir) = state_blyte_dir() {
        let escaped = escape_path(&canonicalize_existing_prefix(&dir));
        profile.push_str(&format!(r#"(deny file-write* (subpath "{escaped}"))"#));
    }
    if let Some(dir) = surface_token_dir() {
        let escaped = escape_path(&canonicalize_existing_prefix(&dir));
        profile.push_str(&format!(
            r#"(deny file-read* (subpath "{escaped}"))(deny file-read-data (subpath "{escaped}"))"#
        ));
    }
    if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        let env_path = PathBuf::from(home).join(".config/blyte/env");
        let escaped = escape_path(&canonicalize_existing_prefix(&env_path));
        profile.push_str(&format!(
            r#"(deny file-read* (subpath "{escaped}"))(deny file-read-data (subpath "{escaped}"))"#
        ));
    }
    profile
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
                "Library/Caches",
                ".gradle",
                ".m2",
                "go",
                ".docker/buildx",
            ] {
                allowed.push(home_path.join(sub));
            }
            push_home_local_grants(&home_path, &mut allowed);
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

fn git_common_dir(root: &Path) -> Option<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().map(str::trim).find(|s| !s.is_empty())?;
    Some(PathBuf::from(line))
}

fn main_checkout_root(root: &Path) -> PathBuf {
    if let Some(common_dir) = git_common_dir(root) {
        if common_dir.file_name() == Some(std::ffi::OsStr::new(".git")) {
            if let Some(main_root) = common_dir.parent() {
                return main_root
                    .canonicalize()
                    .unwrap_or_else(|_| main_root.to_path_buf());
            }
        }
    }
    root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
}

/// Returns the `.wt/tasks` directory under the main checkout for `root`.
pub fn conversation_tasks_dir(root: &Path) -> PathBuf {
    main_checkout_root(root).join(".wt").join("tasks")
}

/// Returns the writable directories allowed for the `--converse` sandboxed shell:
/// `.wt/tasks` under the main checkout, system temporary directories, and common developer caches.
pub fn conversation_allowed(root: &Path) -> Vec<PathBuf> {
    let mut allowed = vec![
        conversation_tasks_dir(root),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/private/var/folders"),
        PathBuf::from("/tmp"),
        PathBuf::from("/var/tmp"),
        PathBuf::from("/dev"),
    ];

    if let Some(tmp) = std::env::var_os("TMPDIR") {
        if !tmp.is_empty() {
            allowed.push(PathBuf::from(tmp));
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        if !home.is_empty() {
            let home_path = PathBuf::from(home);
            for sub in [
                ".cargo",
                ".rustup",
                ".cache",
                ".npm",
                "Library/Caches",
                ".gradle",
                ".m2",
                "go",
                ".docker/buildx",
            ] {
                allowed.push(home_path.join(sub));
            }
            push_home_local_grants(&home_path, &mut allowed);
        }
    }

    allowed
}

fn path_exists(path: &Path) -> Result<bool, String> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("cannot check {}: {e}", path.display())),
    }
}

pub(crate) fn is_open_edit_worktree_id(
    wt_dir: &Path,
    id: &std::ffi::OsStr,
) -> Result<bool, String> {
    let mut edit_name = id.to_os_string();
    edit_name.push(".edit.json");
    let edit_marker = wt_dir.join(edit_name);
    if !path_exists(&edit_marker)? {
        return Ok(false);
    }
    let mut summary_name = id.to_os_string();
    summary_name.push(".json");
    let summary_marker = wt_dir.join(summary_name);
    if path_exists(&summary_marker)? {
        return Ok(false);
    }
    let worktree = wt_dir.join(id);
    match std::fs::metadata(&worktree) {
        Ok(m) => Ok(m.is_dir()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("cannot check {}: {e}", worktree.display())),
    }
}

/// Returns the sorted worktree directories `<main_checkout>/.wt/<id>` for currently open direct
/// edits (where `<id>.edit.json` exists, `<id>.json` does not, and `<id>` is a directory).
pub fn open_edit_worktrees(main_checkout: &Path) -> Result<Vec<PathBuf>, String> {
    let wt_dir = main_checkout.join(".wt");
    let entries = match std::fs::read_dir(&wt_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("cannot read {}: {e}", wt_dir.display())),
    };
    let mut worktrees = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", wt_dir.display()))?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        let Some(id) = name_str.strip_suffix(".edit.json") else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        if is_open_edit_worktree_id(&wt_dir, std::ffi::OsStr::new(id))? {
            worktrees.push(wt_dir.join(id));
        }
    }
    worktrees.sort();
    Ok(worktrees)
}

#[cfg(target_os = "macos")]
fn allow_edit_worktrees(mut base: String, edit_worktrees: &[PathBuf]) -> String {
    for wt in edit_worktrees {
        let canon = wt.canonicalize().unwrap_or_else(|_| wt.to_path_buf());
        let escaped_wt = escape_path(&canon);
        let escaped_git = escape_path(&canon.join(".git"));
        base.push_str(&format!(
            r#"(allow file-write* (subpath "{escaped_wt}"))(deny file-write* (literal "{escaped_git}"))"#
        ));
    }
    base
}

/// Returns the `--converse` sandbox profile for the current platform.
pub fn conversation_profile(root: &Path, allow_read: &[PathBuf]) -> Profile {
    conversation_profile_net(root, allow_read, true)
}

/// Returns the `--converse` sandbox profile with network configuration.
pub fn conversation_profile_net(root: &Path, allow_read: &[PathBuf], network: bool) -> Profile {
    conversation_profile_with_edits(root, allow_read, network, &[])
}

/// Returns the `--converse` sandbox profile allowing writes in the given open edit worktrees.
pub fn conversation_profile_with_edits(
    root: &Path,
    allow_read: &[PathBuf],
    network: bool,
    edit_worktrees: &[PathBuf],
) -> Profile {
    #[cfg(target_os = "macos")]
    {
        let deny_read = match std::env::var_os("HOME") {
            Some(home) if !home.is_empty() => read_denied(Path::new(&home)),
            _ => Vec::new(),
        };
        let base = deny_state_blyte_write(allow_edit_worktrees(
            profile_with(root, &conversation_allowed(root), &deny_read, allow_read),
            edit_worktrees,
        ));
        let profiled = if network {
            base
        } else {
            let socks = match std::env::var_os("HOME") {
                Some(home) if !home.is_empty() => docker_sockets(Path::new(&home)),
                _ => Vec::new(),
            };
            offline(base, &socks)
        };
        Profile::Seatbelt(deny_unix_sockets(profiled, &current_agent_sockets()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let home = std::env::var_os("HOME");
        let home_path = home.as_ref().filter(|h| !h.is_empty()).map(Path::new);
        let mut extra_read = vec![root.to_path_buf()];
        if let Some(common_dir) = git_common_dir(root) {
            extra_read.push(common_dir);
        }
        let main_wt = main_checkout_root(root).join(".wt");
        if main_wt.exists() {
            extra_read.push(main_wt);
        }
        extra_read.extend_from_slice(allow_read);
        let mut write = conversation_allowed(root);
        write.extend(edit_worktrees.iter().cloned());
        Profile::LandlockConfined {
            write,
            read: read_allowed(home_path, &extra_read),
            block_tcp: !network,
        }
    }
}

/// Builds the `--converse` sandbox profile including all currently open edit worktrees.
pub fn conversation_profile_for_open_edits(
    root: &Path,
    allow_read: &[PathBuf],
    network: bool,
) -> Result<Profile, String> {
    let main = main_checkout_root(root);
    let edits = open_edit_worktrees(&main)?;
    Ok(conversation_profile_with_edits(
        root, allow_read, network, &edits,
    ))
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

/// Returns the Unix socket paths used by ssh and gpg agents, in order and without duplicates.
///
/// Includes `ssh_auth_sock` when given, followed by the standard gpg-agent and 1Password agent
/// socket paths under `home`. On Linux, Landlock does not control connecting to a Unix socket by
/// path, so only environment scrubbing (`SSH_AUTH_SOCK` and `SSH_AGENT_PID`) applies there.
pub fn agent_sockets(home: &Path, ssh_auth_sock: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(sock) = ssh_auth_sock {
        if !sock.as_os_str().is_empty() {
            out.push(sock.to_path_buf());
        }
    }
    for rel in [
        ".gnupg/S.gpg-agent.ssh",
        ".gnupg/S.gpg-agent",
        ".1password/agent.sock",
        "Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock",
    ] {
        let path = home.join(rel);
        if !out.contains(&path) {
            out.push(path);
        }
    }
    out
}

#[cfg(target_os = "macos")]
fn current_agent_sockets() -> Vec<PathBuf> {
    let ssh_auth_sock = if crate::shell::is_pass_env("SSH_AUTH_SOCK") {
        None
    } else {
        std::env::var_os("SSH_AUTH_SOCK")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => agent_sockets(Path::new(&home), ssh_auth_sock.as_deref()),
        _ => ssh_auth_sock.into_iter().collect(),
    }
}

fn deny_unix_sockets(mut profile: String, sockets: &[PathBuf]) -> String {
    if sockets.is_empty() {
        return profile;
    }
    let mut paths = Vec::new();
    for s in sockets {
        if !paths.contains(s) {
            paths.push(s.clone());
        }
        let canon = canonicalize_existing_prefix(s);
        if canon != *s && !paths.contains(&canon) {
            paths.push(canon);
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
    profile
}

/// Augments an SBPL profile string with network-denial rules, allowing loopback and unix sockets,
/// then denying the specified Docker unix sockets (and their canonicalised paths if different).
pub fn offline(mut profile: String, docker_sockets: &[PathBuf]) -> String {
    profile.push_str(
        r#"(deny network-outbound)(allow network-outbound (remote ip "localhost:*"))(allow network-outbound (remote unix-socket))"#,
    );
    deny_unix_sockets(profile, docker_sockets)
}

#[cfg(target_os = "macos")]
fn default_profile_base(root: &Path, allow_read: &[PathBuf]) -> String {
    let deny_read = match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => read_denied(Path::new(&home)),
        _ => Vec::new(),
    };
    deny_state_blyte_write(profile_with(
        root,
        &default_allowed(root),
        &deny_read,
        allow_read,
    ))
}

/// Returns the default sandbox profile for the current platform.
///
/// On macOS, returns an SBPL profile denying writes outside `default_allowed` and reads under credential stores.
/// On other platforms, returns a confined Landlock profile with `default_allowed` and `read_allowed`.
pub fn default_profile(root: &Path, allow_read: &[PathBuf]) -> Profile {
    #[cfg(target_os = "macos")]
    {
        Profile::Seatbelt(deny_unix_sockets(
            default_profile_base(root, allow_read),
            &current_agent_sockets(),
        ))
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
        let base = default_profile_base(root, allow_read);
        let socks = match std::env::var_os("HOME") {
            Some(home) if !home.is_empty() => docker_sockets(Path::new(&home)),
            _ => Vec::new(),
        };
        Profile::Seatbelt(deny_unix_sockets(
            offline(base, &socks),
            &current_agent_sockets(),
        ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_edit_worktrees_empty_when_wt_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            open_edit_worktrees(tmp.path()).unwrap(),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn open_edit_worktrees_sorted_and_filtered() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join(".wt");
        std::fs::create_dir_all(wt.join("B")).unwrap();
        std::fs::create_dir_all(wt.join("A")).unwrap();
        std::fs::create_dir_all(wt.join("Done")).unwrap();
        std::fs::create_dir_all(wt.join("NoMarker")).unwrap();
        std::fs::write(wt.join("NotADir"), "file\n").unwrap();

        std::fs::write(wt.join("B.edit.json"), "{}\n").unwrap();
        std::fs::write(wt.join("A.edit.json"), "{}\n").unwrap();
        std::fs::write(wt.join("Done.edit.json"), "{}\n").unwrap();
        std::fs::write(wt.join("Done.json"), "{}\n").unwrap();
        std::fs::write(wt.join("MissingDir.edit.json"), "{}\n").unwrap();
        std::fs::write(wt.join("NotADir.edit.json"), "{}\n").unwrap();

        let got = open_edit_worktrees(tmp.path()).unwrap();
        assert_eq!(got, vec![wt.join("A"), wt.join("B")]);
    }

    #[test]
    fn open_edit_worktrees_errors_on_unreadable_wt() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join(".wt");
        std::fs::write(&wt, "not a directory\n").unwrap();
        let err = open_edit_worktrees(tmp.path()).unwrap_err();
        assert!(err.contains(&wt.display().to_string()), "{err}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn conversation_profile_with_edits_denies_worktree_git() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join(".wt/E1");
        std::fs::create_dir_all(&wt).unwrap();
        let Profile::Seatbelt(sbpl) =
            conversation_profile_with_edits(tmp.path(), &[], false, std::slice::from_ref(&wt))
        else {
            panic!("expected Seatbelt profile on macOS");
        };
        let canon = wt.canonicalize().unwrap();
        let escaped_wt = escape_path(&canon);
        let escaped_git = escape_path(&canon.join(".git"));
        let expected = format!(
            r#"(allow file-write* (subpath "{escaped_wt}"))(deny file-write* (literal "{escaped_git}"))"#
        );
        assert!(sbpl.contains(&expected), "{sbpl}");
    }
}
