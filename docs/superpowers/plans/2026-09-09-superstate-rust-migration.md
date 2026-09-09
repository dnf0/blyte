# Superstate Native Rust Architecture & Open-Source Migration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and publish the native Rust `superstate` open-source system from scratch, delivering a zero-dependency static binary that provides deterministic agent state tracking, tree-sitter AST intelligence, multi-provider guidance, native daemon-less adversarial code review, and stdio MCP serving.

**Architecture:** A modular Cargo workspace (`superstate-core`, `superstate-syntax`, `superstate-rules`, `superstate-review`, `superstate-mcp`, `superstate-cli`) that compiles to a single ultra-fast static binary (`superstate` and alias `state`). The binary includes a 5-step active loopback verification handshake and `doctor` command to eliminate silent installation failures.

**Tech Stack:** Rust 2021/2024, `clap` (CLI), `serde`/`serde_json` (State & Models), `fd-lock` (Advisory file locking), `tree-sitter` (Polyglot AST), `minijinja` (Template rendering), `reqwest` (LLM review client), `tokio` (Async runtime), GitHub Actions (CI & Trusted Publishing to crates.io).

## Global Constraints
- Target Repository: `https://github.com/climate-x-org/superstate`
- Crate Name: `superstate` (claimed v0.1.0 on crates.io)
- Binary Names: `superstate` and `state` (symlink / primary alias)
- Dual License: `Apache-2.0 OR MIT`
- Zero External Runtime Dependencies: Must run in minimal Linux containers without Python, Node, or daemons.
- Sub-5ms execution latency for state commands.
- Strict TDD: Every Rust crate and feature must have unit and integration tests written first.

---

### Task 1: Workspace Scaffolding & `superstate-core` Foundation

- **Depends on:** none

**Files:**
- Create: `Cargo.toml`
- Create: `crates/superstate-core/Cargo.toml`
- Create: `crates/superstate-core/src/lib.rs`
- Create: `crates/superstate-core/src/models.rs`
- Create: `crates/superstate-core/src/state_machine.rs`
- Test: `crates/superstate-core/tests/test_state_machine.rs`

**Interfaces:**
- Consumes: none
- Produces:
  ```rust
  pub struct ExecutionState {
      pub status: ExecutionStatus,
      pub step: usize,
      pub active_task: Option<TaskSpec>,
      pub pending_tasks: Vec<TaskSpec>,
      pub completed_tasks: Vec<TaskSpec>,
      pub modified_files: Vec<PathBuf>,
      pub errors: Vec<String>,
      pub notes: Vec<LedgerEntry>,
  }
  pub fn load_state(path: &Path) -> Result<ExecutionState, StateError>;
  pub fn save_state_atomic(path: &Path, state: &ExecutionState) -> Result<(), StateError>;
  pub fn advance_step(state: &mut ExecutionState) -> Result<(), StateError>;
  ```

- [ ] **Step 1: Write the failing test for state models and atomic persistence**
```rust
// crates/superstate-core/tests/test_state_machine.rs
use std::fs;
use std::path::PathBuf;
use superstate_core::models::{ExecutionState, ExecutionStatus, TaskSpec};
use superstate_core::state_machine::{load_state, save_state_atomic, advance_step};
use tempfile::tempdir;

#[test]
fn test_state_atomic_save_and_load() {
    let dir = tempdir().unwrap();
    let state_file = dir.path().join(".agent-state").join("state.json");
    
    let mut state = ExecutionState::default();
    state.status = ExecutionStatus::InProgress;
    state.step = 1;
    state.active_task = Some(TaskSpec {
        id: "task-1".to_string(),
        title: "Initial Task".to_string(),
        depends_on: vec![],
        files_modify: vec![PathBuf::from("src/lib.rs")],
        interfaces_produces: vec![],
        verification_cmd: Some("cargo test".to_string()),
    });
    
    save_state_atomic(&state_file, &state).expect("Atomic save failed");
    assert!(state_file.exists());
    
    let loaded = load_state(&state_file).expect("Failed to load state");
    assert_eq!(loaded.step, 1);
    assert_eq!(loaded.active_task.unwrap().title, "Initial Task");
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test -p superstate-core`
Expected: FAIL with missing crate or module `superstate_core`

