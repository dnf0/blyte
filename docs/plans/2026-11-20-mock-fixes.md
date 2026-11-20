# Mock fixes plan

Three independent tasks covering the verified problems in `blyte mock`:

- **MF1** — Make `dashboard::collect_jobs` (and thus `GET /api/replay`) fast by resolving git repo names without spawning `git`, pre-filtering out-of-window runs by mtime before reading their files, and speeding up `strip_ansi`.
- **MF2** — Classify research runs as their own replay card kind (`"research"`) and include their report text (`report: Option<String>`) in `replay::Run`.
- **MF3** — Render research runs in the mock page as report-to-read items (`Mark read` / `Hold` / `Read report`, in a `research reports` group) instead of code-diff accept cards, and size review queue cards to their content instead of a hardcoded `184px` height.

(Problem 1 — plan runs titled only `"Task"` — was checked against the code and live `/api/replay` output and found not to exist: all 71 plan-dispatched runs already show their plan task name/heading, while the 22 runs titled `"Task"` were not dispatched from a plan and have `# Task` as their task heading.)

---

### Task MF1: speed up `collect_jobs` and `GET /api/replay`

**Goal:** Eliminate the ~16–17 second latency of `GET /api/replay` (spent 99% inside `dashboard::collect_jobs`) by avoiding `git` subprocess spawns in `repo_name` and runs-dir root lookup, skipping out-of-window historical runs via file mtime before reading their files, and removing char-by-char ANSI stripping overhead when logs contain no escape bytes.

**Files:**
- Modify `crates/blyte-cli/src/dashboard.rs`
- Create `crates/blyte-cli/tests/test_collect_fast.rs`

**What to build:**
1. **Filesystem-only git common-dir resolution (no `git` subprocess spawns):**
   - Add a helper (e.g. `git_common_dir(start: &Path) -> Option<PathBuf>`) in `crates/blyte-cli/src/dashboard.rs` that walks `start.ancestors()` looking for an entry named `.git`:
     - If `<dir>/.git` is a directory, return `Some(<dir>/.git)`.
     - If `<dir>/.git` is a file, read its text and parse the `gitdir: <path>` prefix on the first line (trimmed). Resolve relative `<path>` against `<dir>`. Then check `<gitdir>/commondir`: if that file exists, read its trimmed contents and resolve it (relative to `<gitdir>` if relative) to obtain the common `.git` directory; otherwise use `<gitdir>` itself. Canonicalize or normalize `.` / `..` segments cleanly so `<common>.file_name()` is `".git"` and `<common>.parent()` is the main repository directory.
     - If no `.git` ancestor is found, return `None`.
   - Use `git_common_dir` in `pub fn repo_name(checkout: &Path) -> String`:
     - When `git_common_dir(checkout)` returns `Some(common)` whose `file_name()` is `Some(".git")` and whose `parent()` has a `file_name()`, use that parent name (and apply the existing `split_once("--")` stripping).
     - Otherwise keep the existing `checkout.file_name()` / `canonicalize()` / `"unknown"` logic and `split_once("--")` stripping.
     - Do not spawn `git` in `repo_name`.
   - Use `git_common_dir` in the runs-dir layout of `collect_jobs` (replacing the `Command::new("git").args(["rev-parse", "--git-common-dir"])` block when resolving `root` from a worktree path).

