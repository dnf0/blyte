//! Quote directives in research reports expand to the file's own lines, or report why they cannot.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use blyte_agent::quote_expand::{expand, MAX_QUOTE_LINES};
use blyte_agent::research_check;

fn files() -> HashMap<PathBuf, String> {
    let mut m = HashMap::new();
    m.insert(
        PathBuf::from("/r/a.py"),
        "one\ntwo\nthree\nfour\n".to_string(),
    );
    m.insert(PathBuf::from("/r/Makefile"), "all:\n\tcc\n".to_string());
    m.insert(
        PathBuf::from("/r/long.txt"),
        (1..=201).map(|i| format!("L{i}\n")).collect(),
    );
    m.insert(
        PathBuf::from("/r/ok.txt"),
        (1..=200).map(|i| format!("L{i}\n")).collect(),
    );
    m
}

fn read(map: &HashMap<PathBuf, String>) -> impl Fn(&Path) -> Result<String, String> + '_ {
    move |p: &Path| {
        map.get(p)
            .cloned()
            .ok_or_else(|| format!("no such file {}", p.display()))
    }
}

fn run(report: &str) -> (String, Vec<String>) {
    let m = files();
    let result = expand(report, &read(&m));
    result
}

#[test]
fn range_directive_becomes_citation_and_tagged_block() {
    let (out, problems) = run("Entry, see below:\n<<quote /r/a.py:2-3>>\nDone.\n");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        out,
        "Entry, see below:\n`/r/a.py:2-3`\n```py\ntwo\nthree\n```\nDone.\n"
    );
}

#[test]
fn single_line_form_cites_one_line() {
    let (out, problems) = run("<<quote /r/a.py:4>>\n");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(out, "`/r/a.py:4`\n```py\nfour\n```\n");
}

#[test]
fn path_without_extension_gives_untagged_block() {
    let (out, problems) = run("<<quote /r/Makefile:1-2>>\n");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(out, "`/r/Makefile:1-2`\n```\nall:\n\tcc\n```\n");
}

#[test]
fn report_without_directives_comes_back_identical() {
    for report in [
        "no directives here\n",
        "no trailing newline",
        "crlf line\r\nanother\r\n",
        "",
    ] {
        let (out, problems) = run(report);
        assert_eq!(out, report);
        assert!(problems.is_empty());
    }
}

#[test]
fn crlf_and_missing_trailing_newline_keep_their_endings() {
    let (out, problems) = run("a\r\n<<quote /r/a.py:1>>\r\nb\r\n");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(out, "a\r\n`/r/a.py:1`\r\n```py\r\none\r\n```\r\nb\r\n");

    let (out, problems) = run("<<quote /r/a.py:1>>");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(out, "`/r/a.py:1`\n```py\none\n```");
}

#[test]
fn directive_inside_fenced_block_is_left_alone() {
    let report = "```text\n<<quote /r/a.py:1>>\n```\n";
    let (out, problems) = run(report);
    assert_eq!(out, report);
    assert!(problems.is_empty());
}

#[test]
fn indented_directive_is_expanded() {
    let (out, problems) = run("   <<quote /r/a.py:1>>   \n");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(out, "`/r/a.py:1`\n```py\none\n```\n");
}

#[test]
fn expanding_twice_equals_expanding_once() {
    let (once, _) = run("x\n<<quote /r/a.py:1-2>>\n<<quote /r/Makefile:2>>\n");
    let (twice, problems) = run(&once);
    assert!(problems.is_empty());
    assert_eq!(twice, once);
}

#[test]
fn malformed_directive_stays_and_is_reported() {
    let report = "a\n<<quote relative/a.py:1>>\n<<quote /r/a.py:1-2>>\n";
    let (out, problems) = run(report);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(
        problems[0],
        "report.md:2: <<quote relative/a.py:1>>: the path must be absolute"
    );
    assert!(out.contains("<<quote relative/a.py:1>>\n"));
    assert!(out.contains("```py\none\ntwo\n```"));
}

