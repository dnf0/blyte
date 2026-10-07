//! Durable record of run token usage and costs.
//!
//! Tracks machine-wide runs across sessions and worktrees in a shared JSONL log.

use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// Returns the default path for the usage log.
///
/// Priority:
/// 1. `$BLYTE_USAGE_LOG` if set and non-empty ("off" disables logging and returns None).
/// 2. `$XDG_STATE_HOME/blyte/usage.jsonl` if `$XDG_STATE_HOME` is set and non-empty.
/// 3. `$HOME/.local/state/blyte/usage.jsonl` if `$HOME` is set and non-empty.
/// 4. `None` if none of these is available.
pub fn default_path() -> Option<PathBuf> {
    path_from(
        std::env::var_os("BLYTE_USAGE_LOG").as_deref(),
        std::env::var_os("XDG_STATE_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

/// Resolves the usage log path from the given environment values.
pub fn path_from(
    log: Option<&OsStr>,
    xdg: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Option<PathBuf> {
    if let Some(log) = log.filter(|s| !s.is_empty()) {
        if log == "off" {
            return None;
        }
        return Some(PathBuf::from(log));
    }
    if let Some(xdg) = xdg.filter(|s| !s.is_empty()) {
        return Some(Path::new(xdg).join("blyte").join("usage.jsonl"));
    }
    if let Some(home) = home.filter(|s| !s.is_empty()) {
        return Some(
            Path::new(home)
                .join(".local")
                .join("state")
                .join("blyte")
                .join("usage.jsonl"),
        );
    }
    None
}

/// An entry in the usage log recording metadata and token consumption of a single run.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Entry {
    pub ts: u64,
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<String>,
    pub research: bool,
    pub model: String,
    pub outcome: String,
    pub turns: u32,
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub thinking: u64,
    #[serde(default)]
    pub cost_usd: Option<f64>,
}

/// Returns the repository root path as a string, stripping any `.wt` component and everything after it.
pub fn repo_of(root: &Path) -> String {
    let mut prefix = PathBuf::new();
    let mut found = false;
    for component in root.components() {
        if component.as_os_str() == ".wt" {
            found = true;
            break;
        }
        prefix.push(component);
    }
    if found {
        prefix.to_string_lossy().into_owned()
    } else {
        root.to_string_lossy().into_owned()
    }
}

/// Appends a usage entry as a single JSON line to `path`.
///
/// Creates parent directories if needed, and writes JSON plus newline in a single `write_all`.
pub fn append(path: &Path, entry: &Entry) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut line = serde_json::to_vec(entry)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    line.push(b'\n');
    file.write_all(&line)?;
    Ok(())
}

/// Reads all valid entries from the usage log file at `path`.
///
/// Skips malformed or unparseable lines. Returns an empty Vec if the file does not exist.
pub fn read(path: &Path) -> Vec<Entry> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else {
            continue;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<Entry>(trimmed) {
            entries.push(entry);
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn test_path_from() {
        // BLYTE_USAGE_LOG=/x/u.jsonl wins
        assert_eq!(
            path_from(
                Some(OsStr::new("/x/u.jsonl")),
                Some(OsStr::new("/xdg")),
                Some(OsStr::new("/home")),
            ),
            Some(PathBuf::from("/x/u.jsonl"))
        );

        // off gives None
        assert_eq!(
            path_from(
                Some(OsStr::new("off")),
                Some(OsStr::new("/xdg")),
                Some(OsStr::new("/home")),
            ),
            None
        );

        // XDG gives <xdg>/blyte/usage.jsonl
        assert_eq!(
            path_from(None, Some(OsStr::new("/xdg")), Some(OsStr::new("/home"))),
            Some(PathBuf::from("/xdg/blyte/usage.jsonl"))
        );

        // HOME gives <home>/.local/state/blyte/usage.jsonl
        assert_eq!(
            path_from(None, None, Some(OsStr::new("/home"))),
            Some(PathBuf::from("/home/.local/state/blyte/usage.jsonl"))
        );

        // empty strings are treated as unset
        assert_eq!(
            path_from(
                Some(OsStr::new("")),
                Some(OsStr::new("")),
                Some(OsStr::new("/home")),
            ),
            Some(PathBuf::from("/home/.local/state/blyte/usage.jsonl"))
        );
        assert_eq!(
            path_from(
                Some(OsStr::new("")),
                Some(OsStr::new("/xdg")),
                Some(OsStr::new("")),
            ),
            Some(PathBuf::from("/xdg/blyte/usage.jsonl"))
        );
        assert_eq!(
            path_from(
                Some(OsStr::new("")),
                Some(OsStr::new("")),
                Some(OsStr::new("")),
            ),
            None
        );

        // none gives None
        assert_eq!(path_from(None, None, None), None);
    }

    #[test]
    fn test_repo_of() {
        assert_eq!(repo_of(Path::new("/r/.wt/T1")), "/r");
        assert_eq!(repo_of(Path::new("/r")), "/r");
        assert_eq!(repo_of(Path::new("/r/.wtx/a")), "/r/.wtx/a");
    }

    #[test]
    fn test_append_twice_and_read() {
        let tmp = tempfile::tempdir().unwrap();
        let log_path = tmp.path().join("parent").join("sub").join("usage.jsonl");

        let entry1 = Entry {
            ts: 1700000000,
            repo: "/repo/one".into(),
            session: Some("sess-1".into()),
            dispatch: Some("disp-1".into()),
            research: false,
            model: "claude-3-7-sonnet".into(),
            outcome: "verified".into(),
            turns: 3,
            input: 1000,
            cached: 200,
            output: 300,
            thinking: 50,
            cost_usd: Some(0.015),
        };

        let entry2 = Entry {
            ts: 1700000010,
            repo: "/repo/two".into(),
            session: None,
            dispatch: None,
            research: true,
            model: "gemini-3.8-flash".into(),
            outcome: "budgetexhausted".into(),
            turns: 15,
            input: 5000,
            cached: 1000,
            output: 800,
            thinking: 0,
            cost_usd: None,
        };

        append(&log_path, &entry1).expect("first append should succeed");

        // Inject garbage line
        {
            let mut f = OpenOptions::new()
                .append(true)
                .open(&log_path)
                .expect("open log file to inject garbage");
            f.write_all(b"not valid json here at all\n")
                .expect("write garbage");
        }

        append(&log_path, &entry2).expect("second append should succeed");

        let entries = read(&log_path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], entry1);
        assert_eq!(entries[1], entry2);
    }

    #[test]
    fn test_session_none_serialization() {
        let entry = Entry {
            ts: 1700000000,
            repo: "/r".into(),
            session: None,
            dispatch: None,
            research: false,
            model: "m".into(),
            outcome: "verified".into(),
            turns: 1,
            input: 10,
            cached: 0,
            output: 5,
            thinking: 0,
            cost_usd: None,
        };

        let json_str = serde_json::to_string(&entry).unwrap();
        assert!(
            !json_str.contains("\"session\""),
            "session key must be omitted when None"
        );
        assert!(
            !json_str.contains("\"dispatch\""),
            "dispatch key must be omitted when None"
        );

        let parsed: Entry = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed, entry);
    }
}
