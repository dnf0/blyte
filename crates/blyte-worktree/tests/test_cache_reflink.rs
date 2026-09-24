use blyte_worktree::cache::reflink_cache_dirs;
use std::fs;
use tempfile::TempDir;

#[test]
fn test_reflink_cache_dirs_copies_specified_trees() {
    let src_dir = TempDir::new().unwrap();
    let dst_dir = TempDir::new().unwrap();

    // Create mock cache trees
    let venv_dir = src_dir.path().join(".venv").join("bin");
    fs::create_dir_all(&venv_dir).unwrap();
    fs::write(venv_dir.join("python"), b"#!/usr/bin/env python3").unwrap();

    let target_dir = src_dir.path().join("target").join("debug");
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(target_dir.join("app"), b"binary-bytes").unwrap();

    // A dir not in cache_names should NOT be copied
    let other_dir = src_dir.path().join("src");
    fs::create_dir_all(&other_dir).unwrap();
    fs::write(other_dir.join("main.rs"), b"fn main() {}").unwrap();

    // Run reflink cache copy
    let result = reflink_cache_dirs(src_dir.path(), dst_dir.path(), &[".venv", "target"]);
    assert!(
        result.is_ok(),
        "reflink_cache_dirs failed: {:?}",
        result.err()
    );

    // Verify .venv and target exist in dst_dir
    let dst_python = dst_dir.path().join(".venv").join("bin").join("python");
    assert!(dst_python.exists(), ".venv/bin/python was not copied");
    assert_eq!(fs::read(&dst_python).unwrap(), b"#!/usr/bin/env python3");

    let dst_app = dst_dir.path().join("target").join("debug").join("app");
    assert!(dst_app.exists(), "target/debug/app was not copied");
    assert_eq!(fs::read(&dst_app).unwrap(), b"binary-bytes");

    // Verify src was NOT copied
    assert!(
        !dst_dir.path().join("src").exists(),
        "src/ should not have been copied"
    );
}
