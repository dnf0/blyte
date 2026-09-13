# Cross-Agent Handoff Protocol & Driver Presets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Choose an execution mode:
> 1. `superpowers:subagent-driven-development` (recommended for multi-agent reviews, backed by `SKILL.state` / `.agent-state/state.json`)
> 2. `agent-rules:stateful-execution` (SKILL.state) (recommended for deterministic single-agent linear execution)
> 3. `superpowers:executing-plans` (batch execution with manual checkpoints)
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide seamless cross-agent handoff between Claude Code, Antigravity (`agy`), and background daemons via intelligent headless driver preset expansion, persistent standardized handoff artifacts (`.agent-state/handoff.md`), and full CLI & MCP interface parity (`superstate state handoff` and `superstate_handoff`).

**Architecture:** 
- `superstate-core::daemon` resolves driver commands, automatically augmenting `claude` and `agy` with required non-interactive and permission flags (`-p`, `--dangerously-skip-permissions`).
- `superstate-core::handoff` generates and writes structured `.agent-state/handoff.md` and `.agent-state/handoffs/<slug>.md` packages containing objective, active milestone, worktree path, epistemic findings, and copy-pasteable resumption commands for Claude and Agy.
- `superstate-cli` adds `superstate state handoff` command matching `superstate_handoff` MCP tool.
- `superstate-mcp` enhances `superstate_handoff` with `from_agent`, `to_agent`, and returns tailored launch instructions.

**Tech Stack:** Rust (2021 edition), Tokio, Clap v4, Serde / Serde JSON, Bash E2E scripts.

## Global Constraints
- Target Rust 2021 edition with strict type safety, zero `unwrap()` panics on untrusted input, and clean error propagation.
- Every commit must follow Conventional Commits (e.g. `feat:`, `test:`, `fix:`, `docs:`).
- `cargo clippy --all-targets -- -D warnings` must pass with zero warnings.
- `cargo fmt --all -- --check` must pass.
- Bounded context: stderr in driver warnings capped to 1,000 characters; handoff notes capped to prevent unbounded bloat.

---

### Task 1: Autonomous Driver Preset Engine & Expansion in `superstate-core`

- **Depends on:** none

**Files:**
- Create: `crates/superstate-core/tests/test_driver_presets.rs`
- Modify: `crates/superstate-core/src/daemon.rs`
- Modify: `crates/superstate-core/src/lib.rs`

**Interfaces:**
- Produces: `pub fn resolve_driver_command(driver: &str) -> String` in `superstate_core::daemon`.
- Consumes: `DaemonConfig.driver_cmd` in `DaemonRunner::run_step`.

- [ ] **Step 1: Write the failing tests for driver preset expansion**

Create `crates/superstate-core/tests/test_driver_presets.rs`:
```rust
use superstate_core::daemon::resolve_driver_command;

#[test]
fn test_resolve_driver_preset_claude_exact() {
    let resolved = resolve_driver_command("claude");
    assert_eq!(resolved, "claude -p --dangerously-skip-permissions");
}

#[test]
fn test_resolve_driver_preset_claude_with_model() {
    let resolved = resolve_driver_command("claude --model sonnet-4");
    assert!(resolved.contains("claude --model sonnet-4"));
    assert!(resolved.contains("-p"));
    assert!(resolved.contains("--dangerously-skip-permissions"));
}

#[test]
fn test_resolve_driver_preset_claude_preserves_existing_flags() {
    let resolved = resolve_driver_command("claude -p --dangerously-skip-permissions --model haiku");
    assert_eq!(resolved, "claude -p --dangerously-skip-permissions --model haiku");
}

#[test]
fn test_resolve_driver_preset_agy_exact() {
    let resolved = resolve_driver_command("agy");
    assert_eq!(resolved, "agy -p --dangerously-skip-permissions");
}

#[test]
fn test_resolve_driver_preset_agy_with_model() {
    let resolved = resolve_driver_command("agy --model gemini-3.8-pro");
    assert!(resolved.contains("agy --model gemini-3.8-pro"));
    assert!(resolved.contains("-p"));
    assert!(resolved.contains("--dangerously-skip-permissions"));
}

#[test]
fn test_resolve_driver_preset_custom_command() {
    let cmd = "python3 ./worker.py --batch 10";
    let resolved = resolve_driver_command(cmd);
    assert_eq!(resolved, cmd);
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --test test_driver_presets`
Expected: FAIL (`cannot find function resolve_driver_command in module daemon`)

