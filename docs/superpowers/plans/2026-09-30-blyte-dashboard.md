# `blyte dashboard` — Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan may be shadow-run: agy and blyte each implement it independently. Three tasks run in parallel in separate worktrees; each lists the files it owns. Do not touch another task's files.

**Why (2026-09-30):** the user runs several blyte jobs at once across repositories and loses track
of what is running. `blyte dashboard` serves a local page, one row per job, with a summary, time,
cost, click-to-expand detail and clickable tags that filter to related jobs. The page's design is the
Design canvas "blyte Jobs Dashboard" (https://claude.ai/artifact/HGJWfm7UAVtx5Gk79Z4zgi), and the
user approved it. It runs only on this machine: the server binds 127.0.0.1, and the page makes no
request anywhere but its own server.

## Global Constraints

- Base branch `feat/blyte-dashboard`. It already holds a placeholder
  `crates/blyte-cli/assets/dashboard/index.html` (Task 3 replaces it; Tasks 1 and 2 do not touch it).
- Work only in your own worktree. Existing tests pass unmodified, except where a task says otherwise.
- No new crate dependencies (the workspace already has serde, serde_json, toml, libc, clap).
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message given in your task.

## The data contract (shared by Tasks 2 and 3)

`GET /api/jobs` returns JSON:

```json
{
  "generated_at": 1790780000,
  "summary": {
    "running": 3,
    "last_24h": 21,
    "repos": 5,
    "finished_24h": 18,
    "passed_24h": 16,
    "cost_24h_usd": 9.84
  },
  "jobs": [
    {
      "key": "premise-lookup/TF2-blyte",
      "id": "TF2-blyte",
      "repo": "premise-lookup",
      "status": "running",
      "gate_running": true,
      "title": "Task 2: tile fallback for missing neighbours",
      "task": "…the task text, at most 2000 characters…",
      "plan": "/Users/x/repos/premise-lookup/docs/superpowers/plans/2026-09-29-global-scale.md",
      "now": "bash uv run pytest -q -> exit 0",
      "started_at": 1790779460,
      "finished_at": null,
      "elapsed_s": 540,
      "cost_usd": null,
      "turns": 63,
      "gate_attempts": 1,
      "gate_failures": [],
      "model": null,
      "last_tool": "bash uv run pytest -q -> exit 0",
      "worktree": "/Users/x/repos/premise-lookup--TF2-blyte",
      "log": "/Users/x/.cache/blyte-runs/premise-lookup/TF2-blyte.log",
      "commits": [],
      "changed_files": [],
      "tags": ["repo: premise-lookup", "plan: 2026-09-29-global-scale", "A/B: TF2"],
      "verdict": null
    }
  ]
}
```

- `status` is one of `running`, `verified`, `gate_failed`, `budget_exhausted`, `no_changes`,
  `error`, `stopped` (no report and the process is gone), or the report's `outcome` as written when
  it is none of these. `gate_running` is true only while `status` is `running` and the log's last
  event is a gate start (below).
- Nullable: `plan`, `finished_at`, `elapsed_s`, `cost_usd`, `model`, `last_tool`, `worktree`,
  `verdict`, `started_at`. Always present: every key above.
- `jobs` are sorted: running jobs first, newest `started_at` first; then the rest, newest
  `finished_at` first.
- Every job has the tag `repo: <repo>`; `plan: <file stem>` when `plan` is known; plus the tags
  its files give (below).

---

### Task 1: the gate-start event

**Files:** `crates/blyte-agent/src/run.rs`, `crates/blyte-cli/src/repl.rs`,
`crates/blyte-cli/tests/test_repl_commands.rs` (one test appended).

A running gate leaves no trace in the log until it ends, so a dashboard cannot say "in gate".

- In `crates/blyte-agent/src/run.rs`, add a variant to `pub enum Event<'e>` directly before `Gate`:

```rust
    /// A gate attempt is starting (the check runs next). Emitted so a watcher can tell a run
    /// that is waiting on its gate from one that is thinking (`blyte dashboard`, 2026-09-30).
    GateStart {
        attempt: u32,
    },
```

  and, in the run loop, emit it right after `report.gate_turns.push(turn);` and before
  `let gate_start = std::time::Instant::now();`:

```rust
            self.emit(Event::GateStart {
                attempt: report.gate_attempts,
            });
```

- In `crates/blyte-cli/src/repl.rs`, in the function that renders an `Event` to a line, add a
  match arm next to the `Event::Gate` arm:

```rust
        blyte_agent::run::Event::GateStart { attempt } => {
            format!("⏳ Gate running (attempt {})", attempt)
        }
```

  (and handle the new variant anywhere else the compiler reports a non-exhaustive match).

- [ ] **Step 1: test.** Append to `crates/blyte-cli/tests/test_repl_commands.rs` (it already
  imports `format_event` and `Event`):

```rust
#[test]
fn a_gate_start_renders_as_gate_running() {
    let e = Event::GateStart { attempt: 2 };
    assert_eq!(format_event(&e), "⏳ Gate running (attempt 2)");
}
```

- [ ] **Step 2:** implement, run the checks, commit:
  `git commit --no-gpg-sign -m "feat(agent): a gate-start event, logged as \"⏳ Gate running\""`

---

### Task 2: the server, the job collector and `--tag`

**Files:** create `crates/blyte-cli/src/dashboard.rs` and `crates/blyte-cli/tests/test_dashboard.rs`;
modify `crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs`, `crates/blyte-cli/src/lib.rs`,
`crates/blyte-cli/src/dispatch.rs` (the `--tag` flag only), `docs/usage.md`, `docs/reference/cli.md`.

**The command.** `blyte dashboard [--port <N>] [--root <DIR>]... [--runs-dir <DIR>]... [--hours <N>] [--open]`

- `--port` default 7171; `0` asks the OS for a free port. It binds `127.0.0.1` only. On start it
  prints exactly one line to stdout, `blyte dashboard: http://127.0.0.1:<port>` (the real port),
  then serves until killed. `--open` also opens that URL (`open` on macOS, `xdg-open` elsewhere;
  a failure to open is a warning, not an error).
- Sources, when neither `--root` nor `--runs-dir` is given: `roots` and `runs_dirs` from
  `~/.config/blyte/dashboard.toml` (both optional arrays of paths; a leading `~/` expands to HOME);
  when that gives none either, the current git repository's top level. `--hours` (default 24) is
  how far back finished jobs are listed.
- Serving (std `TcpListener`, a thread per connection, no dependencies): `GET /` and
  `GET /index.html` → the page, `include_str!("../assets/dashboard/index.html")`, `text/html;
  charset=utf-8`; `GET /api/jobs` → the contract JSON, `application/json`, `Cache-Control:
  no-store`; anything else → 404. Only the request line is parsed; the body is ignored.

**Collecting jobs** (`pub fn collect(src: &Sources, now: u64) -> serde_json::Value`, with
`pub struct Sources { pub roots: Vec<PathBuf>, pub runs_dirs: Vec<PathBuf>, pub hours: u64 }`):

1. *Dispatch layout.* For each root `R`: `R/.wt` when it exists, plus `C/.wt` for each directory
   `C` directly in `R`. In a `.wt`, each `<id>.start.json` is one job; `repo` = the name of the
   directory holding `.wt`. `start.json` gives `pid`, `started_at`, `task_file` (its text is
   `task`) and optional `tags` (array of strings). Beside it: `<id>.log`, `<id>.report.json`,
   `<id>.json` (the dispatch summary: `commits` `[{sha, subject}]`, `changed_files`, `final_message`,
   `worktree`).
2. *Runs-dir layout* (what the local dispatch scripts write). For each runs dir `D`, each
   `D/<repo>/<id>.log` is one job, with optional `<id>.pid` (the process id as text; its file
   modified time is `started_at`), `<id>.prompt` (the task text), `<id>.report.json`, and
   `<id>.worktree` (a path as text).
3. Both layouts: optional `<id>.tags.json` (an array of strings, added to the tags) and
   `<id>.verdict.json` (`{"verdict": "accepted", "notes": "…"}` → `verdict`).
4. `title`: the quoted text of a line `Task: "…"` in the task text when there is one, else the task
   text's first non-empty line with leading `#` and spaces removed; at most 120 characters.
   `plan`: the path after `Plan: ` on a line of the task text starting with it, else null.
5. With a report: `status` from its `outcome`; `cost_usd`, `turns`, `gate_attempts`, `model`
   (`model_id`), `gate_failures` (the last two, each cut to its last 1500 characters) from it;
   `finished_at` = the report file's modified time; `elapsed_s` = its `duration_seconds` rounded,
   else `finished_at - started_at`. `now` = the first line of `final_message` (the summary's, else
   the report's), at most 200 characters, else the log's last tool line.
6. Without a report: `running` if the pid is alive (`libc::kill(pid, 0) == 0`), else `stopped`.
   From the log (strip ANSI escapes first): `turns` = the largest `[Turn N]`, `last_tool` = the text
   after `● ` on the last such line (at most 200 characters), `gate_attempts` = the count of lines
   starting `✓ Gate passed` or `✗ Gate failed`, and `gate_running` = the last line of the log that is
   a turn, tool or gate line starts with `⏳ Gate running`. `now` = `last_tool`. `elapsed_s` =
   `now - started_at` when running.
7. Kept: every running job, and any other job whose `finished_at` (or, with no report, the log's
   modified time) is within `hours` of `now`. Jobs whose `key` repeats keep the first found.
8. `summary`: `running`; `last_24h` = jobs started within 24 hours of `now`; `repos` = distinct
   repos among listed jobs; `finished_24h`, `passed_24h` (status `verified`) and `cost_24h_usd`
   (sum of known costs) over jobs finished within 24 hours.

Also `pub fn route(path: &str, src: &Sources) -> (u16, &'static str, Vec<u8>)` (status, content
type, body) for the three routes above, used by the server and the tests.

**`--tag`.** `blyte dispatch` gains `--tag <TEXT>` (repeatable): the values go into
`<id>.start.json` as `"tags": [...]` (an empty array when none).

**Docs.** `docs/usage.md`: a section `## Dashboard` before `## Budgets` describing the command,
its sources, the config file and that it is local only. `docs/reference/cli.md`: the command in
the command lists and a `## blyte dashboard` section with its `--help` text.

- [ ] **Step 1: tests.** Create `crates/blyte-cli/tests/test_dashboard.rs` exactly:

```rust
//! `blyte dashboard` collects running and recent jobs from blyte's own files and serves them
//! locally (2026-09-30).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::{collect, route, Sources};

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn job<'a>(v: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    v["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["key"] == key)
        .unwrap_or_else(|| panic!("no job {key} in {v}"))
}

/// A root with dispatch-layout jobs (repo-a) and a runs dir (repo-b).
fn fixture(dir: &Path) -> Sources {
    let me = std::process::id();
    let t = now();
    let wt = dir.join("repos/repo-a/.wt");
    write(&dir.join("tasks/t1.md"), "# Add slugify\n\nMore detail.\n");
    write(
        &wt.join("T1.start.json"),
        &format!(
            r#"{{"id":"T1","pid":{me},"started_at":{},"task_file":"{}","gate":"true","tags":["planner: claude"]}}"#,
            t - 120,
            dir.join("tasks/t1.md").display()
        ),
    );
    write(&wt.join("T1.log"), "[Turn 3] \n● bash cargo test -> exit 0\n⏳ Gate running (attempt 1)\n");

    write(
        &wt.join("T2.start.json"),
        &format!(r#"{{"id":"T2","pid":999999,"started_at":{},"task_file":"/nonexistent","gate":"true"}}"#, t - 900),
    );
    write(&wt.join("T2.log"), "[Turn 10] \n✓ Gate passed (attempt 1)\n");
    write(
        &wt.join("T2.report.json"),
        r#"{"outcome":"verified","cost_usd":0.5,"turns":10,"gate_attempts":1,"model_id":"gemini/gemini-3.8-flash","gate_failures":[],"duration_seconds":301.4,"final_message":"Done: added the flag.\nMore."}"#,
    );
    write(
        &wt.join("T2.json"),
        r#"{"commits":[{"sha":"abc12345","subject":"feat: add the flag"}],"changed_files":["a.rs","b.rs"],"worktree":"/w/T2"}"#,
    );

    write(
        &wt.join("T3.start.json"),
        &format!(r#"{{"id":"T3","pid":999999,"started_at":{},"task_file":"/nonexistent","gate":"true"}}"#, t - 600),
    );
    write(&wt.join("T3.log"), "[Turn 2] \n● read src/lib.rs -> 40 lines\n");

    let runs = dir.join("runs/repo-b");
    write(&runs.join("R1.log"), "[Turn 7] \n● edit src/x.py -> edited\n");
    write(&runs.join("R1.pid"), &me.to_string());
    write(&runs.join("R1.prompt"), "You are implementing one task.\n\nPlan: /p/docs/plans/2026-09-30-thing.md\nTask: \"Task 1: Foo the bar\" ONLY.\n");
    write(&runs.join("R1.tags.json"), r#"["A/B: R1"]"#);
    write(&runs.join("R1.verdict.json"), r#"{"verdict":"accepted","notes":"fine"}"#);

    write(&runs.join("OLD.log"), "[Turn 1] \n");
    write(&runs.join("OLD.report.json"), r#"{"outcome":"verified","cost_usd":9.0}"#);
    let old = SystemTime::now() - Duration::from_secs(3 * 86400);
    for f in ["OLD.log", "OLD.report.json"] {
        std::fs::File::options().write(true).open(runs.join(f)).unwrap().set_modified(old).unwrap();
    }

    Sources { roots: vec![dir.join("repos")], runs_dirs: vec![dir.join("runs")], hours: 24 }
}

#[test]
fn running_jobs_show_their_progress_and_tags() {
    let d = tempfile::tempdir().unwrap();
    let v = collect(&fixture(d.path()), now());
    let t1 = job(&v, "repo-a/T1");
    assert_eq!(t1["status"], "running");
    assert_eq!(t1["gate_running"], true);
    assert_eq!(t1["turns"], 3);
    assert_eq!(t1["last_tool"], "bash cargo test -> exit 0");
    assert_eq!(t1["title"], "Add slugify");
    assert!(t1["elapsed_s"].as_u64().unwrap() >= 120);
    let tags: Vec<&str> = t1["tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
    assert!(tags.contains(&"repo: repo-a") && tags.contains(&"planner: claude"), "{tags:?}");

    let r1 = job(&v, "repo-b/R1");
    assert_eq!(r1["status"], "running");
    assert_eq!(r1["gate_running"], false);
    assert_eq!(r1["title"], "Task 1: Foo the bar");
    assert_eq!(r1["plan"], "/p/docs/plans/2026-09-30-thing.md");
    assert_eq!(r1["verdict"], "accepted");
    let tags: Vec<&str> = r1["tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
    assert!(tags.contains(&"A/B: R1") && tags.contains(&"plan: 2026-09-30-thing"), "{tags:?}");
}

#[test]
fn finished_and_stopped_jobs_are_reported_and_old_ones_dropped() {
    let d = tempfile::tempdir().unwrap();
    let v = collect(&fixture(d.path()), now());
    let t2 = job(&v, "repo-a/T2");
    assert_eq!(t2["status"], "verified");
    assert_eq!(t2["cost_usd"], 0.5);
    assert_eq!(t2["model"], "gemini/gemini-3.8-flash");
    assert_eq!(t2["elapsed_s"], 301);
    assert_eq!(t2["now"], "Done: added the flag.");
    assert_eq!(t2["commits"].as_array().unwrap().len(), 1);
    assert_eq!(t2["changed_files"].as_array().unwrap().len(), 2);
    assert_eq!(job(&v, "repo-a/T3")["status"], "stopped");
    assert!(v["jobs"].as_array().unwrap().iter().all(|j| j["key"] != "repo-b/OLD"));

    let order: Vec<&str> = v["jobs"].as_array().unwrap().iter().map(|j| j["key"].as_str().unwrap()).collect();
    assert_eq!(&order[..2].iter().filter(|k| ["repo-a/T1", "repo-b/R1"].contains(k)).count(), &2, "running first: {order:?}");

    let s = &v["summary"];
    assert_eq!(s["running"], 2);
    assert_eq!(s["repos"], 2);
    assert_eq!(s["passed_24h"], 1);
    assert_eq!(s["cost_24h_usd"], 0.5);
}

#[test]
fn routes_serve_the_page_the_jobs_and_nothing_else() {
    let d = tempfile::tempdir().unwrap();
    let src = fixture(d.path());
    let (code, ctype, body) = route("/", &src);
    assert_eq!(code, 200);
    assert!(ctype.starts_with("text/html"));
    assert!(String::from_utf8_lossy(&body).contains("<html"));
    let (code, ctype, body) = route("/api/jobs", &src);
    assert_eq!((code, ctype), (200, "application/json"));
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v["jobs"].is_array() && v["summary"].is_object());
    assert_eq!(route("/etc/passwd", &src).0, 404);
}

#[test]
fn the_server_listens_on_localhost_and_answers() {
    let d = tempfile::tempdir().unwrap();
    fixture(d.path());
    let mut child = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["dashboard", "--port", "0", "--root"])
        .arg(d.path().join("repos"))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let url = line.trim().strip_prefix("blyte dashboard: http://").unwrap_or_else(|| panic!("{line}"));
    assert!(url.starts_with("127.0.0.1:"), "{url}");
    let mut s = std::net::TcpStream::connect(url).unwrap();
    s.write_all(b"GET /api/jobs HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    child.kill().unwrap();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.contains("\"jobs\""), "{resp}");
}

#[test]
fn dispatch_tags_are_accepted() {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["dispatch", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("--tag"));
}
```

  Run: `cargo test -p blyte --test test_dashboard`. Expected: it fails to compile (no `dashboard` module).

- [ ] **Step 2:** implement `dashboard.rs`, the `Dashboard` command in `cli.rs` and `main.rs`,
  `pub mod dashboard;` in `lib.rs`, and `--tag` in `dispatch.rs`, so the tests pass. The server
  writes `HTTP/1.1 <code> <reason>\r\nContent-Type: …\r\nContent-Length: …\r\nConnection:
  close\r\n\r\n<body>` and closes the connection.
- [ ] **Step 3:** docs as above; run the checks; commit:
  `git commit --no-gpg-sign -m "feat(cli): blyte dashboard serves running and recent jobs locally"`

---

### Task 3: the page

**Files:** replace `crates/blyte-cli/assets/dashboard/index.html`; create
`crates/blyte-cli/tests/test_dashboard_page.rs`.

One self-contained HTML file (inline CSS and JS, no framework, no external request of any kind:
no fonts, scripts or images from the network) that implements the approved design against the
data contract:

- **Header:** eyebrow `blyte · this machine`, the heading `What blyte is working on.`, and a live
  indicator `Live · updated HH:MM:SS` (a jade dot), which turns into `Offline · last update
  HH:MM:SS` (a grey dot) when a fetch fails.
- **Summary strip:** five tiles from `summary`: running now; jobs in the last 24 hours with the
  number of repositories; passed their check (`passed_24h` of `finished_24h`, as a percentage);
  Gemini cost in the last 24 hours (`$0.00`, plus the average per finished job); and jobs needing
  attention (the count in that view).
- **Views** (pill buttons): All, Running, Needs attention (`gate_failed`, `error`,
  `budget_exhausted`, `stopped`, or a running job with `gate_attempts` ≥ 1 and `gate_running`
  false after a failure: treat `running` with `gate_attempts` > 0 as retrying), Done (`verified`,
  `no_changes`). Each shows its count.
- **Table:** a header row (Status, Job, Repository, Time, Cost, Turns) and one row per job: a status
  dot and label (Running, In gate when `gate_running`, Retrying, Verified, Gate failed, Out of
  budget, No changes, Error, Stopped), the `title` with `now` under it (one line each, ellipsis),
  `repo` in mono, elapsed time (`12m 40s`, `1h 03m`), cost (`$0.62`, `—` when null), turns. Under
  each row, its `tags` as small pill buttons.
- **Click a row** (a real `<button>` with `aria-expanded`) to expand it: three columns: Task (the
  task text, first 600 characters, `plan`), Progress (last tool, gate attempts and whether one is
  running, changed files, and the gate failures in a mono block), Where (worktree, log path, started
  time, verdict, commits as `sha subject`). The expanded row stays expanded across refreshes (keyed
  by `key`).
- **Click a tag** to filter the table to jobs carrying it; a banner `Showing jobs tagged <tag>` with
  a Clear button; clicking the active tag again clears it. The view and tag survive refreshes.
- Fetch `/api/jobs` on load and every 5 seconds.
- Colours (Climate X): page `#f9f9fa`, cards `#ffffff`, lines `#eaecee` / `#ced4db`, text
  `#343a40`, strong `#212529`, muted `#6d767f`, faint `#adb5bd`; status: running and in gate
  `#212529` / amber `#e8a33d`, retrying amber, verified jade `#2bad72` (label `#1c7c51`), failed and
  error red `#e52321` (label `#a01920`), stopped and no changes grey `#adb5bd`. Red appears only on
  failures and the focus ring. Type: `"Graphik", "Helvetica Neue", Helvetica, Arial, sans-serif`;
  mono `SFMono-Regular, Menlo, monospace`; tabular numbers for times and costs. A dark theme under
  `@media (prefers-color-scheme: dark)` (ground `#171b1e`, cards `#1f2428`, text `#e2e6e9`, same
  status colours). Keyboard focus visible; targets at least 44 px high; works at 400 px wide (the
  table scrolls sideways inside its own container).
- Empty state: `Nothing running, and no jobs in the last 24 hours.`

- [ ] **Step 1: test.** Create `crates/blyte-cli/tests/test_dashboard_page.rs` exactly:

```rust
//! The dashboard page is one self-contained file that talks only to its own server (2026-09-30).

const PAGE: &str = include_str!("../assets/dashboard/index.html");

#[test]
fn the_page_polls_its_own_api_and_loads_nothing_from_the_network() {
    assert!(PAGE.contains("/api/jobs"));
    assert!(PAGE.contains("<html"));
    assert!(PAGE.contains("What blyte is working on."));
    assert!(PAGE.contains("prefers-color-scheme: dark"));
    for forbidden in ["http://", "https://", "//fonts.", "<link rel=\"stylesheet\"", "<script src"] {
        assert!(!PAGE.contains(forbidden), "the page must not load {forbidden}");
    }
}

#[test]
fn every_status_in_the_contract_has_a_label() {
    for s in ["running", "verified", "gate_failed", "budget_exhausted", "no_changes", "error", "stopped"] {
        assert!(PAGE.contains(&format!("{s}:")) || PAGE.contains(&format!("\"{s}\"")), "{s}");
    }
}
```

  Run: `cargo test -p blyte --test test_dashboard_page`. Expected: it fails (the placeholder has
  none of this).

- [ ] **Step 2:** write the page, run the checks, commit:
  `git commit --no-gpg-sign -m "feat(cli): the dashboard page"`
