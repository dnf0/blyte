use blyte_core::shunt::{evaluate_and_distill, prune_old_logs, DistilledOutput, ShuntConfig};
use std::fs;
use std::time::Duration;
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
    let result = evaluate_and_distill(
        "echo test",
        dir.path(),
        stdout,
        stderr,
        0,
        Duration::from_millis(100),
        &config,
    )
    .unwrap();

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

    let result = evaluate_and_distill(
        "cargo build",
        dir.path(),
        &stdout,
        stderr,
        0,
        Duration::from_millis(100),
        &config,
    )
    .unwrap();

    match result {
        DistilledOutput::Distilled {
            summary,
            log_path,
            exit_code,
        } => {
            assert_eq!(exit_code, 0);
            assert!(summary.contains("[blyte-shunt] Command succeeded (exit 0)"));
            assert!(summary.contains("lines omitted"));
            assert!(log_path.exists());
            let saved = fs::read_to_string(&log_path).unwrap();
            assert!(saved.contains("Compiling crate_100"));
            assert!(saved.contains("=== Duration:"));
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

    let result = evaluate_and_distill(
        "pytest tests",
        dir.path(),
        &stdout,
        stderr,
        1,
        Duration::from_millis(100),
        &config,
    )
    .unwrap();

    match result {
        DistilledOutput::Distilled {
            summary,
            log_path,
            exit_code,
        } => {
            assert_eq!(exit_code, 1);
            assert!(summary.contains("[blyte-shunt] Command failed with exit code 1"));
            assert!(summary.contains("FAIL: test_json_schema_parsing_error"));
            assert!(summary.contains("AssertionError: expected 200 got 500"));
            assert!(summary.contains("Tests: 1 failed, 79 passed, 80 total"));
            assert!(log_path.exists());
        }
        DistilledOutput::Raw { .. } => panic!("Expected distilled output for failing test"),
    }
}

#[test]
fn test_binary_output_detection_distills_and_writes_raw_bytes() {
    let dir = tempdir().unwrap();
    let config = ShuntConfig {
        threshold_lines: 50,
        threshold_bytes: 4096,
        logs_dir: dir.path().join(".agent-state/logs"),
    };

    // Output contains null bytes indicating binary data
    let stdout = vec![
        0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x00, 0x57, 0x6f, 0x72, 0x6c, 0x64,
    ];
    let stderr = b"";

    let result = evaluate_and_distill(
        "dump_binary",
        dir.path(),
        &stdout,
        stderr,
        0,
        Duration::from_millis(100),
        &config,
    )
    .unwrap();

    match result {
        DistilledOutput::Distilled {
            summary,
            log_path,
            exit_code,
        } => {
            assert_eq!(exit_code, 0);
            assert!(summary
                .contains("[blyte-shunt] Command produced binary output (11 bytes). Full log:"));
            assert!(log_path.exists());
            let raw_saved = fs::read(&log_path).unwrap();
            assert!(raw_saved.contains(&0x00));
        }
        DistilledOutput::Raw { .. } => panic!("Expected distilled output for binary data"),
    }
}

#[test]
fn test_fifo_log_pruning_by_file_count() {
    let dir = tempdir().unwrap();
    let logs_dir = dir.path().join("logs");
    fs::create_dir_all(&logs_dir).unwrap();

    // Create 5 log files with ascending timestamps
    let mut paths = Vec::new();
    for i in 1..=5 {
        let path = logs_dir.join(format!("cmd_{}.log", i));
        fs::write(&path, format!("log content {}", i)).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        paths.push(path);
    }

    // Create a non-.log file (e.g. .gitignore) which must never be pruned
    let gitignore = logs_dir.join(".gitignore");
    fs::write(&gitignore, "*\n").unwrap();

    // Prune down to 3 files
    prune_old_logs(&logs_dir, 3, 10_000_000);

    // Oldest 2 files must be pruned, newest 3 retained, and .gitignore preserved
    assert!(!paths[0].exists(), "oldest log cmd_1.log should be pruned");
    assert!(
        !paths[1].exists(),
        "second oldest cmd_2.log should be pruned"
    );
    assert!(paths[2].exists(), "cmd_3.log should be retained");
    assert!(paths[3].exists(), "cmd_4.log should be retained");
    assert!(paths[4].exists(), "cmd_5.log should be retained");
    assert!(gitignore.exists(), ".gitignore must never be pruned");
}

#[test]
fn test_fifo_log_pruning_by_byte_size() {
    let dir = tempdir().unwrap();
    let logs_dir = dir.path().join("logs");
    fs::create_dir_all(&logs_dir).unwrap();

    // Create 3 log files, each 100 bytes
    let mut paths = Vec::new();
    for i in 1..=3 {
        let path = logs_dir.join(format!("cmd_{}.log", i));
        fs::write(&path, vec![b'x'; 100]).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        paths.push(path);
    }

    // Prune with max_bytes = 150 (only 1 file of 100 bytes should remain)
    prune_old_logs(&logs_dir, 10, 150);

    assert!(
        !paths[0].exists(),
        "oldest log should be pruned due to byte limit"
    );
    assert!(
        !paths[1].exists(),
        "second oldest should be pruned due to byte limit"
    );
    assert!(
        paths[2].exists(),
        "newest log should remain within byte limit"
    );
}
