# `blyte dispatch`, `blyte review` and `blyte skill install` — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API (tests must never need a key); commit locally only. Do not write to the real `~/.claude`: tests use a temporary `HOME`. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** make blyte's intended use (a frontier model plans and reviews; blyte implements on a
cheap model) a built-in workflow. `blyte dispatch` runs one task in its own worktree and prints a
JSON summary for the orchestrator; `blyte review` shows what a run did; `blyte skill install`
installs the Claude Code skill that teaches Claude the workflow. They replace the bash scripts in
`examples/orchestrate/`.

**Why (2026-09-27):** the scripts need bash, git and python3 and must be copied by hand; the
summary could not include blyte's own final report; "no commits" was the wrapper's exit 4, not
an outcome blyte knew about. One binary with the skill compiled in means setup is two commands.

**Tech Stack:** Rust, clap (subcommands), serde_json.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**, with one allowance: literal `RunReport { … }`
  constructions in existing tests may gain `final_message: None` (task 1 only), nothing else.
- `blyte "<task>"`, `blyte` (REPL) and every existing flag behave exactly as today. A prompt that is
  exactly `dispatch`, `review` or `skill` now needs `blyte -- "<prompt>"`; nothing else changes.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message given in the task.

---

### Task 1: `blyte dispatch` and `blyte review`

**Files:** `crates/blyte-agent/src/run.rs` (final message), `crates/blyte-cli/src/report.rs`,
`crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs`, `crates/blyte-cli/src/lib.rs`,
`crates/blyte-cli/src/dispatch.rs` (new), `crates/blyte-cli/tests/test_dispatch.rs` (new), and the
`final_message: None` allowance in existing tests.

**What to build** (names are part of the contract; the tests below check them):

1. **Final message.** `RunReport` gains `pub final_message: Option<String>`: the model's last
   non-empty text response (trimmed), updated wherever the loop already reads `resp.text`. The
   report (`RunRecord`) gains `final_message: Option<String>` (additive; `SCHEMA_VERSION` stays 1),
   copied in `from_run`, `None` in `from_error`/`base`.

2. **Subcommands.** `Cli` gains `#[command(subcommand)] pub command: Option<Command>` with
   `pub enum Command { Dispatch(DispatchArgs), Review(ReviewArgs), Skill(SkillArgs) }`. Configure clap
   so a first positional equal to a subcommand name selects the subcommand, anything else is the
   prompt as today, and `blyte -- dispatch` is the prompt `"dispatch"`. `SkillArgs` is defined
   in this task with no fields and `main` prints `blyte skill: not implemented yet` and exits 2
   (task 2 fills it in).

   `DispatchArgs`:
   - `id: String` (positional 1), `task_file: PathBuf` (positional 2);
   - `-g, --gate <GATE>` **required**;
   - `--base <REF>` (default `HEAD`): the commit a new worktree starts from;
   - passed through to the child run: `--model`, `--max-seconds` (default **3600**), `--max-turns`
     (default **300**), `--max-cost`, `--fold-budget`, `--sandbox`, `--network`, `--allow-read`
     (repeatable), `--pass-env` (repeatable).

   `ReviewArgs`: `id: String`, `--diff` (flag).