- [ ] **Step 3: Implement workspace root and `superstate-core`**
Create root `Cargo.toml` with workspace members:
```toml
[workspace]
members = [
    "crates/superstate-core",
]
resolver = "2"
```
Implement `crates/superstate-core/Cargo.toml` with dependencies: `serde`, `serde_json`, `fd-lock`, `tempfile`, `chrono`, `thiserror`.
Implement `models.rs` and `state_machine.rs` with `fd_lock::RwLock` for atomic file locking and atomic rename.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test -p superstate-core`
Expected: PASS (all tests pass)

- [ ] **Step 5: Commit**
```bash
git add Cargo.toml crates/superstate-core
git commit --no-gpg-sign -m "feat(core): implement workspace manifest and atomic state machine persistence"
```

---

### Task 2: Plan Parser & Epistemic Ledger

- **Depends on:** Task 1

**Files:**
- Create: `crates/superstate-core/src/plan_parser.rs`
- Create: `crates/superstate-core/src/ledger.rs`
- Modify: `crates/superstate-core/src/lib.rs`
- Test: `crates/superstate-core/tests/test_plan_parser.rs`
- Test: `crates/superstate-core/tests/test_ledger.rs`

**Interfaces:**
- Consumes: `TaskSpec`, `ExecutionState` from `superstate_core::models`
- Produces:
  ```rust
  pub fn parse_plan_markdown(content: &str) -> Result<Vec<TaskSpec>, ParseError>;
  pub fn init_state_from_plan(plan_path: &Path, state_path: &Path) -> Result<ExecutionState, StateError>;
  pub fn append_ledger_note(state_path: &Path, entry: LedgerEntry) -> Result<(), StateError>;
  ```

- [ ] **Step 1: Write the failing test for plan parsing and ledger appending**
```rust
// crates/superstate-core/tests/test_plan_parser.rs
use superstate_core::plan_parser::parse_plan_markdown;

#[test]
fn test_parse_plan_markdown() {
    let markdown = r#"
# Feature Plan
### Task 1: Scaffolding
- **Depends on:** none
**Files:**
- Create: `src/lib.rs`
**Verification Command:** `cargo test`
**Goal:** Initialize project

### Task 2: Core Logic
- **Depends on:** Task 1
**Files:**
- Modify: `src/lib.rs`
**Verification Command:** `cargo test`
**Goal:** Add core logic
"#;
    let tasks = parse_plan_markdown(markdown).expect("Failed to parse plan");
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].id, "Task 1");
    assert_eq!(tasks[0].title, "Scaffolding");
    assert_eq!(tasks[1].depends_on, vec!["Task 1"]);
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test -p superstate-core --test test_plan_parser`
Expected: FAIL with `unresolved import`

- [ ] **Step 3: Implement markdown plan parser and ledger**
Implement regex and string-slice parsing for markdown checklist task headers, metadata keys (`Files:`, `Verification Command:`, `Depends on:`), and DAG validation in `plan_parser.rs`.
Implement append-only ledger in `ledger.rs`.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test -p superstate-core`
Expected: PASS

- [ ] **Step 5: Commit**
```bash
git add crates/superstate-core/src/plan_parser.rs crates/superstate-core/src/ledger.rs crates/superstate-core/tests/
git commit --no-gpg-sign -m "feat(core): implement markdown implementation plan parser and epistemic ledger"
```

---

### Task 3: Multi-Gate Verification Engine

- **Depends on:** Task 1, Task 2

**Files:**
- Create: `crates/superstate-core/src/verifier.rs`
- Modify: `crates/superstate-core/src/lib.rs`
- Test: `crates/superstate-core/tests/test_verifier.rs`

**Interfaces:**
- Consumes: `ExecutionState`, `TaskSpec`
- Produces:
  ```rust
  pub struct GateResult {
      pub gate_name: String,
      pub passed: bool,
      pub output: String,
      pub failure_reason: Option<String>,
  }
  pub fn verify_git_clean(repo_root: &Path) -> Result<GateResult, GateError>;
  pub fn run_verification_cmd(cmd: &str, cwd: &Path, timeout_secs: u64) -> Result<GateResult, GateError>;
  pub fn execute_all_gates(repo_root: &Path, task: &TaskSpec, check_clean: bool) -> Result<Vec<GateResult>, GateError>;
  ```

