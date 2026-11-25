# Merged runs without a verdict, and silent failures

Date: 2026-11-25

Two problems, one pull request.

1. A run whose work was combined (cherry-picked) into a request branch and merged never gets a
   verdict. Once `blyte clean` removes its `wip/<id>` branch, `blyte plan` and the dashboard's Plans
   view cannot tell it was merged: both require an `accepted` or `fixed` verdict when the branch is
   gone. The task stays in `review` for ever and tasks that depend on it never become `ready`.
   Decision: do not write a verdict automatically (a verdict is a reviewer's judgement, and the
   acceptance figures are computed from them). Instead `blyte clean` records why it removed each run
   in `.wt/<id>.removed.json`, and the plan views read that record.
2. An audit found places where a missing, unreadable or corrupt file is silently treated as absent
   or replaced by a placeholder. Each becomes an error that names the file.

Rules for every task:

- **No fallbacks.** A file that does not exist may mean "nothing there" where that is the documented
  meaning. A file that exists but cannot be read or parsed is an error whose message names the path
  and the cause. Never substitute `""`, `"unknown"`, `"?"`, `0`, `false` or an empty list.
- Nothing here is a long-running job, so "resumable" does not apply.
- Existing tests pass unmodified unless they asserted the old silent behaviour; change only those and
  say which in the final message.
- Every new behaviour has a test that fails before the change. Tests must not need the network.
- Change only the files the task names. The other tasks run in parallel in other worktrees; their
  changes are not in this worktree, so do not add or "fix" them here.
- Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && python3 scripts/test-changed.py`

## The removal record (contract between Task RR and Task PM)

`blyte clean` (and the clean pass at the start of `blyte dispatch`) writes
`.wt/<id>.removed.json` immediately before it removes a run's worktree and `wip/<id>` branch:

```json
{"id":"T1","reason":"merged","detail":null,"removed_at":1795000000}
```

- `reason` is one of `merged`, `merged_into`, `merged_through`, `session_merged`, `verdict`,
  `no_changes`, `older_than`, plus one snake_case name for each research variant of
  `clean::RemovalReason`. One name per variant, no catch-all.
- `detail` is the variant's payload as a string where it has one (the branch merged into, the
  follow-up run merged through, the verdict, the age in days), otherwise `null`.
- `removed_at` is Unix seconds.
- A run counts as **merged** when `reason` is `merged`, `merged_into`, `merged_through` or
  `session_merged`.

### Task RR: clean records why it removed a run; corrupt verdict files are errors

**Depends on:** none. **Siblings:** PM (reads the record; based on this task), AG, DS, QW, DB.

**Files:** `crates/blyte-cli/src/clean.rs`, new `crates/blyte-cli/src/removal_record.rs`,
`crates/blyte-cli/src/main.rs` (the `mod` line only), tests under `crates/blyte-cli/tests/` or in
the modules.

**Build:**

1. `removal_record.rs` with:
   - `pub struct RemovalRecord { pub id: String, pub reason: String, pub detail: Option<String>, pub removed_at: u64 }` (serde).
   - `pub fn path(root: &Path, id: &str) -> PathBuf` → `<root>/.wt/<id>.removed.json`.
   - `pub fn write(root: &Path, record: &RemovalRecord) -> Result<(), String>`: write to a temporary
     file in `.wt` and rename, so a reader never sees half a file.
   - `pub fn read(root: &Path, id: &str) -> Result<Option<RemovalRecord>, String>`: `Ok(None)` only
     when the file does not exist; unreadable or invalid JSON, or an unknown `reason`, is `Err`
     naming the path.
   - `pub fn is_merged(record: &RemovalRecord) -> bool` per the contract above.
   - `pub fn reason_name(reason: &clean::RemovalReason) -> (&'static str, Option<String>)` (or the
     equivalent on the enum), an exhaustive `match` with no `_` arm.
2. In `clean.rs`, at every place a run is actually removed (not in `--dry-run`), write the record
   before removing the worktree and branch. If the write fails, do not remove that run: report the
   error for it and continue with the others, and make `blyte clean` exit non-zero at the end.
3. `blyte clean` must never delete `.wt/<id>.removed.json` (check the code that removes a run's
   `.wt/<id>.*` files and any code that sweeps orphaned files).
