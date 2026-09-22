//! The read cache must never cost the worker work it cannot recover from.

use std::path::Path;

use superstate_core::read_cache::{
    content_hash, evaluate, suppression_reason, CacheVerdict, ReadRecord,
};

fn rec(path: &str, start: usize, end: usize, hash: u64) -> ReadRecord {
    ReadRecord { path: path.into(), start, end, hash, suppressed: false }
}

fn suppressed(path: &str, start: usize, end: usize, hash: u64) -> ReadRecord {
    ReadRecord { path: path.into(), start, end, hash, suppressed: true }
}

#[test]
fn first_read_of_a_file_is_never_suppressed() {
    let v = evaluate(&[], Path::new("a.rs"), 1, 100, 7);
    assert_eq!(v, CacheVerdict::Miss, "nothing is held yet; the read must go through");
}

#[test]
fn an_exactly_repeated_window_is_suppressed() {
    let records = vec![rec("a.rs", 1, 100, 7)];
    match evaluate(&records, Path::new("a.rs"), 1, 100, 7) {
        CacheVerdict::AlreadyHeld { covered_by } => assert_eq!(covered_by, vec![(1, 100)]),
        other => panic!("expected AlreadyHeld, got {other:?}"),
    }
}

#[test]
fn a_window_inside_one_already_held_is_suppressed() {
    let records = vec![rec("a.rs", 1, 200, 7)];
    assert!(matches!(
        evaluate(&records, Path::new("a.rs"), 50, 150, 7),
        CacheVerdict::AlreadyHeld { .. }
    ));
}

#[test]
fn coverage_spans_the_union_of_adjacent_windows() {
    // A worker that read 1-100 then 101-200 holds 50-150, though no single call fetched it.
    // Testing each window separately would miss the majority of real repeats, which is how
    // 13.18 concluded from one trace that repeats did not happen.
    let records = vec![rec("a.rs", 1, 100, 7), rec("a.rs", 101, 200, 7)];
    assert!(matches!(
        evaluate(&records, Path::new("a.rs"), 50, 150, 7),
        CacheVerdict::AlreadyHeld { .. }
    ));
}

#[test]
fn a_gap_in_the_union_is_not_coverage() {
    let records = vec![rec("a.rs", 1, 100, 7), rec("a.rs", 150, 200, 7)];
    assert_eq!(
        evaluate(&records, Path::new("a.rs"), 50, 160, 7),
        CacheVerdict::Miss,
        "lines 101-149 were never served, so the read must proceed"
    );
}

#[test]
fn partial_overlap_still_goes_through() {
    let records = vec![rec("a.rs", 1, 100, 7)];
    assert_eq!(
        evaluate(&records, Path::new("a.rs"), 80, 180, 7),
        CacheVerdict::Miss,
        "a read that needs ANY new line must be served in full"
    );
}

#[test]
fn editing_the_file_invalidates_every_record_for_it() {
    // The single most important safety property: a read-after-write is never suppressed,
    // whether the write was the worker's own edit or a scoped rollback.
    let records = vec![rec("a.rs", 1, 100, 7)];
    assert_eq!(
        evaluate(&records, Path::new("a.rs"), 1, 100, 9),
        CacheVerdict::Miss,
        "the content hash changed, so what the worker holds is stale"
    );
}

#[test]
fn records_for_another_file_do_not_cover_this_one() {
    let records = vec![rec("b.rs", 1, 100, 7)];
    assert_eq!(evaluate(&records, Path::new("a.rs"), 1, 100, 7), CacheVerdict::Miss);
}

#[test]
fn insisting_on_a_suppressed_window_serves_it() {
    // The escape hatch. A worker whose context was compacted genuinely no longer holds what
    // the cache believes it holds; refusing forever would deadlock it. Worst case is one
    // wasted denial.
    let records = vec![rec("a.rs", 1, 100, 7), suppressed("a.rs", 1, 100, 7)];
    assert_eq!(
        evaluate(&records, Path::new("a.rs"), 1, 100, 7),
        CacheVerdict::Insisted,
        "the identical window asked for twice in a row must be served"
    );
}

#[test]
fn insisting_requires_the_same_window_not_merely_any_suppression() {
    let records = vec![rec("a.rs", 1, 200, 7), suppressed("a.rs", 1, 100, 7)];
    // A DIFFERENT window after a suppression is an ordinary request, still covered.
    assert!(matches!(
        evaluate(&records, Path::new("a.rs"), 150, 180, 7),
        CacheVerdict::AlreadyHeld { .. }
    ));
}

#[test]
fn content_hash_is_stable_and_discriminating() {
    assert_eq!(content_hash("fn main() {}"), content_hash("fn main() {}"));
    assert_ne!(content_hash("fn main() {}"), content_hash("fn main() { }"));
    assert_ne!(content_hash(""), content_hash("a"));
}

#[test]
fn the_denial_is_short_enough_to_be_worth_sending() {
    // The entire mechanism rests on the refusal costing less than the read it replaces. A
    // read carries ~2,500 tokens of preamble alone (13.17); this must stay far below it.
    let reason = suppression_reason(Path::new("src/very/long/path/formatters.go"), 120, 180);
    assert!(
        reason.len() < 500,
        "suppression text must stay far cheaper than a tool call, got {} bytes",
        reason.len()
    );
    assert!(reason.contains("120-180"), "must say which lines, so it can be acted on");
    assert!(
        reason.contains("identical read once more"),
        "must state the escape hatch: a worker that lost context has to know it can insist"
    );
}

#[test]
fn an_inverted_window_is_never_treated_as_covered() {
    let records = vec![rec("a.rs", 1, 100, 7)];
    assert_eq!(evaluate(&records, Path::new("a.rs"), 60, 40, 7), CacheVerdict::Miss);
}
