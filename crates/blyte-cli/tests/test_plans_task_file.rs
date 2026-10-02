//! Plans view: a dispatched plan file is its own plan, wherever it lives.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::Sources;
use blyte_cli::plans::collect_plans;

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
fn plan_from_sibling_worktree_task_file() {
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("repos/app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    write(&repo.join("README.md"), "# App\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "initial", "--no-gpg-sign"]);

    let side = t.path().join("repos/app--side");
    let plan_path = side.join("docs/superpowers/plans/2026-10-01-x.md");
    let plan_content = "# Feature X — Plan

Intro.

### Task S1: first

**Files:** `first.rs`

Depends on: none

### Task S2: second

**Files:** `second.rs`

Depends on: Task S1
";
    write(&plan_path, plan_content);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let wt = repo.join(".wt");

    write(
        &wt.join("W1.start.json"),
        &serde_json::json!({
            "id": "W1",
            "pid": 999999,
            "started_at": now - 300,
            "task_file": plan_path.to_str().unwrap(),
            "gate": "true",
            "plan_task": "Task S1"
        })
        .to_string(),
    );
    write(&wt.join("W1.log"), "[Turn 5] \n");
    write(
        &wt.join("W1.report.json"),
        r#"{"outcome":"verified","cost_usd":0.3,"turns":5,"gate_attempts":1}"#,
    );

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let v = collect_plans(&src, now);
    let plans = v["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 1);

    let plan = &plans[0];
    assert_eq!(plan["key"], "app/2026-10-01-x");

    let tasks = plan["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2);

    let s1 = &tasks[0];
    assert_eq!(s1["id"], "S1");
    assert_eq!(s1["title"], "first");
    assert_eq!(s1["dispatch"], "app/W1");

    let s2 = &tasks[1];
    assert_eq!(s2["id"], "S2");
    assert_eq!(s2["title"], "second");
    assert!(s2["dispatch"].is_null());

    // Also assert that a run whose task_file is an ordinary task file creates no plan
    let ordinary_file = t.path().join("task-S1.md");
    write(&ordinary_file, "# Ordinary task\nSome instructions\n");

    write(
        &wt.join("W2.start.json"),
        &serde_json::json!({
            "id": "W2",
            "pid": 999999,
            "started_at": now - 100,
            "task_file": ordinary_file.to_str().unwrap(),
            "gate": "true",
            "plan_task": "Task S1"
        })
        .to_string(),
    );
    write(&wt.join("W2.log"), "[Turn 2] \n");
    write(
        &wt.join("W2.report.json"),
        r#"{"outcome":"verified","cost_usd":0.1,"turns":2,"gate_attempts":1}"#,
    );

    let v2 = collect_plans(&src, now);
    let plans2 = v2["plans"].as_array().unwrap();
    assert_eq!(
        plans2.len(),
        1,
        "ordinary task file must not create a new plan"
    );
    assert_eq!(plans2[0]["key"], "app/2026-10-01-x");
}

#[test]
fn ordinary_task_file_creates_no_plan() {
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("repos/app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    write(&repo.join("README.md"), "# App\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "initial", "--no-gpg-sign"]);

    let ordinary_file = t.path().join("task-S1.md");
    write(&ordinary_file, "# Ordinary task\nSome instructions\n");

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let wt = repo.join(".wt");

    write(
        &wt.join("W1.start.json"),
        &serde_json::json!({
            "id": "W1",
            "pid": 999999,
            "started_at": now - 300,
            "task_file": ordinary_file.to_str().unwrap(),
            "gate": "true",
            "plan_task": "Task S1"
        })
        .to_string(),
    );
    write(&wt.join("W1.log"), "[Turn 2] \n");

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let v = collect_plans(&src, now);
    let plans = v["plans"].as_array().unwrap();
    assert!(plans.is_empty(), "expected no plan, got: {plans:?}");
}
