use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskStatus {
    pub id: String,
    pub heading: String, // PlanTask.heading
    pub deps: Vec<String>,
    pub state: String, // "merged" | "review" | "running" | "dead" | "ready" | "waiting"
    pub run: Option<String>, // the run id it was matched to
    pub outcome: Option<String>, // the summary's outcome, when there is one
    pub verdict: Option<String>,
    pub waiting_on: Vec<String>, // deps not merged, only for "waiting"
    pub command: Option<String>, // only for "ready"
    pub problem: Option<String>, // only when the plan-task text does not resolve
}

fn is_pid_alive(pid: libc::pid_t) -> bool {
    if pid <= 0 {
        return false;
    }
    let ret = unsafe { libc::kill(pid, 0) };
    if ret == 0 {
        true
    } else {
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

pub fn plan_task_text(id: &str, heading: &str) -> String {
    let prefix = format!("Task {id}");
    if heading.starts_with(&prefix) {
        let rest = &heading[prefix.len()..];
        if rest.is_empty() || rest.starts_with(':') || rest.starts_with(' ') {
            return prefix;
        }
    }
    heading.to_string()
}

fn normalize_components(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        if c != Component::CurDir {
            out.push(c);
        }
    }
    out
}

fn paths_match(root: &Path, start_plan: &str, plan_rel: &str) -> bool {
    let p1 = root.join(start_plan);
    let p2 = root.join(plan_rel);
    match (p1.canonicalize(), p2.canonicalize()) {
        (Ok(c1), Ok(c2)) => c1 == c2,
        _ => normalize_components(&p1) == normalize_components(&p2),
    }
}

#[derive(Debug, Clone)]
struct StartRecord {
    run_id: String,
    started_at: u64,
    plan: Option<String>,
    plan_task: Option<String>,
    pid: Option<libc::pid_t>,
}

fn read_start_files(root: &Path) -> Vec<StartRecord> {
    let wt_dir = root.join(".wt");
    let entries = match std::fs::read_dir(&wt_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if let Some(run_id) = name_str.strip_suffix(".start.json") {
            let path = entry.path();
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let val: serde_json::Value = match serde_json::from_str(&content) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let started_at = val
                .get("started_at")
                .and_then(|v| {
                    v.as_u64()
                        .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
                })
                .unwrap_or(0);
            let plan = val.get("plan").and_then(|v| v.as_str()).map(String::from);
            let plan_task = val
                .get("plan_task")
                .and_then(|v| v.as_str())
                .map(String::from);
            let pid = val
                .get("pid")
                .and_then(|v| v.as_i64())
                .map(|p| p as libc::pid_t);
            records.push(StartRecord {
                run_id: run_id.to_string(),
                started_at,
                plan,
                plan_task,
                pid,
            });
        }
    }
    records
}

fn check_is_ancestor(root: &Path, run_id: &str) -> bool {
    Command::new("git")
        .args([
            "merge-base",
            "--is-ancestor",
            &format!("wip/{run_id}"),
            "HEAD",
        ])
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn check_branch_exists(root: &Path, run_id: &str) -> bool {
    Command::new("git")
        .args(["rev-parse", "--verify", &format!("refs/heads/wip/{run_id}")])
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn read_verdict(root: &Path, run_id: &str) -> Option<String> {
    let verdict_file = root.join(".wt").join(format!("{run_id}.verdict.json"));
    std::fs::read_to_string(&verdict_file)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("verdict").and_then(|x| x.as_str()).map(String::from))
}

fn read_summary(root: &Path, run_id: &str) -> Option<(Option<String>, bool)> {
    let summary_file = root.join(".wt").join(format!("{run_id}.json"));
    std::fs::read_to_string(&summary_file)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| {
            let outcome = v.get("outcome").and_then(|x| x.as_str()).map(String::from);
            let has_commits = v
                .get("commits")
                .and_then(|x| x.as_array())
                .is_some_and(|a| !a.is_empty());
            (outcome, has_commits)
        })
}

struct TaskIntermediate {
    task: crate::plans::PlanTask,
    plan_task_t: String,
    problem: Option<String>,
    state: Option<String>,
    run: Option<String>,
    outcome: Option<String>,
    verdict: Option<String>,
}

/// `root` is the main checkout (holds `.wt/`), `plan_rel` the plan path relative to it, as dispatch takes it.
pub fn statuses(root: &Path, plan_rel: &str, text: &str) -> Vec<TaskStatus> {
    let doc = crate::plans::parse_plan(text);
    let start_files = read_start_files(root);

    let all_task_ids: Vec<String> = doc.tasks.iter().map(|t| t.id.clone()).collect();
    let mut intermediates = Vec::new();

    for task in doc.tasks {
        let t = plan_task_text(&task.id, &task.heading);
        let problem = if crate::plan_section::extract(text, &t).is_none() {
            Some(
                crate::plan_section::explain_missing(text, &t)
                    .unwrap_or_else(|| "heading not found".to_string()),
            )
        } else {
            None
        };

        let task_id_label = format!("Task {}", task.id);
        let mut matching: Vec<&StartRecord> = start_files
            .iter()
            .filter(|s| {
                let Some(ref sp) = s.plan else {
                    return false;
                };
                if !paths_match(root, sp, plan_rel) {
                    return false;
                }
                let Some(ref spt) = s.plan_task else {
                    return false;
                };
                spt == &t || spt == &task_id_label || spt == &task.heading
            })
            .collect();
        matching.sort_by_key(|r| r.started_at);
        let matched_run = matching.last().copied();

        let mut state = None;
        let mut run_out = None;
        let mut outcome_out = None;
        let mut verdict_out = None;

        if let Some(run) = matched_run {
            let is_ancestor = check_is_ancestor(root, &run.run_id);
            let branch_exists = check_branch_exists(root, &run.run_id);
            let verdict_opt = read_verdict(root, &run.run_id);
            let summary = read_summary(root, &run.run_id);
            let has_commits = summary.as_ref().is_some_and(|(_, c)| *c);
            let is_merged = (has_commits && is_ancestor)
                || (!branch_exists && matches!(verdict_opt.as_deref(), Some("accepted" | "fixed")));

            if summary.is_none() && run.pid.map(is_pid_alive).unwrap_or(false) {
                // Rule 1: running
                state = Some("running".to_string());
                run_out = Some(run.run_id.clone());
                verdict_out = verdict_opt;
            } else if is_merged {
                // Rule 2: merged
                state = Some("merged".to_string());
                run_out = Some(run.run_id.clone());
                outcome_out = summary.and_then(|(o, _)| o);
                verdict_out = verdict_opt;
            } else if verdict_opt.as_deref() == Some("redone") {
                // Rule 3: A run with verdict redone is not review: treat the task as if no run matched (go on to rule 5).
                // Leave state as None, run_out as None
            } else if let Some((outcome, _)) = summary {
                // Rule 3: review
                state = Some("review".to_string());
                run_out = Some(run.run_id.clone());
                outcome_out = outcome;
                verdict_out = verdict_opt;
            } else {
                // Rule 4: dead
                state = Some("dead".to_string());
                run_out = Some(run.run_id.clone());
                verdict_out = verdict_opt;
            }
        }

        intermediates.push(TaskIntermediate {
            task,
            plan_task_t: t,
            problem,
            state,
            run: run_out,
            outcome: outcome_out,
            verdict: verdict_out,
        });
    }

    let merged_task_ids: HashSet<String> = intermediates
        .iter()
        .filter(|item| item.state.as_deref() == Some("merged"))
        .map(|item| item.task.id.clone())
        .collect();

    intermediates
        .into_iter()
        .map(|item| {
            let (state, waiting_on, command) = if let Some(st) = item.state {
                (st, Vec::new(), None)
            } else {
                // Rule 5: ready when every dep is merged (deps that are not tasks in this plan count as merged),
                // else waiting with waiting_on = the deps not merged, in plan order.
                let waiting_on: Vec<String> =
                    doc_task_order(&all_task_ids, &item.task.deps, &merged_task_ids);
                if waiting_on.is_empty() {
                    let cmd = if item.problem.is_none() {
                        Some(format!(
                            "blyte dispatch {} --plan {} --plan-task \"{}\"",
                            item.task.id, plan_rel, item.plan_task_t
                        ))
                    } else {
                        None
                    };
                    ("ready".to_string(), Vec::new(), cmd)
                } else {
                    ("waiting".to_string(), waiting_on, None)
                }
            };

            TaskStatus {
                id: item.task.id,
                heading: item.task.heading,
                deps: item.task.deps,
                state,
                run: item.run,
                outcome: item.outcome,
                verdict: item.verdict,
                waiting_on,
                command,
                problem: item.problem,
            }
        })
        .collect()
}

fn doc_task_order(
    all_task_ids: &[String],
    deps: &[String],
    merged_task_ids: &HashSet<String>,
) -> Vec<String> {
    all_task_ids
        .iter()
        .filter(|id| deps.contains(id) && !merged_task_ids.contains(*id))
        .cloned()
        .collect()
}

pub fn render(plan_rel: &str, rows: &[TaskStatus]) -> String {
    let mut counts = Vec::new();
    let n_merged = rows.iter().filter(|r| r.state == "merged").count();
    if n_merged > 0 {
        counts.push(format!("{n_merged} merged"));
    }
    let n_review = rows.iter().filter(|r| r.state == "review").count();
    if n_review > 0 {
        counts.push(format!("{n_review} review"));
    }
    let n_running = rows.iter().filter(|r| r.state == "running").count();
    if n_running > 0 {
        counts.push(format!("{n_running} running"));
    }
    let n_dead = rows.iter().filter(|r| r.state == "dead").count();
    if n_dead > 0 {
        counts.push(format!("{n_dead} dead"));
    }
    let n_ready = rows.iter().filter(|r| r.state == "ready").count();
    if n_ready > 0 {
        counts.push(format!("{n_ready} ready"));
    }
    let n_waiting = rows.iter().filter(|r| r.state == "waiting").count();
    if n_waiting > 0 {
        counts.push(format!("{n_waiting} waiting"));
    }

    let first_line = if counts.is_empty() {
        format!("{plan_rel}: {} tasks", rows.len())
    } else {
        format!("{plan_rel}: {} tasks ({})", rows.len(), counts.join(", "))
    };

    let max_id_len = rows.iter().map(|r| r.id.len()).max().unwrap_or(0);

    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(first_line);

    for r in rows {
        let detail = match r.state.as_str() {
            "merged" => {
                if let Some(ref problem) = r.problem {
                    format!("PROBLEM: {problem}")
                } else {
                    r.heading.clone()
                }
            }
            "waiting" => {
                if let Some(ref problem) = r.problem {
                    format!("PROBLEM: {problem}")
                } else if r.waiting_on.is_empty() {
                    r.heading.clone()
                } else {
                    format!("{}  (waiting on {})", r.heading, r.waiting_on.join(", "))
                }
            }
            "review" => {
                if let Some(ref problem) = r.problem {
                    format!("PROBLEM: {problem}")
                } else {
                    let outcome = r.outcome.as_deref().unwrap_or("unknown");
                    let run = r.run.as_deref().unwrap_or("");
                    format!("{outcome}, run {run}: blyte review {run}")
                }
            }
            "running" | "dead" => {
                if let Some(ref problem) = r.problem {
                    format!("PROBLEM: {problem}")
                } else {
                    let run = r.run.as_deref().unwrap_or("");
                    format!("run {run}: blyte status {run}")
                }
            }
            "ready" => {
                if let Some(ref problem) = r.problem {
                    format!("PROBLEM: {problem}")
                } else {
                    r.command.clone().unwrap_or_default()
                }
            }
            _ => {
                if let Some(ref problem) = r.problem {
                    format!("PROBLEM: {problem}")
                } else {
                    r.heading.clone()
                }
            }
        };

        lines.push(format!("{:<max_id_len$}  {:<7}  {detail}", r.id, r.state));
    }

    lines.join("\n")
}

fn compute_plan_rel(root: &Path, plan: &Path, full_plan_path: &Path) -> String {
    let canon_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canon_plan = full_plan_path
        .canonicalize()
        .unwrap_or_else(|_| full_plan_path.to_path_buf());
    if let Ok(rel) = canon_plan.strip_prefix(&canon_root) {
        return rel.to_string_lossy().to_string();
    }
    if let Ok(rel) = full_plan_path.strip_prefix(root) {
        return rel.to_string_lossy().to_string();
    }
    plan.to_string_lossy().to_string()
}

pub fn run(plan: &Path, json: bool) -> i32 {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&current_dir)
        .output();
    let Ok(out) = out else {
        eprintln!("error: git not found");
        return 2;
    };
    if !out.status.success() {
        eprintln!("error: not in a git repository");
        return 2;
    }

    let toplevel = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let root = crate::config::main_checkout(&current_dir).unwrap_or(toplevel);

    let full_plan_path = if plan.is_absolute() {
        plan.to_path_buf()
    } else {
        current_dir.join(plan)
    };

    let text = match std::fs::read_to_string(&full_plan_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot read plan file '{}': {e}", plan.display());
            return 2;
        }
    };

    let doc = crate::plans::parse_plan(&text);
    if doc.tasks.is_empty() {
        eprintln!("error: plan has no ### Task sections");
        return 2;
    }

    let plan_rel = compute_plan_rel(&root, plan, &full_plan_path);
    let rows = statuses(&root, &plan_rel, &text);

    if json {
        let json_str = serde_json::to_string_pretty(&rows).unwrap_or_default();
        println!("{json_str}");
    } else {
        let rendered = render(&plan_rel, &rows);
        println!("{rendered}");
    }

    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

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

    fn setup_test_repo() -> TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        fs::write(root.join("f.txt"), "hello\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "initial", "--no-gpg-sign"]);
        fs::create_dir_all(root.join(".wt")).unwrap();
        dir
    }

    fn write_start_json(
        root: &Path,
        run_id: &str,
        plan: &str,
        plan_task: &str,
        pid: u32,
        started_at: u64,
    ) {
        let json = serde_json::json!({
            "id": run_id,
            "plan": plan,
            "plan_task": plan_task,
            "pid": pid,
            "started_at": started_at,
        });
        fs::write(
            root.join(".wt").join(format!("{run_id}.start.json")),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();
    }

    fn write_summary_json(root: &Path, run_id: &str, outcome: &str) {
        let json = serde_json::json!({
            "id": run_id,
            "outcome": outcome,
        });
        fs::write(
            root.join(".wt").join(format!("{run_id}.json")),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();
    }

    fn write_verdict_json(root: &Path, run_id: &str, verdict: &str) {
        let json = serde_json::json!({
            "verdict": verdict,
        });
        fs::write(
            root.join(".wt").join(format!("{run_id}.verdict.json")),
            serde_json::to_string_pretty(&json).unwrap(),
        )
        .unwrap();
    }

    const TWO_TASK_PLAN: &str = r#"# Sample Plan

### Task A: one

Depends on: none

Do task A.

### Task B: two

**Depends on:** A

Do task B.
"#;

    // Test 1: A plan with ### Task A: one (Depends on: none) and ### Task B: two (**Depends on:** A), no runs:
    // A is ready with command blyte dispatch A --plan docs/p.md --plan-task "Task A"; B is waiting,
    // waiting_on == ["A"], no command.
    #[test]
    fn test_1_plan_dependencies_no_runs() {
        let repo = setup_test_repo();
        let rows = statuses(repo.path(), "docs/p.md", TWO_TASK_PLAN);

        assert_eq!(rows.len(), 2);
        let a = &rows[0];
        assert_eq!(a.id, "A");
        assert_eq!(a.state, "ready");
        assert_eq!(
            a.command.as_deref(),
            Some("blyte dispatch A --plan docs/p.md --plan-task \"Task A\"")
        );
        assert!(a.waiting_on.is_empty());

        let b = &rows[1];
        assert_eq!(b.id, "B");
        assert_eq!(b.state, "waiting");
        assert_eq!(b.waiting_on, vec!["A"]);
        assert!(b.command.is_none());
    }

    // Test 2: A start file for A (plan docs/p.md, plan_task Task A, pid of the test process) and no summary:
    // A is running, run A.
    #[test]
    fn test_2_start_file_alive_pid_running() {
        let repo = setup_test_repo();
        let pid = std::process::id();
        write_start_json(repo.path(), "A", "docs/p.md", "Task A", pid, 100);

        let rows = statuses(repo.path(), "docs/p.md", TWO_TASK_PLAN);
        assert_eq!(rows[0].id, "A");
        assert_eq!(rows[0].state, "running");
        assert_eq!(rows[0].run.as_deref(), Some("A"));
    }

    // Test 3: Same with pid 999999 (not alive): dead.
    #[test]
    fn test_3_start_file_dead_pid() {
        let repo = setup_test_repo();
        write_start_json(repo.path(), "A", "docs/p.md", "Task A", 999999, 100);

        let rows = statuses(repo.path(), "docs/p.md", TWO_TASK_PLAN);
        assert_eq!(rows[0].id, "A");
        assert_eq!(rows[0].state, "dead");
        assert_eq!(rows[0].run.as_deref(), Some("A"));
    }

    // Test 4: A start file plus .wt/A.json with "outcome":"verified": review, outcome verified.
    #[test]
    fn test_4_start_and_summary_review() {
        let repo = setup_test_repo();
        write_start_json(repo.path(), "A", "docs/p.md", "Task A", 999999, 100);
        write_summary_json(repo.path(), "A", "verified");

        let rows = statuses(repo.path(), "docs/p.md", TWO_TASK_PLAN);
        assert_eq!(rows[0].id, "A");
        assert_eq!(rows[0].state, "review");
        assert_eq!(rows[0].run.as_deref(), Some("A"));
        assert_eq!(rows[0].outcome.as_deref(), Some("verified"));
    }

    // Test 5: A run whose branch wip/A1 is an ancestor of HEAD (create the branch at HEAD), run id A1 different
    // from the task id, plan_task Task A: A is merged with run A1, and B is ready.
    #[test]
    fn test_5_branch_merged_makes_dependent_ready() {
        let repo = setup_test_repo();
        git(repo.path(), &["branch", "wip/A1"]);
        write_start_json(repo.path(), "A1", "docs/p.md", "Task A", 999999, 100);
        fs::write(
            repo.path().join(".wt/A1.json"),
            r#"{"id":"A1","outcome":"verified","commits":[{"sha":"abc","subject":"work"}]}"#,
        )
        .unwrap();

        let rows = statuses(repo.path(), "docs/p.md", TWO_TASK_PLAN);
        assert_eq!(rows[0].id, "A");
        assert_eq!(rows[0].state, "merged");
        assert_eq!(rows[0].run.as_deref(), Some("A1"));

        assert_eq!(rows[1].id, "B");
        assert_eq!(rows[1].state, "ready");
        assert_eq!(
            rows[1].command.as_deref(),
            Some("blyte dispatch B --plan docs/p.md --plan-task \"Task B\"")
        );
    }

    // Test 6: No branch, verdict accepted: merged. Verdict redone with a summary: A is ready again.
    #[test]
    fn test_6_verdict_accepted_then_redone() {
        let repo = setup_test_repo();
        write_start_json(repo.path(), "A", "docs/p.md", "Task A", 999999, 100);
        write_verdict_json(repo.path(), "A", "accepted");

        let rows = statuses(repo.path(), "docs/p.md", TWO_TASK_PLAN);
        assert_eq!(rows[0].id, "A");
        assert_eq!(rows[0].state, "merged");

        // Now change verdict to redone and add a summary: A is ready again.
        write_summary_json(repo.path(), "A", "gate_failed");
        write_verdict_json(repo.path(), "A", "redone");

        let rows2 = statuses(repo.path(), "docs/p.md", TWO_TASK_PLAN);
        assert_eq!(rows2[0].id, "A");
        assert_eq!(rows2[0].state, "ready");
        assert_eq!(
            rows2[0].command.as_deref(),
            Some("blyte dispatch A --plan docs/p.md --plan-task \"Task A\"")
        );
    }

    // Test 7: A start file for the same task in a different plan file: ignored (A stays ready).
    #[test]
    fn test_7_different_plan_file_ignored() {
        let repo = setup_test_repo();
        write_start_json(
            repo.path(),
            "A",
            "docs/other.md",
            "Task A",
            std::process::id(),
            100,
        );

        let rows = statuses(repo.path(), "docs/p.md", TWO_TASK_PLAN);
        assert_eq!(rows[0].id, "A");
        assert_eq!(rows[0].state, "ready");
    }

    // Test 8: Two matching start files: the later started_at wins.
    #[test]
    fn test_8_later_started_at_wins() {
        let repo = setup_test_repo();
        let pid = std::process::id();
        // A1 has earlier started_at and is dead
        write_start_json(repo.path(), "A1", "docs/p.md", "Task A", 999999, 100);
        // A2 has later started_at and is alive
        write_start_json(repo.path(), "A2", "docs/p.md", "Task A", pid, 200);

        let rows = statuses(repo.path(), "docs/p.md", TWO_TASK_PLAN);
        assert_eq!(rows[0].id, "A");
        assert_eq!(rows[0].state, "running");
        assert_eq!(rows[0].run.as_deref(), Some("A2"));
    }

    // Test 9: A heading ### Setup step that parse_plan returns as a task (check what parse_plan does with it;
    // if it does not, use a heading shape it does return that does not start with Task <ID>): the command
    // uses the whole heading as --plan-task.
    #[test]
    fn test_9_heading_shape_without_task_id_prefix() {
        // parse_plan requires 'Task ' prefix and ':'.
        // For '### Task 1: setup step', first_word is '1', which is digits, so id becomes 'T1'.
        // Heading is 'Task 1: setup step', which does not start with 'Task T1'.
        // Therefore plan_task_text returns the full heading: "Task 1: setup step".
        let plan = r#"# Plan

### Task 1: setup step

Do setup.
"#;
        let repo = setup_test_repo();
        let rows = statuses(repo.path(), "docs/p.md", plan);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "T1");
        assert_eq!(rows[0].state, "ready");
        assert_eq!(
            rows[0].command.as_deref(),
            Some("blyte dispatch T1 --plan docs/p.md --plan-task \"Task 1: setup step\"")
        );
    }

    // Test 10: problem set when extract fails: build a TaskStatus path where the plan-task text does not
    // resolve (for example a task heading inside an unclosed fence that parse_plan still lists, or test the
    // helper that picks the plan-task text plus extract directly) and check command is None and problem is Some.
    #[test]
    fn test_10_problem_set_when_extract_fails() {
        // In markdown, `~~~` is a code fence for plan_section::extract, but parse_plan only treats ```` ``` ```` as fences.
        // Therefore parse_plan parses the task, but extract fails to find it.
        let plan = "~~~\n### Task A: fenced\n~~~\n";
        let repo = setup_test_repo();
        let rows = statuses(repo.path(), "docs/p.md", plan);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "A");
        assert!(rows[0].command.is_none());
        assert!(rows[0].problem.is_some());
    }

    // Test 11: render on test 1's rows: the first line is docs/p.md: 2 tasks (1 ready, 1 waiting) and B's line
    // ends with (waiting on A).
    #[test]
    fn test_11_render_output() {
        let repo = setup_test_repo();
        let rows = statuses(repo.path(), "docs/p.md", TWO_TASK_PLAN);
        let rendered = render("docs/p.md", &rows);
        let lines: Vec<&str> = rendered.lines().collect();

        assert_eq!(lines[0], "docs/p.md: 2 tasks (1 ready, 1 waiting)");
        assert!(lines[2].ends_with("(waiting on A)"));
    }

    // Test 12: run with a missing file returns 2.
    #[test]
    fn test_12_run_missing_file_returns_2() {
        let repo = setup_test_repo();
        let missing = repo.path().join("does_not_exist.md");
        let code = run(&missing, false);
        assert_eq!(code, 2);
    }
}