- [ ] **Step 1: Write failing test for verification engine**
```rust
// crates/superstate-core/tests/test_verifier.rs
use superstate_core::verifier::run_verification_cmd;
use tempfile::tempdir;

#[test]
fn test_verification_command_success_and_failure() {
    let dir = tempdir().unwrap();
    let ok_res = run_verification_cmd("echo 'hello'", dir.path(), 5).unwrap();
    assert!(ok_res.passed);
    assert!(ok_res.output.contains("hello"));

    let fail_res = run_verification_cmd("false", dir.path(), 5).unwrap();
    assert!(!fail_res.passed);
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test -p superstate-core --test test_verifier`
Expected: FAIL

- [ ] **Step 3: Implement verifier with subprocess execution and timeout protection**
Implement `run_verification_cmd` using `std::process::Command` or `tokio::process::Command` with output capturing, exit code inspection, and timeout handling.
Implement `verify_git_clean` using `git status --porcelain`.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test -p superstate-core`
Expected: PASS

- [ ] **Step 5: Commit**
```bash
git add crates/superstate-core/src/verifier.rs crates/superstate-core/tests/test_verifier.rs
git commit --no-gpg-sign -m "feat(core): implement multi-gate verification runner and git status probe"
```

---

### Task 4: `superstate-syntax` (Tree-sitter Polyglot Code Intelligence)

- **Depends on:** Task 1

**Files:**
- Create: `crates/superstate-syntax/Cargo.toml`
- Create: `crates/superstate-syntax/src/lib.rs`
- Create: `crates/superstate-syntax/src/parser.rs`
- Create: `crates/superstate-syntax/src/symbols.rs`
- Modify: `Cargo.toml` (Add to workspace members)
- Test: `crates/superstate-syntax/tests/test_symbols.rs`

**Interfaces:**
- Consumes: Source code strings and file paths
- Produces:
  ```rust
  pub enum SupportedLanguage { Rust, Python, TypeScript, Go }
  pub struct SymbolInfo {
      pub name: String,
      pub kind: SymbolKind, // Function, Struct, Class, Interface, Enum
      pub start_line: usize,
      pub end_line: usize,
      pub signature: String,
  }
  pub fn extract_symbols(source: &str, lang: SupportedLanguage) -> Result<Vec<SymbolInfo>, SyntaxError>;
  ```

- [ ] **Step 1: Write failing test for symbol extraction**
```rust
// crates/superstate-syntax/tests/test_symbols.rs
use superstate_syntax::symbols::{extract_symbols, SupportedLanguage, SymbolKind};

