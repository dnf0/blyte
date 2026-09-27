//! `grep` shows the match inside a long line: on sanitize-git-repo a secret sat at character
//! ~15,000 of a 21,367-character minified JSON line, every tool cut the line from the start,
//! and blyte never saw it (2026-09-27).

use blyte_agent::tools::{bound_output, excerpt_around, MAX_LINE_CHARS};

#[test]
fn a_short_text_is_unchanged() {
    assert_eq!(excerpt_around("abc TOKEN def", 4, 9, 500), "abc TOKEN def");
}

#[test]
fn the_excerpt_is_centred_on_the_match_and_says_what_it_dropped() {
    let text = format!("{}hf_SECRET123{}", "a".repeat(15_000), "b".repeat(6_000));
    let start = 15_000;
    let end = start + "hf_SECRET123".len();
    let e = excerpt_around(&text, start, end, 500);
    assert!(e.contains("hf_SECRET123"), "{e}");
    assert!(e.starts_with("…["), "{e}");
    assert!(e.ends_with(" chars]"), "{e}");
    let kept = e.chars().filter(|c| *c == 'a' || *c == 'b').count();
    assert!((480..=500).contains(&kept), "{kept}");
}

#[test]
fn a_match_near_the_start_spends_the_budget_after_it() {
    let text = format!("xTOKEN{}", "z".repeat(5_000));
    let e = excerpt_around(&text, 1, 6, 500);
    assert!(e.starts_with("xTOKEN"), "{e}");
    assert!(!e.starts_with("…["), "nothing dropped before");
}

#[test]
fn multibyte_text_is_never_split() {
    let text = format!("{}TOKEN{}", "é".repeat(3_000), "ü".repeat(3_000));
    let start = "é".len() * 3_000;
    let e = excerpt_around(&text, start, start + 5, 100);
    assert!(e.contains("TOKEN"));
}

#[test]
fn the_clip_note_points_to_grep() {
    let long = "q".repeat(MAX_LINE_CHARS + 50);
    let out = bound_output(&long);
    assert!(
        out.contains("clipped by blyte; the grep tool shows the text around a match"),
        "{out}"
    );
}

#[test]
fn grep_tool_centres_excerpt_on_match_in_long_line() {
    use blyte_agent::fs::LocalFs;
    use blyte_agent::shell::LocalShell;
    use blyte_agent::tools::execute;
    use serde_json::json;

    let d = tempfile::tempdir().unwrap();
    let text = format!("{}hf_SECRET123{}", "a".repeat(15_000), "b".repeat(6_000));
    std::fs::write(d.path().join("minified.json"), text).unwrap();

    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "grep",
        &json!({
            "pattern": "hf_[A-Z0-9]+",
            "path": "minified.json"
        }),
    );
    assert!(!r.is_error, "{}", r.content);
    assert!(r.content.contains("hf_SECRET123"), "{}", r.content);
    let match_line = r
        .content
        .lines()
        .find(|l| l.contains("hf_SECRET123"))
        .expect("must contain match line");
    assert!(match_line.starts_with("1:"), "{match_line}");
}

#[test]
fn grep_tool_centres_excerpt_in_directory_search() {
    use blyte_agent::fs::LocalFs;
    use blyte_agent::shell::LocalShell;
    use blyte_agent::tools::execute;
    use serde_json::json;

    let d = tempfile::tempdir().unwrap();
    let text = format!("{}hf_SECRET123{}", "a".repeat(15_000), "b".repeat(6_000));
    std::fs::write(d.path().join("minified.json"), text).unwrap();

    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "grep",
        &json!({
            "pattern": "hf_[A-Z0-9]+"
        }),
    );
    assert!(!r.is_error, "{}", r.content);
    assert!(r.content.contains("hf_SECRET123"), "{}", r.content);
    let match_line = r
        .content
        .lines()
        .find(|l| l.contains("hf_SECRET123"))
        .expect("must contain match line");
    assert!(match_line.starts_with("./minified.json:1:"), "{match_line}");
}
