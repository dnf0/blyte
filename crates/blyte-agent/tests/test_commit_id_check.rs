use std::collections::HashSet;

use blyte_agent::research_check::{identifiers, unseen_names};

fn seen_from(text: &str) -> HashSet<String> {
    identifiers(text).map(str::to_owned).collect()
}

#[test]
fn abbreviation_of_a_seen_commit_id_is_not_flagged() {
    let seen = seen_from("commit 76dc4041aa22bb33cc44dd55ee66ff7700112233");
    assert!(unseen_names("Fixed in 76dc404.\n", &seen).is_empty());
}

#[test]
fn full_id_citing_a_seen_abbreviation_is_not_flagged() {
    let seen = seen_from("76dc404 chore");
    let report = "Fixed in 76dc4041aa22bb33cc44dd55ee66ff7700112233.\n";
    assert!(unseen_names(report, &seen).is_empty());
}

#[test]
fn unseen_commit_id_is_flagged_once_with_its_line() {
    let problems = unseen_names("Fixed in 1a2b3c4d.\n", &HashSet::new());
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].contains("1a2b3c4d"), "{}", problems[0]);
    assert!(problems[0].contains("report.md:1"), "{}", problems[0]);
}

#[test]
fn same_unseen_commit_id_twice_is_one_problem() {
    let report = "Fixed in 1a2b3c4d and again 1a2b3c4d.\n";
    let problems = unseen_names(report, &HashSet::new());
    assert_eq!(problems.len(), 1, "{problems:?}");
}

#[test]
fn commit_id_only_inside_a_fenced_block_is_not_flagged() {
    let report = "Example:\n```\nFixed in 1a2b3c4d.\n```\n";
    assert!(unseen_names(report, &HashSet::new()).is_empty());
}

#[test]
fn digits_only_letters_only_and_short_tokens_are_not_commit_ids() {
    let report = "Dated 20261118, hash deadbeef, prefix abc123.\n";
    assert!(unseen_names(report, &HashSet::new()).is_empty());
}

#[test]
fn identifiers_yield_commit_like_tokens_that_start_with_a_digit() {
    let found: Vec<&str> = identifiers("x 1a2b3c4 foo_bar 12").collect();
    assert!(found.contains(&"1a2b3c4"), "{found:?}");
    assert!(found.contains(&"foo_bar"), "{found:?}");
    assert!(!found.contains(&"12"), "{found:?}");
}
