use std::collections::HashMap;
use std::path::{Path, PathBuf};

use blyte_agent::research_check::{check, render, Sources};

const SRC: &str = "fn main() {\n    let x = load_config();\n    run(x);\n}\n\nfn run(x: Config) {\n    println!(\"{x:?}\");\n}\n";

fn files() -> HashMap<PathBuf, String> {
    let mut m = HashMap::new();
    m.insert(PathBuf::from("/ws/src/main.rs"), SRC.to_string());
    m.insert(
        PathBuf::from("/ws/Makefile"),
        "test:\n\tcargo test\n".to_string(),
    );
    m.insert(
        PathBuf::from("/repo/lib/util.py"),
        "def f():\n    return 1\n".to_string(),
    );
    m
}

fn run_check(report: &str) -> Vec<String> {
    let m = files();
    let read = |p: &Path| m.get(p).cloned().ok_or_else(|| "No such file".to_string());
    let roots = vec![PathBuf::from("/repo")];
    let listing = vec![
        PathBuf::from("/ws/src/main.rs"),
        PathBuf::from("/repo/lib/util.py"),
    ];
    check(
        report,
        &Sources {
            workspace: Path::new("/ws"),
            read_roots: &roots,
            listing: &listing,
            read: &read,
        },
    )
}

#[test]
fn exact_quote_passes() {
    let r = "The entry point (src/main.rs:1-4):\n\n```rust\nfn main() {\n    let x = load_config();\n    run(x);\n}\n```\n";
    assert!(run_check(r).is_empty(), "{:?}", run_check(r));
}

#[test]
fn numbered_quote_must_match_its_line() {
    let ok = "src/main.rs:2-3\n```\n     2\tlet x = load_config();\n     3\trun(x);\n```\n";
    assert!(run_check(ok).is_empty(), "{:?}", run_check(ok));
    let bad = "src/main.rs:2-3\n```\n     2\trun(x);\n     3\tlet y = 4;\n```\n";
    let p = run_check(bad);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("2 of 2 lines"), "{}", p[0]);
    assert!(p[0].contains("is at line 3"), "{}", p[0]);
    assert!(p[0].contains("not in the file"), "{}", p[0]);
}

#[test]
fn invented_code_is_reported() {
    let r = "See `src/main.rs:6-8`:\n\n```rust\nfn run(x: Config) -> Result<()> {\n    validate(&x)?;\n}\n```\n";
    let p = run_check(r);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(
        p[0].starts_with("report.md:3: code block quoting src/main.rs:6-8"),
        "{}",
        p[0]
    );
}

#[test]
fn quote_far_from_cited_lines_is_reported() {
    // The window is the cited lines plus 5 either side; this file is only 8 lines long.
    let r = "src/main.rs:20-30\n```\nfn main() {\n```\n";
    assert!(!run_check(r).is_empty());
}

#[test]
fn elisions_proposed_blocks_and_uncited_blocks_are_not_checked() {
    let r = "src/main.rs:1-4\n```rust\nfn main() {\n    // ...\n    ...\n}\n```\n\
             \nProposed change to src/main.rs:3:\n```proposed\nrun_all(x);\n```\n\
             \nA sketch:\n```\nnot in any file\n```\n";
    assert!(run_check(r).is_empty(), "{:?}", run_check(r));
}

#[test]
fn prose_citations_must_exist() {
    assert!(run_check("Called from src/main.rs:3.\n").is_empty());
    let p = run_check("Called from src/main.rs:40.\n");
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("past the end"), "{}", p[0]);
    let p = run_check("Defined in src/missing.rs:3.\n");
    assert!(p[0].contains("no such file: src/missing.rs"), "{}", p[0]);
}

#[test]
fn paths_resolve_through_read_roots_suffixes_and_absolute_paths() {
    assert!(
        run_check("lib/util.py:2 and /repo/lib/util.py:1 and util.py:2 and main.rs:4\n").is_empty()
    );
    let p = run_check("/repo/lib/nope.py:1\n");
    assert_eq!(p.len(), 1, "{p:?}");
}