4. The audit item below: a `.verdict.json` or child `.json` that exists but cannot be read or parsed
   is an error naming the file; that run is not removed; clean continues and exits non-zero.

**Tests (each must fail before the change):**

- A run merged into the default branch is removed by `blyte clean` and leaves
  `.wt/<id>.removed.json` with `reason: "merged"`, the right `id`, and `removed_at > 0`.
- A run with verdict `redone` leaves `reason: "verdict"`, `detail: "redone"`.
- `--dry-run` writes no record.
- A second `blyte clean` leaves the record in place, byte for byte.
- `removal_record::read`: missing file → `Ok(None)`; `{` → `Err` containing the path; unknown reason
  → `Err`.
- A run with a corrupt `.verdict.json` (`not json`): clean exits non-zero, stderr names the file, and
  the run's worktree and branch still exist.

**Commit message:** `fix: clean records why it removed a run; corrupt verdict files are errors`

**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 8. `crates/blyte-cli/src/clean.rs:1115-1123, 1490-1498, 3092-3100` — Corrupt `.verdict.json` (and child `.json` at `1134-1142`) coerced to empty string / `false`
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/clean.rs:1489-1501`
```rs
        let verdict_str = if verdict_path.is_file() {
            std::fs::read_to_string(&verdict_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| {
                    v.get("verdict")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string())
                })
                .unwrap_or_default()
        } else {
            String::new()
        };
```
- **Substituted:** `.ok().and_then(|s| serde_json::from_str(...).ok())...unwrap_or_default()` turns an unreadable or corrupt `.verdict.json` into `""` (and corrupt `<child>.json` at `1134-1142` into `is_verified = false`).
- **Wrongly told:** `blyte clean` prints `kept .wt/<id> (no verdict)` (or misses `"redone"` / `"accepted"`) instead of reporting an unreadable verdict file (contrast `KeptReason::UnreadableStartRecord` in the same module).
- **Verdict:** **Real violation** — existing corrupt state files must be reported, not treated as absent.


### Task PM: plan views treat a run removed as merged as merged; unreadable run files are errors

**Depends on:** RR (dispatch with `--base wip/RR`; `removal_record.rs` is in this worktree).
**Siblings:** AG, DS, QW, DB.

**Files:** `crates/blyte-cli/src/plan_cmd.rs`, `crates/blyte-cli/src/plans.rs`, their tests.

**Build:**

1. One shared function decides whether a run's work is merged, used by both
   `plan_cmd.rs` (the `is_merged` expression in the status loop) and `plans.rs::is_job_merged`:
   - if `refs/heads/wip/<id>` exists: the existing test for that view stays as it is
     (`check_is_ancestor` with commits in `plan_cmd.rs`; `is_merged(root, branch, sha)` in `plans.rs`);
   - if the branch is gone: merged when the verdict is `accepted` or `fixed` (as now), **or** when
     `removal_record::read(root, id)` returns a record for which `removal_record::is_merged` is true.
   - A removal record with any other reason (`no_changes`, `older_than`, `verdict`) is not merged.
   Put the branch-gone part in one `pub(crate)` function (in `plans.rs` or `removal_record.rs`'s
   caller side, not duplicated) and call it from both places.
2. An error from `removal_record::read` is not swallowed: `blyte plan` fails with it (exit non-zero,
   message names the file). In `plans.rs`, where the function returns `bool` for the dashboard, carry
   the error to the place the dashboard already reports per-run or per-repository problems; if there
   is no such place, print it once to stderr with `eprintln!("blyte: {e}")` and treat the task state
   as not merged. Say which you did in the final message.
3. The two audit items below (`read_start_files`, `read_verdict`, `read_summary`, and the
   `"unknown"` / `""` renderings): `read_start_files` returns `Result`; only `.wt` not existing
   means no runs; an unreadable or corrupt `.start.json`, `.verdict.json` or `.json` is an error
   naming the file, and `blyte plan` exits non-zero with it.

**Tests (each must fail before the change):**

- `blyte plan <plan>` with: task A's run has a summary with commits, no verdict, no `wip/A` branch,
  and `.wt/A.removed.json` with `reason: "merged"`; task B depends on A. A shows `merged` and B shows
  `ready`. The same set-up with `reason: "older_than"` shows A as `review` and B not `ready`.
- The same for `reason: "merged_into"`, `"merged_through"` and `"session_merged"` (merged).
- `plans.rs`: a `Job` with no verdict, branch gone and a merged removal record is merged; without the
  record it is not.
- `blyte plan` with a corrupt `.wt/X.start.json` exits non-zero and names the file; the same for a
  corrupt `.wt/X.verdict.json`, `.wt/X.json` and `.wt/X.removed.json`.
- `blyte plan` in a repository with no `.wt` directory still works (no runs).

**Commit message:** `fix: plan views count a run removed as merged; unreadable run files fail blyte plan`

**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 4. `crates/blyte-cli/src/plan_cmd.rs:75, 85, 89` — Unreadable `.wt` or `.start.json` silently skipped in `blyte plan`
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/plan_cmd.rs:71-90`
```rs
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
```
- **Substituted:** `Err(_) => return Vec::new()` if `.wt` cannot be read (conflating permission/I/O errors with missing `.wt`), and `Err(_) => continue` if any `.start.json` fails to read or parse.
- **Wrongly told:** `blyte plan` reports already-running or finished tasks as `ready` (emitting a fresh `blyte dispatch` command) or `waiting`, causing duplicate dispatches.
- **Verdict:** **Real violation** — only `ErrorKind::NotFound` on `.wt` means "no runs yet"; unreadable or corrupt `.start.json` files must fail naming the file.