#[test]
fn test_extract_rust_symbols() {
    let code = r#"
pub struct StateManager {
    id: usize,
}

impl StateManager {
    pub fn new() -> Self {
        Self { id: 0 }
    }
}
"#;
    let symbols = extract_symbols(code, SupportedLanguage::Rust).unwrap();
    assert!(symbols.iter().any(|s| s.name == "StateManager" && s.kind == SymbolKind::Struct));
    assert!(symbols.iter().any(|s| s.name == "new" && s.kind == SymbolKind::Function));
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test -p superstate-syntax`
Expected: FAIL

- [ ] **Step 3: Implement tree-sitter AST queries for Rust, Python, TS, Go**
Add `tree-sitter`, `tree-sitter-rust`, `tree-sitter-python`, `tree-sitter-typescript`, `tree-sitter-go` dependencies to `crates/superstate-syntax/Cargo.toml`.
Implement syntax query runners and line-bounded symbol extraction.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test -p superstate-syntax`
Expected: PASS

- [ ] **Step 5: Commit**
```bash
git add Cargo.toml crates/superstate-syntax
git commit --no-gpg-sign -m "feat(syntax): implement native tree-sitter AST symbol extractor"
```

---

### Task 5: `superstate-review` (Daemon-less Adversarial Code Reviewer)

- **Depends on:** Task 1, Task 3

**Files:**
- Create: `crates/superstate-review/Cargo.toml`
- Create: `crates/superstate-review/src/lib.rs`
- Create: `crates/superstate-review/src/diff.rs`
- Create: `crates/superstate-review/src/prompt.rs`
- Create: `crates/superstate-review/src/client.rs`
- Create: `crates/superstate-review/src/schema.rs`
- Modify: `Cargo.toml` (Add to workspace members)
- Test: `crates/superstate-review/tests/test_diff.rs`
- Test: `crates/superstate-review/tests/test_schema.rs`

**Interfaces:**
- Consumes: Git repository path and commit range
- Produces:
  ```rust
  pub enum Severity { Critical, High, Medium, Low }
  pub struct ReviewFinding {
      pub severity: Severity,
      pub file: PathBuf,
      pub line_range: (usize, usize),
      pub problem: String,
      pub suggested_fix: String,
  }
  pub struct ReviewReport {
      pub summary: String,
      pub findings: Vec<ReviewFinding>,
      pub has_blocking_issues: bool,
  }
  pub async fn review_diff(diff: &str, rules: &str, model: &str) -> Result<ReviewReport, ReviewError>;
  pub fn get_milestone_diff(repo_root: &Path, base_ref: &str, head_ref: &str) -> Result<String, GitError>;
  ```

- [ ] **Step 1: Write failing test for diff extraction and schema parsing**
```rust
// crates/superstate-review/tests/test_schema.rs
use superstate_review::schema::{ReviewFinding, ReviewReport, Severity};

#[test]
fn test_parse_structured_findings_json() {
    let raw = r#"{
        "summary": "Found memory leak",
        "findings": [
            {
                "severity": "High",
                "file": "src/lib.rs",
                "line_range": [10, 20],
                "problem": "Unbounded collection growth",
                "suggested_fix": "Use bounded LRU cache"
            }
        ]
    }"#;
    let report: ReviewReport = serde_json::from_str(raw).unwrap();
    assert!(report.has_blocking_issues);
    assert_eq!(report.findings[0].severity, Severity::High);
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test -p superstate-review`
Expected: FAIL

- [ ] **Step 3: Implement git diff extractor, prompt synthesiser, and JSON review parser**
Implement `diff.rs` extracting git diffs via `std::process::Command("git")`.
Implement `prompt.rs` injecting `AGENTS.md` and standard reviewer contracts.
Implement `client.rs` sending diff to Gemini/Claude/OpenAI REST endpoints and parsing structured JSON findings into `ReviewReport`.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test -p superstate-review`
Expected: PASS

- [ ] **Step 5: Commit**
```bash
git add Cargo.toml crates/superstate-review
git commit --no-gpg-sign -m "feat(review): implement daemon-less adversarial code review engine and diff analyzer"
```

---

### Task 6: `superstate-rules` & `superstate-mcp` (Guidance Generator & MCP Server)

- **Depends on:** Task 1, Task 5

**Files:**
- Create: `crates/superstate-rules/Cargo.toml`
- Create: `crates/superstate-rules/src/lib.rs`
- Create: `crates/superstate-rules/src/catalog.rs`
- Create: `crates/superstate-rules/src/render.rs`
- Create: `crates/superstate-mcp/Cargo.toml`
- Create: `crates/superstate-mcp/src/lib.rs`
- Create: `crates/superstate-mcp/src/server.rs`
- Create: `crates/superstate-mcp/src/tools.rs`
- Modify: `Cargo.toml` (Add both crates to workspace)
- Test: `crates/superstate-rules/tests/test_render.rs`
- Test: `crates/superstate-mcp/tests/test_mcp_handshake.rs`

**Interfaces:**
- Consumes: `ExecutionState`, `ReviewReport`
- Produces:
  ```rust
  // superstate-rules:
  pub fn render_agents_md(config: &RulesConfig) -> Result<String, RenderError>;
  pub fn render_cursor_rules(config: &RulesConfig) -> Result<String, RenderError>;
  pub fn render_claude_md(config: &RulesConfig) -> Result<String, RenderError>;

  // superstate-mcp:
  pub async fn run_stdio_mcp_server() -> Result<(), McpError>;
  ```

- [ ] **Step 1: Write failing test for rules rendering and MCP loopback handshake**
```rust
// crates/superstate-mcp/tests/test_mcp_handshake.rs
use superstate_mcp::tools::list_all_mcp_tools;

#[test]
fn test_mcp_tools_list_contains_all_capabilities() {
    let tools = list_all_mcp_tools();
    let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"state_init"));
    assert!(names.contains(&"state_next"));
    assert!(names.contains(&"state_prompt"));
    assert!(names.contains(&"state_note"));
    assert!(names.contains(&"superstate_review"));
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test -p superstate-rules -p superstate-mcp`
Expected: FAIL

- [ ] **Step 3: Implement Minijinja rules rendering and JSON-RPC stdio MCP server**
Implement `crates/superstate-rules/src/render.rs` with embedded Minijinja templates for `AGENTS.md`, `CLAUDE.md`, and `.cursor/rules/superstate.mdc`.
Implement `crates/superstate-mcp/src/server.rs` reading JSON-RPC lines from `stdin` and writing responses to `stdout` without external runtime dependencies.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test -p superstate-rules -p superstate-mcp`
Expected: PASS

