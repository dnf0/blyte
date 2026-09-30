//! Machine-wide registry of repositories blyte has dispatched in (plan 2026-09-30).

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Returns the directory used for blyte's machine-wide state:
/// `$BLYTE_STATE_DIR` when set and non-empty, else
/// `$XDG_STATE_HOME/blyte` when set, else
/// `$HOME/.local/state/blyte`.
pub fn state_dir() -> Option<PathBuf> {
    if let Some(val) = std::env::var_os("BLYTE_STATE_DIR") {
        if !val.is_empty() {
            return Some(PathBuf::from(val));
        }
    }
    if let Some(val) = std::env::var_os("XDG_STATE_HOME") {
        if !val.is_empty() {
            return Some(PathBuf::from(val).join("blyte"));
        }
    }
    if let Some(val) = std::env::var_os("HOME") {
        if !val.is_empty() {
            return Some(PathBuf::from(val).join(".local/state/blyte"));
        }
    }
    None
}

/// Appends the canonical path of `root` as one line to `<state_dir>/repos` (creating the directory),
/// unless it is already listed. Errors are ignored.
pub fn register(root: &Path) {
    let _ = register_in(root, state_dir());
}

fn register_in(root: &Path, dir: Option<PathBuf>) -> std::io::Result<()> {
    let Some(dir) = dir else {
        return Ok(());
    };
    let canonical = root.canonicalize()?;
    let canonical_str = canonical.to_str().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid utf-8 path")
    })?;

    let repos_file = dir.join("repos");
    if repos_file.is_file() {
        let content = std::fs::read_to_string(&repos_file)?;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed == canonical_str || Path::new(trimmed) == canonical.as_path() {
                return Ok(());
            }
        }
    }

    std::fs::create_dir_all(&dir)?;

    let needs_leading_newline = repos_file.is_file()
        && std::fs::read_to_string(&repos_file)
            .map(|s| !s.is_empty() && !s.ends_with('\n'))
            .unwrap_or(false);

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&repos_file)?;

    if needs_leading_newline {
        writeln!(file)?;
    }
    writeln!(file, "{canonical_str}")?;
    Ok(())
}

/// The listed paths that still exist as directories, in file order, without duplicates.
pub fn registered() -> Vec<PathBuf> {
    registered_from(state_dir())
}

fn registered_from(dir: Option<PathBuf>) -> Vec<PathBuf> {
    let Some(dir) = dir else {
        return Vec::new();
    };
    let repos_file = dir.join("repos");
    let Ok(content) = std::fs::read_to_string(&repos_file) else {
        return Vec::new();
    };

    let mut result = Vec::new();
    let mut seen = HashSet::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = PathBuf::from(trimmed);
        if path.is_dir() && seen.insert(path.clone()) {
            result.push(path);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_skips_missing_and_deduplicates_in_order() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let dir1 = temp.path().join("dir1");
        let dir2 = temp.path().join("dir2");
        let dir_missing = temp.path().join("missing");
        std::fs::create_dir(&dir1).unwrap();
        std::fs::create_dir(&dir2).unwrap();

        let repos_file = state.join("repos");
        let content = format!(
            "{}\n{}\n{}\n{}\n",
            dir1.display(),
            dir_missing.display(),
            dir2.display(),
            dir1.display(),
        );
        std::fs::write(&repos_file, content).unwrap();

        let list = registered_from(Some(state));
        assert_eq!(list, vec![dir1, dir2]);
    }

    #[test]
    fn register_in_deduplicates_and_ignores_missing_root() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let repo = temp.path().join("my-repo");
        std::fs::create_dir(&repo).unwrap();

        assert!(register_in(&repo, Some(state.clone())).is_ok());
        let listed = std::fs::read_to_string(state.join("repos")).unwrap();
        assert_eq!(listed.lines().count(), 1);

        // Second registration does not duplicate
        assert!(register_in(&repo, Some(state.clone())).is_ok());
        let listed2 = std::fs::read_to_string(state.join("repos")).unwrap();
        assert_eq!(listed2.lines().count(), 1);

        // Missing root is ignored gracefully by register()
        let missing = temp.path().join("does-not-exist");
        register(&missing);
    }
}
