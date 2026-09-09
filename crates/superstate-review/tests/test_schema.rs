use superstate_review::schema::{ReviewReport, Severity};

#[test]
fn test_parse_structured_findings_json() {
    let raw = r#"{
        "summary": "Found memory leak",
        "findings": [
            {
                "severity": "High",
                "file": "src/lib.rs",
                "line_range": [10, 20],
                "problem": "Unbounded collection growth",
                "suggested_fix": "Use bounded LRU cache"
            }
        ],
        "has_blocking_issues": true
    }"#;
    let report: ReviewReport = serde_json::from_str(raw).unwrap();
    assert!(report.has_blocking_issues);
    assert_eq!(report.findings[0].severity, Severity::High);
    assert_eq!(report.findings[0].file.to_str().unwrap(), "src/lib.rs");
    assert_eq!(report.findings[0].line_range, (10, 20));
}

#[test]
fn test_clean_review_has_no_blocking_issues() {
    let report = ReviewReport::clean("All checks passed cleanly.");
    assert!(!report.has_blocking_issues);
    assert!(report.findings.is_empty());
}