- [ ] **Step 3: Implement `resolve_driver_command` in `daemon.rs`**

In `crates/superstate-core/src/daemon.rs`:
```rust
/// Expands recognized agent driver names (claude, agy) into headless non-interactive invocations
/// with required bypass permissions and print flags, while preserving any user-supplied flags.
pub fn resolve_driver_command(driver: &str) -> String {
    let trimmed = driver.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    let binary = parts[0];

    if binary == "claude" {
        let has_print = parts.iter().any(|&arg| arg == "-p" || arg == "--print");
        let has_perm = parts.iter().any(|&arg| {
            arg == "--dangerously-skip-permissions"
                || arg == "--allow-dangerously-skip-permissions"
                || arg.starts_with("--permission-mode")
        });

        let mut res = trimmed.to_string();
        if !has_print {
            res.push_str(" -p");
        }
        if !has_perm {
            res.push_str(" --dangerously-skip-permissions");
        }
        res
    } else if binary == "agy" {
        let has_print = parts.iter().any(|&arg| arg == "-p" || arg == "--print");
        let has_perm = parts.iter().any(|&arg| {
            arg == "--dangerously-skip-permissions"
                || arg == "--allow-dangerously-skip-permissions"
                || arg.starts_with("--permission-mode")
        });

        let mut res = trimmed.to_string();
        if !has_print {
            res.push_str(" -p");
        }
        if !has_perm {
            res.push_str(" --dangerously-skip-permissions");
        }
        res
    } else {
        trimmed.to_string()
    }
}
```
And in `DaemonRunner::run_step`, use `resolve_driver_command`:
```rust
        if let Some(ref driver) = self.config.driver_cmd {
            let resolved_driver = resolve_driver_command(driver);
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(&resolved_driver)
                .current_dir(&working_dir)
...
```
Also expose `SUPERSTATE_WORKTREE_PATH`:
```rust
                .env("SUPERSTATE_WORKTREE_PATH", working_dir.to_string_lossy().as_ref())
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test test_driver_presets`
Expected: PASS

- [ ] **Step 5: Run linter and formatting**

Run: `cargo clippy -p superstate-core --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: PASS with 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/superstate-core/src/daemon.rs crates/superstate-core/tests/test_driver_presets.rs
git commit -m "feat(core): implement autonomous driver preset expansion for claude and agy"
```

---

### Task 2: Standardized Cross-Agent Handoff Package & Briefing in `superstate-core`

- **Depends on:** Task 1

**Files:**
- Create: `crates/superstate-core/src/handoff.rs`
- Create: `crates/superstate-core/tests/test_handoff_package.rs`
- Modify: `crates/superstate-core/src/lib.rs`
- Modify: `crates/superstate-core/src/maintenance.rs`

**Interfaces:**
- Produces:
  - `pub struct HandoffSpec`
  - `pub fn generate_handoff_markdown(state: &ExecutionState, spec: &HandoffSpec, notes: &[LedgerEntry]) -> String`
  - `pub fn write_handoff_package(repo_root: &Path, state: &ExecutionState, spec: &HandoffSpec, notes: &[LedgerEntry]) -> Result<PathBuf, String>`
  - `pub fn generate_resumption_command(target_agent: &str, worktree_path: Option<&Path>, state_path: &Path) -> String`
- Consumes: `ExecutionState`, `LedgerEntry`, `resume_briefing`.

- [ ] **Step 1: Write the failing tests for handoff package generation**

