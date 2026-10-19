//! A run may read extra directories with read and grep, and never write there: research runs had to
//! read the code they studied through the shell, 79 turns of 80 (2026-10-01).

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, set_read_roots};
use serde_json::json;

#[test]
fn read_roots_are_readable_and_never_writable() {
    let t = tempfile::tempdir().unwrap();
    let ws = t.path().join("scratch");
    let code = t.path().join("repo");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(code.join("src")).unwrap();
    std::fs::write(code.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    let file = code.join("src/lib.rs").to_string_lossy().to_string();
    let dir = code.to_string_lossy().to_string();

    let before = execute(&ws, &LocalShell, &LocalFs, "read", &json!({"path": file}));
    assert_eq!(
        before.content,
        format!(
            "path escapes the workspace: {file}. This run can read the workspace. Do not look for another copy: if the task needs this path, say in your answer that it was not readable."
        )
    );

    set_read_roots(vec![code.clone()]);
    let read = execute(&ws, &LocalShell, &LocalFs, "read", &json!({"path": file}));
    assert!(read.content.contains("answer"), "{}", read.content);
    let grep = execute(
        &ws,
        &LocalShell,
        &LocalFs,
        "grep",
        &json!({"pattern": "fn answer", "path": dir}),
    );
    assert!(grep.content.contains("lib.rs"), "{}", grep.content);

    let write = execute(
        &ws,
        &LocalShell,
        &LocalFs,
        "write",
        &json!({"path": file, "content": "x"}),
    );
    assert_eq!(write.content, format!("path escapes the workspace: {file}"));
    assert!(std::fs::read_to_string(code.join("src/lib.rs"))
        .unwrap()
        .contains("42"));
    let outside = t.path().join("other.txt");
    std::fs::write(&outside, "secret").unwrap();
    let other = execute(
        &ws,
        &LocalShell,
        &LocalFs,
        "read",
        &json!({"path": outside.to_string_lossy()}),
    );
    assert_eq!(
        other.content,
        format!(
            "path escapes the workspace: {}. This run can read the workspace and {}. Do not look for another copy: if the task needs this path, say in your answer that it was not readable.",
            outside.display(),
            code.display()
        )
    );
    set_read_roots(vec![]);
}
