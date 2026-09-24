use blyte_core::impact::build_sliced_test_cmd;
use std::path::{Path, PathBuf};

#[test]
fn test_build_sliced_test_cmd_pytest() {
    let repo = Path::new("/workspace");
    let files = vec![
        PathBuf::from("/workspace/tests/test_auth.py"),
        PathBuf::from("/workspace/tests/test_token.py"),
    ];
    let res = build_sliced_test_cmd("pytest tests/", &files, repo);
    assert_eq!(
        res,
        Some("pytest tests/test_auth.py tests/test_token.py".to_string())
    );
}

#[test]
fn test_build_sliced_test_cmd_pytest_container_prefix() {
    let repo = Path::new("/workspace");
    let files = vec![PathBuf::from("/workspace/tests/test_auth.py")];
    let res = build_sliced_test_cmd("./run-in-container pytest tests/", &files, repo);
    assert_eq!(
        res,
        Some("./run-in-container pytest tests/test_auth.py".to_string())
    );
}

#[test]
fn test_build_sliced_test_cmd_cargo() {
    let repo = Path::new("/workspace");
    let files = vec![PathBuf::from("/workspace/tests/test_lexer.rs")];
    let res = build_sliced_test_cmd("cargo test", &files, repo);
    assert_eq!(res, Some("cargo test --test test_lexer".to_string()));
}

#[test]
fn test_build_sliced_test_cmd_unrecognized_returns_none() {
    let repo = Path::new("/workspace");
    let files = vec![PathBuf::from("/workspace/tests/test_auth.py")];
    let res = build_sliced_test_cmd("sh verify.sh", &files, repo);
    assert_eq!(res, None);
}

#[test]
fn test_build_sliced_test_cmd_empty_impacted_files_returns_none() {
    let repo = Path::new("/workspace");
    let files: Vec<PathBuf> = vec![];
    let res = build_sliced_test_cmd("pytest tests/", &files, repo);
    assert_eq!(res, None);
}

#[test]
fn test_build_sliced_test_cmd_cargo_multiple_targets() {
    let repo = Path::new("/workspace");
    let files = vec![
        PathBuf::from("/workspace/tests/test_parser.rs"),
        PathBuf::from("/workspace/tests/test_lexer.rs"),
    ];
    let res = build_sliced_test_cmd("cargo test", &files, repo);
    assert_eq!(
        res,
        Some("cargo test --test test_lexer --test test_parser".to_string())
    );
}

#[test]
fn test_build_sliced_test_cmd_node_vitest_and_jest() {
    let repo = Path::new("/workspace");
    let files = vec![PathBuf::from("/workspace/tests/auth.test.ts")];

    let res_vitest = build_sliced_test_cmd("npx vitest run tests/", &files, repo);
    assert_eq!(
        res_vitest,
        Some("npx vitest run tests/auth.test.ts".to_string())
    );

    let res_jest = build_sliced_test_cmd("npx jest tests/", &files, repo);
    assert_eq!(res_jest, Some("npx jest tests/auth.test.ts".to_string()));

    let res_npm = build_sliced_test_cmd("npm test", &files, repo);
    assert_eq!(res_npm, Some("npm test -- tests/auth.test.ts".to_string()));
}

#[test]
fn test_build_sliced_test_cmd_go_package() {
    let repo = Path::new("/workspace");
    let files = vec![PathBuf::from("/workspace/pkg/server/server_test.go")];
    let res = build_sliced_test_cmd("go test ./...", &files, repo);
    assert_eq!(res, Some("go test ./pkg/server".to_string()));
}

#[test]
fn test_build_sliced_test_cmd_preserves_flags() {
    let repo = Path::new("/workspace");

    // Pytest preserves -v -s -k "test_foo" while dropping directory tests/
    let py_files = vec![PathBuf::from("/workspace/tests/test_auth.py")];
    let res_py = build_sliced_test_cmd("pytest -v -s -k test_foo tests/", &py_files, repo);
    assert_eq!(
        res_py,
        Some("pytest -v -s -k test_foo tests/test_auth.py".to_string())
    );

    // Cargo preserves --features all --release while replacing with --test
    let rs_files = vec![PathBuf::from("/workspace/tests/test_lexer.rs")];
    let res_cargo = build_sliced_test_cmd("cargo test --features all --release", &rs_files, repo);
    assert_eq!(
        res_cargo,
        Some("cargo test --features all --release --test test_lexer".to_string())
    );

    // Go preserves -v -race while replacing ./... with package dir
    let go_files = vec![PathBuf::from("/workspace/pkg/server/server_test.go")];
    let res_go = build_sliced_test_cmd("go test -v -race ./...", &go_files, repo);
    assert_eq!(res_go, Some("go test -v -race ./pkg/server".to_string()));

    // Vitest preserves --run -t "suite"
    let ts_files = vec![PathBuf::from("/workspace/tests/auth.test.ts")];
    let res_vitest = build_sliced_test_cmd("npx vitest run --coverage tests/", &ts_files, repo);
    assert_eq!(
        res_vitest,
        Some("npx vitest run --coverage tests/auth.test.ts".to_string())
    );
}

#[test]
fn test_build_sliced_test_cmd_stops_at_shell_chaining_operators() {
    let repo = Path::new("/workspace");

    let py_files = vec![PathBuf::from("/workspace/tests/test_auth.py")];
    let res = build_sliced_test_cmd(
        "pytest -v tests/ && touch full_gate_ran.txt && exit 0",
        &py_files,
        repo,
    );
    assert_eq!(res, Some("pytest -v tests/test_auth.py".to_string()));

    let rs_files = vec![PathBuf::from("/workspace/tests/test_lexer.rs")];
    let res_cargo = build_sliced_test_cmd(
        "cargo test --features all tests/ ; echo done",
        &rs_files,
        repo,
    );
    assert_eq!(
        res_cargo,
        Some("cargo test --features all --test test_lexer".to_string())
    );
}

#[test]
fn a_heredoc_command_is_never_sliced() {
    // The runner name lives in the heredoc BODY, not in the command this shell runs.
    // Rewriting around it yields an unterminated heredoc that executes on the host.
    // EXP-072 measured the cost: four spurious "No tests found ... In /workspace" failures
    // on katex, $2.71, and the task lost.
    let repo = tempfile::tempdir().expect("tempdir");
    let impacted = vec![std::path::PathBuf::from("src/x.test.js")];
    for gate in [
        "cat <<'GATEEOF' | ./run-in-container sh -s\nnpx jest\nGATEEOF",
        "cat <<'EOF' | ./run-in-container sh -s\npython -m pytest tests/\nEOF",
    ] {
        assert_eq!(
            blyte_core::impact::build_sliced_test_cmd(gate, &impacted, repo.path()),
            None,
            "sliced a heredoc command: {gate}"
        );
    }
}

#[test]
fn a_plain_command_is_still_sliced() {
    let repo = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(repo.path().join("tests")).expect("mkdir");
    let impacted = vec![repo.path().join("tests/test_x.py")];
    let sliced = blyte_core::impact::build_sliced_test_cmd(
        "./run-in-container python -m pytest tests/",
        &impacted,
        repo.path(),
    );
    let sliced = sliced.expect("a plain pytest command must still slice");
    assert!(sliced.contains("tests/test_x.py"), "{sliced}");
    assert!(
        sliced.starts_with("./run-in-container "),
        "slicing must preserve the container bridge: {sliced}"
    );
}