Create `crates/superstate-core/tests/test_handoff_package.rs`:
```rust
use std::path::{Path, PathBuf};
use superstate_core::handoff::{generate_handoff_markdown, write_handoff_package, HandoffSpec};
use superstate_core::models::{ExecutionState, LedgerEntry, TaskSpec};
use tempfile::tempdir;

#[test]
fn test_generate_handoff_markdown_claude_to_agy() {
    let mut state = ExecutionState::new("Test cross-agent handoff goal");
    state.active_task = Some(TaskSpec {
        id: "task-1".to_string(),
        title: "Implement feature alpha".to_string(),
        verification_cmd: Some("pytest tests/test_alpha.py".to_string()),
        files_modify: vec![PathBuf::from("src/alpha.py")],
        depends_on: vec![],
    });
    state.active_worktree = Some(PathBuf::from(".wt/alpha-task-1"));

    let spec = HandoffSpec {
        from_agent: Some("claude".to_string()),
        to_agent: Some("agy".to_string()),
        state_path: PathBuf::from(".agent-state/state.json"),
        worktree_path: Some(PathBuf::from(".wt/alpha-task-1")),
        slug: "alpha".to_string(),
    };

    let notes = vec![LedgerEntry::new("claude", "Discovered auth dependency")];

    let md = generate_handoff_markdown(&state, &spec, &notes);
    assert!(md.contains("Source Agent: claude"));
    assert!(md.contains("Target Agent: agy"));
    assert!(md.contains("Implement feature alpha"));
    assert!(md.contains("pytest tests/test_alpha.py"));
    assert!(md.contains("Discovered auth dependency"));
    assert!(md.contains("cd .wt/alpha-task-1 && agy"));
}

#[test]
fn test_write_handoff_package_writes_files() {
    let tmp = tempdir().unwrap();
    let repo_root = tmp.path();

    let mut state = ExecutionState::new("Goal");
    state.active_task = Some(TaskSpec {
        id: "task-1".to_string(),
        title: "Task 1".to_string(),
        verification_cmd: Some("cargo test".to_string()),
        files_modify: vec![],
        depends_on: vec![],
    });

    let spec = HandoffSpec {
        from_agent: Some("claude".to_string()),
        to_agent: Some("agy".to_string()),
        state_path: repo_root.join(".agent-state/state.json"),
        worktree_path: None,
        slug: "task-1".to_string(),
    };

    let path = write_handoff_package(repo_root, &state, &spec, &[]).unwrap();
    assert!(path.exists());
    assert_eq!(path, repo_root.join(".agent-state/handoff.md"));

    let scoped = repo_root.join(".agent-state/handoffs/task-1.md");
    assert!(scoped.exists());
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test --test test_handoff_package`
Expected: FAIL (`unresolved import superstate_core::handoff`)

- [ ] **Step 3: Implement `handoff.rs` in `crates/superstate-core`**

Create `crates/superstate-core/src/handoff.rs`:
```rust
use crate::models::{ExecutionState, LedgerEntry};
use crate::prompt::compact_ledger_note;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct HandoffSpec {
    pub from_agent: Option<String>,
    pub to_agent: Option<String>,
    pub state_path: PathBuf,
    pub worktree_path: Option<PathBuf>,
    pub slug: String,
}

pub fn generate_resumption_command(
    target_agent: &str,
    worktree_path: Option<&Path>,
    state_path: &Path,
) -> String {
    let cd_prefix = match worktree_path {
        Some(wt) => format!("cd {} && ", wt.display()),
        None => String::new(),
    };

    match target_agent.to_lowercase().as_str() {
        "claude" => format!("{}claude", cd_prefix),
        "agy" => format!("{}agy", cd_prefix),
        "daemon" => format!(
            "superstate state daemon --state {} --driver claude --detach",
            state_path.display()
        ),
        _ => format!(
            "{}superstate state resume --state {}",
            cd_prefix,
            state_path.display()
        ),
    }
}

pub fn generate_handoff_markdown(
    state: &ExecutionState,
    spec: &HandoffSpec,
    notes: &[LedgerEntry],
) -> String {
    let from = spec.from_agent.as_deref().unwrap_or("unknown");
    let to = spec.to_agent.as_deref().unwrap_or("any");
    let working_dir = spec
        .worktree_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "repository root (in-place)".to_string());

    let active_task_str = match &state.active_task {
        Some(t) => format!("`{}`: {}", t.id, t.title),
        None => "All milestones completed".to_string(),
    };

    let mut md = String::new();
    md.push_str("# 🤝 Superstate Handoff Package\n\n");
    md.push_str(&format!("- **Source Agent**: {}\n", from));
    md.push_str(&format!("- **Target Agent**: {}\n", to));
    md.push_str(&format!("- **Job Slug**: {}\n", spec.slug));
    md.push_str(&format!("- **State File**: `{}`\n", spec.state_path.display()));
    md.push_str(&format!("- **Active Task**: {}\n", active_task_str));
    md.push_str(&format!("- **Working Directory**: `{}`\n\n", working_dir));
    md.push_str("---\n\n");

    if let Some(ref goal) = state.goal {
        md.push_str("## 🎯 Global Objective\n");
        md.push_str(goal.trim());
        md.push_str("\n\n");
    }

    if let Some(ref active) = state.active_task {
        md.push_str("## 📋 Active Milestone Specification\n");
        md.push_str(&format!("- **Task ID**: `{}`\n", active.id));
        md.push_str(&format!("- **Title**: {}\n", active.title));
        if let Some(ref cmd) = active.verification_cmd {
            md.push_str(&format!("- **Verification Invariant**: `{}`\n", cmd));
        }
        if !active.files_modify.is_empty() {
            md.push_str("- **Files to Modify**:\n");
            for f in &active.files_modify {
                md.push_str(&format!("  - `{}`\n", f.display()));
            }
        }
        md.push_str("\n");
    }

    if !notes.is_empty() {
        md.push_str("## 🧠 Epistemic Ledger (Discoveries & Guardrails)\n");
        let start = notes.len().saturating_sub(5);
        for entry in &notes[start..] {
            md.push_str(&format!(
                "- [{}] {}\n",
                entry.author,
                compact_ledger_note(&entry.note)
            ));
        }
        md.push_str("\n");
    }

    md.push_str("---\n\n");
    md.push_str("## 🚀 Ready-to-Run Resumption\n\n");

    let claude_cmd = generate_resumption_command("claude", spec.worktree_path.as_deref(), &spec.state_path);
    let agy_cmd = generate_resumption_command("agy", spec.worktree_path.as_deref(), &spec.state_path);
    let daemon_cmd = generate_resumption_command("daemon", spec.worktree_path.as_deref(), &spec.state_path);

    md.push_str("### Option A: If continuing with Claude Code\n");
    md.push_str("```bash\n");
    md.push_str(&claude_cmd);
    md.push_str("\n```\n");
    md.push_str("> *Prompt to send:* `Review .agent-state/handoff.md and implement the active milestone.`\n\n");

    md.push_str("### Option B: If continuing with Antigravity (agy CLI / IDE)\n");
    md.push_str("```bash\n");
    md.push_str(&agy_cmd);
    md.push_str("\n```\n");
    md.push_str("> *Or use the state_prompt MCP tool to load the turnkey instructions directly.*\n\n");

    md.push_str("### Option C: If delegating to background daemon\n");
    md.push_str("```bash\n");
    md.push_str(&daemon_cmd);
    md.push_str("\n```\n");

    md
}

