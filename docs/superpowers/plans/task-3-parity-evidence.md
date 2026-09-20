# Task 3 Verification Report: Arm Input Parity & Container Execution Check

**Status:** Completed & Verified  
**Repository:** `/Users/danielfisher/repos/superstate-eval`  
**Worktree:** `/Users/danielfisher/repos/superstate-eval/.worktrees/feat-eval-philosophy-autonomous-tdd`  
**Branch:** `feat/eval-philosophy-autonomous-tdd`  
**Binary Tested:** `~/.cargo/bin/superstate` (v0.1.0)  
**Interpreter:** `.venv/bin/python` (Python 3.12.9, pytest-9.1.1, ruff-0.9.9)  
**Date:** 2026-09-20  

---

## Executive Summary

Task 3 performed a rigorous end-to-end verification of arm input parity, container execution and sandboxing, AST symbol inspection, and test/linter cleanliness in `superstate-eval` as specified in `docs/superpowers/plans/task-3-parity-report.md`.

All requirements were verified with empirical execution evidence:
1. **Arm Input Parity:** 100% preserved across all compared arms (`superstate.py`, `superstate_mcp.py`, `superstate_daemon.py`). Zero synthetic prompt engineering, no hardcoded milestone DAG synthesis, and strictly synchronized verification command resolution.
2. **AST Symbol Inspection:** `~/.cargo/bin/superstate inspect` verified across both Python and Rust files, supporting `--lines`, `--outline`, `--symbol`, and `--json` modes.
3. **Container Sandboxing & Binary Shunting:** Real Docker daemon integration tests passed (7/7 passed), and container static binary mounting and isolation tests passed (6/6 passed).
4. **Full Test Suite:** 100% passing rate across all 343 unit and integration tests (`343 passed in 77.14s`).
5. **Code Hygiene:** Ruff linting passed cleanly with 0 errors across `lab` and `tests`.

---

## 1. Arm Input Parity Verification

### Execution
Command:
```bash
.venv/bin/python -m pytest tests/test_arm_input_parity.py -v
```

### Result & Evidence
```text
============================= test session starts ==============================
platform darwin -- Python 3.12.9, pytest-9.1.1, pluggy-1.6.0 -- /Users/danielfisher/repos/superstate-eval/.worktrees/feat-eval-philosophy-autonomous-tdd/.venv/bin/python
cachedir: .pytest_cache
rootdir: /Users/danielfisher/repos/superstate-eval/.worktrees/feat-eval-philosophy-autonomous-tdd
configfile: pyproject.toml
plugins: anyio-4.15.1
collecting ... collected 7 items

tests/test_arm_input_parity.py::test_every_compared_arm_resolves_the_same_verification_command PASSED [ 14%]
tests/test_arm_input_parity.py::test_no_compared_arm_falls_back_to_true_before_checking_fixtures PASSED [ 28%]
tests/test_arm_input_parity.py::test_daemon_worker_invocation_matches_the_shared_driver PASSED [ 42%]
tests/test_arm_input_parity.py::test_daemon_reports_driver_turns_not_state_steps PASSED [ 57%]
tests/test_arm_input_parity.py::test_every_arm_name_resolves_to_a_distinct_adapter_name PASSED [ 71%]
tests/test_arm_input_parity.py::test_no_adapter_synthesizes_milestones_or_harness_prompts PASSED [ 85%]
tests/test_arm_input_parity.py::test_no_adapter_uses_raw_substring_matching_for_sealed_grader PASSED [100%]

============================== 7 passed in 1.16s ===============================
```

### Key Parity Invariants Verified
- **No Harness Prompt Engineering (`test_no_adapter_synthesizes_milestones_or_harness_prompts`):** Confirms that no adapter injects synthetic TDD milestones (`_build_tdd_milestones`), hardcoded test file names (`test_reproduce_issue.py`), or artificial execution DAGs (`tdd_dag`). Adapters remain strictly passive translation shims.
- **Identical Verification Commands (`test_every_compared_arm_resolves_the_same_verification_command` & `test_no_compared_arm_falls_back_to_true_before_checking_fixtures`):** All compared arms resolve `agent_gate_for` from `GATE_FIXTURES` before falling back to `true`, preventing asymmetric gate bias.
- **Worker Invocation Parity (`test_daemon_worker_invocation_matches_the_shared_driver`):** The daemon worker wrapper mirrors all flags passed by `AgyCliDriver`: `--model`, `--output-format`, `--print-timeout`, and `--add-dir`.
- **Honest Turn Accounting (`test_daemon_reports_driver_turns_not_state_steps`):** Turns strictly count worker invocations (`turns=max(1, driver_turns)`), not internal state transitions.
- **Sealed Grader Isolation (`test_no_adapter_uses_raw_substring_matching_for_sealed_grader`):** Safe path-based resolution via `references_sealed_grader()` rather than naive substring matching that falsely flags in-repo test suites.

