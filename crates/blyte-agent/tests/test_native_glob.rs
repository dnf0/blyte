//! `glob` follows fd/ripgrep conventions: `.gitignore`d files and `.git` are skipped, other
//! dotfiles are kept, directories match. Local workspaces expand in-process; containers list the
//! repository with one `git ls-files` and match here.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

use blyte_agent::fs::LocalFs;
use blyte_agent::search::{glob_filter, glob_paths};
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

/// Host files presented as a container's (not local), so the tool takes the container route.
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

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(r)
        .status()
        .unwrap();
    for dir in [
        "src/deep",
        "target/debug",
        "node_modules/pkg",
        ".github/workflows",
    ] {
        std::fs::create_dir_all(r.join(dir)).unwrap();
    }
    std::fs::write(r.join(".gitignore"), "target/\nnode_modules/\n*.log\n").unwrap();
    for f in [
        "a.rs",
        ".env.example",
        "src/b.rs",
        "src/deep/c.rs",
        "target/debug/build.rs",
        "node_modules/pkg/i.js",
        "debug.log",
        ".github/workflows/ci.yml",
    ] {
        std::fs::write(r.join(f), "x").unwrap();
    }
    d
}

#[test]
fn ignored_files_and_dot_git_are_skipped_and_dotfiles_kept() {
    let d = repo();
    assert_eq!(
        glob_paths(d.path(), "**/*.rs", 200).unwrap(),
        vec!["a.rs", "src/b.rs", "src/deep/c.rs"]
    );
    assert_eq!(
        glob_paths(d.path(), "*", 200).unwrap(),
        vec![".env.example", ".github", ".gitignore", "a.rs", "src"]
    );
    assert_eq!(
        glob_paths(d.path(), "**/*.yml", 200).unwrap(),
        vec![".github/workflows/ci.yml"]
    );
    assert_eq!(
        glob_paths(d.path(), "**/*.log", 200).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(
        glob_paths(d.path(), "src/*", 200).unwrap(),
        vec!["src/b.rs", "src/deep"]
    );
}

#[test]
fn limits_and_patterns_it_does_not_handle() {
    let d = repo();
    assert_eq!(glob_paths(d.path(), "**/*.rs", 2).unwrap().len(), 2);
    assert!(glob_paths(d.path(), "/etc/*", 200).is_none());
    assert!(glob_paths(d.path(), "../*", 200).is_none());
    assert!(glob_paths(d.path(), "src/", 200).is_none());
}

#[test]
fn a_file_listing_is_filtered_with_directories_derived() {
    let listing = "a.rs\nsrc/b.rs\nsrc/deep/c.rs\n.github/workflows/ci.yml\n";
    assert_eq!(
        glob_filter(listing, "src/*", 200).unwrap(),
        vec!["src/b.rs", "src/deep"]
    );
    assert_eq!(
        glob_filter(listing, "**/*.rs", 200).unwrap(),
        vec!["a.rs", "src/b.rs", "src/deep/c.rs"]
    );
    assert_eq!(
        glob_filter(listing, "*", 200).unwrap(),
        vec![".github", "a.rs", "src"]
    );
    assert!(glob_filter(listing, "/abs/*", 200).is_none());
}

#[test]
fn the_local_tool_expands_without_spawning() {
    let d = repo();
    let o = execute(
        d.path(),
        &NoShell,
        &LocalFs,
        "glob",
        &json!({"pattern": "src/**/*.rs"}),
    );
    assert_eq!(o.content, "2 file(s):\nsrc/b.rs\nsrc/deep/c.rs");
    let none = execute(
        d.path(),
        &NoShell,
        &LocalFs,
        "glob",
        &json!({"pattern": "*.zz"}),
    );
    assert_eq!(none.content, "no files match *.zz");
}

#[test]
fn the_container_route_lists_with_git_and_matches_the_same() {
    let d = repo();
    let o = execute(
        d.path(),
        &LocalShell,
        &NotLocal,
        "glob",
        &json!({"pattern": "**/*.rs"}),
    );
    assert_eq!(o.content, "3 file(s):\na.rs\nsrc/b.rs\nsrc/deep/c.rs");
}

#[test]
fn outside_a_git_repository_the_container_route_falls_back_to_the_shell() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("target")).unwrap();
    std::fs::write(d.path().join("target/x.rs"), "x").unwrap();
    let o = execute(
        d.path(),
        &LocalShell,
        &NotLocal,
        "glob",
        &json!({"pattern": "**/*.rs"}),
    );
    assert_eq!(o.content, "1 file(s):\ntarget/x.rs");
}
