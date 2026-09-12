use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

fn init_git_repo(path: &std::path::Path) {
    let _ = Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["checkout", "-b", "main"])
        .current_dir(path)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(path)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .unwrap();
}

#[test]
fn test_python_adjacent_and_mirrored_test_detection() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    init_git_repo(root);

    // Create Python files:
    // 1. src/auth/jwt.py with mirrored tests/auth/test_jwt.py
    // 2. src/utils/helpers.py with adjacent src/utils/test_helpers.py
    // 3. src/models/user.py with tests/unit/test_user.py
    // 4. api.py with tests/api_test.py
    fs::create_dir_all(root.join("src/auth")).unwrap();
    fs::create_dir_all(root.join("tests/auth")).unwrap();
    fs::create_dir_all(root.join("src/utils")).unwrap();
    fs::create_dir_all(root.join("src/models")).unwrap();
    fs::create_dir_all(root.join("tests/unit")).unwrap();

    fs::write(root.join("src/auth/jwt.py"), "def encode(): pass\n").unwrap();
    fs::write(
        root.join("tests/auth/test_jwt.py"),
        "def test_encode(): pass\n",
    )
    .unwrap();

    fs::write(root.join("src/utils/helpers.py"), "def help(): pass\n").unwrap();
    fs::write(
        root.join("src/utils/test_helpers.py"),
        "def test_help(): pass\n",
    )
    .unwrap();

    fs::write(root.join("src/models/user.py"), "class User: pass\n").unwrap();
    fs::write(
        root.join("tests/unit/test_user.py"),
        "def test_user(): pass\n",
    )
    .unwrap();

    fs::write(root.join("api.py"), "def get(): pass\n").unwrap();
    fs::write(root.join("tests/api_test.py"), "def test_get(): pass\n").unwrap();

    // Commit baseline on main
    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["commit", "-m", "initial baseline"])
        .current_dir(root)
        .output()
        .unwrap();

    // Create feature branch
    let _ = Command::new("git")
        .args(["checkout", "-b", "feat/update-python"])
        .current_dir(root)
        .output()
        .unwrap();

    // Modify source files
    fs::write(root.join("src/auth/jwt.py"), "def encode(): return 1\n").unwrap();
    fs::write(root.join("src/utils/helpers.py"), "def help(): return 2\n").unwrap();
    fs::write(root.join("src/models/user.py"), "class User: id = 1\n").unwrap();
    fs::write(root.join("api.py"), "def get(): return 200\n").unwrap();

    let impacted = superstate_core::impact::infer_impacted_tests(root, Some("main"));

    let mut expected: Vec<PathBuf> = vec![
        root.join("tests/auth/test_jwt.py"),
        root.join("src/utils/test_helpers.py"),
        root.join("tests/unit/test_user.py"),
        root.join("tests/api_test.py"),
    ];
    expected.sort();

    assert_eq!(impacted, expected);
}

