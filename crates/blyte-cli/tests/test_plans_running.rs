//! Tests for Task DB1: the Plans view answers quickly (2026-10-07).
//!
//! 1. /api/plans?running=1 builds and returns only plans with at least one running task.
//! 2. Unfiltered /api/plans returns all plans.
//! 3. is_merged cache avoids redundant git calls for the same (root, branch, head).
//! 4. index.html polls /api/plans?running=1 and replaces setInterval with setTimeout.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::{self, Sources};
use blyte_cli::plans::{
    clear_is_merged_cache, collect_plans, collect_plans_filtered, collect_plans_with_options,
    is_merged, is_merged_git_call_count, reset_is_merged_git_call_count, CollectPlansOptions,
};

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
fn running_only_returns_exactly_the_running_plan_and_equals_unfiltered_entry() {
    let t = tempfile::tempdir().unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Repo 1: Plan 1 has a running job (live PID)
    let r1 = t.path().join("repos/r1");
    std::fs::create_dir_all(&r1).unwrap();
    git(&r1, &["init", "-q", "-b", "main"]);
    write(
        &r1.join("docs/plans/plan1.md"),
        "# Plan 1\n\n### Task 1.1: Work\nDo work.\n",
    );
    git(&r1, &["add", "."]);
    git(&r1, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let wt1 = r1.join(".wt");
    write(
        &wt1.join("job-1.start.json"),
        &serde_json::json!({
            "id": "job-1",
            "pid": std::process::id(), // live PID => running
            "started_at": now - 300,
            "plan": "docs/plans/plan1.md",
            "plan_task": "Task 1.1",
            "gate": "true",
            "task_file": "/nonexistent",
        })
        .to_string(),
    );
    write(&wt1.join("job-1.log"), "[Turn 1] running\n");

    // Repo 2: Plan 2 has a finished job (dead PID + verified report)
    let r2 = t.path().join("repos/r2");
    std::fs::create_dir_all(&r2).unwrap();
    git(&r2, &["init", "-q", "-b", "main"]);
    write(
        &r2.join("docs/plans/plan2.md"),
        "# Plan 2\n\n### Task 2.1: Work\nDo work.\n",
    );
    git(&r2, &["add", "."]);
    git(&r2, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let wt2 = r2.join(".wt");
    write(
        &wt2.join("job-2.start.json"),
        &serde_json::json!({
            "id": "job-2",
            "pid": 999999, // dead PID
            "started_at": now - 200,
            "plan": "docs/plans/plan2.md",
            "plan_task": "Task 2.1",
            "gate": "true",
            "task_file": "/nonexistent",
        })
        .to_string(),
    );
    write(&wt2.join("job-2.log"), "[Turn 1] done\n");
    write(
        &wt2.join("job-2.report.json"),
        r#"{"outcome":"verified","cost_usd":0.15,"turns":1,"gate_attempts":1}"#,
    );

    // Repo 3: Plan 3 has a finished job (dead PID + verified report)
    let r3 = t.path().join("repos/r3");
    std::fs::create_dir_all(&r3).unwrap();
    git(&r3, &["init", "-q", "-b", "main"]);
    write(
        &r3.join("docs/plans/plan3.md"),
        "# Plan 3\n\n### Task 3.1: Work\nDo work.\n",
    );
    git(&r3, &["add", "."]);
    git(&r3, &["commit", "-qm", "init", "--no-gpg-sign"]);

    let wt3 = r3.join(".wt");
    write(
        &wt3.join("job-3.start.json"),
        &serde_json::json!({
            "id": "job-3",
            "pid": 999999, // dead PID
            "started_at": now - 100,
            "plan": "docs/plans/plan3.md",
            "plan_task": "Task 3.1",
            "gate": "true",
            "task_file": "/nonexistent",
        })
        .to_string(),
    );
    write(&wt3.join("job-3.log"), "[Turn 1] done\n");
    write(
        &wt3.join("job-3.report.json"),
        r#"{"outcome":"verified","cost_usd":0.25,"turns":2,"gate_attempts":1}"#,
    );

    let src = Sources {
        roots: vec![t.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    // 1. collect_plans with running-only option returns exactly the running one
    let filtered_v = collect_plans_filtered(&src, now, true);
    let filtered_plans = filtered_v["plans"].as_array().expect("array of plans");
    assert_eq!(
        filtered_plans.len(),
        1,
        "running-only should return exactly 1 plan: {filtered_v:#?}"
    );
    assert_eq!(filtered_plans[0]["title"], "Plan 1");
    assert_eq!(filtered_plans[0]["stats"]["running"], 1);

    // Also test collect_plans_with_options helper
    let opts_v = collect_plans_with_options(&src, now, CollectPlansOptions { running_only: true });
    assert_eq!(filtered_v["plans"], opts_v["plans"]);

    // Also test HTTP route /api/plans?running=1
    let (code_f, ctype_f, body_f) = dashboard::route("/api/plans?running=1", &src);
    assert_eq!(code_f, 200);
    assert_eq!(ctype_f, "application/json");
    let route_filtered: serde_json::Value = serde_json::from_slice(&body_f).unwrap();
    let route_filtered_plans = route_filtered["plans"].as_array().expect("array of plans");
    assert_eq!(route_filtered_plans.len(), 1);
    assert_eq!(route_filtered_plans[0]["title"], "Plan 1");

    // 2. The unfiltered call still returns all three
    let unfiltered_v = collect_plans(&src, now);
    let unfiltered_plans = unfiltered_v["plans"].as_array().expect("array of plans");
    assert_eq!(
        unfiltered_plans.len(),
        3,
        "unfiltered should return all 3 plans: {unfiltered_v:#?}"
    );

    // Also test HTTP route /api/plans without running=1
    let (code_u, ctype_u, body_u) = dashboard::route("/api/plans", &src);
    assert_eq!(code_u, 200);
    assert_eq!(ctype_u, "application/json");
    let route_unfiltered: serde_json::Value = serde_json::from_slice(&body_u).unwrap();
    let route_unfiltered_plans = route_unfiltered["plans"]
        .as_array()
        .expect("array of plans");
    assert_eq!(route_unfiltered_plans.len(), 3);

    // 3. The running-only plan JSON equals that plan's entry from the unfiltered call
    let matching_unfiltered_entry = unfiltered_plans
        .iter()
        .find(|p| p["key"] == filtered_plans[0]["key"])
        .expect("running plan must exist in unfiltered list");
    assert_eq!(&filtered_plans[0], matching_unfiltered_entry);
}

#[test]
fn is_merged_cache_runs_git_once_per_head_and_runs_again_on_changed_head() {
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    write(&repo.join("file.txt"), "init\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "initial commit", "--no-gpg-sign"]);

    // Create a feature branch with a commit
    let branch = "wip/job-x";
    git(&repo, &["checkout", "-qb", branch]);
    write(&repo.join("file.txt"), "feature 1\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "feat 1", "--no-gpg-sign"]);
    let head1 = git(&repo, &["rev-parse", "HEAD"]);

    clear_is_merged_cache();
    reset_is_merged_git_call_count();

    // Call 1: First call for head1 runs git
    let res1 = is_merged(&repo, branch, &head1);
    assert!(!res1, "head1 should not be merged into main yet");
    assert_eq!(is_merged_git_call_count(), 1, "git ran once for head1");

    // Call 2: Second call for the same (root, branch, head1) should hit cache and NOT run git
    let res2 = is_merged(&repo, branch, &head1);
    assert!(!res2);
    assert_eq!(
        is_merged_git_call_count(),
        1,
        "second call for head1 should be cached (git ran once)"
    );

    // Create a new commit on the branch
    write(&repo.join("file.txt"), "feature 2\n");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "feat 2", "--no-gpg-sign"]);
    let head2 = git(&repo, &["rev-parse", "HEAD"]);
    assert_ne!(head1, head2);

    // Call 3: Changed head runs git again
    let res3 = is_merged(&repo, branch, &head2);
    assert!(!res3, "head2 should not be merged yet");
    assert_eq!(
        is_merged_git_call_count(),
        2,
        "changed head should run git again"
    );

    // Call 4: Repeated call for head2 is cached
    let res4 = is_merged(&repo, branch, &head2);
    assert!(!res4);
    assert_eq!(
        is_merged_git_call_count(),
        2,
        "repeated call for head2 should be cached"
    );
}

#[test]
fn index_html_polls_running_plans_and_does_not_use_set_interval_for_fetches() {
    let page = include_str!("../assets/dashboard/index.html");

    // Must fetch /api/plans?running=1
    assert!(
        page.contains("/api/plans?running=1"),
        "index.html must fetch /api/plans?running=1"
    );

    // Must NOT contain setInterval(fetchPlans or setInterval(fetchJobs
    assert!(
        !page.contains("setInterval(fetchPlans"),
        "index.html must not contain setInterval(fetchPlans"
    );
    assert!(
        !page.contains("setInterval(fetchJobs"),
        "index.html must not contain setInterval(fetchJobs"
    );

    // Must use setTimeout for scheduling subsequent fetches
    assert!(
        page.contains("setTimeout(fetchPlans, 5000)"),
        "index.html should schedule fetchPlans via setTimeout"
    );
    assert!(
        page.contains("setTimeout(fetchJobs, 5000)"),
        "index.html should schedule fetchJobs via setTimeout"
    );
}
