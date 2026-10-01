//! Plans are found from what dispatches already record: a plan tag, a backticked path, the old
//! runs' Plan: line, with the plan file only on a feature branch or in a removed worktree
//! (2026-10-01).

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::Sources;
use blyte_cli::plans::collect_plans;

const PLAN: &str = "# Widget thing — Plan\n\n### Task 1: core parser\n\n**Files:** `crates/widget/src/parse.rs`\n\nDepends on: none\n\n### Task 2: the CLI flag\n\n**Files:** `crates/widget-cli/src/cli.rs`\n\nDepends on: Task 1\n";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn plans_are_found_from_tags_paths_and_old_runs() {
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("repos/app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    write(&repo.join("README.md"), "app\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "init", "--no-gpg-sign"]);
    // The plan and the first task's work exist only on a feature branch.
    git(&repo, &["checkout", "-q", "-b", "feat/widget"]);
    write(&repo.join("docs/plans/widget.md"), PLAN);
    write(&repo.join("parse.rs"), "fn p() {}\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "feat: parser", "--no-gpg-sign"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "main"]);
    assert!(!repo.join("docs/plans/widget.md").exists());

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // A blyte dispatch linked only by its tag and the task text.
    write(
        &repo.join("task-w1.md"),
        "Implement **Task 1** of the plan `docs/plans/widget.md`, exactly as written.\n",
    );
    let wt = repo.join(".wt");
    write(
        &wt.join("W1.start.json"),
        &serde_json::json!({"id": "W1", "pid": 999999, "started_at": now - 900,
            "task_file": repo.join("task-w1.md").to_string_lossy(), "gate": "true", "tags": ["plan: widget"]}).to_string(),
    );
    write(&wt.join("W1.log"), "[Turn 9] \n");
    write(
        &wt.join("W1.report.json"),
        r#"{"outcome":"verified","cost_usd":0.4,"turns":9,"gate_attempts":1}"#,
    );
    write(
        &wt.join("W1.json"),
        &serde_json::json!({"commits": [{"sha": sha, "subject": "feat: parser"}], "changed_files": ["parse.rs"]}).to_string(),
    );

    // An old runs-dir run whose worktree has since been removed.
    let runs = t.path().join("runs/app");
    let gone = repo.join(".wt/R7");
    write(&runs.join("R7.worktree"), &format!("{}\n", gone.display()));
    write(
        &runs.join("R7.prompt"),
        &format!("You are implementing exactly ONE task.\nPlan: {}/docs/plans/widget.md\nTask: \"Task 2: the CLI flag\" ONLY.\n", gone.display()),
    );
    write(&runs.join("R7.log"), "[Turn 5] \n");
    write(
        &runs.join("R7.report.json"),
        r#"{"outcome":"verified","cost_usd":0.2,"turns":5,"gate_attempts":1}"#,
    );

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![t.path().join("runs")],
        hours: 24,
    };
    let v = collect_plans(&src, now);
    let plans = v["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 1, "{v}");
    let plan = &plans[0];
    assert_eq!(plan["key"], "app/widget");
    let state = |id: &str| {
        plan["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == id)
            .unwrap()["state"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        state("T1"),
        "merged",
        "on feat/widget, which is not a wip/ branch"
    );
    assert_eq!(state("T2"), "verified");
}
