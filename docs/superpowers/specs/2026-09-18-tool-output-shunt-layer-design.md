# Design Spec: Tool Output Shunt Layer (Native Context Compressor)

Date: 2026-09-18
Status: Approved
Author: Antigravity & Daniel Fisher

## 1. Problem Statement & Motivation

In long-horizon agentic benchmarks (such as `superstate-eval`), autonomous drivers (`agy`, `claude`) iterate across multiple turns solving complex bugs. During these attempts, test runners (`pytest`, `cargo test`, `go test`, `npm test`) and compilers dump massive volumes of diagnostic output to `stdout` and `stderr`—often 500 to 5,000 lines (50,000 to 100,000+ tokens) per turn.

Empirical measurements from EXP-060:
- Tasks like `arktype`, `expr`, and `katex` consumed over **2,000,000 tokens per solve**.
- The overwhelming majority of these tokens were redundant passes, verbose stack traces, and passing test suites.
- In multi-turn sessions, this causes severe **Context Saturation**: models lose attention over earlier instructions, forget constraints, and hallucinate, while cost balloons.

Spotify's internal engineering blog (*"Portal by Spotify cut my Claude Code token usage by 90%"*) proved that intercepting command execution, writing full logs to disk, and returning only distilled, high-signal failure excerpts cuts agent token consumption by ~90% without degrading capability.

This specification details the native implementation of this pattern in `superstate`.

---

## 2. Core Architecture

The Tool Output Shunt Layer consists of three decoupled components:

1. **`superstate-core::shunt` (Distillation Engine)**:
   - Evaluates process output length and size.
   - Saves unstripped raw outputs to `.agent-state/logs/cmd_<timestamp>_<cmd_hash>.log`.
   - Distills output based on process exit status and error pattern heuristics.
2. **`superstate shunt` (CLI Subcommand)**:
   - CLI entrypoint: `superstate shunt [--threshold <lines>] -- <command> [args...]`.
   - Spawns the child command, connects stdin/stdout/stderr pipes, drains them asynchronously to prevent pipe deadlocks, writes the log, and outputs either raw text or distilled summary.
   - Forwards the exact integer exit code of the child process.
3. **Daemon / Driver Shim Integration**:
   - `DaemonRunner` exports a shell wrapper or aliases so driver commands execute through `superstate shunt` transparently.

```
       +-------------------------------------------------------+
       |             Autonomous Driver Subagent                |
       |                   (agy / claude)                      |
       +-------------------------------------------------------+
                                  |
                      Runs shell command (e.g. `cargo test`)
                                  v
       +-------------------------------------------------------+
       |            `superstate shunt -- <command>`            |
       +-------------------------------------------------------+
                                  |
            +---------------------+---------------------+
            |                                           |
            v                                           v
[Total Output <= 50 lines & <= 4KB]         [Total Output > 50 lines or > 4KB]
            |                                           |
            v                                           v
  Pass raw stdout/stderr                    Write full unstripped log to
     to driver context                   `.agent-state/logs/cmd_<id>.log`
                                                        |
                                       +----------------+----------------+
                                       |                                 |
                                 [exit == 0]                       [exit != 0]
                                       v                                 v
                               1-Line Summary                  Extract Failure Window
                            "Command succeeded.             - Failure signatures (FAIL, Error)
                             480 lines omitted."            - 15 lines context + 5 line footer
```

---

## 3. Detailed Specifications

### 3.1 Threshold & Passthrough Rules

- **Default Threshold**: 50 lines OR 4,096 bytes (whichever is exceeded first).
- **Passthrough Policy**:
  - If output has `<= 50` lines and `<= 4,096` bytes, the raw stdout and stderr are printed to the parent console directly.
  - Zero modifications, zero formatting wrappers, zero latency overhead.
  - Commands like `git status`, `ls -la`, `cat small_file`, `which cargo` behave completely normally.

### 3.2 Log File Management

- **Directory**: `.agent-state/logs/` (relative to the workspace root).
- **Naming Convention**: `cmd_<timestamp>_<hash>.log` (e.g. `cmd_20260918_213500_7f3a9b1c.log`).
- **File Contents**:
  - Header: Command line, start timestamp, working directory, exit code, duration.
  - Body: Unstripped, full combined stdout and stderr (ANSI escape sequences preserved).
- **Git Ignore**:
  - The daemon or CLI auto-creates `.agent-state/logs/.gitignore` with `*` so raw logs are never committed to git.
- **Disk Retention**:
  - If `.agent-state/logs/` exceeds 200 MB or 1,000 files, older log files are pruned (FIFO) to prevent runaway disk consumption.

### 3.3 Distillation Heuristics & Output Formats

#### Case A: Command Succeeded (`exit_code == 0`)
When a command exceeds the threshold but exits successfully:
```text
[superstate-shunt] Command succeeded (exit 0). <N> lines omitted.
Full log: .agent-state/logs/cmd_<timestamp>_<hash>.log
```

#### Case B: Command Failed (`exit_code != 0`)
When a command fails:
1. Extract lines matching failure signatures:
   - `FAIL:`, `--- FAIL:`, `FAILED`, `Error:`, `AssertionError`, `panic:`, `E       AssertionError`, `undefined:`, `TypeError:`, `Exception:`
2. Capture the matching line plus up to 3 preceding and 5 succeeding lines (up to a max of 15 lines total for the excerpt).
3. Capture the last 5 lines of the process output (summary stats, e.g. `Tests: 1 failed, 24 passed`).
4. Output format:
```text
[superstate-shunt] Command failed with exit code <CODE> (<N> lines omitted).
Full log: .agent-state/logs/cmd_<timestamp>_<hash>.log

--- Failure Excerpt ---
<extracted failure context lines>
...
<last 5 summary lines>
--- End Failure Excerpt ---
```

---

## 4. Edge Cases & Safety Constraints

1. **Exact Exit Code Forwarding**:
   - `superstate shunt` must never swallow or normalize non-zero exit codes.
   - If the target process exits with code 127 (command not found) or 137 (OOM killed), `superstate shunt` must exit with 127 or 137.
2. **Deadlock-Free I/O Draining**:
   - Spawning child processes with piped stdout/stderr can deadlock if OS pipe buffers (~64 KB) fill before the parent reads them.
   - `superstate shunt` will drain stdout and stderr concurrently using separate reader threads or non-blocking async channels.
3. **Binary / Non-UTF8 Output**:
   - If output contains null bytes or invalid UTF-8 sequences, treat as binary. Write raw bytes to disk, and display a binary file notification rather than crashing or corrupting terminal state.
4. **Stdin Streaming**:
   - Stdin is inherited or forwarded so child processes expecting input or EOF receive it directly.

---

## 5. Verification & Testing Plan

1. **Unit Tests (`crates/superstate-core/tests/test_shunt.rs`)**:
   - Small output passthrough: `< 50` lines output returned unaltered.
   - Large success output: `> 50` lines with exit 0 returns 1-line truncation.
   - Large failure output: `> 50` lines with exit 1 extracts error signatures and tail summary.
   - Exit code propagation: verifies exit codes 0, 1, 42, 127 are faithfully returned.
2. **CLI Integration Tests (`crates/superstate-cli/tests/test_cli_shunt.rs`)**:
   - Tests `superstate shunt -- echo "hello"` -> passes through.
   - Tests `superstate shunt -- bash -c "seq 1 100"` -> outputs truncation summary and writes `.agent-state/logs/`.
   - Tests `superstate shunt -- bash -c "seq 1 100; echo 'Error: boom'; exit 1"` -> extracts failure excerpt.
