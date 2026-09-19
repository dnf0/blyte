use std::path::{Path, PathBuf};
use superstate_core::impact::build_sliced_test_cmd;

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
