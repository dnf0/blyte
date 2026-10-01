//! A run report carries the effort as its own field (2026-10-01).

#[test]
fn the_effort_field_is_written_and_null_when_unknown() {
    let mut r = blyte_cli::report::RunRecord::default();
    let v = serde_json::to_value(&r).unwrap();
    assert!(v.get("effort").is_some_and(|e| e.is_null()), "{v}");
    r.effort = Some("high".to_string());
    assert_eq!(serde_json::to_value(&r).unwrap()["effort"], "high");
}
