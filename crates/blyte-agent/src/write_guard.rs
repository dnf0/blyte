use std::ffi::OsStr;
use std::path::Path;

const SHELL_STARTUP: &[&str] = &[
    ".bashrc",
    ".bash_profile",
    ".bash_login",
    ".bash_logout",
    ".profile",
    ".zshrc",
    ".zprofile",
    ".zshenv",
    ".zlogin",
    ".zlogout",
    ".cshrc",
    ".tcshrc",
    ".kshrc",
    ".config/fish",
];

const GIT_CONFIG: &[&str] = &[".gitconfig", ".config/git"];

const LOGIN_AUTOSTART: &[&str] = &[
    "Library/LaunchAgents",
    ".config/systemd",
    ".config/autostart",
];

const PATH_BIN_DIRS: &[&str] = &[
    ".cargo/bin",
    ".local/bin",
    "go/bin",
    ".bun/bin",
    ".deno/bin",
];

const EXTRA_CREDENTIAL_STORES: &[&str] =
    &[".npmrc", ".pgpass", ".my.cnf", ".vault-token", ".claude"];

/// Returns a short reason when `real_target` (already resolved through symlinks, absolute) is a
/// protected path or lies under a protected folder, and `None` otherwise.
pub fn protected_write(
    real_target: &Path,
    home: &Path,
    state_dir: Option<&Path>,
) -> Option<&'static str> {
    if crate::sandbox::read_denied(home)
        .iter()
        .any(|p| real_target.starts_with(p))
    {
        return Some("a credential store");
    }

    if real_target.file_name() == Some(OsStr::new(".env")) {
        return Some("an environment file");
    }

    if SHELL_STARTUP
        .iter()
        .any(|sub| real_target.starts_with(home.join(sub)))
    {
        return Some("a shell start-up file");
    }

    if real_target.starts_with(home.join(".config/blyte")) {
        return Some("blyte's own files");
    }
    if let Some(dir) = state_dir {
        if !dir.as_os_str().is_empty() && real_target.starts_with(dir) {
            return Some("blyte's own files");
        }
    }

    if GIT_CONFIG
        .iter()
        .any(|sub| real_target.starts_with(home.join(sub)))
    {
        return Some("git configuration");
    }

    if LOGIN_AUTOSTART
        .iter()
        .any(|sub| real_target.starts_with(home.join(sub)))
    {
        return Some("a place that runs programs at login");
    }

    if PATH_BIN_DIRS
        .iter()
        .any(|sub| real_target.starts_with(home.join(sub)))
    {
        return Some("a folder of programs on PATH");
    }

    if EXTRA_CREDENTIAL_STORES
        .iter()
        .any(|sub| real_target.starts_with(home.join(sub)))
    {
        return Some("a credential store");
    }

    None
}