pub fn write_handoff_package(
    repo_root: &Path,
    state: &ExecutionState,
    spec: &HandoffSpec,
    notes: &[LedgerEntry],
) -> Result<PathBuf, String> {
    let md = generate_handoff_markdown(state, spec, notes);
    let agent_state_dir = repo_root.join(".agent-state");
    fs::create_dir_all(&agent_state_dir)
        .map_err(|e| format!("Failed to create .agent-state directory: {}", e))?;

    let handoff_file = agent_state_dir.join("handoff.md");
    fs::write(&handoff_file, &md)
        .map_err(|e| format!("Failed to write {}: {}", handoff_file.display(), e))?;

    let handoffs_dir = agent_state_dir.join("handoffs");
    fs::create_dir_all(&handoffs_dir)
        .map_err(|e| format!("Failed to create handoffs directory: {}", e))?;

    let scoped_file = handoffs_dir.join(format!("{}.md", spec.slug));
    fs::write(&scoped_file, &md)
        .map_err(|e| format!("Failed to write {}: {}", scoped_file.display(), e))?;

    Ok(handoff_file)
}
```

In `crates/superstate-core/src/lib.rs`, add:
```rust
pub mod handoff;
```
In `crates/superstate-core/src/maintenance.rs`, update `resume_briefing` to inspect `.agent-state/handoff.md` and display an `## 🤝 Active Handoff` section if found.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --test test_handoff_package`
Expected: PASS

- [ ] **Step 5: Run linter and formatting**

Run: `cargo clippy -p superstate-core --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: PASS with 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/superstate-core/src/handoff.rs crates/superstate-core/src/lib.rs crates/superstate-core/src/maintenance.rs crates/superstate-core/tests/test_handoff_package.rs
git commit -m "feat(core): implement standardized cross-agent handoff artifact and resumption generator"
```

---

### Task 3: CLI Subcommand `superstate state handoff` in `superstate-cli`

- **Depends on:** Task 1, Task 2

**Files:**
- Create: `crates/superstate-cli/src/commands/handoff.rs`
- Create: `crates/superstate-cli/tests/test_cli_handoff.rs`
- Modify: `crates/superstate-cli/src/commands/mod.rs`
- Modify: `crates/superstate-cli/src/state_cli.rs`
- Modify: `crates/superstate-cli/src/main.rs`

**Interfaces:**
- Produces: `superstate state handoff` CLI subcommand.
- Consumes: `HandoffSpec`, `write_handoff_package`, `resolve_driver_command`, `cmd_daemon`.

