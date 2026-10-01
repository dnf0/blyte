//! The page has a Plans view fed by /api/plans (2026-10-01).

const PAGE: &str = include_str!("../assets/dashboard/index.html");

#[test]
fn the_page_has_a_plans_view_that_polls_its_own_api() {
    for needle in [
        "/api/plans",
        "#plans",
        "No plans with dispatches in the last 7 days.",
        "Waits on",
        "Unblocks",
    ] {
        assert!(PAGE.contains(needle), "missing {needle}");
    }
    for state in [
        "merged", "verified", "running", "failed", "redone", "ready", "waiting",
    ] {
        assert!(
            PAGE.contains(&format!("{state}:")) || PAGE.contains(&format!("\"{state}\"")),
            "{state}"
        );
    }
    assert!(!PAGE.contains("https://") && !PAGE.contains("http://"));
}
