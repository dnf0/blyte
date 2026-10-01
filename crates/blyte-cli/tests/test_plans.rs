//! Plans: a plan file's tasks, their dependencies and lanes, and each task's state from its
//! dispatches (2026-10-01).

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::{route, Sources};
use blyte_cli::plans::{collect_plans, parse_plan};

const PLAN: &str = "# Widget thing — Plan

Intro.

### Task 1: core parser

**Files:** `crates/widget/src/parse.rs`, `crates/widget/tests/p.rs`

Depends on: none

### Task 2: the CLI flag

**Files:** `crates/widget-cli/src/cli.rs`

Depends on: none
Lane: Interface

### Task 3: wire it together

**Files:** `crates/widget-cli/src/main.rs`

**Depends on:** Task 1, Task 2

### Task 4: docs

**Files:** `docs/usage.md`

Depends on: Task 1

## Decisions

- Parse first, then the flag.
";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn a_plan_file_gives_tasks_dependencies_lanes_and_decisions() {
    let doc = parse_plan(PLAN);
    assert_eq!(doc.title, "Widget thing");
    let ids: Vec<&str> = doc.tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["T1", "T2", "T3", "T4"]);
    assert_eq!(doc.tasks[0].heading, "Task 1: core parser");
    assert_eq!(doc.tasks[0].title, "core parser");
    assert_eq!(doc.tasks[2].deps, ["T1", "T2"]);
    assert_eq!(doc.tasks[3].deps, ["T1"]);
    let lanes: Vec<&str> = doc.tasks.iter().map(|t| t.lane.as_str()).collect();
    assert_eq!(lanes, ["widget", "Interface", "widget-cli", "docs"]);
    assert_eq!(
        doc.tasks[0].files,
        ["crates/widget/src/parse.rs", "crates/widget/tests/p.rs"]
    );
    assert_eq!(doc.decisions, ["Parse first, then the flag."]);
}

#[test]
fn task_states_come_from_their_dispatches() {
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("repos/app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
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
    write(
        &wt.join("W1.start.json"),
        &serde_json::json!({"id": "W1", "pid": 999999, "started_at": now - 600, "task_file": "/nonexistent",
            "gate": "true", "plan": "docs/plans/widget.md", "plan_task": "Task 1: core parser"}).to_string(),
    );
    write(&wt.join("W1.log"), "[Turn 20] \n");
    write(
        &wt.join("W1.report.json"),
        r#"{"outcome":"verified","cost_usd":0.5,"turns":20,"gate_attempts":1}"#,
    );
    write(
        &wt.join("W1.json"),
        &serde_json::json!({"commits": [{"sha": sha, "subject": "feat: parser"}], "changed_files": ["parse.rs"]}).to_string(),
    );
    write(
        &wt.join("W1.verdict.json"),
        r#"{"verdict":"accepted","notes":"to the plan","ts":1}"#,
    );
    write(
        &wt.join("W2.start.json"),
        &serde_json::json!({"id": "W2", "pid": std::process::id(), "started_at": now - 120, "task_file": "/nonexistent",
            "gate": "true", "plan": "docs/plans/widget.md", "plan_task": "Task 2: the CLI flag"}).to_string(),
    );
    write(&wt.join("W2.log"), "[Turn 3] \n● read cli.rs -> 10 lines\n");

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };
    let v = collect_plans(&src, now);
    let plan = &v["plans"][0];
    assert_eq!(plan["key"], "app/widget");
    assert_eq!(plan["title"], "Widget thing");
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
    assert_eq!(state("T1"), "merged");
    assert_eq!(state("T2"), "running");
    assert_eq!(state("T3"), "waiting");
    assert_eq!(state("T4"), "ready");
    assert_eq!(plan["stats"]["merged"], 1);
    assert_eq!(plan["stats"]["not_started"], 2);
    assert_eq!(plan["stats"]["max_parallel"], 2); // W1 ran until its report was written, now; W2 started 120 s ago
    assert!(plan["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["task"] == "T1" && e["text"] == "dispatched"));
    assert!(plan["decisions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["text"] == "to the plan"));

    let (code, ctype, _) = route("/api/plans", &src);
    assert_eq!((code, ctype), (200, "application/json"));
}
