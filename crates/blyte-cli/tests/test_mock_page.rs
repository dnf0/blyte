const PAGE: &str = include_str!("../assets/mock/index.html");

#[test]
fn mock_page_is_self_contained_and_themed() {
    assert!(PAGE.contains("/api/replay"));
    assert!(PAGE.contains("<html"));
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
            "unexpected external reference: {forbidden}"
        );
    }
}

#[test]
fn mock_page_has_all_required_ids() {
    for id in [
        "id=\"bar\"",
        "id=\"play\"",
        "id=\"speed\"",
        "id=\"next\"",
        "id=\"clock\"",
        "id=\"wip\"",
        "id=\"waiting\"",
        "id=\"session\"",
        "id=\"skipped\"",
        "id=\"mode\"",
        "id=\"conversation\"",
        "id=\"queue\"",
        "id=\"map\"",
        "id=\"checkin\"",
        "id=\"offer\"",
        "id=\"save\"",
    ] {
        assert!(PAGE.contains(id), "missing required element {id}");
    }
}

#[test]
fn mock_page_defines_limit_constants() {
    assert!(PAGE.contains("const WIP_CAP = 3"));
    assert!(PAGE.contains("const QUEUE_BOUND = 7"));
}

#[test]
fn mock_page_reads_contract_fields() {
    for field in [
        "started_at",
        "finished_at",
        "card",
        "group",
        "checks_recorded",
        "flags",
        "final_message",
        "changed_files",
        "verdict",
        "skipped",
    ] {
        assert!(PAGE.contains(field), "missing contract field {field}");
    }
}

#[test]
fn mock_page_contains_required_copy_and_log_key() {
    for token in [
        "blyte-mock-log",
        "checks not recorded",
        "recorded then:",
        "Accept all clean",
        "Send back",
        "Hold",
        "Redo",
        "Now",
        "Later",
    ] {
        assert!(PAGE.contains(token), "missing required text {token}");
    }
}

#[test]
fn mock_page_contains_no_placeholder_text() {
    let lower = PAGE.to_lowercase();
    assert!(!lower.contains("sample"));
    assert!(!lower.contains("lorem"));
}
