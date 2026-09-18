use std::fs;
use superstate_core::shunt::{evaluate_and_distill, DistilledOutput, ShuntConfig};
use tempfile::tempdir;

#[test]
fn test_small_output_passes_through_raw() {
    let dir = tempdir().unwrap();
    let config = ShuntConfig {
        threshold_lines: 50,
        threshold_bytes: 4096,
        logs_dir: dir.path().join(".agent-state/logs"),
    };

    let stdout = b"short output\nline 2\n";
    let stderr = b"";
    let result = evaluate_and_distill("echo test", dir.path(), stdout, stderr, 0, &config).unwrap();

    match result {
        DistilledOutput::Raw {
            stdout: out,
            stderr: err,
        } => {
            assert_eq!(out, stdout);
            assert_eq!(err, stderr);
        }
        DistilledOutput::Distilled { .. } => panic!("Expected raw output for short command"),
    }
}

#[test]
fn test_large_successful_output_distills_to_one_line() {
    let dir = tempdir().unwrap();
    let config = ShuntConfig {
        threshold_lines: 50,
        threshold_bytes: 4096,
        logs_dir: dir.path().join(".agent-state/logs"),
    };

    let lines: Vec<String> = (1..=100)
        .map(|i| format!("Compiling crate_{} v0.1.0", i))
        .collect();
    let stdout = lines.join("\n").into_bytes();
    let stderr = b"";

    let result =
        evaluate_and_distill("cargo build", dir.path(), &stdout, stderr, 0, &config).unwrap();

    match result {
        DistilledOutput::Distilled {
            summary,
            log_path,
            exit_code,
        } => {
            assert_eq!(exit_code, 0);
            assert!(summary.contains("[superstate-shunt] Command succeeded (exit 0)"));
            assert!(summary.contains("lines omitted"));
            assert!(log_path.exists());
            let saved = fs::read_to_string(&log_path).unwrap();
            assert!(saved.contains("Compiling crate_100"));
        }
        DistilledOutput::Raw { .. } => panic!("Expected distilled output for >50 lines"),
    }
}

#[test]
fn test_large_failing_output_extracts_failure_excerpt() {
    let dir = tempdir().unwrap();
    let config = ShuntConfig {
        threshold_lines: 50,
        threshold_bytes: 4096,
        logs_dir: dir.path().join(".agent-state/logs"),
    };

    let mut lines: Vec<String> = (1..=60)
        .map(|i| format!("test suite {} passing", i))
        .collect();
    lines.push("FAIL: test_json_schema_parsing_error".to_string());
    lines.push("AssertionError: expected 200 got 500".to_string());
    lines.extend((61..=80).map(|i| format!("test suite {} passing", i)));
    lines.push("Tests: 1 failed, 79 passed, 80 total".to_string());
    let stdout = lines.join("\n").into_bytes();
    let stderr = b"";

    let result =
        evaluate_and_distill("pytest tests", dir.path(), &stdout, stderr, 1, &config).unwrap();

    match result {
        DistilledOutput::Distilled {
            summary,
            log_path,
            exit_code,
        } => {
            assert_eq!(exit_code, 1);
            assert!(summary.contains("[superstate-shunt] Command failed with exit code 1"));
            assert!(summary.contains("FAIL: test_json_schema_parsing_error"));
            assert!(summary.contains("AssertionError: expected 200 got 500"));
            assert!(summary.contains("Tests: 1 failed, 79 passed, 80 total"));
            assert!(log_path.exists());
        }
        DistilledOutput::Raw { .. } => panic!("Expected distilled output for failing test"),
    }
}
