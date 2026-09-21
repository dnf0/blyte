use std::fs;
use std::io::Write;
use superstate_core::guard::{evaluate_file_read_guard, FileReadGuardConfig, GuardDecision};
use tempfile::tempdir;

#[test]
fn test_config_default_threshold() {
    let config = FileReadGuardConfig::default();
    assert_eq!(config.threshold_lines, 200);
}

#[test]
fn test_serde_guard_decision() {
    let allow = GuardDecision::Allow;
    let allow_json = serde_json::to_string(&allow).unwrap();
    assert_eq!(allow_json, r#"{"decision":"allow"}"#);
    let deserialized_allow: GuardDecision = serde_json::from_str(&allow_json).unwrap();
    assert_eq!(deserialized_allow, allow);

    let deny = GuardDecision::Deny {
        reason: "file too large".to_string(),
    };
    let deny_json = serde_json::to_string(&deny).unwrap();
    assert_eq!(deny_json, r#"{"decision":"deny","reason":"file too large"}"#);
    let deserialized_deny: GuardDecision = serde_json::from_str(&deny_json).unwrap();
    assert_eq!(deserialized_deny, deny);
}

#[test]
fn test_non_existent_file_allows() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("does_not_exist.py");
    let config = FileReadGuardConfig::default();

    let decision = evaluate_file_read_guard(&file_path, None, None, &config);
    assert_eq!(decision, GuardDecision::Allow);
}

#[test]
fn test_directory_allows() {
    let dir = tempdir().unwrap();
    let config = FileReadGuardConfig::default();

    let decision = evaluate_file_read_guard(dir.path(), None, None, &config);
    assert_eq!(decision, GuardDecision::Allow);
}

#[test]
fn test_small_file_allows() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("small.rs");
    let content = "fn main() {\n    println!(\"hello\");\n}\n";
    fs::write(&file_path, content).unwrap();

    let config = FileReadGuardConfig::default();
    let decision = evaluate_file_read_guard(&file_path, None, None, &config);
    assert_eq!(decision, GuardDecision::Allow);
}

#[test]
fn test_file_exactly_at_threshold_allows() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("threshold.txt");
    let content = (1..=200)
        .map(|i| format!("line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&file_path, content).unwrap();

    let config = FileReadGuardConfig {
        threshold_lines: 200,
    };
    let decision = evaluate_file_read_guard(&file_path, None, None, &config);
    assert_eq!(decision, GuardDecision::Allow);
}

#[test]
fn test_bounded_slice_under_threshold_allows() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("large.rs");
    // 300 lines file
    let content = (1..=300)
        .map(|i| format!("// line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&file_path, content).unwrap();

    let config = FileReadGuardConfig {
        threshold_lines: 200,
    };
    // Request 50..=240 -> 190 lines (<= 200)
    let decision = evaluate_file_read_guard(&file_path, Some(50), Some(240), &config);
    assert_eq!(decision, GuardDecision::Allow);

    // Request 1..=201 -> 200 lines (<= 200)
    let decision2 = evaluate_file_read_guard(&file_path, Some(1), Some(201), &config);
    assert_eq!(decision2, GuardDecision::Allow);
}

#[test]
fn test_large_file_unbounded_denies_with_ast_outline() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("large.py");

    let mut lines = Vec::new();
    lines.push("class Alpha:".to_string());
    lines.push("    def foo(self):".to_string());
    lines.push("        pass".to_string());
    for i in 4..=250 {
        lines.push(format!("    # filler line {}", i));
    }
    lines.push("def bar():".to_string());
    lines.push("    return 42".to_string());
    let content = lines.join("\n");
    fs::write(&file_path, content).unwrap();

    let config = FileReadGuardConfig {
        threshold_lines: 200,
    };
    let decision = evaluate_file_read_guard(&file_path, None, None, &config);

    match decision {
        GuardDecision::Deny { reason } => {
            assert!(
                reason.contains("[superstate-guard] Full file read blocked"),
                "reason should mention blocked full read: {}",
                reason
            );
            assert!(
                reason.contains("252 lines exceeds 200-line budget"),
                "reason should state line counts: {}",
                reason
            );
            assert!(
                reason.contains("AST Symbol Outline:"),
                "reason should have AST Symbol Outline header: {}",
                reason
            );
            assert!(
                reason.contains("Alpha"),
                "reason should outline Alpha class: {}",
                reason
            );
            assert!(
                reason.contains("bar"),
                "reason should outline bar function: {}",
                reason
            );
            assert!(
                reason.contains("Re-read using bounded StartLine and EndLine (<= 200 lines)"),
                "reason should include re-read instructions: {}",
                reason
            );
        }
        GuardDecision::Allow => panic!("Expected Deny for unbounded large file"),
    }
}

#[test]
fn test_large_file_bounded_slice_exceeding_threshold_denies() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("large.py");

    let mut lines = Vec::new();
    lines.push("class Beta:".to_string());
    for i in 2..=300 {
        lines.push(format!("    # filler line {}", i));
    }
    let content = lines.join("\n");
    fs::write(&file_path, content).unwrap();

    let config = FileReadGuardConfig {
        threshold_lines: 200,
    };
    // Request range 1..=250 -> 249 lines > 200
    let decision = evaluate_file_read_guard(&file_path, Some(1), Some(250), &config);

    match decision {
        GuardDecision::Deny { reason } => {
            assert!(reason.contains("[superstate-guard] Full file read blocked"));
            assert!(reason.contains("AST Symbol Outline:"));
            assert!(reason.contains("Beta"));
        }
        GuardDecision::Allow => panic!("Expected Deny for slice exceeding threshold"),
    }
}

