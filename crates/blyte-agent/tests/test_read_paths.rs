use blyte_agent::fs::LocalFs;
use blyte_agent::run::extract_action_target;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::execute;
use serde_json::json;

#[test]
fn two_files_come_back_with_headers_numbering_and_a_blank_line_between() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "first line\nsecond line\n").unwrap();
    std::fs::write(d.path().join("b.txt"), "hello world\n").unwrap();

    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"paths": ["a.txt", "b.txt"]}),
    );
    assert!(!o.is_error);
    let expected = "\
==> a.txt <==
     1\tfirst line
     2\tsecond line

==> b.txt <==
     1\thello world";
    assert_eq!(o.content, expected);
}

#[test]
fn missing_file_among_good_ones_is_reported_inline_and_the_call_is_not_an_error() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "aaa\n").unwrap();
    std::fs::write(d.path().join("c.txt"), "ccc\n").unwrap();

    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"paths": ["a.txt", "b_missing.txt", "c.txt"]}),
    );
    assert!(!o.is_error);
    assert!(o.content.contains("==> a.txt <==\n     1\taaa"));
    assert!(o.content.contains("==> b_missing.txt <==\ncannot read"));
    assert!(o.content.contains("==> c.txt <==\n     1\tccc"));
}

#[test]
fn all_missing_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"paths": ["m1.txt", "m2.txt"]}),
    );
    assert!(o.is_error);
    assert!(o.content.contains("==> m1.txt <==\ncannot read"));
    assert!(o.content.contains("==> m2.txt <==\ncannot read"));
}

#[test]
fn argument_validation_errors() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("a.txt"), "aaa\n").unwrap();

    // path and paths together
    let o1 = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"path": "a.txt", "paths": ["a.txt"]}),
    );
    assert!(o1.is_error);

    // neither
    let o2 = execute(d.path(), &LocalShell, &LocalFs, "read", &json!({}));
    assert!(o2.is_error);

    // 11 paths
    let paths_11: Vec<String> = (1..=11).map(|i| format!("{i}.txt")).collect();
    let o3 = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"paths": paths_11}),
    );
    assert!(o3.is_error);

    // paths with start_line
    let o4 = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"paths": ["a.txt"], "start_line": 1}),
    );
    assert!(o4.is_error);

    // paths with end_line
    let o5 = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"paths": ["a.txt"], "end_line": 1}),
    );
    assert!(o5.is_error);
}

#[test]
fn three_files_whose_total_exceeds_max_output_chars_third_is_listed_as_not_read_and_first_two_are_whole(
) {
    let d = tempfile::tempdir().unwrap();
    let line_content = "x".repeat(92);
    let body: String = (1..=250).map(|_| format!("{line_content}\n")).collect();
    std::fs::write(d.path().join("f1.txt"), &body).unwrap();
    std::fs::write(d.path().join("f2.txt"), &body).unwrap();
    std::fs::write(d.path().join("f3.txt"), &body).unwrap();

    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"paths": ["f1.txt", "f2.txt", "f3.txt"]}),
    );
    assert!(!o.is_error);
    assert!(
        !o.content.contains("elided by blyte"),
        "neither file should be cut"
    );
    assert!(o.content.contains("==> f1.txt <=="));
    assert!(o.content.contains("   250\t"));
    assert!(o.content.contains("==> f2.txt <=="));
    assert!(o
        .content
        .contains("not read (over this call's budget; read separately): f3.txt"));
    assert!(!o.content.contains("==> f3.txt <=="));
}

#[test]
fn single_path_read_is_byte_identical_to_before() {
    let d = tempfile::tempdir().unwrap();
    let body = "first line\nsecond line\nthird line\n";
    std::fs::write(d.path().join("a.txt"), body).unwrap();

    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"path": "a.txt"}),
    );
    assert!(!o.is_error);
    let expected = "     1\tfirst line\n     2\tsecond line\n     3\tthird line";
    assert_eq!(o.content, expected);
    assert!(!o.content.contains("==>"));
}

#[test]
fn extract_action_target_for_four_path_read() {
    let target = extract_action_target("read", &json!({"paths": ["a", "b", "c", "d"]}));
    assert_eq!(target, "a, b, c (+1)");

    let target3 = extract_action_target("read", &json!({"paths": ["a", "b", "c"]}));
    assert_eq!(target3, "a, b, c");

    let target2 = extract_action_target("read", &json!({"paths": ["a", "b"]}));
    assert_eq!(target2, "a, b");

    let target5 = extract_action_target("read", &json!({"paths": ["a", "b", "c", "d", "e"]}));
    assert_eq!(target5, "a, b, c (+2)");
}

#[test]
fn single_file_larger_than_whole_budget_behaves_as_single_file_read_only_when_first() {
    let d = tempfile::tempdir().unwrap();
    let body: String = (1..=2_500).map(|i| format!("line {i}\n")).collect();
    std::fs::write(d.path().join("big.txt"), &body).unwrap();
    std::fs::write(d.path().join("small.txt"), "small\n").unwrap();

    let o = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"paths": ["big.txt", "small.txt"]}),
    );
    assert!(!o.is_error);
    assert!(o.content.contains("==> big.txt <=="));
    assert!(o.content.contains("elided by blyte"));
    assert!(o
        .content
        .contains("not read (over this call's budget; read separately): small.txt"));
}
