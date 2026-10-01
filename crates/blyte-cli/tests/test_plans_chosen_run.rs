//! Plans view: show the run that counts for a task, and an honest gate line (2026-10-01).

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::Sources;
use blyte_cli::plans::collect_plans;

const PLAN: &str = "# Widget thing — Plan

Intro.

### Task 1: core parser

**Files:** `crates/widget/src/parse.rs`

Depends on: none
";

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
fn merged_job_is_chosen_over_newer_redone_job() {
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("repos/app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    write(&repo.join("docs/plans/widget.md"), PLAN);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "plan", "--no-gpg-sign"]);

    write(&repo.join("parse.rs"), "fn p() {}\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "feat: parser", "--no-gpg-sign"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let wt = repo.join(".wt");

    // Older verified job whose commit is on main (merged)
    write(
        &wt.join("W1.start.json"),
        &serde_json::json!({
            "id": "W1",
            "pid": 999999,
            "started_at": now - 600,
            "task_file": "/nonexistent",
            "gate": "true",
            "plan": "docs/plans/widget.md",
            "plan_task": "Task 1: core parser"
        })
        .to_string(),
    );
    write(&wt.join("W1.log"), "[Turn 20] \n");
    write(
        &wt.join("W1.report.json"),
        r#"{"outcome":"verified","cost_usd":0.5,"turns":20,"gate_attempts":1}"#,
    );
    write(
        &wt.join("W1.json"),
        &serde_json::json!({
            "commits": [{"sha": sha, "subject": "feat: parser"}],
            "changed_files": ["parse.rs"]
        })
        .to_string(),
    );

    // Newer job with verdict redone
    write(
        &wt.join("W2.start.json"),
        &serde_json::json!({
            "id": "W2",
            "pid": 999999,
            "started_at": now - 120,
            "task_file": "/nonexistent",
            "gate": "true",
            "plan": "docs/plans/widget.md",
            "plan_task": "Task 1: core parser"
        })
        .to_string(),
    );
    write(&wt.join("W2.log"), "[Turn 15] \n");
    write(
        &wt.join("W2.report.json"),
        r#"{"outcome":"verified","cost_usd":0.4,"turns":15,"gate_attempts":1}"#,
    );
    write(
        &wt.join("W2.verdict.json"),
        r#"{"verdict":"redone","notes":"rewriting it","ts":1}"#,
    );

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };
    let v = collect_plans(&src, now);
    let plan = &v["plans"][0];
    let task = plan["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "T1")
        .unwrap();
    assert_eq!(task["state"], "merged");
    assert_eq!(task["dispatch"], "app/W1");
}

#[test]
fn accepted_job_is_chosen_over_newer_failed_job() {
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("repos/app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    write(&repo.join("docs/plans/widget.md"), PLAN);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "plan", "--no-gpg-sign"]);

    // Work committed only on wip branch, not on main
    git(&repo, &["checkout", "-q", "-b", "wip/W1"]);
    write(&repo.join("parse.rs"), "fn p() {}\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "feat: parser", "--no-gpg-sign"]);
    let wip_sha = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "-q", "main"]);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let wt = repo.join(".wt");

    // Older job with verdict accepted, but not merged (wip/W1 branch exists)
    write(
        &wt.join("W1.start.json"),
        &serde_json::json!({
            "id": "W1",
            "pid": 999999,
            "started_at": now - 600,
            "task_file": "/nonexistent",
            "gate": "true",
            "plan": "docs/plans/widget.md",
            "plan_task": "Task 1: core parser"
        })
        .to_string(),
    );
    write(&wt.join("W1.log"), "[Turn 10] \n");
    write(
        &wt.join("W1.report.json"),
        r#"{"outcome":"verified","cost_usd":0.5,"turns":10,"gate_attempts":1}"#,
    );
    write(
        &wt.join("W1.json"),
        &serde_json::json!({
            "commits": [{"sha": wip_sha, "subject": "feat: parser"}],
            "changed_files": ["parse.rs"]
        })
        .to_string(),
    );
    write(
        &wt.join("W1.verdict.json"),
        r#"{"verdict":"accepted","notes":"looks good","ts":1}"#,
    );

    // Newer failed job
    write(
        &wt.join("W2.start.json"),
        &serde_json::json!({
            "id": "W2",
            "pid": 999999,
            "started_at": now - 120,
            "task_file": "/nonexistent",
            "gate": "true",
            "plan": "docs/plans/widget.md",
            "plan_task": "Task 1: core parser"
        })
        .to_string(),
    );
    write(&wt.join("W2.log"), "[Turn 5] \n");
    write(
        &wt.join("W2.report.json"),
        r#"{"outcome":"failed","cost_usd":0.2,"turns":5,"gate_attempts":1}"#,
    );

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };
    let v = collect_plans(&src, now);
    let plan = &v["plans"][0];
    let task = plan["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "T1")
        .unwrap();
    assert_eq!(task["dispatch"], "app/W1");
    assert_eq!(task["state"], "verified");
}

#[test]
fn gate_line_in_task_detail() {
    let page = include_str!("../assets/dashboard/index.html");
    assert!(page.contains("passed on the first attempt"));
    assert!(page.contains(r#"gateText = "passed on attempt " + t.gate_attempts + " (" + (t.gate_attempts - 1) + " failed)""#));
    assert!(!page.contains(r#"gateText = t.gate_attempts + " of " + t.gate_attempts + " passed""#));
}