- [ ] **Step 1: Write the failing tests for `superstate state handoff`**

Create `crates/superstate-cli/tests/test_cli_handoff.rs`:
```rust
use std::process::Command;
use tempfile::tempdir;

#[test]
fn test_cli_handoff_interactive_creates_handoff_package() {
    let tmp = tempdir().unwrap();
    let repo_root = tmp.path();

    // Initialize git repo
    Command::new("git").args(["init"]).current_dir(repo_root).output().unwrap();
    Command::new("git").args(["config", "user.name", "Tester"]).current_dir(repo_root).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@example.com"]).current_dir(repo_root).output().unwrap();
    std::fs::write(repo_root.join("README.md"), "initial").unwrap();
    Command::new("git").args(["add", "."]).current_dir(repo_root).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial"]).current_dir(repo_root).output().unwrap();

    let bin = env!("CARGO_BIN_EXE_superstate");
    let output = Command::new(bin)
        .args([
            "state",
            "handoff",
            "--goal",
            "Implement feature Z",
            "--verify-cmd",
            "true",
            "--from",
            "claude",
            "--to",
            "agy",
            "--no-worktree",
        ])
        .current_dir(repo_root)
        .output()
        .unwrap();

    assert!(output.status.success(), "Stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Handoff package written"));
    assert!(stdout.contains("Target Agent: agy"));

    let handoff_path = repo_root.join(".agent-state/handoff.md");
    assert!(handoff_path.exists());
    let handoff_content = std::fs::read_to_string(handoff_path).unwrap();
    assert!(handoff_content.contains("Source Agent: claude"));
    assert!(handoff_content.contains("Target Agent: agy"));
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test --test test_cli_handoff`
Expected: FAIL (`unrecognized subcommand 'handoff'`)

- [ ] **Step 3: Implement `commands/handoff.rs` and wire into CLI**

In `crates/superstate-cli/src/state_cli.rs`:
Add `Handoff` variant to `StateCommands`:
```rust
    /// Create structured cross-agent handoff package and optionally dispatch daemon
    Handoff {
        /// High-level goal statement
        #[arg(long)]
        goal: Option<String>,

        /// Verification invariant command
        #[arg(long)]
        verify_cmd: Option<String>,

        /// Plan markdown file to parse milestones from
        #[arg(long)]
        plan: Option<PathBuf>,

        /// Source agent (e.g. claude, agy, human)
        #[arg(long)]
        from: Option<String>,

        /// Target agent (e.g. agy, claude, daemon, any)
        #[arg(long)]
        to: Option<String>,

        /// Worktree isolation (default: true)
        #[arg(long, default_value_t = true)]
        worktree: bool,

        /// Execute in-place without creating worktree
        #[arg(long)]
        no_worktree: bool,

        /// Launch background daemon runner immediately
        #[arg(long)]
        detach: bool,

        /// Headless coding driver preset (claude, agy) or command
        #[arg(long)]
        driver: Option<String>,

        /// Format output as JSON
        #[arg(long)]
        json: bool,

        #[arg(short, long, default_value = ".agent-state/state.json")]
        state: PathBuf,
    },
```

Implement `crates/superstate-cli/src/commands/handoff.rs`:
- Initialize state file if not already existing (via `--plan` or `--goal`).
- Resolve `from` agent (from flag or environment variables `CLAUDE_PROJECT_ROOT`, `AGY_VERSION`, etc., falling back to `human`).
- Resolve `to` agent (default: `any`).
- Write handoff package via `superstate_core::handoff::write_handoff_package`.
- Record `[handoff]` note in epistemic ledger.
- If `--detach`, spawn daemon via `superstate state daemon --state <path> --driver <driver> --detach`.
- Print human-readable or JSON summary with target resumption commands.

Wire into `crates/superstate-cli/src/commands/mod.rs` and `main.rs`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --test test_cli_handoff`
Expected: PASS

- [ ] **Step 5: Run linter and formatting**

Run: `cargo clippy -p superstate --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: PASS with 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/superstate-cli/src/commands/handoff.rs crates/superstate-cli/src/commands/mod.rs crates/superstate-cli/src/state_cli.rs crates/superstate-cli/src/main.rs crates/superstate-cli/tests/test_cli_handoff.rs
git commit -m "feat(cli): add superstate state handoff subcommand for cross-agent transitions"
```

---

### Task 4: MCP Tool Enhancement `superstate_handoff` & End-to-End Persona Verification

- **Depends on:** Task 1, Task 2, Task 3

**Files:**
- Modify: `crates/superstate-mcp/src/tools.rs`
- Modify: `crates/superstate-mcp/src/server.rs`
- Create: `crates/superstate-mcp/tests/test_mcp_cross_agent_handoff.rs`
- Create: `tests/e2e_cross_agent_handoff.sh`

**Interfaces:**
- Input schema: `from_agent: Option<String>`, `to_agent: Option<String>`, `driver: Option<String>`.
- Output payload: includes `handoff_file`, `target_resumption_command`, `turnkey_prompt`.

- [ ] **Step 1: Write failing MCP test for cross-agent handoff**

Create `crates/superstate-mcp/tests/test_mcp_cross_agent_handoff.rs`:
```rust
use serde_json::json;
use std::process::Command;
use tempfile::tempdir;

