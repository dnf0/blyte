use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use blyte_cli::dispatch::DISPATCH_RULES;
use blyte_cli::outside_env::{borrowed_env_commands, flag, normalize_path, shell_words};

fn wt() -> &'static Path {
    Path::new("/r/.wt/T1")
}

fn log_line(cmd: &str) -> String {
    format!("  ● bash {cmd} -> exit 0\n")
}

// 1. `cp -cR /r/.venv .venv` is returned (main checkout).
#[test]
fn cp_from_main_checkout_is_returned() {
    let cmd = "cp -cR /r/.venv .venv";
    let log = log_line(cmd);
    assert_eq!(borrowed_env_commands(&log, wt()), vec![cmd.to_string()]);
}

// 2. `rm -rf .venv && cp -cR ../T2/.venv .venv` is returned (sibling worktree, relative).
#[test]
fn cp_from_sibling_worktree_relative_is_returned() {
    let cmd = "rm -rf .venv && cp -cR ../T2/.venv .venv";
    let log = log_line(cmd);
    assert_eq!(borrowed_env_commands(&log, wt()), vec![cmd.to_string()]);
}

// 3. `cp -R ~/.cache/uv/archive-v0/abc/lib .venv/lib/python3.12/site-packages/x` is returned.
#[test]
fn cp_from_home_uv_cache_is_returned() {
    let cmd = "cp -R ~/.cache/uv/archive-v0/abc/lib .venv/lib/python3.12/site-packages/x";
    let log = log_line(cmd);
    assert_eq!(borrowed_env_commands(&log, wt()), vec![cmd.to_string()]);
}

// 4. `ln -s /other/repo/node_modules node_modules` is returned.
#[test]
fn ln_from_other_repo_is_returned() {
    let cmd = "ln -s /other/repo/node_modules node_modules";
    let log = log_line(cmd);
    assert_eq!(borrowed_env_commands(&log, wt()), vec![cmd.to_string()]);
}

// 5. `cp -R .venv/lib/a .venv/lib/b` is not returned (inside the worktree).
#[test]
fn cp_inside_worktree_is_not_returned() {
    let cmd = "cp -R .venv/lib/a .venv/lib/b";
    let log = log_line(cmd);
    assert!(borrowed_env_commands(&log, wt()).is_empty());
}

// 6. `cp ../T2/src/a.py src/a.py` is not returned (no environment directory).
#[test]
fn cp_without_env_dir_is_not_returned() {
    let cmd = "cp ../T2/src/a.py src/a.py";
    let log = log_line(cmd);
    assert!(borrowed_env_commands(&log, wt()).is_empty());
}

// 7. `ls /r/.venv` and `cat ../T2/.venv/pyvenv.cfg` are not returned (not a copy or link).
#[test]
fn non_copy_or_link_commands_are_not_returned() {
    let log = format!(
        "{}{}",
        log_line("ls /r/.venv"),
        log_line("cat ../T2/.venv/pyvenv.cfg")
    );
    assert!(borrowed_env_commands(&log, wt()).is_empty());
}

// 8. The same command logged twice is returned once.
#[test]
fn duplicate_command_is_returned_once() {
    let cmd = "cp -cR /r/.venv .venv";
    let log = format!("{}{}", log_line(cmd), log_line(cmd));
    assert_eq!(borrowed_env_commands(&log, wt()), vec![cmd.to_string()]);
}

// 9. `flag(&[])` is `None`; `flag` of two commands contains `2 command(s)` and the first command.
#[test]
fn flag_formats_count_and_first_command() {
    assert_eq!(flag(&[]), None);
    let cmds = vec![
        "cp -cR /r/.venv .venv".to_string(),
        "ln -s /other/repo/node_modules node_modules".to_string(),
    ];
    let f = flag(&cmds).expect("expected Some flag");
    assert_eq!(
        f,
        "the gate may have run in an environment from outside the worktree: 2 command(s), first: cp -cR /r/.venv .venv"
    );
    assert!(f.contains("2 command(s)"));
    assert!(f.contains("cp -cR /r/.venv .venv"));
}

// 10. `DISPATCH_RULES` contains `Never copy, link or use a dependency environment from outside the worktree`.
#[test]
fn dispatch_rules_forbids_borrowing_outside_environment() {
    assert!(DISPATCH_RULES
        .contains("Never copy, link or use a dependency environment from outside the worktree"));
}

