use blyte_review::runner::{
    build_review_prompt, cap_diff, clean_json_response, execute_review, parse_review_report,
    resolve_review_driver, resolve_review_driver_with_env, ReviewConfig, ReviewError,
    DIFF_TRUNCATION_NOTICE, MAX_DIFF_CHARS,
};
use blyte_review::schema::{ReviewFinding, ReviewReport, Severity};
use std::path::{Path, PathBuf};

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
            if key == "BLYTE_REVIEW_DRIVER" {
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
            if key == "BLYTE_DRIVER" {
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

#[test]
fn test_parse_review_report_case_insensitive_severity() {
    let raw = r#"[
        {
            "severity": "critical",
            "file": "src/crit.rs",
            "line_range": [1, 2],
            "problem": "Critical flaw",
            "suggested_fix": "Fix it"
        },
        {
            "severity": "high",
            "file": "src/high.rs",
            "line_range": [3, 4],
            "problem": "High risk",
            "suggested_fix": "Fix high"
        },
        {
            "severity": "MEDIUM",
            "file": "src/med.rs",
            "line_range": [5, 6],
            "problem": "Medium issue",
            "suggested_fix": "Fix med"
        },
        {
            "severity": "low",
            "file": "src/low.rs",
            "line_range": [7, 8],
            "problem": "Low issue",
            "suggested_fix": "Fix low"
        }
    ]"#;
    let report = parse_review_report(raw).expect("Failed to parse report with case variations");
    assert_eq!(report.findings.len(), 4);
    assert_eq!(report.findings[0].severity, Severity::Critical);
    assert_eq!(report.findings[1].severity, Severity::High);
    assert_eq!(report.findings[2].severity, Severity::Medium);
    assert_eq!(report.findings[3].severity, Severity::Low);
    assert!(report.has_blocking_issues);
}

#[test]
fn test_diff_truncation_logic() {
    let large_diff = "a".repeat(130_000);
    let capped = cap_diff(&large_diff);
    assert!(capped.contains("[Diff truncated: exceeded 120,000 character limit]"));
    assert_eq!(
        capped.chars().count(),
        MAX_DIFF_CHARS + DIFF_TRUNCATION_NOTICE.chars().count()
    );
    assert_eq!(&capped[..MAX_DIFF_CHARS], &large_diff[..MAX_DIFF_CHARS]);

    let prompt = build_review_prompt(&large_diff, None);
    assert!(prompt.contains("```diff\n"));
    assert!(prompt.contains("[Diff truncated: exceeded 120,000 character limit]\n```\n"));

    let small_diff = "diff --git a/foo.rs b/foo.rs\n+bar\n";
    let small_capped = cap_diff(small_diff);
    assert_eq!(small_capped, small_diff);
    let small_prompt = build_review_prompt(small_diff, None);
    assert!(small_prompt.contains("```diff\ndiff --git a/foo.rs b/foo.rs\n+bar\n```\n"));
}

#[test]
fn test_execute_review_diff_truncation() {
    let large_diff = "a".repeat(130_000);
    // Mock driver that asserts:
    // 1. BLYTE_PROMPT is NOT in env because prompt > 32_768 chars (ARG_MAX safety)
    // 2. stdin contains the diff truncation notice
    let mock_driver = r#"if [ -n "$BLYTE_PROMPT" ]; then exit 1; fi; grep -q "exceeded 120,000 character limit" && echo '{"summary":"Truncated diff verified","findings":[],"has_blocking_issues":false}'"#;
    let config = ReviewConfig {
        driver: Some(mock_driver.to_string()),
        timeout_secs: 10,
    };
    let report = execute_review(Path::new("."), &large_diff, None, &config).unwrap();
    assert_eq!(report.summary, "Truncated diff verified");
}

#[test]
fn test_execute_review_driver_timeout() {
    let config = ReviewConfig {
        driver: Some("sleep 5".to_string()),
        timeout_secs: 1,
    };
    let diff = "diff --git a/test.rs b/test.rs\n+new line\n";
    let start = std::time::Instant::now();
    let err = execute_review(Path::new("."), diff, None, &config).unwrap_err();
    let elapsed = start.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "Expected timeout around 1s, but took {:?}",
        elapsed
    );
    match err {
        ReviewError::Driver(msg) => {
            assert_eq!(msg, "Driver timed out after 1 seconds");
        }
        other => panic!(
            "Expected ReviewError::Driver timeout error, got {:?}",
            other
        ),
    }
}

#[test]
fn test_parse_review_report_driver_cli_envelope() {
    let raw = r#"{
        "conversation_id": "test-uuid",
        "status": "SUCCESS",
        "response": "```json\n{\n  \"summary\": \"Unchecked array index\",\n  \"findings\": [\n    {\n      \"severity\": \"Critical\",\n      \"file\": \"src/parse.rs\",\n      \"line_range\": [12, 15],\n      \"problem\": \"Out of bounds access\",\n      \"suggested_fix\": \"Add bounds check\"\n    }\n  ],\n  \"has_blocking_issues\": true\n}\n```\n",
        "duration_seconds": 2.5
    }"#;
    let report = parse_review_report(raw).expect("Failed to parse driver envelope");
    assert_eq!(report.summary, "Unchecked array index");
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].severity, Severity::Critical);
    assert!(report.has_blocking_issues);
}
