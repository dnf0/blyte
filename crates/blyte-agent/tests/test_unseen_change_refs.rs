use std::collections::HashSet;

use blyte_agent::research_check::{identifiers, unseen_change_refs, unseen_names};

fn seen_from(text: &str) -> HashSet<String> {
    identifiers(text).map(str::to_owned).collect()
}

#[test]
fn an_invented_pr_number_is_flagged() {
    let p = unseen_change_refs("This was fixed in #512.", &seen_from(""));
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("#512"), "{}", p[0]);
}

#[test]
fn an_invented_pr_word_form_is_flagged() {
    let p = unseen_change_refs("PR 7 changed the parser.", &seen_from(""));
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("#7"), "{}", p[0]);
}

#[test]
fn a_number_seen_in_a_commit_subject_is_clean() {
    let seen = seen_from("fix(clean): keep runs (#512)");
    let p = unseen_change_refs("This was fixed in #512.", &seen);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_number_seen_in_a_url_is_clean() {
    let seen = seen_from("https://github.com/o/r/pull/512");
    let p = unseen_change_refs("See pull request 512.", &seen);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_number_seen_in_gh_list_output_is_clean() {
    let seen = seen_from("341\tchore(release): v0.40.0\tmain\tMERGED");
    let p = unseen_change_refs("PR #341 is the release.", &seen);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_number_seen_in_gh_json_is_clean() {
    let seen = seen_from(r#"{"number": 88, "title": "x"}"#);
    let p = unseen_change_refs("Issue 88 tracks it.", &seen);
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_line_number_is_not_a_seen_reference() {
    let seen = seen_from("120: let x = 1;\nsrc/a.rs:120");
    let p = unseen_change_refs("This was fixed in #120.", &seen);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("#120"), "{}", p[0]);
}

#[test]
fn a_rank_is_not_a_reference() {
    let p = unseen_change_refs("The #1 cause is the cache.", &seen_from(""));
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_number_in_code_is_clean() {
    let p = unseen_change_refs("Set the colour to `#102030`.", &seen_from(""));
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_number_in_a_fenced_block_is_clean() {
    let p = unseen_change_refs("```text\n#512\n```\n", &seen_from(""));
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn a_number_is_reported_once() {
    let p = unseen_change_refs("First in #512.\nAgain in #512.\n", &seen_from(""));
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].starts_with("report.md:1:"), "{}", p[0]);
    assert!(p[0].contains("#512"), "{}", p[0]);
}

#[test]
fn unseen_names_includes_the_references() {
    let p = unseen_names("This was fixed in #512.", &seen_from(""));
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("#512"), "{}", p[0]);
}
