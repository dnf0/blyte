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

const PLAN_P: &str = "# Multi-repo Plan\n\n### Task A: first task\n\nDepends on: none\n\n### Task B: second task\n\nDepends on: none\n\n### Task C: third task\n\nDepends on: none\n";

#[test]
fn test_1_multi_repo_plan_shares_one_plan_with_repo_lanes() {
    let t = tempfile::tempdir().unwrap();

    let specrepo = t.path().join("specrepo");
    std::fs::create_dir_all(&specrepo).unwrap();
    git(&specrepo, &["init", "-q", "-b", "main"]);
    write(&specrepo.join("README.md"), "# Spec Repo\n");
    git(&specrepo, &["add", "."]);
    git(&specrepo, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let plan_path = specrepo.join("docs/plans/p.md");
    write(&plan_path, PLAN_P);
    git(&specrepo, &["add", "."]);
    git(&specrepo, &["commit", "-qm", "add plan", "--no-gpg-sign"]);

    let r1 = t.path().join("repos/r1");
    std::fs::create_dir_all(&r1).unwrap();
    git(&r1, &["init", "-q", "-b", "main"]);
    write(&r1.join("README.md"), "# R1\n");
    git(&r1, &["add", "."]);
    git(&r1, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let r2 = t.path().join("repos/r2");
    std::fs::create_dir_all(&r2).unwrap();
    git(&r2, &["init", "-q", "-b", "main"]);
    write(&r2.join("README.md"), "# R2\n");
    git(&r2, &["add", "."]);
    git(&r2, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // r1 has a running job for Task A
    let wt1 = r1.join(".wt");
    write(
        &wt1.join("job-a.start.json"),
        &serde_json::json!({
            "id": "job-a",
            "pid": std::process::id(),
            "started_at": now - 300,
            "plan": plan_path.to_str().unwrap(),
            "plan_task": "Task A",
            "gate": "true",
            "task_file": "/nonexistent",
        })
        .to_string(),
    );
    write(&wt1.join("job-a.log"), "[Turn 1] \n");

    // r2 has a verified job for Task B
    let wt2 = r2.join(".wt");
    write(
        &wt2.join("job-b.start.json"),
        &serde_json::json!({
            "id": "job-b",
            "pid": 999999,
            "started_at": now - 200,
            "plan": plan_path.to_str().unwrap(),
            "plan_task": "Task B",
            "gate": "true",
            "task_file": "/nonexistent",
        })
        .to_string(),
    );
    write(&wt2.join("job-b.log"), "[Turn 2] \n");
    write(
        &wt2.join("job-b.report.json"),
        r#"{"outcome":"verified","cost_usd":0.2,"turns":2,"gate_attempts":1}"#,
    );

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let v = collect_plans(&src, now);
    let plans = v["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 1, "expected 1 plan, got: {v}");
    let plan = &plans[0];

    assert_eq!(plan["path"], plan_path.to_str().unwrap());
    assert_eq!(plan["repo"], "specrepo");
    assert_eq!(plan["repos"], serde_json::json!(["r1", "r2"]));

    let lanes = plan["lanes"].as_array().unwrap();
    assert_eq!(lanes.len(), 3);
    assert_eq!(lanes[0], serde_json::json!({"id": "r1", "label": "r1"}));
    assert_eq!(lanes[1], serde_json::json!({"id": "r2", "label": "r2"}));
    assert_eq!(
        lanes[2],
        serde_json::json!({"id": "not-dispatched", "label": "Not dispatched"})
    );

    let tasks = plan["tasks"].as_array().unwrap();
    let find_task = |id: &str| tasks.iter().find(|t| t["id"] == id).unwrap();

    let task_a = find_task("A");
    assert_eq!(task_a["lane"], "r1");
    assert_eq!(task_a["state"], "running");

    let task_b = find_task("B");
    assert_eq!(task_b["lane"], "r2");
    assert_eq!(task_b["state"], "verified");

    let task_c = find_task("C");
    assert_eq!(task_c["lane"], "not-dispatched");
    assert_eq!(task_c["state"], "ready");
}

#[test]
fn test_2_single_repo_dispatch_preserves_lanes_with_absolute_path() {
    let t = tempfile::tempdir().unwrap();

    let specrepo = t.path().join("specrepo");
    std::fs::create_dir_all(&specrepo).unwrap();
    git(&specrepo, &["init", "-q", "-b", "main"]);
    write(&specrepo.join("README.md"), "# Spec Repo\n");
    git(&specrepo, &["add", "."]);
    git(&specrepo, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let plan_path = specrepo.join("docs/plans/p.md");
    write(&plan_path, PLAN_P);
    git(&specrepo, &["add", "."]);
    git(&specrepo, &["commit", "-qm", "add plan", "--no-gpg-sign"]);

    let r1 = t.path().join("repos/r1");
    std::fs::create_dir_all(&r1).unwrap();
    git(&r1, &["init", "-q", "-b", "main"]);
    write(&r1.join("README.md"), "# R1\n");
    git(&r1, &["add", "."]);
    git(&r1, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let wt1 = r1.join(".wt");
    write(
        &wt1.join("job-a.start.json"),
        &serde_json::json!({
            "id": "job-a",
            "pid": std::process::id(),
            "started_at": now - 300,
            "plan": plan_path.to_str().unwrap(),
            "plan_task": "Task A",
            "gate": "true",
            "task_file": "/nonexistent",
        })
        .to_string(),
    );
    write(&wt1.join("job-a.log"), "[Turn 1] \n");

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let v = collect_plans(&src, now);
    let plans = v["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 1, "expected 1 plan, got: {v}");
    let plan = &plans[0];

    assert_eq!(plan["path"], plan_path.to_str().unwrap());
    assert_eq!(plan["repo"], "specrepo");
    assert_eq!(plan["repos"], serde_json::json!(["r1"]));

    assert_eq!(
        plan["lanes"],
        serde_json::json!([{"id": "tasks", "label": "Tasks"}])
    );
}

#[test]
fn test_3_internal_plan_output_unchanged() {
    let t = tempfile::tempdir().unwrap();

    let r1 = t.path().join("repos/r1");
    std::fs::create_dir_all(&r1).unwrap();
    git(&r1, &["init", "-q", "-b", "main"]);
    write(&r1.join("README.md"), "# R1\n");

    let plan_rel = "docs/plans/q.md";
    write(
        &r1.join(plan_rel),
        "# Q Plan\n\n### Task 1: do q\n\nDepends on: none\n",
    );
    git(&r1, &["add", "."]);
    git(&r1, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let wt1 = r1.join(".wt");
    write(
        &wt1.join("W1.start.json"),
        &serde_json::json!({
            "id": "W1",
            "pid": std::process::id(),
            "started_at": now - 300,
            "plan": plan_rel,
            "plan_task": "Task 1",
            "gate": "true",
            "task_file": "/nonexistent",
        })
        .to_string(),
    );
    write(&wt1.join("W1.log"), "[Turn 1] \n");

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let v = collect_plans(&src, now);
    let plans = v["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 1, "expected 1 plan, got: {v}");
    let plan = &plans[0];

    assert_eq!(plan["key"], "r1/q");
    assert_eq!(plan["repo"], "r1");
    assert_eq!(plan["repos"], serde_json::json!(["r1"]));
    assert_eq!(plan["path"], r1.join(plan_rel).to_str().unwrap());
}

#[test]
fn test_4_running_job_without_plan_is_one_task_plan() {
    let t = tempfile::tempdir().unwrap();

    let r1 = t.path().join("repos/r1");
    std::fs::create_dir_all(&r1).unwrap();
    git(&r1, &["init", "-q", "-b", "main"]);
    write(&r1.join("README.md"), "# R1\n");
    git(&r1, &["add", "."]);
    git(&r1, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let task_file = t.path().join("task-j1.md");
    write(
        &task_file,
        "# Implement special feature\nSome detailed description\n",
    );

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let wt1 = r1.join(".wt");
    write(
        &wt1.join("J1.start.json"),
        &serde_json::json!({
            "id": "J1",
            "pid": std::process::id(),
            "started_at": now - 300,
            "task_file": task_file.to_str().unwrap(),
            "gate": "true",
        })
        .to_string(),
    );
    write(&wt1.join("J1.log"), "[Turn 1] \n");

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let v = collect_plans(&src, now);
    let plans = v["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 1, "expected 1 plan, got: {v}");
    let plan = &plans[0];

    assert_eq!(plan["key"], "r1/J1");
    assert_eq!(plan["repo"], "r1");
    assert_eq!(plan["repos"], serde_json::json!(["r1"]));
    assert_eq!(
        plan["lanes"],
        serde_json::json!([{"id": "tasks", "label": "Tasks"}])
    );

    let tasks = plan["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task["id"], "J1");
    assert_eq!(task["title"], "Implement special feature");
    assert_eq!(task["state"], "running");
}

#[test]
fn test_5_research_job_creates_no_plan() {
    let t = tempfile::tempdir().unwrap();

    let r1 = t.path().join("repos/r1");
    std::fs::create_dir_all(&r1).unwrap();
    git(&r1, &["init", "-q", "-b", "main"]);
    write(&r1.join("README.md"), "# R1\n");
    git(&r1, &["add", "."]);
    git(&r1, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let task_file = t.path().join("task-j1.md");
    write(
        &task_file,
        "# Implement special feature\nSome detailed description\n",
    );

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let wt1 = r1.join(".wt");
    write(
        &wt1.join("J1.start.json"),
        &serde_json::json!({
            "id": "J1",
            "pid": std::process::id(),
            "started_at": now - 300,
            "task_file": task_file.to_str().unwrap(),
            "gate": "true",
            "research": true,
        })
        .to_string(),
    );
    write(&wt1.join("J1.log"), "[Turn 1] \n");

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let v = collect_plans(&src, now);
    let plans = v["plans"].as_array().unwrap();
    assert!(
        plans.is_empty(),
        "expected no plan for research run, got: {plans:?}"
    );
}
