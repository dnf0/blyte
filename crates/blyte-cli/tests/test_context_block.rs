use std::path::Path;
use std::process::Command;

use blyte_cli::skill::{ensure_context_block, remove_block, CONTEXT_BLOCK, DEFAULT_BLOCK};

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn adds_block_to_existing_claude_md() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_md = claude_dir.join("CLAUDE.md");
    let initial = "# Mine\n\nkeep this\n";
    std::fs::write(&claude_md, initial).unwrap();

    let res = ensure_context_block(home.path()).unwrap();
    assert_eq!(res, Some(claude_md.clone()));

    let text = std::fs::read_to_string(&claude_md).unwrap();
    assert!(text.starts_with(initial));
    assert_eq!(text.matches(CONTEXT_BLOCK).count(), 1);
    assert!(text.ends_with("<!-- /blyte:context -->\n"));
}

#[test]
fn creates_claude_md_when_only_the_dir_exists() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_md = claude_dir.join("CLAUDE.md");

    let res = ensure_context_block(home.path()).unwrap();
    assert_eq!(res, Some(claude_md.clone()));

    let text = std::fs::read_to_string(&claude_md).unwrap();
    assert_eq!(text, format!("{CONTEXT_BLOCK}\n"));
}

#[test]
fn does_nothing_without_dot_claude() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");

    let res = ensure_context_block(home.path()).unwrap();
    assert_eq!(res, None);
    assert!(!claude_dir.exists());
}

#[test]
fn second_call_is_a_no_op() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_md = claude_dir.join("CLAUDE.md");
    std::fs::write(&claude_md, "# Mine\n").unwrap();

    let res1 = ensure_context_block(home.path()).unwrap();
    assert_eq!(res1, Some(claude_md.clone()));
    let bytes1 = std::fs::read(&claude_md).unwrap();

    let res2 = ensure_context_block(home.path()).unwrap();
    assert_eq!(res2, None);
    let bytes2 = std::fs::read(&claude_md).unwrap();

    assert_eq!(bytes1, bytes2);
}

#[test]
fn updates_a_stale_block_in_place() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_md = claude_dir.join("CLAUDE.md");
    let initial =
        "# Before\n\n<!-- blyte:context -->\nold rules\n<!-- /blyte:context -->\n\n## After\n";
    std::fs::write(&claude_md, initial).unwrap();

    let res = ensure_context_block(home.path()).unwrap();
    assert_eq!(res, Some(claude_md.clone()));

    let text = std::fs::read_to_string(&claude_md).unwrap();
    assert!(!text.contains("old rules"));
    assert_eq!(text.matches(CONTEXT_BLOCK).count(), 1);
    assert!(text.starts_with("# Before\n\n"));
    assert!(text.ends_with("\n\n## After\n"));
    assert_eq!(text, format!("# Before\n\n{CONTEXT_BLOCK}\n\n## After\n"));
}

#[test]
fn leaves_default_block_alone() {
    let home = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_md = claude_dir.join("CLAUDE.md");
    std::fs::write(&claude_md, format!("{DEFAULT_BLOCK}\n")).unwrap();

    let res = ensure_context_block(home.path()).unwrap();
    assert_eq!(res, Some(claude_md.clone()));

    let text = std::fs::read_to_string(&claude_md).unwrap();
    assert_eq!(text.matches(DEFAULT_BLOCK).count(), 1);
    assert_eq!(text.matches(CONTEXT_BLOCK).count(), 1);

    let stripped = remove_block(&text, CONTEXT_BLOCK);
    assert!(stripped.contains(DEFAULT_BLOCK));
    assert!(!stripped.contains("<!-- blyte:context -->"));
}

#[test]
fn install_and_uninstall_manage_the_block() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_md = claude_dir.join("CLAUDE.md");

    let out = blyte(
        cwd.path(),
        home.path(),
        &["skill", "install", "--no-statusline"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&claude_md).unwrap();
    assert!(text.contains(CONTEXT_BLOCK), "{text}");

    let out = blyte(cwd.path(), home.path(), &["skill", "uninstall"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text_after = std::fs::read_to_string(&claude_md).unwrap();
    assert!(
        !text_after.contains("<!-- blyte:context -->"),
        "{text_after}"
    );
}

#[test]
fn install_with_no_default_still_writes_context_block() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let claude_md = claude_dir.join("CLAUDE.md");

    let out = blyte(
        cwd.path(),
        home.path(),
        &["skill", "install", "--no-default", "--no-statusline"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = std::fs::read_to_string(&claude_md).unwrap();
    assert!(text.contains(CONTEXT_BLOCK), "{text}");
    assert!(!text.contains("<!-- blyte:default -->"), "{text}");
}
