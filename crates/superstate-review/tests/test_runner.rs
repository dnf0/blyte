use std::path::{Path, PathBuf};
use superstate_review::runner::{
    clean_json_response, execute_review, parse_review_report, resolve_review_driver,
    resolve_review_driver_with_env, ReviewConfig, ReviewError,
};
use superstate_review::schema::{ReviewFinding, ReviewReport, Severity};

#[test]
fn test_clean_json_response() {
    assert_eq!(
        clean_json_response("{\"summary\": \"ok\"}"),
        "{\"summary\": \"ok\"}"
    );
    assert_eq!(
        clean_json_response("```json\n{\"summary\": \"ok\"}\n```"),
        "{\"summary\": \"ok\"}"
    );
    assert_eq!(
        clean_json_response("```\n{\"summary\": \"ok\"}\n```"),
        "{\"summary\": \"ok\"}"
    );
    assert_eq!(
        clean_json_response("Preamble\n```json\n{\"summary\": \"ok\"}\n```\nPostamble"),
        "{\"summary\": \"ok\"}"
    );
}

#[test]
fn test_parse_review_report_dict() {
    let raw = r#"{
        "summary": "Potential memory leak",
        "findings": [
            {
                "severity": "High",
                "file": "src/main.rs",
                "line_range": [10, 20],
                "problem": "Unbounded buffer",
                "suggested_fix": "Add capacity limit"
            }
        ],
        "has_blocking_issues": false
    }"#;
    let report = parse_review_report(raw).expect("Failed to parse report");
    assert_eq!(report.summary, "Potential memory leak");
    assert_eq!(report.findings.len(), 1);
    assert!(
        report.has_blocking_issues,
        "High severity should promote has_blocking_issues"
    );
}

#[test]
fn test_parse_review_report_list() {
    let raw = r#"[
        {
            "severity": "Critical",
            "file": "src/lib.rs",
            "line_range": [5, 6],
            "problem": "Buffer overflow",
            "suggested_fix": "Bounds check"
        }
    ]"#;
    let report = parse_review_report(raw).expect("Failed to parse report list");
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].severity, Severity::Critical);
    assert!(report.has_blocking_issues);
    assert_eq!(report.summary, "Review generated 1 findings");
}

#[test]
fn test_parse_review_report_clean() {
    let raw = r#"{
        "summary": "All checks passed cleanly.",
        "findings": [],
        "has_blocking_issues": false
    }"#;
    let report = parse_review_report(raw).expect("Failed to parse clean report");
    assert_eq!(report.summary, "All checks passed cleanly.");
    assert!(report.findings.is_empty());
    assert!(!report.has_blocking_issues);
}

#[test]
fn test_parse_review_report_invalid_json() {
    let raw = "not a valid json string";
    let err = parse_review_report(raw).unwrap_err();
    match err {
        ReviewError::Json(_) => {}
        other => panic!("Expected ReviewError::Json, got {:?}", other),
    }
}

#[test]
fn test_resolve_review_driver_configured() {
    let claude = resolve_review_driver(Some("claude")).unwrap();
    assert_eq!(claude, "claude -p --dangerously-skip-permissions");

    let agy = resolve_review_driver(Some("agy")).unwrap();
    assert_eq!(agy, "agy -p --dangerously-skip-permissions");

    let custom = resolve_review_driver(Some("my-custom-reviewer --flag")).unwrap();
    assert_eq!(custom, "my-custom-reviewer --flag");
}

#[test]
fn test_resolve_review_driver_from_env() {
    let driver = resolve_review_driver_with_env(
        None,
        |key| {
            if key == "SUPERSTATE_REVIEW_DRIVER" {
                Some("custom-review-tool".to_string())
            } else {
                None
            }
        },
        |_| false,
    )
    .unwrap();
    assert_eq!(driver, "custom-review-tool");

    let driver2 = resolve_review_driver_with_env(
        None,
        |key| {
            if key == "SUPERSTATE_DRIVER" {
                Some("claude".to_string())
            } else {
                None
            }
        },
        |_| false,
    )
    .unwrap();
    assert_eq!(driver2, "claude -p --dangerously-skip-permissions");
}

#[test]
fn test_resolve_review_driver_from_path() {
    // agy found in PATH
    let driver_agy = resolve_review_driver_with_env(None, |_| None, |bin| bin == "agy").unwrap();
    assert_eq!(
        driver_agy,
        "agy --model gemini-3.8-flash-low --print --dangerously-skip-permissions"
    );

    // claude found in PATH when agy is not
    let driver_claude =
        resolve_review_driver_with_env(None, |_| None, |bin| bin == "claude").unwrap();
    assert_eq!(driver_claude, "claude -p --dangerously-skip-permissions");

    // neither found
    let err = resolve_review_driver_with_env(None, |_| None, |_| false).unwrap_err();
    match err {
        ReviewError::DriverNotFound => {}
        other => panic!("Expected DriverNotFound, got {:?}", other),
    }
}

#[test]
fn test_review_report_display() {
    let report = ReviewReport {
        summary: "Code review found 1 security issue".to_string(),
        findings: vec![ReviewFinding {
            severity: Severity::High,
            file: PathBuf::from("src/auth.rs"),
            line_range: (42, 50),
            problem: "Missing authentication check".to_string(),
            suggested_fix: "Validate session token".to_string(),
        }],
        has_blocking_issues: true,
    };

    let display_str = report.to_string();
    assert!(display_str.contains("Code review found 1 security issue"));
    assert!(display_str.contains("High"));
    assert!(display_str.contains("src/auth.rs:42-50"));
    assert!(display_str.contains("Missing authentication check"));
    assert!(display_str.contains("Validate session token"));
    assert!(display_str.contains("Blocking"));
}

#[test]
fn test_execute_review_empty_diff() {
    let config = ReviewConfig {
        driver: Some("nonexistent-driver".to_string()),
        timeout_secs: 10,
    };
    let report = execute_review(Path::new("."), "", None, &config).unwrap();
    assert_eq!(report.summary, "No changes detected in diff.");
    assert!(!report.has_blocking_issues);
    assert!(report.findings.is_empty());

    let report_whitespace = execute_review(Path::new("."), "   \n\t  ", None, &config).unwrap();
    assert_eq!(report_whitespace.summary, "No changes detected in diff.");
}

#[test]
fn test_execute_review_with_mock_driver() {
    let mock_json = r#"{"summary":"All good","findings":[],"has_blocking_issues":false}"#;
    let config = ReviewConfig {
        driver: Some(format!("echo '{}'", mock_json)),
        timeout_secs: 10,
    };
    let diff = "diff --git a/test.rs b/test.rs\n+new line\n";
    let report = execute_review(Path::new("."), diff, Some("Check logic"), &config).unwrap();
    assert_eq!(report.summary, "All good");
    assert!(!report.has_blocking_issues);
}

#[test]
fn test_execute_review_failing_driver() {
    let config = ReviewConfig {
        driver: Some("exit 1".to_string()),
        timeout_secs: 10,
    };
    let diff = "diff --git a/test.rs b/test.rs\n+new line\n";
    let err = execute_review(Path::new("."), diff, None, &config).unwrap_err();
    match err {
        ReviewError::Driver(_) => {}
        other => panic!("Expected ReviewError::Driver, got {:?}", other),
    }
}
