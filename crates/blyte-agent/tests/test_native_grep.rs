//! `grep` on a local workspace runs ripgrep's own libraries in-process: rg's line format and
//! regex semantics, `.gitignore` respected, in a deterministic order, with no process spawned.

use std::path::Path;

use blyte_agent::fs::LocalFs;
use blyte_agent::search::grep_lines;
use blyte_agent::shell::{Output, Shell};
use blyte_agent::tools::execute;
use serde_json::json;

/// Fails the test if anything tries to spawn a command.
struct NoShell;
impl Shell for NoShell {
    fn exec(&self, command: &str, _: &Path, _: std::time::Duration) -> Result<Output, String> {
        panic!("grep spawned a command: {command}");
    }
    fn describe(&self) -> String {
        String::new()
    }
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::create_dir_all(r.join("src/deep")).unwrap();
    std::fs::create_dir_all(r.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(r.join(".git")).unwrap();
    std::fs::write(r.join(".gitignore"), "node_modules/\n").unwrap();
    std::fs::write(
        r.join("src/a.rs"),
        "fn alpha() {}\nlet x = 1;\nfn beta() {}\n",
    )
    .unwrap();
    std::fs::write(r.join("src/deep/b.rs"), "fn gamma() {}\n").unwrap();
    std::fs::write(r.join("src/c.txt"), "fn not_rust\n").unwrap();
    std::fs::write(r.join("node_modules/pkg/i.js"), "fn ignored() {}\n").unwrap();
    d
}

#[test]
fn matches_rg_format_and_respects_gitignore() {
    let d = repo();
    let lines = grep_lines(d.path(), ".", "fn [a-z]+\\(", "", 200).unwrap();
    assert_eq!(
        lines,
        vec![
            "./src/a.rs:1:fn alpha() {}",
            "./src/a.rs:3:fn beta() {}",
            "./src/deep/b.rs:1:fn gamma() {}",
        ]
    );
}

#[test]
fn a_sub_path_prefixes_as_given_and_a_single_file_has_no_prefix() {
    let d = repo();
    assert_eq!(
        grep_lines(d.path(), "src/deep", "gamma", "", 200).unwrap(),
        vec!["src/deep/b.rs:1:fn gamma() {}"]
    );
    assert_eq!(
        grep_lines(d.path(), "src/a.rs", "beta", "", 200).unwrap(),
        vec!["3:fn beta() {}"]
    );
}

#[test]
fn a_glob_filters_like_rg_and_matches_are_capped() {
    let d = repo();
    let only_txt = grep_lines(d.path(), ".", "fn", "*.txt", 200).unwrap();
    assert_eq!(only_txt, vec!["./src/c.txt:1:fn not_rust"]);
    assert_eq!(grep_lines(d.path(), ".", "fn", "", 2).unwrap().len(), 2);
    assert!(grep_lines(d.path(), ".", "(unclosed", "", 200).is_err());
}

#[test]
fn the_grep_tool_uses_it_on_a_local_workspace_without_spawning() {
    let d = repo();
    let o = execute(
        d.path(),
        &NoShell,
        &LocalFs,
        "grep",
        &json!({"pattern": "gamma"}),
    );
    assert!(!o.is_error, "{}", o.content);
    assert_eq!(o.content, "1 match(es):\n./src/deep/b.rs:1:fn gamma() {}");
    let none = execute(
        d.path(),
        &NoShell,
        &LocalFs,
        "grep",
        &json!({"pattern": "zzz"}),
    );
    assert_eq!(none.content, "no matches for zzz");
}

#[test]
fn sd_and_tokei_are_accelerators_and_pretty_printers_are_not() {
    use blyte_agent::tools::ACCELERATORS;
    for t in ["rg", "fd", "sd", "tokei"] {
        assert!(ACCELERATORS.contains(&t), "{t}");
    }
    for t in ["bat", "eza", "dust", "delta"] {
        assert!(!ACCELERATORS.contains(&t), "{t}");
    }
}
