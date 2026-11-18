//! Breadth and split checks for `blyte review` on research runs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// A prompt above this many tokens (fresh plus cached input) counts as large.
pub const LARGE_PROMPT_TOKENS: u64 = 100_000;
/// A run with at least this many large-prompt turns is flagged as broad.
pub const BROAD_TURNS: usize = 10;
/// Most directories a split suggestion lists.
pub const MAX_DIRS: usize = 8;

/// (turns with a prompt over `LARGE_PROMPT_TOKENS`, turns in all) from a run report's `turn_usage`.
/// `Err` names what is wrong when `turn_usage` is missing or an entry lacks `input` or `cached`.
pub fn large_prompt_turns(report: &serde_json::Value) -> Result<(usize, usize), String> {
    let Some(turns) = report.get("turn_usage").and_then(|v| v.as_array()) else {
        return Err("report is missing turn_usage array".to_string());
    };
    let mut large = 0usize;
    for (idx, turn) in turns.iter().enumerate() {
        let Some(input) = turn.get("input").and_then(|v| v.as_u64()) else {
            return Err(format!("turn_usage[{idx}] is missing u64 input"));
        };
        let Some(cached) = turn.get("cached").and_then(|v| v.as_u64()) else {
            return Err(format!("turn_usage[{idx}] is missing u64 cached"));
        };
        if input.saturating_add(cached) > LARGE_PROMPT_TOKENS {
            large += 1;
        }
    }
    Ok((large, turns.len()))
}

/// The flag for a broad run, or `None` when fewer than `BROAD_TURNS` turns were large.
pub fn breadth_flag(large: usize, total: usize) -> Option<String> {
    if large < BROAD_TURNS {
        return None;
    }
    Some(format!(
        "broad research run: {large} of {total} turns had prompts over 100K tokens; narrower questions run faster and fail less"
    ))
}

/// Directories the run read files from, with the number of distinct files read in each,
/// most files first, then by name; at most `MAX_DIRS`.
pub fn read_dirs(log: &str, root: &Path) -> Vec<(String, usize)> {
    let mut files = BTreeSet::new();
    for line in log.lines() {
        let trimmed = line.trim_start_matches(' ');
        let Some(after_read) = trimmed.strip_prefix("● read ") else {
            continue;
        };
        let Some((raw_path, _)) = after_read.split_once(" -> ") else {
            continue;
        };
        let path_str = strip_line_range(raw_path);
        if !path_str.is_empty() {
            files.insert(path_str.to_string());
        }
    }

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for file in files {
        let path = Path::new(&file);
        let Some(parent) = path.parent() else {
            continue;
        };
        let dir = match parent.strip_prefix(root) {
            Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
            Ok(rel) => rel.to_string_lossy().into_owned(),
            Err(_) if parent.as_os_str().is_empty() => ".".to_string(),
            Err(_) => parent.to_string_lossy().into_owned(),
        };
        *counts.entry(dir).or_insert(0) += 1;
    }

    let mut dirs: Vec<(String, usize)> = counts.into_iter().collect();
    dirs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    dirs.truncate(MAX_DIRS);
    dirs
}

fn strip_line_range(s: &str) -> &str {
    let Some(before_close) = s.strip_suffix(')') else {
        return s;
    };
    let Some((path, range)) = before_close.rsplit_once(" (L") else {
        return s;
    };
    let Some((start, end)) = range.split_once("-L") else {
        return s;
    };
    if !start.is_empty()
        && start.bytes().all(|b| b.is_ascii_digit())
        && !end.is_empty()
        && end.bytes().all(|b| b.is_ascii_digit())
    {
        path
    } else {
        s
    }
}

/// The flag for a run that ran out of budget, or `None` when `dirs` is empty.
pub fn split_flag(dirs: &[(String, usize)]) -> Option<String> {
    if dirs.is_empty() {
        return None;
    }
    let listed: Vec<String> = dirs
        .iter()
        .map(|(dir, count)| format!("{dir} ({count})"))
        .collect();
    Some(format!(
        "ran out of budget: split the question; files read by directory: {}",
        listed.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn large_turns_are_counted() {
        let report = json!({
            "turn_usage": [
                {"input": 60000, "cached": 50000, "output": 100},
                {"input": 100000, "cached": 0, "output": 100},
                {"input": 1, "cached": 100000, "output": 100}
            ]
        });
        assert_eq!(large_prompt_turns(&report), Ok((2, 3)));
    }

    #[test]
    fn a_report_without_turn_usage_is_an_error() {
        let empty = json!({});
        let err = large_prompt_turns(&empty).unwrap_err();
        assert!(err.contains("turn_usage"), "unexpected error: {err}");

        let missing_cached = json!({
            "turn_usage": [
                {"input": 1000}
            ]
        });
        let err = large_prompt_turns(&missing_cached).unwrap_err();
        assert!(err.contains("cached"), "unexpected error: {err}");
    }

    #[test]
    fn the_breadth_flag_needs_ten_large_turns() {
        assert_eq!(breadth_flag(9, 40), None);
        assert_eq!(
            breadth_flag(10, 40),
            Some(
                "broad research run: 10 of 40 turns had prompts over 100K tokens; narrower questions run faster and fail less"
                    .to_string()
            )
        );
    }

    #[test]
    fn read_dirs_groups_distinct_files() {
        let log = "\
  ● read /r/crates/a/src/x.rs -> 800 lines
  ● read /r/crates/a/src/x.rs (L1-L800) -> 800 lines
● read /r/crates/a/src/y.rs -> 200 lines
  ● read /r/docs/u.md -> 50 lines
  ● read /r/README.md -> 30 lines
  ● read /other/z.rs -> 10 lines
  ● bash ls -> exit 0
  ● read /r/broken
";
        let dirs = read_dirs(log, Path::new("/r"));
        assert_eq!(
            dirs,
            vec![
                ("crates/a/src".to_string(), 2),
                (".".to_string(), 1),
                ("/other".to_string(), 1),
                ("docs".to_string(), 1),
            ]
        );
    }

    #[test]
    fn read_dirs_lists_at_most_eight() {
        let mut log = String::new();
        for i in 0..10 {
            log.push_str(&format!("● read /r/dir{i}/file.rs -> 10 lines\n"));
        }
        let dirs = read_dirs(&log, Path::new("/r"));
        assert_eq!(dirs.len(), 8);
    }

    #[test]
    fn the_split_flag_lists_directories() {
        let dirs = vec![("crates/a/src".to_string(), 12), ("docs".to_string(), 3)];
        assert_eq!(
            split_flag(&dirs),
            Some(
                "ran out of budget: split the question; files read by directory: crates/a/src (12), docs (3)"
                    .to_string()
            )
        );
        assert_eq!(split_flag(&[]), None);
    }
}
