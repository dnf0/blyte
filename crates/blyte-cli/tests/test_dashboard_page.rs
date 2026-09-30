//! The dashboard page is one self-contained file that talks only to its own server (2026-09-30).

const PAGE: &str = include_str!("../assets/dashboard/index.html");

#[test]
fn the_page_polls_its_own_api_and_loads_nothing_from_the_network() {
    assert!(PAGE.contains("/api/jobs"));
    assert!(PAGE.contains("<html"));
    assert!(PAGE.contains("What blyte is working on."));
    assert!(PAGE.contains("prefers-color-scheme: dark"));
    for forbidden in [
        "http://",
        "https://",
        "//fonts.",
        "<link rel=\"stylesheet\"",
        "<script src",
    ] {
        assert!(
            !PAGE.contains(forbidden),
            "the page must not load {forbidden}"
        );
    }
}

#[test]
fn every_status_in_the_contract_has_a_label() {
    for s in [
        "running",
        "verified",
        "gate_failed",
        "budget_exhausted",
        "no_changes",
        "error",
        "stopped",
    ] {
        assert!(
            PAGE.contains(&format!("{s}:")) || PAGE.contains(&format!("\"{s}\"")),
            "{s}"
        );
    }
}
