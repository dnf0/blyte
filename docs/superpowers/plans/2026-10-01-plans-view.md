# The dashboard's Plans view — Plan

The dashboard lists jobs one by one. A plan's tasks belong together, and how they depend on each other
decides what can run now: show each plan as a swimlane flow (a box per task, coloured by its state,
arrows from a task to the work waiting on it), with the selected task's details, the plan's activity
and its decisions. Everything comes from files blyte already writes; the orchestrator updates nothing.
The design is the mock-up at `docs/superpowers/plans/assets/2026-10-01-plans-view-mockup.html`.

## Global Constraints

- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.
- Each task creates its new test file exactly as given; do not edit existing test files. No new
  dependencies. Conventional commit, `--no-gpg-sign`, the message the task gives.
- Tasks 1, 2 and 3 are independent (Depends on: none for each); they meet at the `/api/plans`
  contract below and the start-file fields `plan` / `plan_task`.

## The `/api/plans` contract

`GET /api/plans` returns:

```json
{ "generated_at": 1790000000,
  "plans": [ {
    "key": "app/widget", "repo": "app", "path": "/abs/repos/app/docs/plans/widget.md", "title": "Widget thing",
    "lanes": [ { "id": "widget", "label": "widget" } ],
    "tasks": [ {
      "id": "T1", "title": "core parser", "heading": "Task 1: core parser", "lane": "widget", "deps": [],
      "state": "merged", "dispatch": "app/W1", "now": "…", "gate_running": false, "gate_attempts": 1,
      "elapsed_s": 300, "cost_usd": 0.5, "turns": 20,
      "files": ["crates/widget/src/parse.rs"], "commits": [ { "sha": "abc1234", "subject": "feat: parser" } ],
      "verdict": "accepted", "notes": "…" } ],
    "events": [ { "at": 1790000000, "task": "T1", "text": "dispatched" } ],
    "decisions": [ { "text": "…", "source": "plan" } ],
    "stats": { "tasks": 4, "merged": 1, "verified": 0, "running": 1, "not_started": 2,
               "cost_usd": 0.5, "first_dispatch_at": 1790000000, "max_parallel": 2 }
  } ] }
```

- `state` is one of `merged`, `verified` (passed, not yet merged), `running`, `failed` (any finished
  outcome other than verified), `redone` (its latest verdict is `redone`), `ready` (not dispatched;
  every dependency merged) and `waiting` (not dispatched; a dependency not merged).
