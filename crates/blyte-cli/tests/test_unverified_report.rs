use blyte_cli::provenance::{mark_if_unverified, unverified_banner};

#[test]
fn the_banner_names_the_outcome_and_the_turns() {
    assert_eq!(
        unverified_banner("budget_exhausted", Some(300)),
        "> **Not verified.** This research run ended as `budget_exhausted` after 300 turns. The report below did not pass blyte's checks and may be an unfinished draft: check it before relying on it.\n\n"
    );
}

#[test]
fn the_banner_without_a_turn_count() {
    assert_eq!(
        unverified_banner("error", None),
        "> **Not verified.** This research run ended as `error`. The report below did not pass blyte's checks and may be an unfinished draft: check it before relying on it.\n\n"
    );
}

#[test]
fn an_unverified_report_gets_the_banner_first() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("report.md");
    let orig = "# Stub\n\nbody\n";
    std::fs::write(&p, orig).unwrap();

    mark_if_unverified(&p, "budget_exhausted", Some(300)).unwrap();

    let expected = format!("{}{orig}", unverified_banner("budget_exhausted", Some(300)));
    assert_eq!(std::fs::read_to_string(&p).unwrap(), expected);
}

#[test]
fn a_verified_report_is_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("report.md");
    let orig = "# Stub\n\nbody\n";
    std::fs::write(&p, orig).unwrap();

    mark_if_unverified(&p, "verified", Some(300)).unwrap();

    assert_eq!(std::fs::read_to_string(&p).unwrap(), orig);
}

#[test]
fn marking_twice_adds_one_banner() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("report.md");
    let orig = "# Stub\n\nbody\n";
    std::fs::write(&p, orig).unwrap();

    mark_if_unverified(&p, "budget_exhausted", Some(300)).unwrap();
    let after_one = std::fs::read(&p).unwrap();

    mark_if_unverified(&p, "budget_exhausted", Some(300)).unwrap();
    let after_two = std::fs::read(&p).unwrap();

    assert_eq!(after_two, after_one);
}

#[test]
fn a_missing_report_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("missing.md");

    let res = mark_if_unverified(&p, "budget_exhausted", Some(300));
    assert!(res.is_err());
    assert!(!p.exists());
}
