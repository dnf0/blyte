//! Plans view: show follow-up tasks dispatched against a plan but not written in it.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::Sources;
use blyte_cli::plans::collect_plans;

const PLAN: &str = "# Widget thing — Plan

Intro.

### Task 1: a

**Files:** `parse.rs`

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
fn follow_up_task_shows_as_added_during_the_work() {
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

    // Job for Task 1
    write(
        &wt.join("W1.start.json"),
        &serde_json::json!({
            "id": "W1",
            "pid": 999999,
            "started_at": now - 600,
            "task_file": "/nonexistent",
            "gate": "true",
            "plan": "docs/plans/widget.md",
            "plan_task": "Task 1: a"
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
            "commits": [{"sha": sha, "subject": "feat: parser"}],
            "changed_files": ["parse.rs"]
        })
        .to_string(),
    );

    // Job dispatched with the plan (start_plan) and plan_task "Task H2: pin signature"
    write(
        &wt.join("W2.start.json"),
        &serde_json::json!({
            "id": "W2",
            "pid": 999999,
            "started_at": now - 300,
            "task_file": "/nonexistent",
            "gate": "true",
            "plan": "docs/plans/widget.md",
            "plan_task": "Task H2: pin signature"
        })
        .to_string(),
    );
    write(&wt.join("W2.log"), "[Turn 5] \n");
    write(
        &wt.join("W2.report.json"),
        r#"{"outcome":"verified","cost_usd":0.3,"turns":5,"gate_attempts":1}"#,
    );

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let v = collect_plans(&src, now);
    let plan = &v["plans"][0];
    let tasks = plan["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2);

    let t1 = &tasks[0];
    assert_eq!(t1["id"], "T1");
    assert_eq!(t1["added"], false);

    let h2 = &tasks[1];
    assert_eq!(h2["id"], "H2");
    assert_eq!(h2["title"], "pin signature");
    assert_eq!(h2["heading"], "Task H2: pin signature");
    assert_eq!(h2["lane"], "added");
    assert_eq!(h2["added"], true);

    let lanes = plan["lanes"].as_array().unwrap();
    assert!(
        lanes
            .iter()
            .any(|l| l["id"] == "added" && l["label"] == "Added during the work"),
        "lanes must include added lane: {lanes:?}"
    );

    // Add a job whose task text only mentions the plan path in backticks with a non-matching plan_task
    let task_path = repo.join("tasks/other.md");
    write(
        &task_path,
        "Fix something related to `docs/plans/widget.md` in passing.\n",
    );
    write(
        &wt.join("W3.start.json"),
        &serde_json::json!({
            "id": "W3",
            "pid": 999999,
            "started_at": now - 100,
            "task_file": task_path.to_str().unwrap(),
            "gate": "true",
            "plan_task": "Task Other: not a plan task"
        })
        .to_string(),
    );
    write(&wt.join("W3.log"), "[Turn 3] \n");
    write(
        &wt.join("W3.report.json"),
        r#"{"outcome":"verified","cost_usd":0.1,"turns":3,"gate_attempts":1}"#,
    );

    let v2 = collect_plans(&src, now);
    let plan2 = &v2["plans"][0];
    let tasks2 = plan2["tasks"].as_array().unwrap();
    assert_eq!(
        tasks2.len(),
        2,
        "job grouped only via backticks must not add a task"
    );
}

#[test]
fn plans_view_added_task_ui_elements() {
    let page = include_str!("../assets/dashboard/index.html");
    assert!(
        page.contains("Added during the work: dispatched with this plan but not written in it.")
    );
    assert!(page.contains(r#">added</text>"#));
}