---

## 2. AST Symbol & Code Inspection Verification

The installed release binary `~/.cargo/bin/superstate` (built from `crates/superstate-cli`) was evaluated across Python and Rust files using bounded line slicing, outline extraction, and symbol extraction.

### Python Verification (`lab/benchmarks/task_loader.py`)

#### 1. Bounded Line Slicing (`--lines 1:15`)
Command:
```bash
~/.cargo/bin/superstate inspect lab/benchmarks/task_loader.py --lines 1:15
```
Output:
```text
=== Lines 1-15 in lab/benchmarks/task_loader.py ===
1: """Benchmark task specification and loader module.
2: 
3: Parses SWE-bench / Harbor style benchmark task definitions (task.toml and instruction.md).
4: """
5: 
6: from __future__ import annotations
7: 
8: from dataclasses import dataclass, field
9: import logging
10: from pathlib import Path
11: import tomllib
12: from typing import Any
13: 
14: logger = logging.getLogger("superstate_eval.task_loader")
15: 
```

#### 2. Outline Generation (`--outline`)
Command:
```bash
~/.cargo/bin/superstate inspect lab/benchmarks/task_loader.py --outline
```
Output:
```text
=== Outline: lab/benchmarks/task_loader.py (Python) ===
- [Class] TaskSpec [lines 18-109]
- [Function] __post_init__ [lines 41-48]
- [Function] task_id [lines 51-52]
- [Function] tests_dir [lines 55-64]
- [Function] has_verifier_dockerfile [lines 67-109]
- [Function] _extract_collect_commands [lines 112-126]
- [Function] load_task_from_dir [lines 129-198]
- [Function] load_benchmark_tasks [lines 201-214]
```

#### 3. Symbol Extraction (`--symbol TaskSpec`)
Command:
```bash
~/.cargo/bin/superstate inspect lab/benchmarks/task_loader.py --symbol TaskSpec
```
Output:
```text
=== Symbol: TaskSpec [lines 18-109] in lab/benchmarks/task_loader.py ===
18: class TaskSpec:
19:     """Specification of an evaluation benchmark task."""
20: 
21:     id: str
22:     name: str
23:     instruction: str
...
109:         return False
```

---

### Rust Verification (`crates/superstate-core/src/checkpoint.rs`)

#### 1. Bounded Line Slicing (`--lines 1:20`)
Command:
```bash
~/.cargo/bin/superstate inspect /Users/danielfisher/repos/superstate/crates/superstate-core/src/checkpoint.rs --lines 1:20
```
Output:
```text
=== Lines 1-20 in /Users/danielfisher/repos/superstate/crates/superstate-core/src/checkpoint.rs ===
1: //! Git checkpoints around risky milestone edits.
2: //!
3: //! A checkpoint is a lightweight tag on the current commit, named for the step it
4: //! guards. Rollback restores the working tree to that tag *and* removes untracked
5: //! files: a milestone that half-created files and then failed leaves debris which
6: //! a plain `reset --hard` does not touch, and that debris would otherwise be
7: //! carried into the next milestone and into the graded diff.
8: 
9: use std::path::Path;
10: use std::process::Command;
11: use thiserror::Error;
12: 
13: /// Tag namespace for checkpoints, matching the reference implementation.
14: pub const CHECKPOINT_PREFIX: &str = "agent-checkpoint/step-";
15: 
16: #[derive(Error, Debug)]
17: pub enum CheckpointError {
18:     #[error("git command failed: {0}")]
19:     Git(String),
20:     #[error("io error: {0}")]
```