#[test]
fn not_matching_the_form_is_reported() {
    let (out, problems) = run("<<quote /r/a.py 1-2>>\n");
    assert_eq!(out, "<<quote /r/a.py 1-2>>\n");
    assert_eq!(
        problems,
        vec![format!(
            "report.md:1: <<quote /r/a.py 1-2>>: not a quote directive; write <<quote /absolute/path:N-M>> alone on a line"
        )]
    );
}

#[test]
fn unreadable_file_is_reported() {
    let (out, problems) = run("<<quote /r/missing.py:1>>\n");
    assert_eq!(out, "<<quote /r/missing.py:1>>\n");
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("report.md:1: <<quote /r/missing.py:1>>: cannot read the file: ")
    );
    assert!(problems[0].contains("no such file"));
}

#[test]
fn bad_line_range_is_reported() {
    let (out, problems) = run("<<quote /r/a.py:0-2>>\n<<quote /r/a.py:3-2>>\n");
    assert_eq!(out, "<<quote /r/a.py:0-2>>\n<<quote /r/a.py:3-2>>\n");
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert_eq!(
        problems[0],
        "report.md:1: <<quote /r/a.py:0-2>>: bad line range"
    );
    assert_eq!(
        problems[1],
        "report.md:2: <<quote /r/a.py:3-2>>: bad line range"
    );
}

#[test]
fn line_past_end_is_reported() {
    let (out, problems) = run("<<quote /r/a.py:3-9>>\n");
    assert_eq!(out, "<<quote /r/a.py:3-9>>\n");
    assert_eq!(
        problems,
        vec!["report.md:1: <<quote /r/a.py:3-9>>: the file has 4 lines".to_string()]
    );
}

#[test]
fn too_many_lines_is_reported() {
    let (out, problems) = run("<<quote /r/long.txt:1-201>>\n");
    assert_eq!(out, "<<quote /r/long.txt:1-201>>\n");
    assert_eq!(
        problems,
        vec![format!(
            "report.md:1: <<quote /r/long.txt:1-201>>: quotes 201 lines; the most is {MAX_QUOTE_LINES}, quote the part that matters"
        )]
    );
}

#[test]
fn failing_directive_does_not_stop_the_others() {
    let report = "<<quote /r/missing.py:1>>\n<<quote /r/a.py:1>>\n";
    let (out, problems) = run(report);
    assert_eq!(problems.len(), 1);
    assert!(out.starts_with("<<quote /r/missing.py:1>>\n"));
    assert!(out.contains("`/r/a.py:1`\n```py\none\n```\n"));
}

#[test]
fn two_hundred_lines_expand() {
    let (out, problems) = run("<<quote /r/ok.txt:1-200>>\n");
    assert!(problems.is_empty(), "{problems:?}");
    assert!(out.contains("L200\n```\n"));
}

#[test]
fn expanded_block_passes_the_research_check() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src.py");
    std::fs::write(&src, "def main():\n    run()\n").unwrap();
    let abs = src.to_string_lossy().to_string();
    let report = format!("Entry point:\n<<quote {abs}:1-2>>\n");

    let read = |p: &Path| std::fs::read_to_string(p).map_err(|e| e.to_string());
    let (expanded, problems) = expand(&report, &read);
    assert!(problems.is_empty(), "{problems:?}");

    let listing = vec![src.clone()];
    let problems = research_check::check(
        &expanded,
        &research_check::Sources {
            workspace: dir.path(),
            read_roots: &[],
            listing: &listing,
            read: &read,
        },
    );
    assert!(problems.is_empty(), "{problems:?}\n{expanded}");
}

#[test]
fn lines_holding_a_code_fence_are_not_quoted() {
    let mut m = files();
    m.insert(
        PathBuf::from("/r/doc.md"),
        "intro\n```sh\nls\n```\nend\n".to_string(),
    );
    let report = "<<quote /r/doc.md:1-3>>\n<<quote /r/doc.md:5>>\n";
    let (out, problems) = expand(report, &read(&m));
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with(
            "report.md:1: <<quote /r/doc.md:1-3>>: line 2 of the file is a code fence"
        ),
        "{problems:?}"
    );
    assert!(
        out.starts_with("<<quote /r/doc.md:1-3>>\n`/r/doc.md:5`\n```md\nend\n```\n"),
        "{out}"
    );
    assert_eq!(expand(&out, &read(&m)).0, out);
}
