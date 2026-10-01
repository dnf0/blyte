//! Plans as real plan files write them: example plans in code fences, `Task 0 (HUMAN)` headings,
//! backticked titles (2026-10-01).

use blyte_cli::plans::parse_plan;

const PLAN: &str = "# Real thing — Plan

### Task 0 (HUMAN): bump the version

Depends on: none

### Task 1: `blyte dispatch --plan`

**Files:** `crates/blyte-cli/src/cli.rs`

Depends on: Task 0

```rust
const EXAMPLE: &str = \"
### Task 9: not a real task
Lane: Interface
Depends on: Task 1
\";
```

### Task 2: the collector

**Files:** `crates/blyte-cli/src/plans.rs`

Depends on: Task 1
";

#[test]
fn fences_human_tasks_and_backticks_are_handled() {
    let doc = parse_plan(PLAN);
    let ids: Vec<&str> = doc.tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["T0", "T1", "T2"]);
    assert_eq!(doc.tasks[0].title, "(HUMAN) bump the version");
    assert_eq!(doc.tasks[1].title, "blyte dispatch --plan");
    assert_eq!(doc.tasks[1].heading, "Task 1: `blyte dispatch --plan`");
    assert_eq!(doc.tasks[1].deps, ["T0"]);
    assert_eq!(doc.tasks[1].lane, "blyte-cli");
    assert_eq!(doc.tasks[2].deps, ["T1"]);
}

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

fn write_file(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn squash_merged_task_is_marked_merged() {
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("repos/app");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    write_file(&repo.join("docs/plans/real.md"), PLAN);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "plan", "--no-gpg-sign"]);

    // Create wip/W1 branch and commit work there
    git(&repo, &["checkout", "-q", "-b", "wip/W1"]);
    write_file(&repo.join("crates/blyte-cli/src/cli.rs"), "fn main() {}\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "feat: cli", "--no-gpg-sign"]);
    let wip_sha = git(&repo, &["rev-parse", "HEAD"]);

    // Squash merge into main
    git(&repo, &["checkout", "-q", "main"]);
    git(&repo, &["merge", "--squash", "wip/W1"]);
    git(&repo, &["commit", "-qm", "squash: cli", "--no-gpg-sign"]);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let wt = repo.join(".wt");
    write_file(
        &wt.join("W1.start.json"),
        &serde_json::json!({
            "id": "W1",
            "pid": 999999,
            "started_at": now - 600,
            "task_file": "/nonexistent",
            "gate": "true",
            "plan": "docs/plans/real.md",
            "plan_task": "Task 1: `blyte dispatch --plan`"
        })
        .to_string(),
    );
    write_file(&wt.join("W1.log"), "[Turn 10] \n");
    write_file(
        &wt.join("W1.report.json"),
        r#"{"outcome":"verified","cost_usd":0.5,"turns":10,"gate_attempts":1}"#,
    );
    write_file(
        &wt.join("W1.json"),
        &serde_json::json!({
            "commits": [{"sha": wip_sha, "subject": "feat: cli"}],
            "changed_files": ["crates/blyte-cli/src/cli.rs"]
        })
        .to_string(),
    );
    write_file(
        &wt.join("W1.verdict.json"),
        r#"{"verdict":"accepted","notes":"LGTM","ts":1}"#,
    );

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    // Before wip/W1 is deleted, the branch still exists, so task is "verified"
    let v1 = collect_plans(&src, now);
    let plan1 = &v1["plans"][0];
    let task1 = plan1["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "T1")
        .unwrap();
    assert_eq!(task1["state"], "verified");

    // After deleting wip/W1, with accepted verdict, it is recognized as merged
    git(&repo, &["branch", "-D", "wip/W1"]);

    let v2 = collect_plans(&src, now);
    let plan2 = &v2["plans"][0];
    let task2 = plan2["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "T1")
        .unwrap();
    assert_eq!(task2["state"], "merged");

    // Also verify "fixed" verdict is recognized as merged when wip branch is gone
    write_file(
        &wt.join("W1.verdict.json"),
        r#"{"verdict":"fixed","notes":"fixed bug","ts":2}"#,
    );
    let v3 = collect_plans(&src, now);
    let task3 = &v3["plans"][0]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "T1")
        .unwrap();
    assert_eq!(task3["state"], "merged");

    // A verdict other than accepted or fixed (e.g. rejected) is not treated as merged
    write_file(
        &wt.join("W1.verdict.json"),
        r#"{"verdict":"rejected","notes":"rejected","ts":3}"#,
    );
    let v4 = collect_plans(&src, now);
    let task4 = &v4["plans"][0]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == "T1")
        .unwrap();
    assert_eq!(task4["state"], "verified");
}