2. **Pre-filter by time window before reading run files in `collect_jobs`:**
   - Compute `let cutoff = now.saturating_sub(src.hours.saturating_mul(3600));` at the top of `collect_jobs`.
   - Keep the exact existing Rule 7 semantics: a job is kept if it is currently `"running"`, OR if its `finished_at` (or, with no report/summary, its log's mtime) is `>= cutoff`.
   - **Dispatch layout (`{id}.start.json`):**
     - After `if !seen_keys.insert(key) { continue; }`:
       - Compute `report_path = wt_dir.join(format!("{id}.report.json"))` and `log_path = wt_dir.join(format!("{id}.log"))`.
       - Check `file_mtime(&report_path)` (which only succeeds if `report_path` exists; check `report_path.is_file()` or `metadata().is_file()` so directories are not treated as files):
         - If `report_path` is a file and its mtime is `Some(m)` with `m < cutoff`, this job has a report (so it cannot be `"running"`) and `finished_at < cutoff` — `continue` immediately without reading `{id}.start.json`, `task_file`, `{id}.json`, `{id}.tags.json`, `{id}.verdict.json`, `{id}.log`, or `{id}.report.json`.
       - If `report_path` is not a file:
         - Check `log_mtime = file_mtime(&log_path)`.
         - Read `{id}.start.json` (which is needed to check `pid`). If `log_mtime.is_none_or(|m| m < cutoff)`: check liveness (`pid.is_some_and(|p| p > 0 && unsafe { libc::kill(p as libc::pid_t, 0) == 0 })`). If not alive, `continue` immediately without reading `task_file`, `{id}.json`, `{id}.tags.json`, `{id}.verdict.json`, or `{id}.log`.
   - **Runs-dir layout (`{id}.log`):**
     - After `if !seen_keys.insert(key) { continue; }`:
       - Compute `report_path = repo_path.join(format!("{id}.report.json"))`, `agy_path = repo_path.join(format!("{id}.json"))`, `log_path = repo_path.join(format!("{id}.log"))`.
       - If `report_path.is_file()` and `file_mtime(&report_path).is_some_and(|m| m < cutoff)`, `continue` immediately without reading any files.
       - Else if `!report_path.is_file()` and `agy_path.is_file()` and `file_mtime(&agy_path).is_some_and(|m| m < cutoff)`, `continue` immediately without reading any files.
       - Else if `!report_path.is_file()` and `!agy_path.is_file()` and `file_mtime(&log_path).is_none_or(|m| m < cutoff)`:
         - Only read `{id}.pid` (and check liveness including `started_with_job` with `get_process_etime`). If not alive, `continue` immediately without reading `.prompt`, `.worktree`, or `.log`.
   - Make sure the post-loop Rule 7 check stays intact for jobs that do pass the pre-filter (for example if a report file is unreadable JSON so `report_json` becomes `None`, or a `pid` exits between phases). Notice one subtle edge case: if `report_path` exists and its mtime is `< cutoff`, wait — what if `report_path` contains invalid JSON and `log_path` has mtime `>= cutoff`? Only pre-skip via `report_mtime < cutoff` when `file_mtime(&log_path).is_none_or(|m| m < cutoff)` as well (checking two `stat`s is negligible and guarantees 100% identical behavior even for corrupt `report.json` with a newer `.log`!).

3. **Fast `strip_ansi`:**
   - Change `strip_ansi(s: &str)` to return `std::borrow::Cow<'_, str>`:
     - If `!s.as_bytes().contains(&0x1b)`, return `Cow::Borrowed(s)` immediately with zero allocation and zero character iteration.
     - Otherwise preserve the exact existing CSI (`\x1b[`) stripping behavior using slice appends between `\x1b` occurrences so non-escape segments are copied in bulk.

**Tests:**
Write `crates/blyte-cli/tests/test_collect_fast.rs` covering:
1. `repo_name_resolves_worktree_nested_and_plain_without_git_subprocess`: creates a temporary git repo (`alpha`), a linked worktree (`alpha-wt`), a nested subdirectory inside the worktree, a non-git directory (`beta`), and a `--` prefixed non-git directory (`gamma--feature`). Verifies `repo_name` returns `"alpha"`, `"alpha"`, `"alpha"`, `"beta"`, `"gamma"` even when `git` is not invoked (verify all assertions against the real worktree layout).
2. `old_finished_runs_are_skipped_without_reading_logs_or_start_json`: creates a temp repo with 200 historical runs whose `.report.json` and `.log` mtimes are 10 days old (set via `std::fs::File::set_times`) — including historical runs whose `.start.json` is intentionally invalid/huge or whose `.log` is multi-hundred-KB — plus 5 recent runs within the 24-hour window. Calls `collect_jobs(&src, now)` and asserts:
   - only the 5 recent runs are returned,
   - `collect_jobs` finishes in well under 1 second (e.g. `< 1000ms`), proving the 200 old runs were skipped by mtime before reading.
3. `old_runs_dir_jobs_are_skipped_by_mtime_while_live_pid_is_kept`: creates a temp `runs_dir` with an old finished agy job (mtime 10 days old), an old dead job (mtime 10 days old, dead pid `999999`), and a live job using `std::process::id()` with an old log mtime. Verifies only the live job is returned.
4. `dispatch_live_pid_with_old_log_mtime_is_kept`: creates a dispatch-layout run with no `report.json`, an old `.log` mtime, and `pid` set to `std::process::id()`. Verifies `collect_jobs` still returns it with `status == "running"`.
5. `strip_ansi_and_parse_log_handle_plain_and_colored_logs`: verifies `collect_jobs` extracts `turns`, `last_tool`, and `gate_attempts` identically from a plain log (no `\x1b`) and an ANSI-colored log (`\x1b[32m[Turn 4]\x1b[0m`, `\x1b[1m● bash\x1b[0m`, `\x1b[32m✓ Gate passed\x1b[0m`).

Run with: `cargo test -p blyte-cli --test test_collect_fast --test test_dashboard_repo_name --test test_replay` (and the full gate).

**Constraints:**
- No fallbacks: fail loudly where errors matter, never substitute guesses or silently swallow corrupt data outside the documented existing behavior.
- Add no comments to code (the gate runs `blyte comments`).
- Existing tests pass unmodified.

**Depends on:** none.

**Siblings:** MF2 (`crates/blyte-cli/src/replay.rs`, `crates/blyte-cli/tests/test_replay.rs`) and MF3 (`crates/blyte-cli/assets/mock/index.html`, `crates/blyte-cli/tests/test_mock_page.rs`) run in parallel in separate worktrees; do not touch their files.

**Commit message:** `perf(dashboard): pre-filter old runs by mtime and avoid git spawns in repo_name`

---

### Task MF2: classify research runs and include report text in `replay`

**Goal:** Represent research runs in `replay::Run` as their own card kind (`"research"`) with their markdown report content (`report: Option<String>`) so `blyte mock` can present them as reports to read rather than code diffs to accept.

**Files:**
- Modify `crates/blyte-cli/src/replay.rs`
- Modify `crates/blyte-cli/tests/test_replay.rs`

**What to build:**
1. **Extend `replay::Run`:**
   - Add `pub report: Option<String>` to `pub struct Run` (serialized to JSON as `"report": string | null`).
2. **Research card classification:**
   - Keep `pub fn card_of(outcome: &str, checks_recorded: bool, flags: &[String]) -> &'static str` unchanged so existing callers and tests stay untouched.
   - Add `pub fn card_for_run(research: bool, outcome: &str, checks_recorded: bool, flags: &[String]) -> &'static str`:
     - If `research` is `true`, return `"research"`.
     - Otherwise return `card_of(outcome, checks_recorded, flags)`.
   - Use `card_for_run(job.research, &summary.outcome, checks_recorded, &flags)` in `collect`.
3. **Read the research report file:**
   - Add a helper `fn read_report(path: &Path) -> Result<Option<String>, String>`:
     - `std::fs::read_to_string(path)`:
       - `Ok(content)` -> `Ok(Some(content))`
       - `Err(e) if e.kind() == ErrorKind::NotFound` -> `Ok(None)`
       - `Err(e)` -> `Err(e.to_string())`
   - In `collect`:
     - When `job.research` is `true`, read `root.join(".wt").join(format!("{}.report.md", job.id))` via `read_report`:
       - On `Ok(r)`, store `r` in `Run.report`.
       - On `Err(err)`, push `Skipped { key: job.key, reason: format!("report unreadable: {err}") }` and `continue` (no silent fallback!).
     - When `job.research` is `false`, set `report: None` without touching `.report.md`.

**Tests:**
In `crates/blyte-cli/tests/test_replay.rs`:
1. Update `serialized_run_has_exact_contract_keys` to include `"report"` in the expected key set (this is the only modification to an existing test; all other existing tests pass unmodified).
2. Add `card_for_run_classifies_research_independently_of_outcome_and_checks`: asserts `card_for_run(true, "verified", true, &[]) == "research"`, `card_for_run(true, "verified", false, &[]) == "research"`, `card_for_run(true, "budget_exhausted", false, &["broad research run".to_string()]) == "research"`, and that `card_for_run(false, ...)` matches `card_of(...)` for clean and exception cases.
3. Add `research_run_gets_research_card_and_report_markdown`: creates a research run (`"research": true`) with `.wt/R1.report.md` containing markdown findings, runs `collect`, asserts `run.research == true`, `run.card == "research"`, and `run.report.as_deref() == Some(<exact markdown>)`.
4. Add `research_run_without_report_file_has_none_report_and_research_card`: creates a research run with no `.report.md`, runs `collect`, asserts `run.card == "research"` and `run.report.is_none()`.
5. Add `research_run_with_unreadable_report_file_is_skipped`: creates a research run whose `.wt/RBAD.report.md` contains invalid UTF-8 bytes (e.g. `[0xff, 0xfe]`) or is a directory, runs `collect`, asserts `replay.runs` is empty and `replay.skipped` contains `key` with `reason.starts_with("report unreadable: ")`.
6. Add `non_research_run_ignores_report_file`: creates a non-research run (`"research": false`) alongside a `.wt/T1.report.md` file, runs `collect`, asserts `run.report.is_none()` and `run.card != "research"`.

Run with: `cargo test -p blyte-cli --test test_replay --test test_mock` (and the full gate).

**Constraints:**
- No fallbacks: unreadable `.report.md` for a research run must fail loudly by skipping with `"report unreadable: ..."`, never silently returning `None`.
- Add no comments to code (the gate runs `blyte comments`).
- Existing tests pass unmodified except updating the expected key list in `serialized_run_has_exact_contract_keys`.

**Depends on:** none.

**Siblings:** MF1 (`crates/blyte-cli/src/dashboard.rs`, `crates/blyte-cli/tests/test_collect_fast.rs`) and MF3 (`crates/blyte-cli/assets/mock/index.html`, `crates/blyte-cli/tests/test_mock_page.rs`) run in parallel in separate worktrees; do not touch their files.

**Commit message:** `feat(replay): classify research runs and include their report text`

---

### Task MF3: render research runs as reports to read and fit review cards to their content

**Goal:** Update `blyte mock`'s UI so (a) research runs (`card === "research"`) appear as their own queue item kind — a report to read rather than a code diff to accept — and (b) review queue cards size naturally to their content instead of stretching to a hardcoded `184px` height.

**Files:**
- Modify `crates/blyte-cli/assets/mock/index.html`
- Modify `crates/blyte-cli/tests/test_mock_page.rs`

**What to build:**
1. **Replay contract validation (`validateContract`):**
   - Add `"report"` to `nullableStrings`.
   - Accept `r.card === "clean" || r.card === "exception" || r.card === "research"`; reject any other `r.card` value with a clear error message naming `"clean"`, `"exception"`, or `"research"`.

2. **Research runs as their own item kind in the review queue:**
   - Add `const RESEARCH_GROUP = "research reports";` alongside `NO_FILES_GROUP`.
   - Update `runGroupName(run)`:
     - If `run.card === "research"`, return `RESEARCH_GROUP`.
     - Otherwise return `run.group === null ? NO_FILES_GROUP : run.group`.
   - Update `allGroups` construction in `init()`:
     - Track `hasResearch`, `hasNoFiles`, and `namedGroups` (for non-research runs with `r.group !== null`).
     - Build `allGroups = Array.from(namedGroups).sort();`, push `RESEARCH_GROUP` if `hasResearch`, and push `NO_FILES_GROUP` if `hasNoFiles`.
   - Update `renderQueue(state, force)`:
     - Partition `undecided` into `researchRuns = undecided.filter(r => r.card === "research")`, `exceptionRuns = undecided.filter(r => r.card === "exception")`, and `cleanRuns = undecided.filter(r => r.card === "clean")`.
     - Keep `"Accept all clean"` strictly for `cleanRuns.length >= 2` (never include research runs).
     - Render each undecided research run as a `.research-card` (inside a `.research-wrap` or `.exception-wrap`) with:
       - Distinct visual styling (e.g. `border-left: 3px solid #3b6ea5;` or slate-blue accent, distinct from amber/red exception cards), natural content height, and held state `.research-card.is-held`.
       - Held banner `.hold-note-first` when `isHeld(r.key)`.
       - Top row: `<span class="run-id">`, `<span class="run-title">`, a `"research"` kind pill (e.g. `<span class="research-pill">research</span>`), and the existing `.outcome-pill` (`verified` / `failed`).
       - Compact body showing:
         - A brief one-line summary preview from `r.final_message` (if present and non-empty), OR if `r.flags.length > 0`, up to 3 flags (+ `"+N more"`), OR a muted one-line hint (`"Report ready to read"` when `r.report !== null && r.report.length > 0`, or `"No report file recorded"` when `r.report === null || r.report.length === 0`).
       - Footer actions:
         - Primary/pass button: **Mark read** (`data-action="read"`, `data-run-key="..."`).
         - Secondary button: **Hold** (`data-action="hold"`, `data-run-key="..."`, reusing the exact same one-line note prompt as exception cards).
         - Right side: `verdictHtml` (`recorded then: ...` when held and `r.verdict !== null`) and **Read report** / **Hide report** button (`data-toggle-details="..."`).
         - Do NOT render `Accept`, `Send back`, or `Redo` buttons on research cards.
     - Render `researchHtml + excHtml + cleanHtml + decidedHtml` inside `.group-body`.
     - In `decidedHtml`:
       - Map `dec.action === "read"` to `"Read"`.
       - For research runs (`r.card === "research"`), label the toggle button `"Hide report"` / `"Read report"` instead of `"Hide details"` / `"Details"`.
   - Update click handling in `wireEvents()` (around `data-action`):
     - Treat `action === "read"` like `"accept"` and `"redo"`: call `applyDecision(key, action, "")` directly.
     - Verify `isResolved(runKey)` already treats `"read"` as resolved (`dec.action !== "hold"`).
   - Update `renderDetailsHtml(run)`:
     - When `run.card === "research"`, render a report-reading view instead of code-diff sections:
       - `report` section: if `run.report !== null && run.report.length > 0`, render `<pre class="details-pre report-pre">` (with `max-height: 420px;` and `white-space: pre-wrap;`) containing `escapeHtml(run.report)`. Otherwise render `<div class="mono" style="color: var(--color-muted);">No report file recorded for this run.</div>`.
       - `final_message` section (when `run.final_message !== null && run.final_message.length > 0`).
       - `flags` section (when `run.flags.length > 0`).
       - `task` section (`<pre class="details-pre">` with `escapeHtml(run.task)`).
       - Do NOT render `changed_files` or `commits` for research runs.
     - Keep non-research `renderDetailsHtml` unchanged.

3. **Size review queue cards to their content (remove hardcoded `184px` height):**
   - In `.exception-card`:
     - Remove `height: 184px;`.
     - Use natural content height (`height: auto;` or omit `height`), `display: flex; flex-direction: column; gap: 6px; padding: 10px 12px;` (removing `justify-content: space-between;`).
   - In `.exc-flags`:
     - Remove `flex: 1; justify-content: center;` so flags sit directly beneath the header without extra vertical stretching, while keeping `gap: 2px; margin: 2px 0; overflow: hidden;` and the existing 3-flag + `"+N more"` cap so cards remain compact and bounded.
   - Apply the same compact natural-height layout (`gap: 6px; padding: 10px 12px;`, no fixed pixel `height`) to `.research-card`.

**Tests:**
In `crates/blyte-cli/tests/test_mock_page.rs`:
1. Keep all existing tests unmodified and passing.
2. Add `mock_page_handles_research_card_kind_and_report_field`: asserts `PAGE` contains `"report"`, `"research"`, `"research reports"`, `"research-card"`, `"Mark read"`, `"Read report"`, `"Hide report"`, and `data-action="read"`.
3. Add `mock_page_exception_cards_do_not_have_hardcoded_fixed_height`: extracts the `.exception-card { ... }` CSS rule block from `PAGE` and asserts it does NOT contain `height: 184px` or any fixed pixel `height:` declaration (e.g. no `\n      height:` with `px`), and extracts the `.research-card { ... }` CSS rule block and asserts it exists and has no fixed pixel `height:`.

Run with: `cargo test -p blyte-cli --test test_mock_page --test test_mock` (and the full gate).

**Constraints:**
- No fallbacks: unknown `card` values or invalid `report` field types in `/api/replay` must fail contract validation loudly.
- Add no comments to code (the gate runs `blyte comments`; avoid adding JS/CSS comments).
- Existing tests in `test_mock_page.rs` pass unmodified.

**Depends on:** none.

**Siblings:** MF1 (`crates/blyte-cli/src/dashboard.rs`, `crates/blyte-cli/tests/test_collect_fast.rs`) and MF2 (`crates/blyte-cli/src/replay.rs`, `crates/blyte-cli/tests/test_replay.rs`) run in parallel in separate worktrees; do not touch their files.

**Commit message:** `feat(mock): render research runs as reports to read and fit cards to content`