#[test]
fn non_paths_are_ignored() {
    let r = "Listens on 127.0.0.1:8080 and https://example.com/a.py:12; version.major:3 too.\n";
    assert!(run_check(r).is_empty(), "{:?}", run_check(r));
}

#[test]
fn render_lists_and_caps() {
    let probs: Vec<String> = (0..45).map(|i| format!("p{i}")).collect();
    let out = render(&probs);
    assert!(out.contains("45 problem(s)"));
    assert!(out.contains("- p39\n"));
    assert!(!out.contains("- p40\n"));
    assert!(out.contains("and 5 more"));
}

#[test]
fn lines_with_elided_parts_match() {
    let r = "src/main.rs:6-8\n```\nfn run(x: ...) {\n    println!(...);\n```\n";
    assert!(run_check(r).is_empty(), "{:?}", run_check(r));
    let r = "src/main.rs:6-8\n```\nfn walk(x: ...) {\n```\n";
    assert_eq!(run_check(r).len(), 1);
}

#[test]
fn paths_resolve_under_a_read_roots_parent() {
    assert!(run_check("repo/lib/util.py:2\n").is_empty());
}

#[test]
fn a_first_line_comment_names_the_source() {
    let r =
        "Somewhere in src/main.rs:1:\n```python\n# lib/util.py:1-2\ndef f():\n    return 1\n```\n";
    assert!(run_check(r).is_empty(), "{:?}", run_check(r));
}

#[test]
fn lead_ins_naming_a_path_without_a_colon_cite_it() {
    let ok = [
        "In `src/main.rs`:\n```rust\n    run(x);\n```\n",
        "- **`src/main.rs`** (lines 6-8):\n```rust\nfn run(x: Config) {\n```\n",
        "The test target, Makefile:1-2:\n```makefile\ntest:\n\tcargo test\n```\n",
        "File: `Makefile`\n```\ntest:\n```\n",
    ];
    for r in ok {
        assert!(run_check(r).is_empty(), "{r}: {:?}", run_check(r));
    }
    let p = run_check("In `src/main.rs`:\n```rust\n    walk(x);\n```\n");
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("quoting src/main.rs:"), "{}", p[0]);
}

#[test]
fn line_numbers_alone_refer_to_the_last_cited_file() {
    let r = "See src/main.rs:1.\n\nIn line 3:\n```rust\nrun(x);\n```\n";
    assert!(run_check(r).is_empty(), "{:?}", run_check(r));
    let r = "See src/main.rs:1.\n\nLine 7:\n```rust\nfn main() {\n```\n";
    assert_eq!(run_check(r).len(), 1);
}

#[test]
fn uncited_code_blocks_fail_unless_commands_output_or_proposed() {
    let p = run_check("The loader does:\n\n```python\nconfig = load()\n```\n");
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(
        p[0].contains("python code block with no citation"),
        "{}",
        p[0]
    );
    let p = run_check("Output of the read:\n```\n  12\tfoo()\n  13\tbar()\n```\n");
    assert!(
        p[0].contains("numbered code block with no citation"),
        "{}",
        p[0]
    );
    let fine = [
        "Run it:\n```bash\ncargo test\n```\n",
        "It prints:\n```text\nok\n```\n",
        "A diagram:\n```\na -> b\n```\n",
        "Add a matcher:\n```proposed\ndef m(): pass\n```\n",
    ];
    for r in fine {
        assert!(run_check(r).is_empty(), "{r}: {:?}", run_check(r));
    }
}

#[test]
fn a_proposed_file_named_in_a_lead_in_must_be_tagged() {
    let p = run_check("Draft `jobs/drought.yaml`:\n```yaml\njob: drought\n```\n");
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("no such file: jobs/drought.yaml"), "{}", p[0]);
    assert!(run_check("Draft `jobs/drought.yaml`:\n```proposed\njob: drought\n```\n").is_empty());
}

#[test]
fn unresolved_bare_source_files_are_reported() {
    let p = run_check("See scorer.py:40.\n");
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("no such file: scorer.py"), "{}", p[0]);
    assert!(run_check("See notes.txt:4 and README.md:3.\n").is_empty());
}
