# Native Harness Autonomous TDD

**Date:** 2026-09-23
**Status:** Draft for review
**Depends on:** `2026-09-23-native-harness-eval-surface-design.md` (Part A must merge first: both change `run.rs`, `cli.rs`, `task_runner.rs` and `report.rs`)

## Why

The native harness can only be gated by a command the caller supplies. The benchmark tasks ship none that an agent may run: their only verifier is the sealed grader. So the native harness currently runs ungated. The model stops, an empty gate "passes", and the harness's central mechanism (external verification, rollback, retry) never engages.

The daemon solved this with autonomous TDD (`superstate-core/src/tdd.rs`, Manifesto Invariant V). From the repository alone it derives two milestones:
- **RED:** the worker authors a reproduction test that compiles and fails.
- **GREEN:** the worker makes that test and the repository's own suite pass.

The harness supplies no assertion, so nothing leaks from the answer key. EXP-074 showed this path executes end to end. The native harness should have the same gate source, so its gated runs can be compared with `superstate_daemon_full`, and it can stop running ungated.

## Decisions

1. **Reuse `superstate_core::tdd`, do not re-derive it.** `detect_profile` / `tdd_milestones` are the single definition of the RED/GREEN commands, the reproduction path and the milestone wording. Using the same `MilestoneSpec` text keeps native-vs-daemon comparisons about the harness, not about differently worded instructions. `superstate-agent` stays free of a `superstate-core` dependency: the CLI converts milestones into a harness-level `Phase { id, instruction, verify_cmd }`.
2. **A flag, never a sentinel.** `--tdd` (conflicts with `--gate`). EXP-073 found the daemon's `__superstate_autonomous_tdd__` sentinel reaching state as a literal verification command. A flag cannot be mistaken for a command.
3. **Detection fails closed.** An unrecognised repository is `outcome: "error"` with the `TddError` text in the report. It never degrades to ungated. The eval records such a task as void, as it does for the daemon.
4. **Phases run in sequence inside one harness process.** `Runner::run_phases(state, phases)`:
   - For each phase, set `orchestrator.verification_cmd` to the phase's command, and set the prompt's goal to the task plus `## Current milestone (i/n)` and the phase instruction.
   - Run the existing loop. The original task text is kept separately so the goal never compounds.
   - On `Verified`, commit a checkpoint (`chore(superstate): checkpoint phase <id>`, `--no-gpg-sign`, the daemon's convention), so the next phase's rollback baseline includes the reproduction test. A GREEN-phase rollback must never delete RED's verified work.
   - Any other outcome ends the run with that outcome.
5. **The deadline is per run, the turn and gate budgets are per phase.** `max_duration` covers the whole run, so a slow RED phase cannot give GREEN a fresh clock. `max_turns` and `max_gate_attempts` apply to each phase, matching the daemon's per-milestone retry budget. This needs `run()` refactored around an absolute deadline (`run_until(state, deadline)`); `run()` keeps its signature and behaviour.
6. **Two mechanical checks the daemon lacks.** Both are cheap, deterministic and in the harness's authority, not the worker's:
   - **RED scope:** after `red_cmd` passes, `git status --porcelain` must show only the reproduction path. Otherwise the phase fails with the list of other files. This stops a worker fixing the bug during RED, which would make RED's "fails against unfixed code" meaningless.
   - **GREEN tamper:** the GREEN gate additionally requires `git diff --quiet <red-checkpoint> -- <reproduction path>`. The worker may not weaken the test that defines done after seeing it fail. This is strict: a buggy-but-failing RED test makes GREEN unreachable. That is accepted, and it will show up in the report as `gate_failed` / `budget_exhausted` on GREEN, where it can be measured.
7. **The report gains two additive fields, schema stays v1.** `gate_source` is `"none"`, `"user"` or `"tdd"`. `phases` is a list of `{ "id", "outcome", "turns", "gate_attempts" }`, empty unless `--tdd`. Existing v1 readers ignore unknown keys; `superstate-eval`'s reader already does.
8. **Eval side.** `SuperstateNativeDriver(gate="tdd")` passes `--tdd`, and `paired_run --gate tdd` exposes it. The driver's `harness_name` encodes its configuration, for example `superstate[state-only,tdd]`, because `harness` is the field results are pooled by.

## Out of scope

- Multi-milestone plans beyond the two TDD phases, and any worker-authored decomposition.
- Changing `superstate-core::tdd`'s profiles or wording.
- Gating from the task's declared native runner (the daemon's first gate source). None of the canary tasks has one. Add it when a task set needs it.

## Open question for review

Decision 6's GREEN tamper check is stricter than the daemon. If the first gated runs show GREEN failing mostly on tamper, the choice is between loosening it (allowing edits that keep the test failing on the RED checkpoint) and keeping it as a quality bar. The data decides; the report makes it visible.
