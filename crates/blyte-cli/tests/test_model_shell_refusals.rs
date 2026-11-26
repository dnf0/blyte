use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "i", "--no-gpg-sign"]);
    d
}

fn blyte_with_marker(dir: &Path, args: &[&str], marker: bool) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.args(args)
        .current_dir(dir)
        .env("BLYTE_NO_UPDATE_CHECK", "1");
    if marker {
        cmd.env("BLYTE_MODEL_SHELL", "1");
    } else {
        cmd.env_remove("BLYTE_MODEL_SHELL");
    }
    cmd.output().unwrap()
}

#[test]
fn verdict_accepted_and_fixed_are_refused_in_a_model_shell() {
    let r = repo();

    let accepted = blyte_with_marker(r.path(), &["review", "T1", "--verdict", "accepted"], true);
    assert_eq!(accepted.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&accepted.stderr);
    assert!(
        stderr.contains(
            "error: blyte review --verdict accepted writes a record only the user or the harness may write, and cannot be run from a model's shell"
        ),
        "{stderr}"
    );
    assert!(!r.path().join(".wt/T1.verdict.json").exists());

    let fixed = blyte_with_marker(r.path(), &["review", "T2", "--verdict", "fixed"], true);
    assert_eq!(fixed.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&fixed.stderr);
    assert!(
        stderr.contains(
            "error: blyte review --verdict fixed writes a record only the user or the harness may write, and cannot be run from a model's shell"
        ),
        "{stderr}"
    );
    assert!(!r.path().join(".wt/T2.verdict.json").exists());
}

#[test]
fn combine_and_model_review_are_refused_in_a_model_shell() {
    let r = repo();
    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/T3", ".wt/T3"],
    );

    let combined = blyte_with_marker(r.path(), &["combine", "r1700000000-1"], true);
    assert_eq!(combined.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&combined.stderr);
    assert!(
        stderr.contains(
            "error: blyte combine writes a record only the user or the harness may write, and cannot be run from a model's shell"
        ),
        "{stderr}"
    );
    assert!(!r.path().join(".wt/combined").exists());

    let model_review = blyte_with_marker(r.path(), &["review", "T3", "--model-review"], true);
    assert_eq!(model_review.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&model_review.stderr);
    assert!(
        stderr.contains(
            "error: blyte review --model-review writes a record only the user or the harness may write, and cannot be run from a model's shell"
        ),
        "{stderr}"
    );
    assert!(!r.path().join(".wt/T3.verdict.json").exists());
    assert!(!r.path().join(".wt/combined").exists());
}

#[test]
fn verdict_redone_is_allowed_in_a_model_shell() {
    let r = repo();
    let out = blyte_with_marker(r.path(), &["review", "T4", "--verdict", "redone"], true);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(r.path().join(".wt/T4.verdict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(v["verdict"], "redone");
}

#[test]
fn the_same_commands_work_without_the_marker() {
    let r = repo();
    let out = blyte_with_marker(r.path(), &["review", "T5", "--verdict", "accepted"], false);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(r.path().join(".wt/T5.verdict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(v["verdict"], "accepted");

    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/T6", ".wt/T6"],
    );
    let model_review = blyte_with_marker(r.path(), &["review", "T6", "--model-review"], false);
    assert_eq!(
        model_review.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&model_review.stderr)
    );
}

fn blyte_dispatch_with_marker(
    dir: &Path,
    home: &Path,
    args: &[&str],
    marker: bool,
) -> std::process::Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.args(args)
        .current_dir(dir)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("HOME", home)
        .env("BLYTE_STATE_DIR", home.join("state"));
    if marker {
        cmd.env("BLYTE_MODEL_SHELL", "1");
    } else {
        cmd.env_remove("BLYTE_MODEL_SHELL");
    }
    cmd.output().unwrap()
}

fn assert_cli_dispatch_option_refused(args_variants: &[&[&str]], expected_option: &str) {
    let r = repo();
    let home = tempfile::tempdir().unwrap();
    let expected = format!(
        "do not pass {expected_option}; only the user can loosen a run's sandbox or replace its checks"
    );
    for args in args_variants {
        let out = blyte_dispatch_with_marker(r.path(), home.path(), args, true);
        assert_eq!(out.status.code(), Some(2), "for {args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(&expected),
            "expected {expected:?} in stderr for {args:?}, got: {stderr}"
        );
        assert!(
            !r.path().join(".wt/T1").exists(),
            "worktree created for {args:?}"
        );
    }
}

#[test]
fn refuses_dispatch_sandbox_in_model_shell() {
    assert_cli_dispatch_option_refused(
        &[
            &["dispatch", "T1", "t.md", "--sandbox", "off"],
            &["dispatch", "T1", "t.md", "--sandbox=off"],
            &["dispatch", "T1", "t.md", "--sandbox", "on"],
        ],
        "--sandbox",
    );
}

#[test]
fn refuses_dispatch_network_in_model_shell() {
    assert_cli_dispatch_option_refused(
        &[
            &["dispatch", "T1", "t.md", "--network", "off"],
            &["dispatch", "T1", "t.md", "--network=on"],
        ],
        "--network",
    );
}

#[test]
fn refuses_dispatch_allow_read_in_model_shell() {
    assert_cli_dispatch_option_refused(
        &[
            &["dispatch", "T1", "t.md", "--allow-read", "/etc"],
            &["dispatch", "T1", "t.md", "--allow-read=/etc"],
        ],
        "--allow-read",
    );
}

#[test]
fn refuses_dispatch_pass_env_in_model_shell() {
    assert_cli_dispatch_option_refused(
        &[
            &["dispatch", "T1", "t.md", "--pass-env", "SECRET"],
            &["dispatch", "T1", "t.md", "--pass-env=SECRET"],
        ],
        "--pass-env",
    );
}

#[test]
fn refuses_dispatch_gate_in_model_shell() {
    assert_cli_dispatch_option_refused(
        &[
            &["dispatch", "T1", "t.md", "--gate", "true"],
            &["dispatch", "T1", "t.md", "--gate=true"],
            &["dispatch", "T1", "t.md", "-g", "true"],
            &["dispatch", "T1", "t.md", "-gtrue"],
        ],
        "--gate",
    );
}

#[test]
fn refuses_dispatch_no_path_check_in_model_shell() {
    assert_cli_dispatch_option_refused(
        &[&["dispatch", "T1", "t.md", "--no-path-check"]],
        "--no-path-check",
    );
}

#[test]
fn dispatch_sandbox_option_is_not_refused_without_model_shell_marker() {
    let r = repo();
    let home = tempfile::tempdir().unwrap();
    let args = &["dispatch", "T1", "missing.md", "--sandbox", "off"];

    let refused = blyte_dispatch_with_marker(r.path(), home.path(), args, true);
    assert_eq!(refused.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&refused.stderr).contains(
        "do not pass --sandbox; only the user can loosen a run's sandbox or replace its checks"
    ));

    let allowed = blyte_dispatch_with_marker(r.path(), home.path(), args, false);
    let stderr = String::from_utf8_lossy(&allowed.stderr);
    assert!(
        !stderr.contains("only the user can loosen a run's sandbox or replace its checks"),
        "expected no sandbox refusal without marker, got: {stderr}"
    );
}