3. **`crates/blyte-cli/src/dispatch.rs`** (`pub mod dispatch;` in `lib.rs`):
   - `pub const DISPATCH_RULES: &str`: the rules appended to every task, exactly:
     ```text
     Rules:
     - Stay within the task. If it cannot pass without a small change elsewhere, make the smallest one and say so.
     - Never delete, clean or modify anything outside the worktree, and never change the machine to make a check pass. If a check fails because of the environment (disk, network, a missing service), stop and say so.
     - Do not push, open pull requests, or read secrets.
     - Commit locally before you stop, with the exact commit message the task gives (or a conventional one if it gives none).
     - When you believe the task is done and committed, stop calling tools: the harness then runs the check. Before that, run the check's quick steps (formatting, lint, the tests for your change) yourself; the full run happens at the gate.
     - Finish with a short report: what you changed, what you ran, and anything that deviated from the task.
     ```
   - `pub fn wrap_task(task: &str) -> String`: `"You are implementing exactly one task. Your working directory is a git worktree on its own branch; work only there.\n\n" + task.trim() + "\n\n" + DISPATCH_RULES`.
   - `pub fn dispatch_outcome(report_outcome: &str, commits: usize) -> &'static str`: `"no_changes"`
     when `report_outcome == "verified"` and `commits == 0`; otherwise the report's outcome
     (`verified`, `gate_failed`, `budget_exhausted`, `error`), and `"error"` for anything unknown.
   - `pub fn exit_code(outcome: &str) -> i32`: `verified` → 0, `no_changes` → 4, anything else → 1.
   - `pub fn run_dispatch(args: &DispatchArgs) -> i32`:
     1. The repository is `git rev-parse --show-toplevel` of the current directory (error: exit 2,
        message on stderr). The worktree is `<root>/.wt/<id>`, branch `wip/<id>`. If the worktree
        directory exists, reuse it (a follow-up); otherwise `git worktree add -b wip/<id> <path> <base>`.
     2. Make sure `.wt/` is ignored without touching tracked files: if
        `<git common dir>/info/exclude` has no line `.wt/`, append one (create the file if needed).
     3. Record `base_sha = git -C <worktree> rev-parse HEAD`.
     4. Read the task file (error: exit 2). Spawn **this binary** (`std::env::current_exe()`) with
        current directory = the worktree, stdout and stderr to `<root>/.wt/<id>.log`, and args:
        `--gate <gate> --rollback=false --max-seconds <n> --max-turns <n> --report <root>/.wt/<id>.report.json`
        plus each pass-through flag that was given, then `--` and `wrap_task(task)`. Wait for it.
     5. Read the report (a missing or unreadable report means outcome `error`, with the reason).
        Collect `commits` (`git log --format=%h%x09%s <base_sha>..HEAD` in the worktree, oldest
        first, as `{sha, subject}`) and `changed_files` (`git diff --name-only <base_sha>..HEAD`).
     6. Print one JSON object on stdout (pretty) with exactly these keys: `id`, `worktree`,
        `branch`, `outcome` (from `dispatch_outcome`), `exit` (from `exit_code`), `error`,
        `commits`, `changed_files`, `final_message`, `turns`, `gate_attempts`, `gate_failures`
        (each cut to 1,000 characters), `cost_usd`, `seconds` (`duration_seconds`, rounded),
        `sandbox`, `log`, `report`. Return the exit code.
   - `pub fn run_review(args: &ReviewArgs) -> i32`: for `<root>/.wt/<id>` (error: exit 2 if missing),
     print sections `== commits`, `== files` (`--stat`), `== tree` (`git status --short`, then
     `(clean)` if empty), `== report` (`outcome`, `turns`, `cost_usd`, `final_message` from the
     report, if present) and, with `--diff`, `== diff`. The base is
     `git merge-base HEAD <root's HEAD>`. Exit 0.

4. **`main.rs`**: dispatch on `cli.command` before the existing prompt/REPL logic:
   `Dispatch` → `exit(run_dispatch)`, `Review` → `exit(run_review)`.

- [ ] **Step 1: Write the failing tests.** Create `crates/blyte-cli/tests/test_dispatch.rs`:

```rust
//! `blyte dispatch` runs one task in its own worktree and prints a JSON summary for the
//! orchestrator; `blyte review` shows what it did. They replace examples/orchestrate's bash
//! scripts (2026-09-27). No test here needs a model: a run without an API key ends `error`.

use std::path::Path;
use std::process::Command;

use blyte_cli::cli::{Cli, Command as Sub};
use blyte_cli::dispatch::{dispatch_outcome, exit_code, wrap_task, DISPATCH_RULES};
use clap::Parser;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git").args(args).current_dir(dir).status().unwrap().success();
    assert!(ok, "git {args:?}");
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init", "--no-gpg-sign"]);
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap()
}

#[test]
fn subcommands_parse_and_a_plain_prompt_is_unchanged() {
    let cli = Cli::try_parse_from(["blyte", "dispatch", "T1", "task.md", "--gate", "cargo test"]).unwrap();
    match cli.command {
        Some(Sub::Dispatch(a)) => {
            assert_eq!(a.id, "T1");
            assert_eq!(a.gate, "cargo test");
            assert_eq!(a.max_turns, 300);
            assert_eq!(a.max_seconds, 3600);
        }
        other => panic!("expected dispatch, got {other:?}"),
    }
    assert!(Cli::try_parse_from(["blyte", "dispatch", "T1", "task.md"]).is_err(), "--gate is required");
    let plain = Cli::try_parse_from(["blyte", "fix the lexer"]).unwrap();
    assert!(plain.command.is_none());
    assert_eq!(plain.prompt.as_deref(), Some("fix the lexer"));
    let escaped = Cli::try_parse_from(["blyte", "--", "dispatch"]).unwrap();
    assert!(escaped.command.is_none());
    assert_eq!(escaped.prompt.as_deref(), Some("dispatch"));
}

#[test]
fn outcomes_and_exit_codes() {
    assert_eq!(dispatch_outcome("verified", 2), "verified");
    assert_eq!(dispatch_outcome("verified", 0), "no_changes");
    assert_eq!(dispatch_outcome("gate_failed", 0), "gate_failed");
    assert_eq!(dispatch_outcome("weird", 1), "error");
    assert_eq!(exit_code("verified"), 0);
    assert_eq!(exit_code("no_changes"), 4);
    assert_eq!(exit_code("gate_failed"), 1);
}

#[test]
fn the_task_is_wrapped_with_the_rules() {
    let p = wrap_task("  Append b to f.txt.\n");
    assert!(p.starts_with("You are implementing exactly one task."), "{p}");
    assert!(p.contains("\n\nAppend b to f.txt.\n\n"), "{p}");
    assert!(p.ends_with(DISPATCH_RULES), "{p}");
}

#[test]
fn a_dispatch_makes_a_worktree_and_reports_json_even_when_the_run_fails() {
    let r = repo();
    std::fs::write(r.path().join("task.md"), "Append b to f.txt.\n").unwrap();
    let out = blyte(r.path(), &["dispatch", "T1", "task.md", "--gate", "true", "--sandbox", "off"]);
    assert_eq!(out.status.code(), Some(1), "{}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout is one JSON object");
    assert_eq!(v["id"], "T1");
    assert_eq!(v["branch"], "wip/T1");
    assert_eq!(v["outcome"], "error");
    assert_eq!(v["exit"], 1);
    assert_eq!(v["commits"], serde_json::json!([]));
    assert!(v["error"].as_str().unwrap_or("").contains("API key") || v["error"].as_str().unwrap_or("").contains("GEMINI_API_KEY"), "{v}");
    assert!(r.path().join(".wt/T1/f.txt").exists());
    assert!(r.path().join(".wt/T1.log").exists());
    let exclude = std::fs::read_to_string(r.path().join(".git/info/exclude")).unwrap();
    assert!(exclude.lines().any(|l| l == ".wt/"), "{exclude}");
    // A second dispatch under the same id reuses the worktree.
    let again = blyte(r.path(), &["dispatch", "T1", "task.md", "--gate", "true", "--sandbox", "off"]);
    assert_eq!(again.status.code(), Some(1));
}

#[test]
fn review_shows_the_run() {
    let r = repo();
    std::fs::write(r.path().join("task.md"), "x").unwrap();
    blyte(r.path(), &["dispatch", "T2", "task.md", "--gate", "true", "--sandbox", "off"]);
    let out = blyte(r.path(), &["review", "T2"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    for s in ["== commits", "== files", "== tree", "(clean)"] {
        assert!(text.contains(s), "{s} missing from:\n{text}");
    }
    assert_eq!(blyte(r.path(), &["review", "nope"]).status.code(), Some(2));
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte --test test_dispatch`: compile error.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(cli): blyte dispatch runs one task in its own worktree and prints a JSON summary; blyte review shows it"
```

---

### Task 2: `blyte skill install`

**Files:** `crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs`,
`crates/blyte-cli/src/skill.rs` (new), `crates/blyte-cli/src/lib.rs`,
`crates/blyte-cli/assets/blyte-implementer/SKILL.md` (new; content below, verbatim),
`crates/blyte-cli/tests/test_skill.rs` (new).

**What to build:**

- `SkillArgs` becomes `{ #[command(subcommand)] action: SkillAction }` with
  `pub enum SkillAction { Install { #[arg(long)] project: bool, #[arg(long)] force: bool }, Print }`.
- `crates/blyte-cli/src/skill.rs` (`pub mod skill;`):
  - `pub const SKILL_MD: &str = include_str!("../assets/blyte-implementer/SKILL.md");`
  - `pub fn skill_dir(project: bool) -> Result<PathBuf, String>`: with `project`, the current git
    repository's root (`git rev-parse --show-toplevel`) joined with `.claude/skills/blyte-implementer`;
    otherwise `$HOME/.claude/skills/blyte-implementer` (error if `HOME` is unset or empty).
  - `pub fn install(project: bool, force: bool) -> Result<PathBuf, String>`: create the directory
    and write `SKILL.md`. If a `SKILL.md` already exists there with different content and `force`
    is false, return an error naming the path and saying to pass `--force`. Identical content is
    success. Returns the file's path.
- `main.rs`: `blyte skill install [--project] [--force]` prints `installed <path>` and a second line
  `ask Claude Code: "Plan this change, then use blyte to implement each task and review the results."`,
  exit 0; on error prints it to stderr and exits 1. `blyte skill print` writes `SKILL_MD` to stdout
  (for other harnesses), exit 0.

`crates/blyte-cli/assets/blyte-implementer/SKILL.md`, verbatim:

````markdown
---
name: blyte-implementer
description: Use when a task can be handed to a cheaper model to implement while you plan and review: write a precise task, dispatch it with `blyte dispatch` into its own git worktree with the repository's check command as its gate, then verify the result yourself and merge it. Use for well-specified changes in a git repository with a runnable check (tests, lint).
---

# blyte as your implementer

You plan and review; blyte implements, on a cheaper model, and its gate (the repository's check
command) decides whether the work is done. Your job is to make each task unambiguous and to verify
the result, not to trust it.

## When to delegate

Delegate when a careful engineer could do the task from its description alone, without asking
anything, and the repository has a check that would catch a wrong result. Keep for yourself:
design decisions, ambiguous requirements, anything with no check, and anything touching
credentials, deployment or other repositories.

## 1. Write the task

One task per dispatch, in a Markdown file. blyte sees only this file and the repository, nothing
from your conversation. Include:

- **Goal**, in one or two sentences, and **why**.
- **Files** to create or change.
- **What to build**: names, signatures and behaviour that are part of the contract; exact code
  where it matters.
- **Tests** that prove it, written out, and the command that runs them.
- **Constraints**, e.g. "existing tests pass unmodified".
- **The exact commit message.**

Before dispatching, check that the tests fail on the current code and can pass with the intended
change.

## 2. Dispatch

From the repository's main checkout, in the background (you are notified when it exits):

```bash
blyte dispatch T1 /abs/path/task-T1.md --gate 'cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test'
```

- Each line of the gate is a step; all must pass. Put the slow full test run last.
- `--base <branch>` picks the starting commit; independent tasks (disjoint files) can run in
  parallel under different ids.
- The implementer model's API key must be in the environment; blyte never prints it.

It prints one JSON object: `outcome` (`verified`, `no_changes`, `gate_failed`,
`budget_exhausted`, `error`), `exit` (0 only for `verified`), `commits`, `changed_files`,
`final_message` (blyte's own report), `gate_failures`, `turns`, `cost_usd`, `seconds`, and the
`log` and `report` paths. `no_changes` means the gate passed on an untouched tree: not done.

## 3. Verify. Do not trust the summary

```bash
blyte review T1 --diff
```

1. **Scope**: only the files the task names changed, in the commit(s) you asked for, with the exact message.
2. **Fidelity**: code you gave verbatim landed verbatim; read everything else in full.
3. **Checks yourself**: run the gate in the worktree (`.wt/T1`). A passing gate is necessary, not sufficient.
4. **Deviations**: every deviation in `final_message` is justified, and there are none it did not mention.
5. **Out-of-scope effects**: skim the log (`.wt/T1.log`) for commands that touched anything outside the worktree.

If it falls short: fix a trivial issue yourself, or write a follow-up task saying exactly what to
change and dispatch it (`blyte dispatch T1b follow-up.md --base wip/T1 --gate ...`). If the task was
wrong, fix the task, not the result.

## 4. Merge

```bash
git merge --ff-only wip/T1        # or cherry-pick if the branch moved
git worktree remove .wt/T1 && git branch -d wip/T1
```

Re-run the checks after merging parallel tasks together. Push or open a pull request only if the
user asks.

## Rules

- One task per dispatch; never hand over a whole plan.
- blyte never pushes, merges or reads secrets. If its log shows otherwise, stop and tell the user.
- Report to the user what **you verified**, not what blyte claimed.
````

- [ ] **Step 1: Write the failing tests.** Create `crates/blyte-cli/tests/test_skill.rs`:

```rust
//! `blyte skill install` puts the Claude Code skill for the plan → delegate → review workflow
//! where Claude Code finds it; the skill is compiled into the binary so it matches it (2026-09-27).

use std::path::Path;
use std::process::Command;

use blyte_cli::skill::SKILL_MD;

fn blyte(cwd: &Path, home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .output()
        .unwrap()
}

#[test]
fn the_embedded_skill_is_the_dispatch_workflow() {
    assert!(SKILL_MD.starts_with("---\nname: blyte-implementer\n"), "{}", &SKILL_MD[..60]);
    for s in ["blyte dispatch", "blyte review", "no_changes", "final_message"] {
        assert!(SKILL_MD.contains(s), "{s} missing");
    }
}

#[test]
fn install_writes_the_personal_skill_and_refuses_to_clobber_an_edited_one() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let out = blyte(cwd.path(), home.path(), &["skill", "install"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let path = home.path().join(".claude/skills/blyte-implementer/SKILL.md");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SKILL_MD);
    assert!(String::from_utf8_lossy(&out.stdout).contains("installed"));
    // Same content again: fine.
    assert_eq!(blyte(cwd.path(), home.path(), &["skill", "install"]).status.code(), Some(0));
    // Edited by the user: refused without --force, replaced with it.
    std::fs::write(&path, "my edits").unwrap();
    let refused = blyte(cwd.path(), home.path(), &["skill", "install"]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--force"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "my edits");
    assert_eq!(blyte(cwd.path(), home.path(), &["skill", "install", "--force"]).status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SKILL_MD);
}

#[test]
fn project_install_goes_into_the_repository() {
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    assert!(Command::new("git").args(["init", "-q"]).current_dir(repo.path()).status().unwrap().success());
    let out = blyte(repo.path(), home.path(), &["skill", "install", "--project"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(repo.path().join(".claude/skills/blyte-implementer/SKILL.md").exists());
    assert!(!home.path().join(".claude").exists());
}

#[test]
fn print_writes_the_skill_to_stdout() {
    let home = tempfile::tempdir().unwrap();
    let out = blyte(home.path(), home.path(), &["skill", "print"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), SKILL_MD);
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte --test test_skill`: compile error.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(cli): blyte skill install puts the blyte-implementer skill where Claude Code finds it"
```