#[test]
fn test_binary_file_denies() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("data.bin");

    let mut file = fs::File::create(&file_path).unwrap();
    let mut data = vec![0x41u8; 100];
    data[50] = 0x00; // null byte within first 8KB
    file.write_all(&data).unwrap();

    let config = FileReadGuardConfig::default();
    let decision = evaluate_file_read_guard(&file_path, None, None, &config);

    match decision {
        GuardDecision::Deny { reason } => {
            assert!(
                reason.contains("[superstate-guard] Binary file read blocked:"),
                "reason should mention binary blocked: {}",
                reason
            );
            assert!(
                reason.contains(&file_path.display().to_string()),
                "reason should include path: {}",
                reason
            );
        }
        GuardDecision::Allow => panic!("Expected Deny for binary file"),
    }
}

#[test]
fn test_unsupported_language_falls_back_to_preview() {
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("data.unsupported_ext");

    let lines: Vec<String> = (1..=250)
        .map(|i| format!("unsupported raw line content number {}", i))
        .collect();
    fs::write(&file_path, lines.join("\n")).unwrap();

    let config = FileReadGuardConfig {
        threshold_lines: 200,
    };
    let decision = evaluate_file_read_guard(&file_path, None, None, &config);

    match decision {
        GuardDecision::Deny { reason } => {
            assert!(
                reason.contains("[superstate-guard] Full file read blocked (250 lines exceeds 200-line budget)"),
                "header mismatch: {}",
                reason
            );
            assert!(
                reason.contains("File preview (first 30 lines):"),
                "should mention preview: {}",
                reason
            );
            assert!(
                reason.contains("unsupported raw line content number 1"),
                "should include first line: {}",
                reason
            );
            assert!(
                reason.contains("unsupported raw line content number 30"),
                "should include 30th line: {}",
                reason
            );
            assert!(
                !reason.contains("unsupported raw line content number 31"),
                "should not include 31st line: {}",
                reason
            );
            assert!(
                reason.contains("...\nRe-read using bounded StartLine and EndLine (<= 200 lines)."),
                "should include ellipsis and re-read guidance: {}",
                reason
            );
        }
        GuardDecision::Allow => panic!("Expected Deny for unsupported language exceeding threshold"),
    }
}

#[test]
fn test_empty_ast_outline_falls_back_to_preview() {
    let dir = tempdir().unwrap();
    // A python file with 250 lines of comments only (no symbols)
    let file_path = dir.path().join("comments_only.py");
    let lines: Vec<String> = (1..=250)
        .map(|i| format!("# just a comment {}", i))
        .collect();
    fs::write(&file_path, lines.join("\n")).unwrap();

    let config = FileReadGuardConfig {
        threshold_lines: 200,
    };
    let decision = evaluate_file_read_guard(&file_path, None, None, &config);

    match decision {
        GuardDecision::Deny { reason } => {
            assert!(
                reason.contains("File preview (first 30 lines):"),
                "empty AST outline should fall back to file preview: {}",
                reason
            );
            assert!(
                reason.contains("# just a comment 1"),
                "preview should show first line: {}",
                reason
            );
        }
        GuardDecision::Allow => panic!("Expected Deny for comments-only large file"),
    }
}

#[test]
fn bulk_read_target_catches_whole_file_dumps_but_not_bounded_reads() {
    use superstate_core::guard::bulk_read_target;

    // A whole-file dump is a file read wearing a shell command's clothes. The file guard only
    // matched the viewing tool, so this channel bypassed the entire perimeter -- 29 of 152
    // commands recorded across runs were `cat` or `grep`.
    for cmd in [
        "cat internal/scanner.go",
        "cat internal/scanner.go | grep pool",
        "less internal/scanner.go",
        "/bin/cat internal/scanner.go",
        "tail internal/scanner.go",
    ] {
        assert_eq!(
            bulk_read_target(cmd).as_deref(),
            Some(std::path::Path::new("internal/scanner.go")),
            "should intercept: {cmd}"
        );
    }

    // Bounded reads are the behaviour the guard exists to encourage. Intercepting them would
    // punish an agent for complying.
    for cmd in [
        "head -50 internal/scanner.go",
        "head -n 50 internal/scanner.go",
        "tail -n 20 internal/scanner.go",
        "go test ./...",
        "grep -rn pool internal/",
        "git status",
    ] {
        assert_eq!(bulk_read_target(cmd), None, "should pass through: {cmd}");
    }
}