- `files` is the dispatch's changed files when it has any, otherwise the plan's listed files.
- `events` are newest first: `dispatched` (start time), `verified` / `gate failed` / `out of budget` /
  `error` / `no changes` / `stopped` (when it finished), and `verdict <v>` (the verdict's time).
- Plans are listed when any of their tasks was dispatched in the last 7 days; plans with a running
  task first, then by latest activity.

### Task 1: `blyte dispatch --plan` and `--plan-task`

**Files:** `crates/blyte-cli/src/cli.rs` (`DispatchArgs`), `crates/blyte-cli/src/dispatch.rs`
(`run_dispatch`), new `crates/blyte-cli/tests/test_dispatch_plan_fields.rs` (exactly as below),
`docs/usage.md` (one sentence in the dispatch section), `docs/reference/cli.md`.

Depends on: none

**What to build.** `--plan <PATH>` and `--plan-task <TEXT>` (both optional strings, kept exactly as
given). The start file (`<id>.start.json`) gains `"plan"` and `"plan_task"`, `null` when not given.
They change nothing else about the run.

```rust
//! A dispatch can say which plan task it implements, so the dashboard can show plans (2026-10-01).

use std::path::Path;
use std::process::Command;

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

fn start(r: &Path, extra: &[&str], id: &str) -> serde_json::Value {
    let mut args = vec!["dispatch", id, "task.md", "--gate", "true", "--sandbox", "off"];
    args.extend_from_slice(extra);
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(&args)
        .current_dir(r)
        .env("BLYTE_STATE_DIR", r.join(".state"))
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap();
    assert!(!out.stdout.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_str(&std::fs::read_to_string(r.join(format!(".wt/{id}.start.json"))).unwrap()).unwrap()
}

#[test]
fn the_start_file_records_the_plan_task() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(r.join("task.md"), "x").unwrap();

    let s = start(r, &["--plan", "docs/plans/widget.md", "--plan-task", "Task 1: core parser"], "P1");
    assert_eq!(s["plan"], "docs/plans/widget.md");
    assert_eq!(s["plan_task"], "Task 1: core parser");

    let s = start(r, &[], "P2");
    assert!(s["plan"].is_null() && s["plan_task"].is_null(), "{s}");
}
```

**Commit message:** `feat(cli): blyte dispatch --plan and --plan-task`

### Task 2: plans from plan files and dispatches, at `/api/plans`

**Files:** new `crates/blyte-cli/src/plans.rs` (`pub mod plans;` in `lib.rs`),
`crates/blyte-cli/src/dashboard.rs` (job fields and the route), new
`crates/blyte-cli/tests/test_plans.rs` (exactly as below).

Depends on: none

**What to build.**

1. **Parsing.** `pub fn parse_plan(text: &str) -> PlanDoc`, with `pub struct PlanDoc { pub title:
   String, pub tasks: Vec<PlanTask>, pub decisions: Vec<String> }` and `pub struct PlanTask { pub id:
   String, pub title: String, pub heading: String, pub deps: Vec<String>, pub lane: String, pub files:
   Vec<String> }` (both `Debug, Clone, PartialEq`).
   - `title`: the first `# ` line, without a trailing ` — Plan` or ` - Plan`.
   - A task is a heading of level 2–4 (`##`, `###`, `####`) whose text is `Task <N>: <title>`; its
     section runs to the next heading of the same or a higher level. `id` is `T<N>` when `<N>` is all
     digits, else `<N>` itself; `heading` is the heading text (`Task 1: core parser`).
   - `deps`: from the first line in the section containing `Depends on:` (any case, bold or not):
     `none` gives `[]`; otherwise every `Task <N>` in that line, as ids, in order, without duplicates.
     With no such line, also accept a line starting `Needs Task` the same way; otherwise `[]`.
   - `files`: every backticked path on the section's `**Files:**` line (a word containing `/` or `.`).
   - `lane`: the text after `Lane:` on a line of its own, trimmed; otherwise from the first file:
     `crates/<x>/…` gives `x`, any other path its first component, no files `Tasks`.
   - `decisions`: the `- ` bullets of a `## Decisions` section, without the dash.
2. **Job fields.** In `dashboard.rs`, each job of the dispatch layout gains `root` (the checkout that
   holds the `.wt/`), `plan_file` and `plan_task`: from the start file's `plan` / `plan_task` when
   present; else `plan_file` is the job's existing plan path (`Plan: <path>`) and `plan_task` is the
   prompt's `Task: "<title>"` or its first `Task <id>`.
3. **Collection.** `pub fn collect_plans(src: &Sources, now: u64) -> serde_json::Value`, building the
   contract above from `dashboard::collect`'s jobs (with a 7-day window for plans):
   - Group jobs by `(root, plan_file)`; resolve `plan_file` against `root` when relative; skip plans
     whose file cannot be read. The key is `<repo>/<plan file stem>`.
   - Match a job to a task when its `plan_task` equals the task's `heading`, `title` or `id`, or
     starts with `Task <N>` for that task. A task's latest matching job (by `started_at`) decides its
     state: running → `running`; verified → `merged` when its last commit is an ancestor of `root`'s
     `HEAD` (`git merge-base --is-ancestor`), else `verified`; any other outcome → `failed`; a latest
     verdict `redone` → `redone`. Unmatched tasks are `ready` or `waiting` as the contract says.
   - Lanes in order of first appearance; `id` is the label lowercased with spaces as `-`.
   - `decisions`: the plan's (source `plan`), then each task's verdict notes (source `<id> verdict
     note`).
   - `stats.max_parallel`: the most matched jobs whose `[started_at, finished_at or now]` overlap.
4. **Route.** `route("/api/plans", …)` returns `(200, "application/json", …)`.

```rust
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
    assert_eq!(doc.tasks[0].files, ["crates/widget/src/parse.rs", "crates/widget/tests/p.rs"]);
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

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let wt = repo.join(".wt");
    write(
        &wt.join("W1.start.json"),
        &serde_json::json!({"id": "W1", "pid": 999999, "started_at": now - 600, "task_file": "/nonexistent",
            "gate": "true", "plan": "docs/plans/widget.md", "plan_task": "Task 1: core parser"}).to_string(),
    );
    write(&wt.join("W1.log"), "[Turn 20] \n");
    write(&wt.join("W1.report.json"), r#"{"outcome":"verified","cost_usd":0.5,"turns":20,"gate_attempts":1}"#);
    write(
        &wt.join("W1.json"),
        &serde_json::json!({"commits": [{"sha": sha, "subject": "feat: parser"}], "changed_files": ["parse.rs"]}).to_string(),
    );
    write(&wt.join("W1.verdict.json"), r#"{"verdict":"accepted","notes":"to the plan","ts":1}"#);
    write(
        &wt.join("W2.start.json"),
        &serde_json::json!({"id": "W2", "pid": std::process::id(), "started_at": now - 120, "task_file": "/nonexistent",
            "gate": "true", "plan": "docs/plans/widget.md", "plan_task": "Task 2: the CLI flag"}).to_string(),
    );
    write(&wt.join("W2.log"), "[Turn 3] \n● read cli.rs -> 10 lines\n");

    let src = Sources { roots: vec![t.path().join("repos")], runs_dirs: vec![], hours: 24 };
    let v = collect_plans(&src, now);
    let plan = &v["plans"][0];
    assert_eq!(plan["key"], "app/widget");
    assert_eq!(plan["title"], "Widget thing");
    let state = |id: &str| {
        plan["tasks"].as_array().unwrap().iter().find(|t| t["id"] == id).unwrap()["state"].as_str().unwrap().to_string()
    };
    assert_eq!(state("T1"), "merged");
    assert_eq!(state("T2"), "running");
    assert_eq!(state("T3"), "waiting");
    assert_eq!(state("T4"), "ready");
    assert_eq!(plan["stats"]["merged"], 1);
    assert_eq!(plan["stats"]["not_started"], 2);
    assert_eq!(plan["stats"]["max_parallel"], 2); // W1 ran until its report was written, now; W2 started 120 s ago
    assert!(plan["events"].as_array().unwrap().iter().any(|e| e["task"] == "T1" && e["text"] == "dispatched"));
    assert!(plan["decisions"].as_array().unwrap().iter().any(|d| d["text"] == "to the plan"));

    let (code, ctype, _) = route("/api/plans", &src);
    assert_eq!((code, ctype), (200, "application/json"));
}
```

**Commit message:** `feat(cli): plans from plan files and dispatches, at /api/plans`

### Task 3: the Plans view in the page

**Files:** `crates/blyte-cli/assets/dashboard/index.html`, new
`crates/blyte-cli/tests/test_dashboard_plans_page.rs` (exactly as below).

Depends on: none

**What to build.** Port the mock-up (`docs/superpowers/plans/assets/2026-10-01-plans-view-mockup.html`)
into the dashboard page, keeping the page's own tokens, fonts (served locally), header and Jobs
view as they are:
- A **Jobs / Plans** switch in the header; the hash `#plans` opens Plans, anything else Jobs; the
  switch updates the hash. Jobs keeps polling `/api/jobs`; Plans polls `/api/plans` every 5 s while
  shown.
- Plans: the plan picker (one chip per plan: stem, repo, progress such as `1 of 5` or `done`), the
  plan summary (title, repo and path, the state bar, the stats), the swimlane flow (lanes, columns by
  dependency depth, boxes filled by state with a dashed outline for `waiting`, the pulsing dot for
  `running`, edges `done` when both ends are merged and `hot` for the selected task's), the selected
  task's details (Waits on / Unblocks chips, now, gate, time and cost, files, commits, verdict, notes,
  and a "Next" line for `verified`, `ready` and `waiting`), the activity list and the decisions list,
  laid out as in the mock-up. States add `redone` (styled like `failed`) and `failed` to the
  mock-up's set.
- Every value from the API goes into the page escaped, as the Jobs view does; nothing loads from the
  network. With no plans, show "No plans with dispatches in the last 7 days."
- Keep the selected plan and task across polls (and in `localStorage`, guarded with try/catch).

```rust
//! The page has a Plans view fed by /api/plans (2026-10-01).

const PAGE: &str = include_str!("../assets/dashboard/index.html");

#[test]
fn the_page_has_a_plans_view_that_polls_its_own_api() {
    for needle in ["/api/plans", "#plans", "No plans with dispatches in the last 7 days.", "Waits on", "Unblocks"] {
        assert!(PAGE.contains(needle), "missing {needle}");
    }
    for state in ["merged", "verified", "running", "failed", "redone", "ready", "waiting"] {
        assert!(PAGE.contains(&format!("{state}:")) || PAGE.contains(&format!("\"{state}\"")), "{state}");
    }
    assert!(!PAGE.contains("https://") && !PAGE.contains("http://"));
}
```

**Commit message:** `feat(cli): the dashboard's Plans view`