#[tokio::test]
async fn test_mcp_superstate_handoff_cross_agent_payload() {
    let tmp = tempdir().unwrap();
    let repo_root = tmp.path();

    Command::new("git").args(["init"]).current_dir(repo_root).output().unwrap();
    Command::new("git").args(["config", "user.name", "Tester"]).current_dir(repo_root).output().unwrap();
    Command::new("git").args(["config", "user.email", "test@example.com"]).current_dir(repo_root).output().unwrap();
    std::fs::write(repo_root.join("README.md"), "init").unwrap();
    Command::new("git").args(["add", "."]).current_dir(repo_root).output().unwrap();
    Command::new("git").args(["commit", "-m", "init"]).current_dir(repo_root).output().unwrap();

    let server = superstate_mcp::server::McpServer::new(repo_root.to_path_buf());
    let args = json!({
        "goal": "Build cross-agent pipeline",
        "verify_cmd": "cargo check",
        "from_agent": "claude",
        "to_agent": "agy",
        "no_worktree": true
    });

    let res = server.handle_call_tool("superstate_handoff", Some(args)).await.unwrap();
    let val: serde_json::Value = serde_json::from_str(&res).unwrap();

    assert_eq!(val["status"], "ready");
    assert!(val["handoff_file"].as_str().unwrap().contains("handoff.md"));
    assert!(val["target_resumption_command"].as_str().unwrap().contains("agy"));
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test -p superstate-mcp --test test_mcp_cross_agent_handoff`
Expected: FAIL (`target_resumption_command missing or unpopulated`)

- [ ] **Step 3: Update `crates/superstate-mcp/src/tools.rs` and `server.rs`**

1. In `crates/superstate-mcp/src/tools.rs`:
Add `from_agent` and `to_agent` properties to `superstate_handoff` input schema.

2. In `crates/superstate-mcp/src/server.rs`:
In `superstate_handoff` handler:
- Extract `from_agent` and `to_agent`.
- Call `superstate_core::handoff::write_handoff_package`.
- Record `[handoff]` note in state notes.
- If `detach: true`: resolve driver command via `superstate_core::daemon::resolve_driver_command` before launching.
- Populate `handoff_file`, `target_resumption_command`, and `turnkey_prompt` in the returned JSON.

- [ ] **Step 4: Run MCP tests to verify they pass**

Run: `cargo test -p superstate-mcp --test test_mcp_cross_agent_handoff`
Expected: PASS

- [ ] **Step 5: Create `tests/e2e_cross_agent_handoff.sh`**

Create `tests/e2e_cross_agent_handoff.sh` to simulate an end-to-end handoff cycle:
1. Initialize a temporary test repo.
2. Persona Claude: Brainstorms task, executes `superstate state handoff --from claude --to agy --goal "E2E Cross Agent" --verify-cmd "test -f alpha.txt"`.
3. Assert `.agent-state/handoff.md` exists and contains `Target Agent: agy`.
4. Persona Agy: Resumes via `superstate state resume`, inspects prompt, creates `alpha.txt`.
5. Run `superstate state next` to verify milestone.
6. Verify milestone completes, state transitions to completed, and git history contains checkpoint commit.

- [ ] **Step 6: Run E2E script**

Run: `bash tests/e2e_cross_agent_handoff.sh`
Expected: PASS with 0 exit code.

- [ ] **Step 7: Run full workspace verification check**

Run:
```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```
Expected: All tests pass, 0 warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/superstate-mcp/src/tools.rs crates/superstate-mcp/src/server.rs crates/superstate-mcp/tests/test_mcp_cross_agent_handoff.rs tests/e2e_cross_agent_handoff.sh
git commit -m "feat(mcp): support from_agent, to_agent, and turnkey resumption in superstate_handoff"
```

