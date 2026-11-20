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

#[test]
fn mock_page_has_map_ids() {
    for id in [
        "id=\"map-tabs\"",
        "id=\"map-repo\"",
        "id=\"map-wide\"",
        "id=\"map-system\"",
        "id=\"map-data\"",
        "id=\"map-data-all\"",
        "id=\"map-detail\"",
        "id=\"map-notes\"",
        "id=\"map-outside\"",
    ] {
        assert_eq!(
            PAGE.matches(id).count(),
            1,
            "expected {id} to appear exactly once"
        );
    }
}

#[test]
fn mock_page_reads_map_contract_fields() {
    for token in [
        "/api/map?repo=",
        "encodeURIComponent",
        "referenced_by",
        "serialisable",
        "unparsed",
        "unresolved",
        "level",
    ] {
        assert!(PAGE.contains(token), "missing map contract field {token}");
    }
}

#[test]
fn mock_page_has_map_copy() {
    for text in [
        "reading the code",
        "files outside the code",
        "files not parsed",
        "references not resolved",
        "Shows references written in the code. Uses through an inferred type are not shown.",
    ] {
        assert!(PAGE.contains(text), "missing map copy {text}");
    }
}

#[test]
fn mock_page_records_map_actions() {
    for action in [
        "map_tab",
        "map_repo",
        "map_group",
        "map_unit",
        "map_structure",
        "map_wide",
    ] {
        assert!(PAGE.contains(action), "missing map action {action}");
    }
}

#[test]
fn mock_page_has_guide_ids() {
    for id in ["guide", "guide-panel"] {
        let needle = format!(r#"id="{id}""#);
        let count = PAGE.matches(&needle).count();
        assert_eq!(count, 1, "expected id={id} once, found {count}");
    }
}

#[test]
fn mock_page_guide_has_ten_steps_in_order() {
    let steps = [
        (
            "replay",
            "The replay",
            "This is a week of your real blyte runs, played back. Play, change the speed, or jump to the next event. Nothing here changes a repository.",
        ),
        (
            "conversation",
            "The conversation",
            "Where you talk with the orchestrator. Results do not interrupt you here: they wait in the queue until you reach a stopping point.",
        ),
        (
            "queue",
            "The review queue",
            "Finished work, grouped by the part of the system it touches. Clean results fold into one line. Only exceptions ask for your attention.",
        ),
        (
            "offer",
            "Now or later",
            "When a result arrives while you are mid-thought, blyte offers it once. Take it now or leave it; it stays in the queue.",
        ),
        (
            "runs",
            "Runs by area",
            "Each box is an area of the code with work in progress or waiting. Click one to focus the queue on it.",
        ),
        (
            "system",
            "The system map",
            "Crates or packages, and the dependencies between them, read from the code. Open one to see its modules. Lit boxes are being changed right now.",
        ),
        (
            "data",
            "The data map",
            "The data structures the code defines, with their fields and the modules that name them.",
        ),
        (
            "limits",
            "Load limits",
            "How much is in progress and how much is waiting for you. Past the limit blyte warns you; it never blocks you.",
        ),
        (
            "checkin",
            "Check-ins",
            "At a natural boundary blyte asks two short questions about how you are doing. Skip them whenever you like.",
        ),
        (
            "record",
            "Your record",
            "What you do in this mock is recorded in your browser only. Save it to tell us what worked.",
        ),
    ];

    let mut cursor = 0;
    for (id, title, text) in steps {
        let id_needle = format!(r#"id: "{id}""#);
        let rel = PAGE[cursor..]
            .find(&id_needle)
            .unwrap_or_else(|| panic!("missing step id {id} in order"));
        cursor += rel + id_needle.len();
        assert!(PAGE.contains(title), "missing step title {title}");
        assert!(PAGE.contains(text), "missing step text for {id}");
    }
}

#[test]
fn mock_page_guide_records_and_remembers() {
    for token in [
        "guide_open",
        "guide_step",
        "guide_close",
        "blyte-mock-guide-seen",
    ] {
        assert!(PAGE.contains(token), "missing guide token {token}");
    }
}