**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 12. `crates/blyte-cli/src/plan_cmd.rs:144-162, 385-386, 394` — Corrupt verdict/summary ignored and missing outcome/run rendered as `"unknown"` / `""`
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/plan_cmd.rs:150-163`
```rs
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
```
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/plan_cmd.rs:381-397`
```rs
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
```
- **Substituted:** `.ok()` discards corrupt `.verdict.json` (`145-146`) and `.json` (`153-154`); `render` substitutes `"unknown"` for missing `outcome` (`385`) and `""` for missing `run` (`386, 394`).
- **Wrongly told:** A finished task whose summary JSON is corrupt is shown as `dead` (`blyte status <run>`), and a review row without an outcome prints `unknown, run : blyte review `.
- **Verdict:** **Real violation** — corrupt summaries and missing required fields must produce an explicit error.


### Task AG: agent crate: failed environment probe and dispatch tool write errors

**Depends on:** none. **Siblings:** RR, PM, DS, QW, DB.

**Files:** `crates/blyte-agent/src/shell.rs`, `crates/blyte-agent/src/tools.rs`, and the callers
inside `crates/blyte-agent` that the changed signatures require; their tests.

**Build:** the two audit items below. The environment probe returns `Result`; its callers surface
the error (naming the command that failed and its exit status or I/O error) instead of continuing
with an empty list. In the `dispatch` tool, an argument serialisation or stdin write failure returns
`ToolOutcome::err` naming what failed. Do not change sandbox profiles or which environment
variables are passed through.

**Tests (each must fail before the change):** a probe whose command cannot be spawned or exits
non-zero returns `Err` naming it (use a seam or an injected command; do not depend on the host's
tools); the dispatch tool returns an error outcome when the stdin write fails (use a child that
closes stdin at once, or a seam).

**Commit message:** `fix: a failed environment probe or dispatch tool write is an error`

**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 7. `crates/blyte-agent/src/shell.rs:220` — Failed environment probe returns empty binary list
`/Users/danielfisher/repos/blyte/crates/blyte-agent/src/shell.rs:213-222`
```rs
        match self.exec(&probe, cwd, Duration::from_secs(30)) {
            Ok(o) => o
                .stdout
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect(),
            Err(_) => Vec::new(),
        }
    }
```
- **Substituted:** `Err(_) => Vec::new()` when `self.exec(&probe, ...)` fails (broken container, sandbox failure, missing bash).
- **Wrongly told:** The run proceeds and the system prompt omits all available binaries (telling the model none exist) instead of failing immediately with the shell error.
- **Verdict:** **Real violation** — a broken shell at startup should fail naming the error rather than producing an empty capability list.