#### 2. Outline Generation (`--outline`)
Command:
```bash
~/.cargo/bin/superstate inspect /Users/danielfisher/repos/superstate/crates/superstate-core/src/checkpoint.rs --outline
```
Output:
```text
=== Outline: /Users/danielfisher/repos/superstate/crates/superstate-core/src/checkpoint.rs (Rust) ===
- [Enum] CheckpointError [lines 17-22]
- [Function] checkpoint_ref [lines 25-27]
- [Function] git [lines 29-40]
- [Function] create_checkpoint [lines 46-50]
- [Function] rollback_to [lines 58-87]
```

#### 3. Symbol Extraction (`--symbol create_checkpoint` & `--symbol CheckpointError`)
Commands:
```bash
~/.cargo/bin/superstate inspect /Users/danielfisher/repos/superstate/crates/superstate-core/src/checkpoint.rs --symbol create_checkpoint
~/.cargo/bin/superstate inspect /Users/danielfisher/repos/superstate/crates/superstate-core/src/checkpoint.rs --symbol CheckpointError
```
Output:
```text
=== Symbol: create_checkpoint [lines 46-50] in /Users/danielfisher/repos/superstate/crates/superstate-core/src/checkpoint.rs ===
46: pub fn create_checkpoint(repo_root: &Path, step: usize) -> Result<String, CheckpointError> {
47:     let tag = checkpoint_ref(step);
48:     git(repo_root, &["tag", "-f", &tag])?;
49:     Ok(tag)
50: }

=== Symbol: CheckpointError [lines 17-22] in /Users/danielfisher/repos/superstate/crates/superstate-core/src/checkpoint.rs ===
17: pub enum CheckpointError {
18:     #[error("git command failed: {0}")]
19:     Git(String),
20:     #[error("io error: {0}")]
21:     Io(#[from] std::io::Error),
22: }
```

#### 4. Rust CLI Unit and Integration Test Suite
Command:
```bash
cargo test --test test_cli_inspect (in repos/superstate)
```
Output:
```text
running 10 tests
test test_cli_inspect_file_not_found ... ok
test test_cli_inspect_unsupported_language_ast ... ok
test test_cli_inspect_symbol_found_text ... ok
test test_cli_inspect_symbol_found_json ... ok
test test_cli_inspect_outline_json ... ok
test test_cli_inspect_lines_valid ... ok
test test_cli_inspect_symbol_not_found ... ok
test test_cli_inspect_outline_default_text ... ok
test test_cli_inspect_help ... ok
test test_cli_inspect_lines_invalid_syntax ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.55s
```

---

## 3. Container Sandboxing & Execution Check

### Docker Integration Suite
Command:
```bash
.venv/bin/python -m pytest tests/test_docker_integration.py -v
```
Output:
```text
============================= test session starts ==============================
platform darwin -- Python 3.12.9, pytest-9.1.1, pluggy-1.6.0 -- /Users/danielfisher/repos/superstate-eval/.worktrees/feat-eval-philosophy-autonomous-tdd/.venv/bin/python
cachedir: .pytest_cache
rootdir: /Users/danielfisher/repos/superstate-eval/.worktrees/feat-eval-philosophy-autonomous-tdd
configfile: pyproject.toml
plugins: anyio-4.15.1
collecting ... collected 7 items

tests/test_docker_integration.py::test_seeded_workspace_preserves_task_repo_under_bind_mount PASSED [ 14%]
tests/test_docker_integration.py::test_no_network_mode_actually_blocks_egress PASSED [ 28%]
tests/test_docker_integration.py::test_seeding_fails_loudly_when_image_has_no_repo PASSED [ 42%]
tests/test_docker_integration.py::test_seeding_fails_loudly_when_image_is_missing PASSED [ 57%]
tests/test_docker_integration.py::test_held_out_verifier_builds_and_grades_in_its_own_container PASSED [ 71%]
tests/test_docker_integration.py::test_verifier_crash_sentinel_is_not_treated_as_a_score PASSED [ 85%]
tests/test_docker_integration.py::test_container_exec_mcp_runs_in_the_container_not_the_host PASSED [100%]

============================== 7 passed in 4.91s ===============================
```

