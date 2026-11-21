fn load_surface_html() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/surface/index.html");
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"))
}

#[test]
fn page_has_its_parts() {
    let html = load_surface_html();
    for id in [
        "conversation",
        "conversation-form",
        "model-select",
        "effort-select",
        "queue",
        "map",
        "report-reader",
        "attached-reports",
        "lost-server",
        "missing-token",
    ] {
        let needle = format!("id=\"{id}\"");
        assert!(
            html.contains(&needle),
            "surface page must contain element with {needle}"
        );
    }
}

#[test]
fn page_calls_every_route() {
    let html = load_surface_html();
    for route in [
        "/api/session",
        "/api/session/events",
        "/api/session/messages",
        "/api/session/approvals/",
        "/api/models",
        "/api/session/model",
        "/api/session/effort",
        "/api/runs",
        "/decision",
        "/diff",
        "/api/map",
    ] {
        assert!(html.contains(route), "surface page must call route {route}");
    }
}

#[test]
fn page_handles_every_entry_kind() {
    let html = load_surface_html();
    for kind in [
        "user_message",
        "turn",
        "tool_started",
        "tool_finished",
        "retry",
        "error",
        "idle",
        "model_changed",
        "effort_changed",
        "decision",
        "approval_requested",
        "approval_answered",
    ] {
        let double_quoted = format!("\"{kind}\"");
        let single_quoted = format!("'{kind}'");
        assert!(
            html.contains(&double_quoted) || html.contains(&single_quoted),
            "surface page must handle entry kind {kind} as a quoted string"
        );
    }
    assert!(
        html.contains("unknown entry kind"),
        "surface page must handle unknown entry kinds explicitly"
    );
}

#[test]
fn mock_only_parts_are_gone() {
    let html = load_surface_html();
    for forbidden in [
        "/api/replay",
        "guide-close",
        "checkin-skip",
        "Save record",
        "Next arrival",
    ] {
        assert!(
            !html.contains(forbidden),
            "surface page must not contain mock-only string {forbidden}"
        );
    }
}

#[test]
fn no_external_resources() {
    let html = load_surface_html();
    let lower = html.to_ascii_lowercase();
    for attr in [
        "src=\"http://",
        "src='http://",
        "src=\"https://",
        "src='https://",
        "href=\"http://",
        "href='http://",
        "href=\"https://",
        "href='https://",
    ] {
        assert!(
            !lower.contains(attr),
            "surface page must not reference external resources via {attr}"
        );
    }
    assert!(
        !lower.contains("@import"),
        "surface page must not use @import"
    );
    for forbidden in ["window.prompt(", "alert(", "confirm("] {
        assert!(
            !html.contains(forbidden),
            "surface page must not call {forbidden}"
        );
    }
}

#[test]
fn no_raw_html_from_the_server() {
    let html = load_surface_html();
    assert!(
        html.contains("function escapeHtml"),
        "surface page must define function escapeHtml"
    );
    let start = html
        .find("function renderTurn(")
        .expect("surface page must define function renderTurn");
    let rest = &html[start..];
    let end = rest[1..]
        .find("\n    function ")
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    let render_turn_body = &rest[..end];
    assert!(
        render_turn_body.contains("renderReportMarkdown("),
        "renderTurn must call renderReportMarkdown("
    );
}

#[test]
fn page_carries_surface_token_handling() {
    let html = load_surface_html();
    assert!(
        html.contains("Cannot send until this page is opened from the URL blyte printed."),
        "missing missing-token banner text"
    );
    assert!(
        html.contains("X-Blyte-Token"),
        "missing surface token request header name"
    );
    assert!(
        html.contains("sessionStorage") && html.contains("replaceState"),
        "missing fragment token persistence and address-bar cleanup"
    );
}

#[test]
fn page_renders_approvals_as_escaped_text_without_approve_all() {
    let html = load_surface_html();
    let start = html
        .find("case \"approval_requested\":")
        .expect("surface page must render approval_requested entries");
    let rest = &html[start..];
    let end = rest
        .find("default:")
        .expect("approval cases must precede default case");
    let body = &rest[..end];
    assert!(
        body.contains("escapeHtml(entry.command)"),
        "approval rendering must escape entry.command"
    );
    assert!(
        body.contains("escapeHtml(entry.reason)"),
        "approval rendering must escape entry.reason"
    );
    assert!(
        body.contains("data-approval-answer=\"yes\"")
            && body.contains("data-approval-answer=\"no\""),
        "approval rendering must provide approve and refuse buttons"
    );
    let lower = body.to_ascii_lowercase();
    assert!(
        !lower.contains("approve all") && !lower.contains("remember"),
        "approval rendering must not offer approve-all or remembered answers"
    );
}
