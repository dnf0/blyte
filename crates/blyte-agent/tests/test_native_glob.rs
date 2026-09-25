//! `glob` on a local workspace expands in-process, matching what the shell expands (bash
//! globstar + dotglob + nullglob, or zsh where bash lacks globstar), without spawning.
#![cfg(unix)]

use std::collections::BTreeSet;
use std::path::Path;

use blyte_agent::fs::LocalFs;
use blyte_agent::search::glob_paths;
use blyte_agent::shell::{LocalShell, Output, Shell};
use blyte_agent::tools::execute;
use serde_json::json;

struct NoShell;
impl Shell for NoShell {
    fn exec(&self, command: &str, _: &Path, _: std::time::Duration) -> Result<Output, String> {
        panic!("glob spawned a command: {command}");
    }
    fn describe(&self) -> String {
        String::new()
    }
}

fn tree() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    for dir in ["src/deep", "src/.dot", "docs"] {
        std::fs::create_dir_all(r.join(dir)).unwrap();
    }
    for f in [
        "a.rs",
        ".hidden.rs",
        "src/b.rs",
        "src/c.rs",
        "src/deep/d.rs",
        "src/.dot/e.rs",
        "docs/readme.md",
    ] {
        std::fs::write(r.join(f), "x").unwrap();
    }
    std::os::unix::fs::symlink(r.join("src"), r.join("link")).unwrap();
    d
}

/// What the harness's shell glob returns for `pattern`, as a set.
fn shell_glob(root: &Path, pattern: &str) -> BTreeSet<String> {
    let o = execute(
        root,
        &LocalShell,
        &NotLocal,
        "glob",
        &json!({ "pattern": pattern }),
    );
    o.content
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

struct NotLocal;
impl blyte_agent::fs::Fs for NotLocal {
    fn read(&self, p: &Path) -> Result<String, String> {
        blyte_agent::fs::Fs::read(&LocalFs, p)
    }
    fn write(&self, p: &Path, c: &str) -> Result<(), String> {
        blyte_agent::fs::Fs::write(&LocalFs, p, c)
    }
    fn canonicalize(&self, p: &Path) -> Result<std::path::PathBuf, String> {
        blyte_agent::fs::Fs::canonicalize(&LocalFs, p)
    }
}

#[test]
fn native_expansion_matches_the_shell() {
    let d = tree();
    for pattern in [
        "**/*.rs",
        "*.rs",
        "src/*",
        "src/**/*.rs",
        "**/*.md",
        "src/[bc]*.rs",
        "*",
    ] {
        let native: BTreeSet<String> = glob_paths(d.path(), pattern, 200)
            .expect("emulated")
            .into_iter()
            .collect();
        assert_eq!(native, shell_glob(d.path(), pattern), "{pattern}");
    }
}

#[test]
fn results_are_sorted_capped_and_unmatched_is_empty() {
    let d = tree();
    let all = glob_paths(d.path(), "**/*.rs", 200).unwrap();
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(all, sorted);
    assert_eq!(glob_paths(d.path(), "**/*.rs", 2).unwrap().len(), 2);
    assert_eq!(
        glob_paths(d.path(), "nomatch*", 200).unwrap(),
        Vec::<String>::new()
    );
    assert!(glob_paths(d.path(), "/etc/*", 200).is_none());
    assert!(glob_paths(d.path(), "../*", 200).is_none());
    assert!(glob_paths(d.path(), "src/", 200).is_none());
}

#[test]
fn the_glob_tool_uses_it_on_a_local_workspace_without_spawning() {
    let d = tree();
    let o = execute(
        d.path(),
        &NoShell,
        &LocalFs,
        "glob",
        &json!({"pattern": "src/*.rs"}),
    );
    assert_eq!(o.content, "2 file(s):\nsrc/b.rs\nsrc/c.rs");
    let none = execute(
        d.path(),
        &NoShell,
        &LocalFs,
        "glob",
        &json!({"pattern": "*.zz"}),
    );
    assert_eq!(none.content, "no files match *.zz");
}