fn blyte_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_blyte"))
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn blyte(dir: &Path, args: &[&str]) -> Output {
    Command::new(blyte_bin())
        .args(args)
        .current_dir(dir)
        .env("BLYTE_STATE_DIR", dir.join(".state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .output()
        .expect("blyte")
}

#[test]
fn review_flags_borrowed_env_and_errors_when_summary_lacks_worktree() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    git(r, &["init", "-q", "-b", "main"]);
    git(r, &["config", "user.email", "t@example.com"]);
    git(r, &["config", "user.name", "T"]);
    std::fs::create_dir_all(r.join("src")).unwrap();
    std::fs::write(r.join("src/a.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(r.join("task.md"), "Add b.\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "init", "--no-gpg-sign"]);

    std::fs::create_dir_all(r.join(".wt")).unwrap();
    git(r, &["worktree", "add", "-q", "-b", "wip/T1", ".wt/T1"]);
    let wt_path = r.join(".wt/T1");
    std::fs::write(wt_path.join("src/a.rs"), "pub fn a() {}\npub fn b() {}\n").unwrap();
    git(&wt_path, &["add", "src/a.rs"]);
    git(&wt_path, &["commit", "-qm", "feat: add b", "--no-gpg-sign"]);

    std::fs::write(
        r.join(".wt/T1.start.json"),
        serde_json::json!({
            "id": "T1",
            "task_file": "task.md"
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        r.join(".wt/T1.report.json"),
        serde_json::json!({
            "outcome": "verified"
        })
        .to_string(),
    )
    .unwrap();
    let copy_cmd = format!("cp -cR {}/.venv .venv", r.display());
    std::fs::write(r.join(".wt/T1.log"), log_line(&copy_cmd)).unwrap();

    // Summary with worktree field: review flags the borrowed environment.
    std::fs::write(
        r.join(".wt/T1.json"),
        serde_json::json!({
            "id": "T1",
            "worktree": wt_path.display().to_string(),
            "outcome": "verified"
        })
        .to_string(),
    )
    .unwrap();

    let out = blyte(r, &["review", "T1"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(
            "the gate may have run in an environment from outside the worktree: 1 command(s), first:"
        ),
        "expected borrowed env flag in review output:\n{stdout}"
    );

    // Summary without worktree field: error naming the missing field, not a skip.
    std::fs::write(
        r.join(".wt/T1.json"),
        serde_json::json!({
            "id": "T1",
            "outcome": "verified"
        })
        .to_string(),
    )
    .unwrap();

    let out_missing = blyte(r, &["review", "T1"]);
    let missing_stdout = String::from_utf8_lossy(&out_missing.stdout);
    assert!(
        missing_stdout.contains("missing `worktree` field in summary"),
        "expected == checks to report missing `worktree` field, got:\n{missing_stdout}"
    );
}

#[test]
fn normalize_path_handles_parent_and_cur_components() {
    assert_eq!(normalize_path(Path::new("../..")), PathBuf::from("../.."));
    assert_eq!(
        normalize_path(Path::new("a/../../b")),
        PathBuf::from("../b")
    );
    assert_eq!(normalize_path(Path::new("/a/../../b")), PathBuf::from("/b"));
    assert_eq!(normalize_path(Path::new("a/./b/..")), PathBuf::from("a"));
    assert_eq!(normalize_path(Path::new("./a")), PathBuf::from("a"));
    assert_eq!(normalize_path(Path::new("/..")), PathBuf::from("/"));
}

#[test]
fn shell_words_handles_quotes_and_escapes() {
    assert_eq!(
        shell_words(r#"cp -r "/a b/.venv" ."#),
        vec!["cp", "-r", "/a b/.venv", "."]
    );
    assert_eq!(
        shell_words("cp '/a b/.venv' x"),
        vec!["cp", "/a b/.venv", "x"]
    );
    assert_eq!(
        shell_words(r"cp /a\ b/.venv x"),
        vec!["cp", "/a b/.venv", "x"]
    );
    assert_eq!(
        shell_words(r#"--from="/a b/.venv""#),
        vec!["--from=/a b/.venv"]
    );
    assert_eq!(shell_words(r#""a \"b\" c""#), vec![r#"a "b" c"#]);
    assert_eq!(shell_words(r#""a \\b\\ c""#), vec![r"a \b\ c"]);
    assert_eq!(shell_words(r#"cp "/a b/.venv"#), vec!["cp", "/a b/.venv"]);
    assert!(shell_words("").is_empty());
}

#[test]
fn borrowed_env_commands_handles_quoted_and_escaped_paths_with_spaces() {
    let worktree = Path::new("/work/repo/.wt/T1");
    let quoted_outside = r#"cp -r "/Users/me/my repo/.venv" ."#;
    let escaped_outside = r"cp -r /Users/me/my\ repo/.venv .";
    let quoted_inside = r#"cp -r "sub dir/.venv" backup"#;
    let log = format!(
        "{}{}{}",
        log_line(quoted_outside),
        log_line(escaped_outside),
        log_line(quoted_inside)
    );
    assert_eq!(
        borrowed_env_commands(&log, worktree),
        vec![quoted_outside.to_string(), escaped_outside.to_string()]
    );
}

#[test]
fn borrowed_env_commands_resolves_relative_worktree_parent_traversal() {
    let worktree = Path::new(".wt/T1");
    let leaves_repo = "cp -r ../../../.venv .";
    let stays_in_wt = "cp -r ../T1/.venv x";
    let log = format!("{}{}", log_line(leaves_repo), log_line(stays_in_wt));
    assert_eq!(
        borrowed_env_commands(&log, worktree),
        vec![leaves_repo.to_string()]
    );
}
