//! Implementation of the `blyte comments` command.

use crate::comment_lint::{added_lines, comments_in, is_supported, Comment};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

/// Checks the working tree in `dir` against `base` for added comments.
pub fn run(base: &str, dir: &Path) -> i32 {
    let diff_out = match Command::new("git")
        .args(["diff", "--no-color", "--no-ext-diff", "--unified=0", base])
        .current_dir(dir)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("blyte comments: git diff failed: {e}");
            return 2;
        }
    };

    if !diff_out.status.success() {
        let stderr = String::from_utf8_lossy(&diff_out.stderr);
        let first_line = stderr.lines().next().unwrap_or("").trim();
        eprintln!("blyte comments: git diff failed: {first_line}");
        return 2;
    }

    let diff_text = match String::from_utf8(diff_out.stdout) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("blyte comments: git diff failed: {e}");
            return 2;
        }
    };

    let ls_out = match Command::new("git")
        .args(["ls-files", "--others", "--exclude-standard"])
        .current_dir(dir)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("blyte comments: git ls-files failed: {e}");
            return 2;
        }
    };

    if !ls_out.status.success() {
        let stderr = String::from_utf8_lossy(&ls_out.stderr);
        let first_line = stderr.lines().next().unwrap_or("").trim();
        eprintln!("blyte comments: git ls-files failed: {first_line}");
        return 2;
    }

    let ls_text = match String::from_utf8(ls_out.stdout) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("blyte comments: git ls-files failed: {e}");
            return 2;
        }
    };

    let mut tracked_added: BTreeMap<String, Option<BTreeSet<usize>>> = added_lines(&diff_text)
        .into_iter()
        .map(|(path, lines)| (path, Some(lines)))
        .collect();

    for raw in ls_text.lines() {
        let path = raw.trim();
        if !path.is_empty() {
            tracked_added.insert(path.to_string(), None);
        }
    }

    let mut found: Vec<Comment> = Vec::new();
    for (path, lines_opt) in tracked_added {
        if !is_supported(&path) {
            continue;
        }
        let source = match std::fs::read_to_string(dir.join(&path)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("blyte comments: cannot read {path}: {e}");
                return 2;
            }
        };
        for comment in comments_in(&path, &source) {
            let is_added = match &lines_opt {
                Some(lines) => lines.contains(&comment.line),
                None => true,
            };
            if is_added {
                found.push(comment);
            }
        }
    }

    if found.is_empty() {
        return 0;
    }

    println!(
        "comments added ({}). Remove each one. If it explained an assumption, a shortcut or a deviation from the task, fix that or say it in your final report.",
        found.len()
    );
    for c in &found {
        println!("{}:{}: {}", c.file, c.line, c.text);
    }
    println!(
        "Not counted: documentation comments (///, //!, /** */) and tool directives (noqa, eslint-disable, @ts-expect-error, SAFETY:)."
    );

    1
}
