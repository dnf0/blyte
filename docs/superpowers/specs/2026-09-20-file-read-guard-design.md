# Design Specification: Deterministic PreToolUse File Read Guard & AST Shunting

## 1. Executive Summary & Problem Statement

### 1.1 Context & Problem
In EXP-066 (`build-cython-ext` under `superstate_daemon_full`), an autonomous coding agent successfully passed 11/11 tests, but consumed **738,681 input tokens** and cost **$0.75** across 2 turns.
Inspection of the execution transcript revealed that the driver invoked `view_file` **52 times** (40 in Turn 1, 12 in Turn 2), continuously dumping large source files into its attention window.

While `superstate inspect` was available on `$PATH`, the model never invoked it:
1. LLM foundation models have deeply entrenched priors to use native IDE tools (`view_file`, `cat`) to ingest complete files.
2. Under our strict **Zero Prompt Engineering** rule, we do not modify system prompts, prompt contracts, or guidance templates to urge tool adoption.
3. While `superstate shunt` already transparently intercepts shell commands exceeding 50 lines (tests, builds, compilers), internal runtime tools like `view_file` (in `agy`) and `View` (in `claude`) bypass the shell entirely.

### 1.2 The Spotify Portal PreToolUse Pattern
Spotify Engineering faced this exact token saturation problem in large codebases. Their solution was **not prompt engineering**, but **runtime interception via `PreToolUse` hooks**:
- A hook intercepts file read attempts before their payload enters the LLM context.
- When an agent requests a file read exceeding a strict line threshold (e.g. 200 lines) without bounds, the hook blocks the read (`decision: "deny"`).
- Instead of raw file contents, the hook returns an AST symbol outline (generated via tree-sitter) with exact line ranges, instructing the agent to re-read specific bounded slices.

This specification details the native implementation of deterministic file read shunting in `superstate`.

---

## 2. Core Architecture

The architecture consists of three decoupled components:

1. **`superstate-core::guard` (Guard Engine)**:
   - Evaluates file path, existence, line count, and requested range (`StartLine`, `EndLine`).
   - If line count $\le 200$ (or bounded range $\le 200$ lines): permits the read (`allow`).
   - If line count $> 200$ without bounds (or range $> 200$ lines): denies the read (`deny`) and constructs an informative AST outline using `superstate-syntax`.
   - Supports fallback line summaries for unsupported languages and handles edge cases (missing files, binary files, empty files).

2. **`superstate guard pre-tool-use` (CLI Entrypoint)**:
   - Reads JSON payload from `stdin` adhering to the standard agent `PreToolUse` protocol.
   - Evaluates the tool name (`view_file`, `View`, `read_file`) and arguments.
   - Writes JSON response to `stdout` (`{"decision": "allow"}` or `{"decision": "deny", "reason": "..."}`).
   - Sub-millisecond execution time in native compiled Rust.

3. **Autonomous Workspace Hook Provisioning**:
   - When `DaemonRunner` initializes or executes within a workspace, it provisions the hook configuration:
     - For Antigravity (`agy`): writes `.agents/hooks.json`.
     - For Claude Code (`claude`): writes `.claude/settings.json` hook.
   - Zero prompt changes, zero manual developer configuration.

```
       +-------------------------------------------------------+
       |             Autonomous Driver Subagent                |
       |                   (agy / claude)                      |
       +-------------------------------------------------------+
                                   |
                  Calls internal `view_file(path)`
                                   v
       +-------------------------------------------------------+
       |             `PreToolUse` Hook Interceptor             |
       |         `superstate guard pre-tool-use`               |
       +-------------------------------------------------------+
                                   |
            +----------------------+----------------------+
            |                                             |
            v                                             v
[File <= 200 lines OR                      [File > 200 lines & unbounded
 Requested range <= 200]                    OR requested range > 200]
            |                                             |
            v                                             v
  {"decision": "allow"}                         {"decision": "deny",
                                                 "reason": "[superstate-guard] ...
                                                            + AST Symbol Outline"}
```

---

## 3. Protocol & Hook Specifications

### 3.1 Input Payload Schema (`stdin`)
The hook accepts JSON matching standard agent hook contracts:

```json
{
  "toolCall": {
    "name": "view_file",
    "args": {
      "AbsolutePath": "/path/to/source.py",
      "StartLine": 1,
      "EndLine": 1200
    }
  },
  "stepIdx": 12
}
```

Tool argument normalization:
- `AbsolutePath` / `path` / `file_path`: Path to the target file.
- `StartLine` / `start_line` / `offset`: Starting line number (1-indexed).
- `EndLine` / `end_line` / `limit`: Ending line number (1-indexed).

### 3.2 Output Payload Schema (`stdout`)

#### Permitted Read:
```json
{
  "decision": "allow"
}
```

#### Denied / Shunted Read:
```json
{
  "decision": "deny",
  "reason": "[superstate-guard] Full file read blocked (1,420 lines exceeds 200-line budget).\n\nAST Symbol Outline:\n- [Class] Representation [lines 45-480]\n  - [Function] _construct_planar_graph [lines 310-365]\n  - [Function] crossings [lines 370-410]\n- [Class] DTNotation [lines 485-920]\n\nRe-read using bounded StartLine and EndLine (<= 200 lines), or inspect specific symbols."
}
```

### 3.3 Evaluation Rules
1. **File non-existent:** Return `{"decision": "allow"}` so the runtime tool produces standard error messages.
2. **File $\le 200$ lines:** Return `{"decision": "allow"}`.
3. **Explicit range $\le 200$ lines (`EndLine - StartLine <= 200`):** Return `{"decision": "allow"}`.
4. **Range $> 200$ lines or unbounded:**
   - Attempt AST symbol outline extraction via `superstate_syntax::inspect::format_outline`.
   - If language is unsupported or parse fails: output a structural line summary (total lines, first 30 lines, and guidance to specify line range).
5. **Binary file:** Return `{"decision": "deny", "reason": "[superstate-guard] Binary file read blocked."}`.

---

## 4. Workspace Hook Provisioning

In `superstate-core::daemon` (and `lab/engine/adapters/superstate_daemon.py`), the workspace is provisioned with `.agents/hooks.json`:

```json
{
  "superstate-file-guard": {
    "PreToolUse": [
      {
        "matcher": "view_file|View|read_file",
        "hooks": [
          {
            "type": "command",
            "command": "superstate guard pre-tool-use --threshold-lines 200",
            "timeout": 10
          }
        ]
      }
    ]
  }
}
```

---

## 5. Verification & Testing Strategy

1. **Unit Tests (`superstate-core`):**
   - Test `evaluate_file_read_guard` with small files ($\le 200$ lines) $\rightarrow$ `Allow`.
   - Test with bounded ranges $\le 200$ lines $\rightarrow$ `Allow`.
   - Test with unbounded large files $\rightarrow$ `Deny` with AST outline containing classes and functions.
   - Test with non-existent and binary files.
   - Test with unsupported languages (text fallback).

2. **Integration Tests (`superstate-cli`):**
   - Test `superstate guard pre-tool-use` over standard stdin/stdout JSON streaming.
   - Validate exit code 0 and proper JSON output on both allowed and denied cases.

3. **Canary Benchmark (`superstate-eval`):**
   - Run `build-cython-ext` under `superstate_daemon_full`.
   - Assert `view_file` calls over 200 lines are intercepted.
   - Compare input token consumption against EXP-066 (738k tokens). Target: $\ge 40\%$ reduction in token consumption.