**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 20. `crates/blyte-agent/src/tools.rs:1272-1273` — `dispatch` tool ignores arg serialization and stdin write errors
`/Users/danielfisher/repos/blyte/crates/blyte-agent/src/tools.rs:1271-1275`
```rs
            if let Some(mut stdin) = child.stdin.take() {
                let json_bytes = serde_json::to_vec(args).unwrap_or_default();
                let _ = std::io::Write::write_all(&mut stdin, &json_bytes);
            }

```
- **Substituted:** `serde_json::to_vec(args).unwrap_or_default()` sends empty bytes on serialization error, and `let _ = write_all(...)` discards broken-pipe/write failures.
- **Wrongly told:** The child command receives empty/truncated stdin and fails with a confusing downstream parse error instead of the actual serialization or stdin write error.
- **Verdict:** **Real violation** — fail with `ToolOutcome::err` naming the serialization/write error.


### Task DS: dispatch, status, setup and watchdog no longer hide file failures

**Depends on:** none. **Siblings:** RR, PM, AG, QW, DB.

**Files:** `crates/blyte-cli/src/dispatch.rs`, `crates/blyte-cli/src/setup.rs`,
`crates/blyte-cli/src/task_runner.rs`, their tests. Callers of `setup::run_setup` in other files may
change only as far as a changed signature forces.

**Build:** the audit items below, plus:

- `setup::run_setup`'s error text ends with a line `full log: <absolute path of the setup log>`, and
  the tail is taken so that it keeps the last 20 lines of stdout **and** the last 20 lines of stderr
  (each under a `stdout:` / `stderr:` label, a label omitted when that stream is empty), not the
  last 20 lines of stdout-then-stderr joined.
- If the setup log cannot be written, the error says so and names the path (in addition to the
  setup failure itself); a setup that succeeded but whose log could not be written is an error.
- `blyte status <id>`: a `.report.json` or `.log` that exists but cannot be read or parsed is an
  error naming the file, exit non-zero; it is not reported as `dead`.

**Tests (each must fail before the change):**

- A failing setup command that prints 30 lines to stdout (the cause on stdout line 30) then 30 lines
  to stderr: the returned error contains the stdout cause line, the last stderr line, and
  `full log: ` followed by the log path.
- `blyte status X` with `.wt/X.report.json` containing `not json` exits non-zero and names the file.
- An unreadable `blyte.toml` at the gate-drift check (a directory at that path is enough) is an
  error naming it.
- The watchdog report write failure is returned or logged with the path, and `model_id` is not `""`
  (test at the unit level with a seam).

**Commit message:** `fix: dispatch, status and setup report unreadable files; setup errors name the full log`

