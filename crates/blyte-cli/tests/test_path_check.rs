use std::path::Path;
use std::process::Command;

use blyte_cli::dispatch::unreadable_paths;

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

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_ENV_FILE")
        .env("HOME", dir)
        .output()
        .unwrap()
}

#[test]
fn unreadable_paths_filters_and_trims() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = tmp.path().join("checkout");
    let read_root = tmp.path().join("allowed");
    let outside_dir = tmp.path().join("outside");
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::create_dir_all(&read_root).unwrap();
    std::fs::create_dir_all(&outside_dir).unwrap();

    let in_checkout = checkout.join("inside.rs");
    let in_root = read_root.join("lib.rs");
    let out_a = outside_dir.join("a.rs");
    let out_b = outside_dir.join("b.rs");
    let out_c = outside_dir.join("c.rs");
    let missing = outside_dir.join("does_not_exist.rs");
    std::fs::write(&in_checkout, "x").unwrap();
    std::fs::write(&in_root, "x").unwrap();
    std::fs::write(&out_a, "x").unwrap();
    std::fs::write(&out_b, "x").unwrap();
    std::fs::write(&out_c, "x").unwrap();

    let text = format!(
        "Read `{}` and ({}) and {}. Skip {}, {}, and `{}`.",
        out_a.display(),
        out_b.display(),
        out_c.display(),
        in_checkout.display(),
        in_root.display(),
        missing.display(),
    );

    let got = unreadable_paths(&text, &[checkout, read_root]);
    assert_eq!(got, vec![out_a, out_b, out_c]);
}

#[test]
fn research_dispatch_refuses_unreadable_paths_until_allowed_or_skipped() {
    let r = repo();
    let other = tempfile::tempdir().unwrap();
    let target = other.path().join("mod.rs");
    std::fs::write(&target, "pub fn x() {}\n").unwrap();
    std::fs::write(
        r.path().join("q.md"),
        format!("Compare `{}` with f.txt.\n", target.display()),
    )
    .unwrap();

    let refused = blyte(
        r.path(),
        &["dispatch", "Q1", "q.md", "--research", "--sandbox", "off"],
    );
    assert_eq!(refused.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("blyte: the task names paths this run cannot read:"),
        "{stderr}"
    );
    assert!(stderr.contains(&target.display().to_string()), "{stderr}");
    assert!(
        stderr.contains("Add --allow-read <dir> for each (or --no-path-check to start anyway)."),
        "{stderr}"
    );
    assert!(
        !r.path().join(".wt/Q1").exists(),
        "worktree should not be created when refused"
    );

    let allowed = blyte(
        r.path(),
        &[
            "dispatch",
            "Q1",
            "q.md",
            "--research",
            "--sandbox",
            "off",
            "--allow-read",
            other.path().to_str().unwrap(),
        ],
    );
    let allowed_err = String::from_utf8_lossy(&allowed.stderr);
    assert!(
        !allowed_err.contains("cannot read"),
        "expected no path refusal with --allow-read, got: {allowed_err}"
    );
    assert!(r.path().join(".wt/Q1").exists());

    let skipped = blyte(
        r.path(),
        &[
            "dispatch",
            "Q2",
            "q.md",
            "--research",
            "--sandbox",
            "off",
            "--no-path-check",
        ],
    );
    let skipped_err = String::from_utf8_lossy(&skipped.stderr);
    assert!(
        !skipped_err.contains("cannot read"),
        "expected no path refusal with --no-path-check, got: {skipped_err}"
    );
    assert!(r.path().join(".wt/Q2").exists());
}

#[test]
fn non_research_dispatch_warns_and_continues() {
    let r = repo();
    let other = tempfile::tempdir().unwrap();
    let target = other.path().join("ext.rs");
    std::fs::write(&target, "pub fn y() {}\n").unwrap();
    std::fs::write(
        r.path().join("task.md"),
        format!("Check `{}` and update f.txt.\n", target.display()),
    )
    .unwrap();

    let out = blyte(
        r.path(),
        &[
            "dispatch",
            "T1",
            "task.md",
            "--gate",
            "true",
            "--sandbox",
            "off",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("blyte: the task names paths this run cannot read:"),
        "{stderr}"
    );
    assert!(
        r.path().join(".wt/T1").exists(),
        "non-research dispatch should warn and still create worktree"
    );
}

#[test]
fn review_flags_refused_reads_in_checks() {
    let r = repo();
    git(
        r.path(),
        &["worktree", "add", "-q", "-b", "wip/R1", ".wt/R1"],
    );
    std::fs::write(r.path().join("q.md"), "question\n").unwrap();
    std::fs::write(
        r.path().join(".wt/R1.start.json"),
        serde_json::json!({ "task_file": "q.md" }).to_string(),
    )
    .unwrap();
    std::fs::write(
        r.path().join(".wt/R1.log"),
        "\
[Turn 1]
\x1b[31m● read /outside/one.rs -> path escapes the workspace: /outside/one.rs. This run can read the workspace. Do not look for another copy: if the task needs this path, say in your answer that it was not readable.\x1b[0m
\x1b[31m● read /outside/one.rs -> path escapes the workspace: /outside/one.rs. This run can read the workspace. Do not look for another copy: if the task needs this path, say in your answer that it was not readable.\x1b[0m
\x1b[31m● grep /outside/two -> path escapes the workspace: /outside/two. This run can read the workspace. Do not look for another copy: if the task needs this path, say in your answer that it was not readable.\x1b[0m
\x1b[31m● read /outside/three.rs -> path escapes the workspace: /outside/three.rs\x1b[0m
\x1b[31m● read /outside/four.rs -> path escapes the workspace: /outside/four.rs\x1b[0m
",
    )
    .unwrap();

    let out = blyte(r.path(), &["review", "R1"]);
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(
            "! refused reads: 5 (outside the workspace and --allow-read): /outside/one.rs, /outside/two, /outside/three.rs"
        ),
        "{stdout}"
    );
}

#[test]
fn unreadable_paths_ignores_system_paths() {
    let text = "run /usr/bin/env, write to /tmp and read /etc/hosts or /dev/null";
    assert!(unreadable_paths(text, &[]).is_empty());
}