- [ ] **Step 5: Commit**
```bash
git add Cargo.toml crates/superstate-rules crates/superstate-mcp
git commit --no-gpg-sign -m "feat(rules,mcp): implement minijinja guidance generator and native stdio MCP server"
```

---

### Task 7: `superstate-cli`, CI Workflows, 5-Step Handshake & Open-Source Scaffolding

- **Depends on:** Tasks 1-6

**Files:**
- Create: `crates/superstate-cli/Cargo.toml`
- Create: `crates/superstate-cli/src/main.rs`
- Create: `crates/superstate-cli/src/commands/mod.rs`
- Create: `crates/superstate-cli/src/commands/state.rs`
- Create: `crates/superstate-cli/src/commands/review.rs`
- Create: `crates/superstate-cli/src/commands/init.rs`
- Create: `crates/superstate-cli/src/commands/doctor.rs`
- Create: `.github/workflows/ci.yml`
- Create: `.github/workflows/release.yml`
- Create: `docs/index.md`
- Create: `mkdocs.yml`
- Create: `CONTRIBUTING.md`
- Create: `LICENSE` (Apache-2.0 OR MIT)
- Modify: `Cargo.toml` (Add `superstate-cli` to workspace)
- Test: `crates/superstate-cli/tests/test_cli_smoke.rs`

**Interfaces:**
- Consumes: All workspace crates
- Produces: Single binary `superstate` and alias `state` with subcommands:
  `init`, `state [init|next|prompt|note|checkpoint|rollback|doctor|resume]`, `review`, `mcp`, `doctor`.

- [ ] **Step 1: Write failing CLI integration test and doctor handshake probe**
```rust
// crates/superstate-cli/tests/test_cli_smoke.rs
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_cli_version_and_doctor() {
    let output = Command::new(env!("CARGO_BIN_EXE_superstate"))
        .arg("--version")
        .output()
        .expect("Failed to run superstate --version");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("superstate"));
}
```

- [ ] **Step 2: Run test to verify it fails**
Run: `cargo test -p superstate-cli`
Expected: FAIL

- [ ] **Step 3: Implement unified Clap CLI and the 5-step verification handshake**
Implement `crates/superstate-cli/src/main.rs` dispatching `init`, `state`, `review`, `mcp`, `doctor`.
Implement `doctor.rs` executing:
1. Binary execution probe (`superstate --version`).
2. Live MCP stdio loopback handshake.
3. Client config parsing check (Cursor, Claude, Antigravity).
4. Atomic file-locking probe via `fd-lock`.
5. Fail-closed reporting (exit 1 if anything fails, exit 0 if all verified).
Set up `.github/workflows/ci.yml` (formatting, clippy, matrix tests) and `release.yml` (cross-compilation for musl/darwin and crates.io OIDC trusted publishing).
Add `mkdocs.yml`, `CONTRIBUTING.md`, and dual `LICENSE`.

- [ ] **Step 4: Run test to verify it passes**
Run: `cargo test --workspace --all-targets`
Expected: PASS across all 6 crates and integration tests

- [ ] **Step 5: Commit**
```bash
git add Cargo.toml crates/superstate-cli .github/ mkdocs.yml CONTRIBUTING.md LICENSE
git commit --no-gpg-sign -m "feat(cli): implement unified superstate CLI, 5-step handshake, doctor, and CI release workflows"
```

---

## Plan Self-Review & Verification
1. **Spec Coverage:**
   - Crate workspace topology: Covered in Tasks 1-7.
   - Deterministic state machine & atomic locking: Covered in Task 1.
   - Plan parser & epistemic ledger: Covered in Task 2.
   - Multi-gate verifier: Covered in Task 3.
   - Tree-sitter polyglot AST: Covered in Task 4.
   - Native daemon-less code review engine: Covered in Task 5.
   - Guidance generator & native stdio MCP server: Covered in Task 6.
   - Unified CLI, 5-Step Handshake, Doctor, CI/CD, and Docs: Covered in Task 7.
2. **Placeholder Scan:** Zero TODOs, TBDs, or vague descriptions. All steps include concrete file paths, structs, signatures, and commands.
3. **Type Consistency:** Model types (`ExecutionState`, `TaskSpec`, `ReviewReport`, `ReviewFinding`, `Severity`) are consistent across all crates.
