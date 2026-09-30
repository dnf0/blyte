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
///
/// A repository under the system temp directory is not registered unless `BLYTE_STATE_DIR` is set:
/// it is transient, and every test run that dispatched in one wrote it to the operator's real list.
pub fn register(root: &Path) {
    if std::env::var_os("BLYTE_STATE_DIR").is_none() && is_transient(root) {
        return;
    }
    let _ = register_in(root, state_dir());
}

/// Whether `root` lies under the system temp directory.
pub fn is_transient(root: &Path) -> bool {
    let tmp = std::env::temp_dir();
    let tmp = tmp.canonicalize().unwrap_or(tmp);
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    root.starts_with(&tmp)
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

    // One write of the whole line: an O_APPEND write this small is atomic, so dispatches
    // registering at the same moment cannot run their lines together (they did, 2026-09-30).
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&repos_file)?;
    file.write_all(format!("{canonical_str}\n").as_bytes())?;
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
    fn parallel_registrations_each_get_a_whole_line() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let roots: Vec<PathBuf> = (0..24)
            .map(|i| {
                let d = temp.path().join(format!("repo-{i}"));
                std::fs::create_dir(&d).unwrap();
                d
            })
            .collect();
        std::thread::scope(|s| {
            for r in &roots {
                let state = state.clone();
                s.spawn(move || register_in(r, Some(state)).unwrap());
            }
        });
        let listed = std::fs::read_to_string(state.join("repos")).unwrap();
        assert_eq!(listed.lines().count(), 24, "{listed}");
        assert!(listed.lines().all(|l| Path::new(l).is_dir()), "{listed}");
    }

    #[test]
    fn a_repository_in_the_temp_directory_is_transient() {
        let temp = tempfile::tempdir().unwrap();
        assert!(is_transient(temp.path()));
        if let Some(home) = std::env::var_os("HOME") {
            assert!(!is_transient(Path::new(&home)));
        }
    }

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