### Static Superstate Mount & Shunting Suite
Command:
```bash
.venv/bin/python -m pytest tests/test_docker_manager_shunting.py -v
```
Output:
```text
============================= test session starts ==============================
platform darwin -- Python 3.12.9, pytest-9.1.1, pluggy-1.6.0 -- /Users/danielfisher/repos/superstate-eval/.worktrees/feat-eval-philosophy-autonomous-tdd/.venv/bin/python
cachedir: .pytest_cache
rootdir: /Users/danielfisher/repos/superstate-eval/.worktrees/feat-eval-philosophy-autonomous-tdd
configfile: pyproject.toml
plugins: anyio-4.15.1
collecting ... collected 6 items

tests/test_docker_manager_shunting.py::test_spawn_sandbox_mounts_static_superstate_when_file_exists PASSED [ 16%]
tests/test_docker_manager_shunting.py::test_spawn_sandbox_omits_mount_when_binary_absent PASSED [ 33%]
tests/test_docker_manager_shunting.py::test_spawn_sandbox_does_not_duplicate_existing_superstate_mount PASSED [ 50%]
tests/test_docker_manager_shunting.py::test_find_static_superstate_binary_env_var PASSED [ 66%]
tests/test_docker_manager_shunting.py::test_find_static_superstate_binary_env_var_nonexistent PASSED [ 83%]
tests/test_docker_manager_shunting.py::test_find_static_superstate_binary_none_when_no_candidates_exist PASSED [100%]

============================== 6 passed in 0.76s ===============================
```

### Container Execution Contract
- The harness mounts the host workspace into `/app` within the sandbox container.
- When `SUPERSTATE_LINUX_BIN` or a Linux binary candidate exists, `DockerManager` mounts it read-only to `/usr/local/bin/superstate` without colliding with existing custom mounts.
- Host drivers invoke container commands using `./run-in-container <cmd>`, adhering to `CONTAINER_GUIDANCE_TEMPLATE`.
- Host-level AST inspections (`superstate inspect`) run locally on the host repository files without requiring in-container binaries, providing fast, low-overhead context bounds.

---

## 4. Full Evaluation Test Suite & Code Hygiene

### 1. Full Pytest Suite (343 tests)
Command:
```bash
.venv/bin/python -m pytest tests/ -q
```
Output:
```text
........................................................................ [ 20%]
........................................................................ [ 41%]
........................................................................ [ 62%]
........................................................................ [ 83%]
.......................................................                  [100%]
343 passed in 77.14s (0:01:17)
```
**Pass Rate:** 343 / 343 (100.0%)

### 2. Ruff Linter Verification
Command:
```bash
.venv/bin/ruff check lab tests
```
Output:
```text
All checks passed!
```
**Errors:** 0

---

## 5. Verification Checklist

| Requirement | Command / Check | Result |
|---|---|---|
| Arm Input Parity | `.venv/bin/python -m pytest tests/test_arm_input_parity.py -v` | **PASS (7/7 passed)** |
| Python AST Inspect Lines | `superstate inspect lab/benchmarks/task_loader.py --lines 1:15` | **PASS** |
| Python AST Inspect Outline | `superstate inspect lab/benchmarks/task_loader.py --outline` | **PASS** |
| Python AST Inspect Symbol | `superstate inspect lab/benchmarks/task_loader.py --symbol TaskSpec` | **PASS** |
| Rust Inspect Lines | `superstate inspect <path>/checkpoint.rs --lines 1:20` | **PASS** |
| Rust Inspect Outline | `superstate inspect <path>/checkpoint.rs --outline` | **PASS** |
| Rust Inspect Symbol | `superstate inspect <path>/checkpoint.rs --symbol create_checkpoint` | **PASS** |
| Rust Inspect CLI Tests | `cargo test --test test_cli_inspect` | **PASS (10/10 passed)** |
| Docker Sandbox Tests | `.venv/bin/python -m pytest tests/test_docker_integration.py -v` | **PASS (7/7 passed)** |
| Binary Mount Tests | `.venv/bin/python -m pytest tests/test_docker_manager_shunting.py -v` | **PASS (6/6 passed)** |
| Full Test Suite | `.venv/bin/python -m pytest tests/ -q` | **PASS (343/343 passed)** |
| Linter Cleanliness | `.venv/bin/ruff check lab tests` | **PASS (0 errors)** |