#[test]
fn test_rust_and_typescript_and_go_test_detection() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    init_git_repo(root);

    // 1. Rust: src/calculator.rs -> tests/test_calculator.rs
    // 2. Rust: src/parser.rs -> tests/parser.rs
    // 3. TypeScript: src/components/button.ts -> tests/button.test.ts
    // 4. TypeScript: src/components/card.tsx -> src/components/card.spec.tsx (adjacent)
    // 5. Go: pkg/server/server.go -> pkg/server/server_test.go
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::create_dir_all(root.join("pkg/server")).unwrap();

    fs::write(root.join("src/calculator.rs"), "pub fn add() {}\n").unwrap();
    fs::write(root.join("tests/test_calculator.rs"), "#[test] fn t() {}\n").unwrap();

    fs::write(root.join("src/parser.rs"), "pub fn parse() {}\n").unwrap();
    fs::write(root.join("tests/parser.rs"), "#[test] fn t() {}\n").unwrap();

    fs::write(
        root.join("src/components/button.ts"),
        "export const Button = 1;\n",
    )
    .unwrap();
    fs::write(
        root.join("tests/button.test.ts"),
        "test('button', () => {});\n",
    )
    .unwrap();

    fs::write(
        root.join("src/components/card.tsx"),
        "export const Card = 1;\n",
    )
    .unwrap();
    fs::write(
        root.join("src/components/card.spec.tsx"),
        "test('card', () => {});\n",
    )
    .unwrap();

    fs::write(root.join("pkg/server/server.go"), "package server\n").unwrap();
    fs::write(root.join("pkg/server/server_test.go"), "package server\n").unwrap();

    // Baseline commit
    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(root)
        .output()
        .unwrap();

    // Feature branch
    let _ = Command::new("git")
        .args(["checkout", "-b", "feat/multi-lang"])
        .current_dir(root)
        .output()
        .unwrap();

    // Modify source files
    fs::write(
        root.join("src/calculator.rs"),
        "pub fn add() -> i32 { 1 }\n",
    )
    .unwrap();
    fs::write(root.join("src/parser.rs"), "pub fn parse() -> i32 { 2 }\n").unwrap();
    fs::write(
        root.join("src/components/button.ts"),
        "export const Button = 2;\n",
    )
    .unwrap();
    fs::write(
        root.join("src/components/card.tsx"),
        "export const Card = 2;\n",
    )
    .unwrap();
    fs::write(
        root.join("pkg/server/server.go"),
        "package server\n// modified\n",
    )
    .unwrap();

    let impacted = superstate_core::impact::infer_impacted_tests(root, Some("main"));

    let mut expected: Vec<PathBuf> = vec![
        root.join("tests/test_calculator.rs"),
        root.join("tests/parser.rs"),
        root.join("tests/button.test.ts"),
        root.join("src/components/card.spec.tsx"),
        root.join("pkg/server/server_test.go"),
    ];
    expected.sort();

    assert_eq!(impacted, expected);
}

#[test]
fn test_direct_modifications_to_test_files() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    init_git_repo(root);

    fs::create_dir_all(root.join("tests")).unwrap();
    fs::create_dir_all(root.join("src/components")).unwrap();
    fs::write(root.join("tests/test_auth.py"), "# auth test\n").unwrap();
    fs::write(root.join("tests/test_calc.rs"), "// calc test\n").unwrap();
    fs::write(root.join("src/components/card.test.ts"), "// card test\n").unwrap();

    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(root)
        .output()
        .unwrap();

    let _ = Command::new("git")
        .args(["checkout", "-b", "feat/direct-test-edit"])
        .current_dir(root)
        .output()
        .unwrap();

    // Directly edit test files
    fs::write(root.join("tests/test_auth.py"), "# auth test modified\n").unwrap();
    fs::write(root.join("tests/test_calc.rs"), "// calc test modified\n").unwrap();
    fs::write(
        root.join("src/components/card.test.ts"),
        "// card test modified\n",
    )
    .unwrap();

    let impacted = superstate_core::impact::infer_impacted_tests(root, Some("main"));

    let mut expected: Vec<PathBuf> = vec![
        root.join("tests/test_auth.py"),
        root.join("tests/test_calc.rs"),
        root.join("src/components/card.test.ts"),
    ];
    expected.sort();

    assert_eq!(impacted, expected);
}

#[test]
fn test_clean_git_diff_returns_empty_vector() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    init_git_repo(root);

    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(root.join("src/auth.py"), "def login(): pass\n").unwrap();
    fs::write(root.join("tests/test_auth.py"), "def test_login(): pass\n").unwrap();

    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(root)
        .output()
        .unwrap();

    // No changes on branch
    let _ = Command::new("git")
        .args(["checkout", "-b", "feat/no-changes"])
        .current_dir(root)
        .output()
        .unwrap();

    // Explicit base
    let impacted_explicit = superstate_core::impact::infer_impacted_tests(root, Some("main"));
    assert!(impacted_explicit.is_empty());

    // Default base (None -> "main")
    let impacted_default = superstate_core::impact::infer_impacted_tests(root, None);
    assert!(impacted_default.is_empty());
}

#[test]
fn test_invalid_base_or_non_git_returns_empty() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // Non-git directory
    let impacted = superstate_core::impact::infer_impacted_tests(root, Some("main"));
    assert!(impacted.is_empty());

    // Invalid base branch on initialized repo
    init_git_repo(root);
    fs::write(root.join("file.txt"), "hello").unwrap();
    let _ = Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .unwrap();
    let _ = Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(root)
        .output()
        .unwrap();

    let impacted_invalid_branch =
        superstate_core::impact::infer_impacted_tests(root, Some("non-existent-branch"));
    assert!(impacted_invalid_branch.is_empty());
}
