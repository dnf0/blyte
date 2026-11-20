use blyte_cli::dispatch::summary_error;

#[test]
fn blocked_error_is_cut_to_first_line() {
    assert_eq!(
        summary_error("blocked", Some("gate failed at base\nline two")),
        Some("gate failed at base".to_string())
    );
}

#[test]
fn non_blocked_outcomes_keep_full_error() {
    let msg = "provider failed\ncaused by: timeout";
    assert_eq!(summary_error("error", Some(msg)), Some(msg.to_string()));
    assert_eq!(
        summary_error("gate_failed", Some(msg)),
        Some(msg.to_string())
    );
    assert_eq!(
        summary_error("budget_exhausted", Some(msg)),
        Some(msg.to_string())
    );
}

#[test]
fn none_and_empty_blocked_give_none() {
    assert_eq!(summary_error("error", None), None);
    assert_eq!(summary_error("blocked", None), None);
    assert_eq!(summary_error("blocked", Some("")), None);
}
