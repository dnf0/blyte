use std::collections::HashSet;
use std::path::Path;

use blyte_agent::research_check::{identifiers, paths, unseen_paths, Sources};

fn problems(report: &str, seen: &[&str]) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    let routes = dir.path().join("src/api/routes.py");
    std::fs::create_dir_all(routes.parent().unwrap()).unwrap();
    std::fs::write(&routes, "def handle():\n    pass\n").unwrap();
    let makefile = dir.path().join("Makefile");
    std::fs::write(&makefile, "test:\n\tcargo test\n").unwrap();

    let listing = vec![routes, makefile];
    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| e.to_string());
    let src = Sources {
        workspace: dir.path(),
        read_roots: &[],
        listing: &listing,
        read: &read,
    };
    let seen_set: HashSet<String> = seen.iter().map(|s| (*s).to_string()).collect();
    unseen_paths(report, &src, &seen_set)
}

#[test]
fn an_invented_path_in_prose_is_flagged() {
    let p = problems("The handler is in src/api/handlers.py.", &[]);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("`src/api/handlers.py`"), "{}", p[0]);
    assert!(p[0].contains("no such file"), "{}", p[0]);
}

#[test]
fn an_invented_backticked_path_is_flagged() {
    let p = problems("See `src/api/handlers.py` for it.", &[]);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("`src/api/handlers.py`"), "{}", p[0]);
    assert!(p[0].contains("no such file"), "{}", p[0]);
}

#[test]
fn an_invented_absolute_path_is_flagged() {
    let p = problems("Read /nonexistent-root/src/a.rs first.", &[]);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("`/nonexistent-root/src/a.rs`"), "{}", p[0]);
    assert!(p[0].contains("no such file"), "{}", p[0]);
}

#[test]
fn an_existing_path_is_clean() {
    let p = problems("The handler is in src/api/routes.py.", &[]);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_path_seen_in_tool_output_is_clean() {
    let p = problems(
        "The workflow calls scripts/deploy.sh, which fails.",
        &["scripts/deploy.sh"],
    );
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_seen_longer_path_covers_its_suffix() {
    let p = problems(
        "It lives in `api/handlers.py`.",
        &["/repo/src/api/handlers.py"],
    );
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_path_said_to_be_new_is_clean() {
    let p = problems("Put the check in a new `src/api/checks.py`.", &[]);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_path_said_not_to_exist_is_clean() {
    let p = problems(
        "The config names `scripts/deploy.sh`, which does not exist.",
        &[],
    );
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_lead_in_to_a_proposed_block_is_clean() {
    let p = problems(
        "Draft `jobs/drought.yaml`:\n```proposed\njob: 1\n```\n",
        &[],
    );
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_url_is_clean() {
    let p = problems("See https://example.com/a/b.py for the source.", &[]);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_glob_and_a_placeholder_are_clean() {
    let p = problems("Match `src/**/*.rs` and `runs/<id>/x.json`.", &[]);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_branch_and_a_mime_type_are_clean() {
    let p = problems("Based on `origin/main`, served as `application/json`.", &[]);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_non_code_extension_is_clean() {
    let p = problems("The notes are in docs/notes/missing.md.", &[]);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_path_inside_a_fenced_block_is_clean() {
    let p = problems("```text\nsrc/api/handlers.py\n```\n", &[]);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_path_is_reported_once() {
    let p = problems(
        "The handler is in src/api/handlers.py.\nThe handler is in src/api/handlers.py.\n",
        &[],
    );
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].starts_with("report.md:1:"), "{}", p[0]);
    assert!(p[0].contains("`src/api/handlers.py`"), "{}", p[0]);
    assert!(p[0].contains("no such file"), "{}", p[0]);
}

#[test]
fn a_cited_path_is_left_to_the_line_check() {
    let p = problems("See src/api/handlers.py:12 for it.", &[]);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn identifiers_yield_paths() {
    let found: Vec<&str> = identifiers("ran scripts/deploy.sh and foo_bar").collect();
    assert!(found.contains(&"scripts/deploy.sh"), "{found:?}");
    assert!(found.contains(&"foo_bar"), "{found:?}");
}

#[test]
fn paths_finds_only_slash_paths() {
    let found: Vec<&str> = paths("see routes.py and src/api/routes.py.").collect();
    assert_eq!(found, vec!["src/api/routes.py"]);
}
