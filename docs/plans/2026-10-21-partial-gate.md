# Plan: say when a dispatch runs with a partial gate (2026-10-21)

## Why

`blyte init` leaves a CI step out of the gate when it cannot run here (for example
`# make typecheck: from ci.yml, left out: pyright requires node`) or fails on HEAD
(`# make lint: from ci.yml, failed on HEAD: fix it before dispatching`; `init.rs`, around the
`Verdict::Environment` match). `gate_drift::is_ci_step_covered` counts any `blyte.toml` comment naming
the step as covered, so the drift warning stays quiet (test `left_out_comment_produces_no_warning`),
the run goes ahead with the rest of the gate, and nothing in the run, its report or `blyte review`
says which checks did not run.

Decision (user, 2026-10-21): run and record, do not fail. Steps are left out mostly because they
cannot run on this machine, so failing would block every dispatch in such repositories. Fail only
when the gate from `blyte.toml` is empty.

### Task G1: report left-out gate steps at dispatch, in the run record and in review

Files: `crates/blyte-cli/src/gate_drift.rs`, `crates/blyte-cli/src/dispatch.rs` (only the places
named), `crates/blyte-cli/src/report.rs`, new test `crates/blyte-cli/tests/test_partial_gate.rs`,
`docs/usage.md` (one or two sentences where the gate or `blyte review`'s checks are described;
`grep -n "gate is missing CI steps\|== checks" docs/usage.md`).

**`dispatch.rs` is 1,800 lines: never read it whole.** Read the ranges named; use `grep -n` for
anything else. Keep the existing drift behaviour and its tests unchanged.

1. **`gate_drift.rs`:** add `pub fn left_out_steps(blyte_toml_content: &str) -> Vec<(String, String)>`,
   returning `(command, reason)` for every line of the form `# <command>: from <file>, <rest>` where
   `<rest>` starts with `left out: ` (reason = the text after `left out: `) or `failed on HEAD`
   (reason = `failed on HEAD`). Also include the setup form
   `# setup = "<command>"   # left out: <reason>` as `("setup: <command>", reason)`. Ignore every
   other comment. Unit-test it in the new test file with the three forms plus an unrelated comment.
2. **`dispatch.rs`, choosing the gate** (`grep -n "gate_drift::missing_ci_steps"`, read 25 lines
   around it; this is the `else if let Some(ref g) = dispatch_cfg.gate` branch, where
   `toml_content` is already read):
   - If `g.trim()` is empty: print
     `error: the gate in blyte.toml is empty; every step was left out. Fix the steps blyte init named, or pass --gate.`
     followed by one line per left-out step (`  <command>: <reason>`), and return exit code 2.
     (First check what an empty gate does today with `grep -n "no gate" crates/blyte-cli/src/dispatch.rs`;
     keep the `--gate` flag path unchanged.)
   - Otherwise compute `left_out = left_out_steps(&toml_content)`. When it is not empty, print one
     line to stderr: `blyte: the gate is partial; left out: <command> (<reason>); <command> (<reason>)`.
   - Keep `left_out` in a variable available later in the function (empty for `--gate`, research
     and every other branch).
3. **Run record:** where `start_json` is built (`grep -n "let mut start_json"`, read 30 lines), add
   `"gate_left_out": [{"command": ..., "reason": ...}]` when `left_out` is not empty.
4. **Run report:** in `crates/blyte-cli/src/report.rs`, add `#[serde(default)] pub gate_left_out: Vec<String>`
   (each entry `"<command> (<reason>)"`) next to `gate_source`, set to empty in the default
   constructor (`grep -n "read_loop: false"`). Set it from `left_out` where dispatch writes the run
   report (`grep -n "from_run\|gate_source" crates/blyte-cli/src/dispatch.rs`); if the report is built
   somewhere that cannot see `left_out`, read it back from `.wt/<id>.start.json` instead.
5. **Review:** where `checks.flags` gets the `read loop:` line (`grep -n "read loop:" crates/blyte-cli/src/dispatch.rs`,
   read 40 lines around it), add `gate left out: <command> (<reason>); ...` when the run report's
   `gate_left_out` (or the start record's `gate_left_out`, whichever that section already reads) is
   not empty.
6. **Tests** (`test_partial_gate.rs`; copy the repository setup and the `blyte` runner from
   `crates/blyte-cli/tests/test_gate_drift.rs`, lines 1-87, and the end-to-end pattern from its
   `left_out_comment_produces_no_warning`):
   - the `left_out_steps` unit test from step 1;
   - dispatch with a `blyte.toml` holding a left-out comment and a non-empty gate: stderr contains
     `the gate is partial; left out: make typecheck (pyright requires node)`, and the
     `.wt/<id>.start.json` it writes contains `gate_left_out`;
   - dispatch with `gate = ""` and a left-out comment: exit code 2 and stderr contains
     `the gate in blyte.toml is empty`;
   - dispatch with `--gate "true"` and the same `blyte.toml`: no partial-gate line.

Gate: the repository checks. Commit: `fix(dispatch): say when the gate leaves CI steps out, and record them for review`
