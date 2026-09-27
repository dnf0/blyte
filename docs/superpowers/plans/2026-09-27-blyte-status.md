# `blyte status` for a Running Dispatch — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** an orchestrator can ask what a dispatch is doing while it runs: `blyte status <id>` prints
whether it is running, finished or dead, how long it has run, the current turn, the last tool call,
gate attempts, and (once finished) the outcome and cost.

**Why (2026-09-27):** a dispatch can run for ten minutes or more; until it exits the orchestrator
sees nothing, so it cannot tell a run making progress from one stuck in a loop, or decide to kill
and re-plan.

**Tech Stack:** Rust.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**.
- `blyte dispatch`'s JSON summary and exit codes are unchanged.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `.wt/<id>.start.json` and `blyte status`

**Files:** `crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs`,
`crates/blyte-cli/src/dispatch.rs`, `crates/blyte-cli/assets/blyte-implementer/SKILL.md`,
`crates/blyte-cli/tests/test_status.rs` (new).

**What to build:**

- `run_dispatch`, right after spawning the child run: write `<root>/.wt/<id>.start.json`:
  `{"id", "pid" (the child's), "started_at" (seconds since the Unix epoch), "task_file", "gate"}`.
- `pub fn parse_log(log: &str) -> LogProgress` in `dispatch.rs`, with
  `pub struct LogProgress { pub turn: u32, pub last_tool: Option<String>, pub gate_attempts: u32, pub gate_passed: bool }`,
  reading blyte's log format (after stripping ANSI colour codes `\x1b[…m`):
  - `turn`: the largest `N` in a line starting `[Turn N]` (0 if none);
  - `last_tool`: the last line starting `● ` (without the `● `), cut to 200 characters;
  - `gate_attempts`: the number of lines starting `✓ Gate passed` or `✗ Gate failed`;
  - `gate_passed`: whether the last of those starts `✓`.
- New subcommand `Status { id: String, #[arg(long)] json: bool }` ("Show what a dispatch is doing").
  `pub fn run_status(id, json) -> i32`:
  - The repository root as for `dispatch`; `<root>/.wt/<id>.start.json` missing: exit 2
    (`no dispatch <id> here`).
  - `state`: `finished` when `<root>/.wt/<id>.report.json` exists; else `running` when the pid is
    alive (`kill(pid, 0)` succeeds or fails with `EPERM`); else `dead`.
  - `elapsed_s`: now minus `started_at` (for `finished`, the report's `duration_seconds`).
  - From the log: `turn`, `last_tool`, `gate_attempts`, `gate_passed`.
  - Finished: also `outcome`, `cost_usd` and `final_message` (first 500 characters) from the report.
  - `--json`: one JSON object with exactly the keys `id`, `state`, `elapsed_s`, `turn`,
    `last_tool`, `gate_attempts`, `gate_passed`, `outcome`, `cost_usd`, `final_message` (the last
    three `null` until finished). Otherwise a short readable block, one `key: value` per line.
  - Exit 0 for `running` and `finished`, 1 for `dead`.
- `SKILL.md`, section 3 (Dispatch), after the paragraph on starting parallel tasks, add:
  ```markdown
  **A long dispatch:** `blyte status T1` shows whether it is still running, the current turn, the
  last command and gate attempts. A run that is `dead`, or many turns in with no gate attempt,
  is worth a look at its log (`.wt/T1.log`) before you wait longer.
  ```

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-cli/tests/test_status.rs`:

```rust
//! `blyte status` shows what a running dispatch is doing: until a dispatch exited the
//! orchestrator could not tell progress from a loop (2026-09-27).

use std::path::Path;
use std::process::Command;

use blyte_cli::dispatch::parse_log;

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git").args(args).current_dir(dir).status().unwrap().success());
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "i", "--no-gpg-sign"]);
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte")).args(args).current_dir(dir)
        .env_remove("GEMINI_API_KEY").env_remove("GOOGLE_API_KEY").env_remove("BLYTE_MODEL")
        .output().unwrap()
}

#[test]
fn the_log_parser_reads_turns_tools_and_gates() {
    let log = "[Turn 1] \n\x1b[31m● bash cargo test -> exit 101\x1b[0m\n[Turn 2] \n● edit src/lib.rs -> edited src/lib.rs\n✗ Gate failed (attempt 1): boom\n[Turn 3] done\n✓ Gate passed (attempt 2)\n";
    let p = parse_log(log);
    assert_eq!(p.turn, 3);
    assert_eq!(p.last_tool.as_deref(), Some("edit src/lib.rs -> edited src/lib.rs"));
    assert_eq!(p.gate_attempts, 2);
    assert!(p.gate_passed);
    let empty = parse_log("");
    assert_eq!((empty.turn, empty.gate_attempts), (0, 0));
    assert!(empty.last_tool.is_none());
}

#[test]
fn status_of_a_finished_dispatch_reports_its_outcome() {
    let r = repo();
    std::fs::write(r.path().join("task.md"), "x").unwrap();
    blyte(r.path(), &["dispatch", "T1", "task.md", "--gate", "true", "--sandbox", "off"]);
    assert!(r.path().join(".wt/T1.start.json").exists());
    let out = blyte(r.path(), &["status", "T1", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["state"], "finished");
    assert_eq!(v["outcome"], "error"); // no API key in the test environment
    for k in ["elapsed_s", "turn", "last_tool", "gate_attempts", "gate_passed", "cost_usd", "final_message"] {
        assert!(v.get(k).is_some(), "{k} missing");
    }
}

#[test]
fn an_unknown_id_is_an_error() {
    let r = repo();
    assert_eq!(blyte(r.path(), &["status", "nope"]).status.code(), Some(2));
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte --test test_status`: compile error.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(cli): blyte status shows what a running dispatch is doing"
```