**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 10. `crates/blyte-cli/src/dispatch.rs:1122` — Unreadable `blyte.toml` replaced with `""` during gate-drift check
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/dispatch.rs:1116-1134`
```rs
    } else if let Some(ref g) = dispatch_cfg.gate {
        let toml_path = if from_main {
            root.join("blyte.toml")
        } else {
            start_dir.join("blyte.toml")
        };
        let toml_content = std::fs::read_to_string(&toml_path).unwrap_or_default();
        left_out = crate::gate_drift::left_out_steps(&toml_content);
        if g.trim().is_empty() {
            eprintln!(
                "error: the gate in blyte.toml is empty; every step was left out. Fix the steps blyte init named, or pass --gate."
            );
            for (cmd, reason) in &left_out {
                eprintln!("  {cmd}: {reason}");
            }
            return 2;
        }
        let missing = crate::gate_drift::missing_ci_steps(&root, g, &toml_content);
        if let Some(warning) = crate::gate_drift::format_gate_drift_warning(&missing) {
```
- **Substituted:** `std::fs::read_to_string(&toml_path).unwrap_or_default()` substitutes `""`.
- **Wrongly told:** Silently suppresses gate-drift warnings and omits `left_out` steps from the run record and start record.
- **Verdict:** **Real violation** — `blyte.toml` is known to exist on this branch; a read error must fail naming `toml_path`.

##### 11. `crates/blyte-cli/src/dispatch.rs:465, 477` — Ignored `child.wait()` and `gate_left_out` report rewrite failure
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/dispatch.rs:465-481`
```rs
    let _ = child.wait();

    if !cfg.left_out.is_empty() {
        if let Ok(report_str) = std::fs::read_to_string(report_path) {
            if let Ok(mut report_val) = serde_json::from_str::<serde_json::Value>(&report_str) {
                let formatted: Vec<String> = cfg
                    .left_out
                    .iter()
                    .map(|(cmd_name, reason)| format!("{cmd_name} ({reason})"))
                    .collect();
                report_val["gate_left_out"] = serde_json::json!(formatted);
                if let Ok(updated) = serde_json::to_string_pretty(&report_val) {
                    let _ = std::fs::write(report_path, updated + "\n");
                }
            }
        }
    }
```
- **Substituted:** Discards `child.wait()` error (`465`) and silently drops read/parse/write failures when patching `gate_left_out` into `report_path` (`468-478`).
- **Wrongly told:** Downstream review/report readers are not told that gate steps were left out when writing `gate_left_out` fails.
- **Verdict:** **Real violation** — discarding the write error hides missing gate coverage from reviewers.



**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 17. `crates/blyte-cli/src/dispatch.rs:3079-3115, 3128` — `blyte status` silently hides corrupt `.report.json` and unreadable `.log`
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/dispatch.rs:3095-3129`
```rs
    let (outcome, cost_usd, final_message, report_duration) = if state == "finished" {
        if let Ok(content) = std::fs::read_to_string(&report_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                let outcome = v.get("outcome").and_then(|x| x.as_str()).map(String::from);
                let cost_usd = v.get("cost_usd").and_then(|x| x.as_f64());
                let final_message = v.get("final_message").and_then(|x| x.as_str()).map(|s| {
                    if s.chars().count() > 500 {
                        s.chars().take(500).collect()
                    } else {
                        s.to_string()
                    }
                });
                let duration = v.get("duration_seconds").and_then(|x| x.as_f64());
                (outcome, cost_usd, final_message, duration)
            } else {
                (None, None, None, None)
            }
        } else {
            (None, None, None, None)
        }
    } else {
        (None, None, None, None)
    };

    let elapsed_s = if state == "finished" {
        report_duration
            .map(|d| d.round() as u64)
            .unwrap_or_else(|| now.saturating_sub(started_at))
    } else {
        now.saturating_sub(started_at)
    };

    let log_path = root.join(".wt").join(format!("{id}.log"));
    let log_content = std::fs::read_to_string(&log_path).unwrap_or_default();
    let progress = parse_log(&log_content);
```
- **Substituted:** Unreadable/corrupt `.report.json` becomes `(None, None, None, None)` while still reporting `state: finished` and exiting `0`; unreadable `.log` becomes `""` (`turn: 0`, `last_tool: none`, `gate_attempts: 0`).
- **Wrongly told:** User is told the run is `finished` with `turn: 0` and no outcome/error even when the report JSON is corrupt or the log cannot be read (and `3062` defaults missing `started_at` to `0`, making `elapsed_s` equal the Unix epoch ~1.7B seconds!).
- **Verdict:** **Real violation** — existing unreadable/corrupt report or log files must fail naming what failed.

##### 18. `crates/blyte-cli/src/task_runner.rs:480-486` — Watchdog discards report write error and substitutes `""` for `model_id`
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/task_runner.rs:480-488`
```rs
                let model_id = blyte_provider::resolve_provider(watchdog_config.model.as_deref())
                    .map(|pr| pr.model_id())
                    .ok()
                    .or_else(|| watchdog_config.model.clone())
                    .unwrap_or_default();
                record.cost_usd = blyte_provider::pricing(&model_id).map(|pr| pr.cost(&p.usage));
                let _ = crate::report::write(path, &record);
            }
            eprintln!("blyte: {msg}");
```
- **Substituted:** `resolve_provider(...).ok()...unwrap_or_default()` substitutes `""` for `model_id` (making `pricing("")` return `None` -> `cost_usd: None`), and `let _ = crate::report::write(path, &record)` ignores write failure.
- **Wrongly told:** Watchdog report omits spend (`cost_usd: null`) or is never written at all with no error logged to stderr.
- **Verdict:** **Real violation** — write failure should be printed to stderr before `exit(1)` and provider resolution failure should be noted rather than silently zeroing cost lookup.

##### 19. `crates/blyte-cli/src/setup.rs:48, 65` — Silent drop of `.setup.log` write failure
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/setup.rs:47-66`
```rs
            let combined = output.merged();
            let _ = std::fs::write(log_path, &combined);
            if output.exit_code == 0 {
                return Ok(());
            }
            let is_timeout = output.exit_code == 124 && output.stderr.contains("killed by blyte");
            let header = if is_timeout {
                if timeout.as_secs() > 0 {
                    format!("setup failed: timed out after {}s", timeout.as_secs())
                } else {
                    "setup failed: timed out".to_string()
                }
            } else {
                format!("setup failed (exit {})", output.exit_code)
            };
            (header, combined)
        }
        Err(e) => {
            let _ = std::fs::write(log_path, format!("{e}\n"));
            let header = format!("setup failed: {e}");
```
- **Substituted:** `let _ = std::fs::write(log_path, ...)` discards I/O errors writing the full setup log.
- **Wrongly told:** `dispatch.rs:1934` records `summary.log = setup_log_path` even when `.setup.log` failed to write and does not exist on disk.
- **Verdict:** **Real violation** — failing to write the setup log should surface an error rather than advertising a nonexistent log path.


### Task QW: review, wait, model review and fallback check no longer use placeholders

**Depends on:** none. **Siblings:** RR, PM, AG, DS, DB.

**Files:** `crates/blyte-cli/src/queue_actions.rs`, `crates/blyte-cli/src/wait.rs`,
`crates/blyte-cli/src/model_review.rs`, `crates/blyte-cli/src/fallback_check.rs`, their tests.

**Build:** the audit items below. `blyte wait` keeps exit 2 for "none finished yet"; a summary that
exists but is corrupt, or lacks `outcome` or `turns`, is an error naming the file with a different
non-zero exit (use 1). A summary still being written must not be read half-way: check how the
summary is written (rename or direct write) and, if it is a direct write, treat a JSON parse failure
of a file modified in the last 2 seconds as "not finished yet" rather than as corrupt; say what you
found in the final message.

**Tests (each must fail before the change):**

- `blyte review X` with a corrupt `.wt/X.report.json` exits non-zero and names the file; with an
  unreadable `.wt/X.log` (a directory at that path) likewise.
- `blyte wait X` with `.wt/X.json` = `{"id":"X"}` (older than 2 seconds) exits 1 and names the file
  and the missing field; it prints no `?`.
- A malformed model-review response produces an error carrying the response's actual content (or its
  first 200 characters), not `"unknown error"` or `"?"`.
- A `.model-review.json` that cannot be saved (its parent is a file) is an error naming the path.

**Commit message:** `fix: review, wait and model review name corrupt files instead of using placeholders`

**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 9. `crates/blyte-cli/src/queue_actions.rs:435, 450-457` — Unreadable `.log` or corrupt `.report.json` silently ignored in review checks
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/queue_actions.rs:434-457`
```rs
    let log_file = root.join(".wt").join(format!("{id}.log"));
    let log_content_opt = std::fs::read_to_string(&log_file).ok();
    let (hcmds, reads, (refused_count, refused_paths)) =
        if let Some(ref log_content) = log_content_opt {
            let hcmds = host_commands(log_content);
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"));
            let allowed = [root.to_path_buf(), wt.clone()];
            let reads = outside_reads(log_content, &allowed, &home);
            let refused = refused_reads(log_content);
            (hcmds, reads, refused)
        } else {
            (Vec::new(), Vec::new(), (0, Vec::new()))
        };

    let report_val = std::fs::read_to_string(&report_file)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        .or_else(|| {
            std::fs::read_to_string(&summary_file)
                .ok()
                .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
        });
```
- **Substituted:** `.ok()` turns unreadable `.wt/<id>.log` into empty host-command / outside-read / refusal lists, and `.ok()` turns corrupt `.report.json` / `.json` into `None`.
- **Wrongly told:** `blyte review` and the surface review queue show `clean` with zero security/outside-read flags even when the log was unreadable, or omit the report without warning when `.report.json` was corrupt.
- **Verdict:** **Real violation** — only `NotFound` means a file is absent; I/O or JSON parse failures must error.



**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 13. `crates/blyte-cli/src/wait.rs:28-34, 56, 64` — Corrupt summary treated as `dead`; missing `outcome`/`turns` rendered as `"?"`
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/wait.rs:26-38`
```rs
fn try_read_summary(wt: &Path, id: &str) -> Option<serde_json::Value> {
    let path = wt.join(format!("{id}.json"));
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

fn read_start_pid(path: &Path) -> Option<libc::pid_t> {
    let s = std::fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&s).ok()?;
    val.get("pid")
        .and_then(|v| v.as_i64())
        .map(|v| v as libc::pid_t)
}
```
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/wait.rs:52-70`
```rs
fn format_summary_line(id: &str, summary: &serde_json::Value) -> String {
    let outcome = summary
        .get("outcome")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let turns = summary
        .get("turns")
        .and_then(|v| {
            v.as_i64()
                .map(|t| t.to_string())
                .or_else(|| v.as_u64().map(|t| t.to_string()))
        })
        .unwrap_or_else(|| "?".to_string());
    let cost = match summary.get("cost_usd").and_then(|v| v.as_f64()) {
        Some(c) => format!("${:.2}", c),
        None => "-".to_string(),
    };
    format!("{id} {outcome} {turns} {cost}")
}
```
- **Substituted:** `.ok()?` hides corrupt `.json` and `.start.json`; `unwrap_or("?")` (`56`) and `unwrap_or_else(|| "?")` (`64`) stand in for missing `outcome` and `turns`.
- **Wrongly told:** `blyte wait` prints `<id> dead - -` for a finished run with corrupt `.json`, or `<id> ? ? -` when `outcome` or `turns` is missing.
- **Verdict:** **Real violation** — `"?"` is a direct placeholder for unknown required fields, and parse errors on existing files should be reported.



**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 15. `crates/blyte-cli/src/model_review.rs:99, 806, 816` & `crates/blyte-cli/src/fallback_check.rs:150` — `"unknown error"` and `"?"` placeholders on malformed review results
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/model_review.rs:97-102`
```rs
    pub fn flag(&self) -> Option<String> {
        if self.is_failed() {
            let reason = self.error.as_deref().unwrap_or("unknown error");
            return Some(sanitize_flag(format!("model review failed: {reason}")));
        }
        if self.has_findings() {
```
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/model_review.rs:804-821`
```rs
pub fn print_result(result: &ModelReviewResult) {
    if result.is_failed() {
        let err = result.error.as_deref().unwrap_or("unknown error");
        println!("== model review");
        if err == "task file not found" || err.starts_with("no API key (set ") {
            println!("skipped: {err}");
        } else {
            println!("error: {err}");
        }
        return;
    }

    let model_id = result.model.as_deref().unwrap_or("?");
    let cost_str = match result.cost_usd {
        Some(c) => format!("${c:.4}"),
        None => "?".to_string(),
    };
    println!("== model review (trial: {model_id}, {cost_str})");
```
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/fallback_check.rs:149-152`
```rs
    if mr.is_failed() {
        let why = mr.error.as_deref().unwrap_or("unknown error");
        return vec![format_not_judged_why(candidates.len(), why)];
    }
```
- **Substituted:** `unwrap_or("unknown error")` when `is_failed()` has `error: None`, and `unwrap_or("?")` for missing `model` (and `"?"` for `cost_usd: None`).
- **Wrongly told:** Review flags and CLI output say `model review failed: unknown error`, `not judged (unknown error)`, or `(trial: ?, ?)` instead of rejecting an invalid review record missing its error/model.
- **Verdict:** **Real violation** for `error` (`99, 806`, `fallback_check.rs:150`) and `model` (`816`) (note `cost_usd: None` on line `819` is valid when a model has no pricing entry, though `"price unknown"` is clearer than `"?"`).

##### 16. `crates/blyte-cli/src/model_review.rs:895` — Silent drop of `.model-review.json` save failure
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/model_review.rs:880-897`
```rs
    let _ = std::fs::create_dir_all(&wt_dir);
    let review_path = wt_dir.join(format!("{id}.model-review.json"));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let record = SavedModelReview {
        model: model.map(str::to_string),
        cost_usd,
        concerns,
        fallbacks,
        ts,
        error,
    };
    if let Ok(json_str) = serde_json::to_string_pretty(&record) {
        let _ = std::fs::write(&review_path, json_str + "\n");
    }
}
```
- **Substituted:** `let _ = std::fs::write(&review_path, ...)` and `if let Ok(...)` discard serialization/write errors.
- **Wrongly told:** Fresh model review completes and charges API cost, but its result is silently lost from `.wt/<id>.model-review.json` without informing the user or caller.
- **Verdict:** **Real violation** — failing to persist a review artifact must be surfaced.


### Task DB: dashboard reports unreadable run files; update rollback does not invent a version

**Depends on:** none. **Siblings:** RR, PM, AG, DS, QW.

**Files:** `crates/blyte-cli/src/dashboard.rs`, `crates/blyte-cli/src/update.rs`, their tests. Do
not change `crates/blyte-cli/src/plans.rs` (Task PM owns it).

**Build:** the audit items below. The dashboard must keep rendering when one run's files are bad:
collect a per-run problem (`<path>: <cause>`) and show it on that run's row or card in the place the
dashboard already shows per-run notes or errors; if there is none, add a `problems: Vec<String>`
field to the job and render it as a plainly visible line on the row. A missing file keeps its
existing meaning. A report without `outcome` is a problem, not `"unknown"`. Leave the repository
name items (`dashboard.rs` "unknown" repo name) alone.

In `update.rs`, a rolled-back binary that fails `--version` is an error that says the rollback
binary could not be verified and gives its path.

**Tests (each must fail before the change):**

- A run directory with `.wt/X.report.json` = `not json`: the collected job for X carries a problem
  naming that file, and other runs in the same repository are still listed.
- A report lacking `outcome`: a problem naming the file and the field; the job's outcome is not
  `"unknown"`.
- The update rollback path with a binary that exits non-zero on `--version` returns `Err`.

**Commit message:** `fix: dashboard shows unreadable run files as problems; update rollback verifies the binary`

**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 14. `crates/blyte-cli/src/dashboard.rs:1175` — Missing `outcome` in `.report.json` replaced with `"unknown"`
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/dashboard.rs:1174-1182`
```rs
        ) = if let Some(ref report) = report_json {
            let outcome = report["outcome"].as_str().unwrap_or("unknown").to_string();
            let cost = report["cost_usd"].as_f64();
            let turns = report["turns"].as_u64().unwrap_or(log_info.turns);
            let gate_attempts = report["gate_attempts"]
                .as_u64()
                .unwrap_or(log_info.gate_attempts);
            let model = report["model_id"].as_str().map(|s| s.to_string());

```
- **Substituted:** `report["outcome"].as_str().unwrap_or("unknown")` substitutes `"unknown"` (and `1190` substitutes `""` for non-string `gate_failures` elements).
- **Wrongly told:** Dashboard shows a run's status as `"unknown"` instead of reporting a malformed `.report.json`.
- **Verdict:** **Real violation** — explicit `"unknown"` placeholder for a required schema field.



**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 21. `crates/blyte-cli/src/dashboard.rs:416, 767-768, 1119-1121, 1140-1142, 1155-1157` — Unreadable/corrupt run artifacts silently treated as absent
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/dashboard.rs:1137-1145`
```rs
        let log_info = parse_log(&raw.log_path);

        let report_json = if raw.report_path.is_file() {
            std::fs::read_to_string(&raw.report_path)
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        } else {
            None
        };
```
- **Substituted:** `.unwrap_or_default()` on `.log` (`416`) and `.ok()` on `.start.json` (`767-768`), `.verdict.json` (`1119-1121`), `.report.json` (`1140-1142`), and `agy` summary (`1155-1157`).
- **Wrongly told:** A finished run with a corrupt `.report.json` appears as `dead` with 0 turns; a corrupt `.verdict.json` makes a reviewed run appear unreviewed.
- **Verdict:** **Real violation** — corrupt files should be surfaced as unreadable rather than silently dropped.



**Audit excerpt (line numbers are from main at 9e30575d; find the code by content if they have moved):**

##### 23. `crates/blyte-cli/src/update.rs:194` — Rollback returns `Ok("blyte (version unknown)")` when swapped binary fails `--version`
`/Users/danielfisher/repos/blyte/crates/blyte-cli/src/update.rs:190-195`
```rs
    let swap = exe.with_file_name(".blyte-swap");
    std::fs::rename(exe, &swap).map_err(|e| format!("rollback failed: {e}"))?;
    std::fs::rename(&prev, exe).map_err(|e| format!("rollback failed: {e}"))?;
    std::fs::rename(&swap, &prev).map_err(|e| format!("rollback failed: {e}"))?;
    Ok(version_of(exe).unwrap_or_else(|| "blyte (version unknown)".into()))
}
```
- **Substituted:** `version_of(exe).unwrap_or_else(|| "blyte (version unknown)".into())` returns `Ok(...)` even if the rolled-back binary fails to spawn or exits non-zero.
- **Wrongly told:** User is told rollback succeeded to `"blyte (version unknown)"` when the active `blyte` binary may be broken or unexecutable.
- **Verdict:** **Real violation** — `--version` failing on the newly restored binary should fail loudly.


