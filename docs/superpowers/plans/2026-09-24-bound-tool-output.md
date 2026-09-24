# Bound Tool Output by Size Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** No tool result can overflow the model's context. Every tool output is bounded **by size**, not only by line count, and the `grep` fallback never searches dependency or build directories.

**Why (superstate-eval, 2026-09-24):** A `superstate[folded,tdd,review]` run on katex died after 50 turns with `provider returned 400: The input token count exceeds the maximum number of tokens allowed (1048576)`. Two defects combined:
- **Output was bounded only by line count.** `read`, `bash` and `grep` cap output at 400 lines (`distil`) or 200 matches, but nothing caps characters. One line of minified JavaScript can be megabytes.
- **The `grep` fallback searched everything.** The task container has no ripgrep, so `grep -rnI` ran over `node_modules`. ripgrep would have honoured `.gitignore`.

A single `grep` for `test.*\.ts` returned huge minified lines, and no context strategy can fold the newest message. Every context is affected.

**Design:**
- `tools::bound_output(text)`:
  1. Clip each line to `MAX_LINE_CHARS` (500), noting how much was cut.
  2. Distil to `OUTPUT_LINE_BUDGET` lines, as before.
  3. Cap the total at `MAX_OUTPUT_CHARS` (60,000, about 15K tokens), keeping head and tail with a marker.
- `tools::execute` becomes a thin wrapper that applies `bound_output` to every tool's result, so every current and future tool is covered. The gate's output is bounded the same way.
- The `grep` command is built by a pure `grep_command`. The fallback adds `--exclude-dir` for `node_modules`, `.git`, `target`, `.venv`, `dist` and `build`.

**Tech Stack:** Rust 2021.

## Global Constraints

- Base branch: `fix/bound-tool-output` (from `main`). The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. `superstate-mcp`'s `test_superstate_status_and_cancel_lifecycle` and `superstate-cli`'s `test_cli_daemon_detached_forwarding` are known to be timing-flaky: if one alone fails, re-run once and report both results.
- Existing tests must pass unmodified. Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `bound_output`, applied to every tool result and the gate; grep fallback excludes

**Files:**
- Modify: `crates/superstate-agent/src/tools.rs`, `crates/superstate-agent/src/run.rs`
- Test: `crates/superstate-agent/tests/test_bound_output.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/superstate-agent/tests/test_bound_output.rs`:

```rust
//! No tool result can overflow the model's context: output is bounded by size, not only lines,
//! and the grep fallback never searches dependency or build directories.

use serde_json::json;
use superstate_agent::fs::LocalFs;
use superstate_agent::shell::LocalShell;
use superstate_agent::tools::{
    bound_output, execute, grep_command, MAX_LINE_CHARS, MAX_OUTPUT_CHARS,
};

#[test]
fn a_huge_single_line_is_clipped() {
    let out = bound_output(&"a".repeat(5_000_000));
    assert!(out.len() < MAX_LINE_CHARS + 100, "{} chars", out.len());
    assert!(out.contains("more chars clipped"), "{}", &out[out.len() - 80..]);
}

#[test]
fn many_long_lines_are_capped_in_total() {
    let text: String = (0..300).map(|i| format!("{i} {}\n", "x".repeat(450))).collect();
    let out = bound_output(&text);
    assert!(out.len() <= MAX_OUTPUT_CHARS + 200, "{} chars", out.len());
    assert!(out.contains("elided by superstate"));
    assert!(out.starts_with("0 "), "the head is kept");
    assert!(out.trim_end().starts_with("0 ") && out.contains("299 "), "the tail is kept");
}

#[test]
fn ordinary_output_is_untouched() {
    let text = "exit 0\nall 11 tests passed\n";
    assert_eq!(bound_output(text), text);
}

#[test]
fn every_tool_result_is_bounded() {
    let d = tempfile::tempdir().unwrap();
    // One 2 MB line, as minified JavaScript produces.
    let r = execute(
        d.path(),
        &LocalShell,
        &LocalFs,
        "bash",
        &json!({"command": "head -c 2000000 /dev/zero | tr '\\0' a"}),
    );
    assert!(r.content.len() < MAX_OUTPUT_CHARS, "{} chars", r.content.len());
    assert!(r.content.starts_with("exit 0"));

    std::fs::write(d.path().join("min.js"), "b".repeat(3_000_000)).unwrap();
    let r = execute(d.path(), &LocalShell, &LocalFs, "read", &json!({"path": "min.js"}));
    assert!(r.content.len() < MAX_OUTPUT_CHARS, "{} chars", r.content.len());
}

#[test]
fn the_grep_fallback_skips_dependency_and_build_directories() {
    let cmd = grep_command(false, "test.*\\.ts", ".", "");
    for dir in ["node_modules", ".git", "target", ".venv", "dist", "build"] {
        assert!(cmd.contains(&format!("--exclude-dir={dir}")), "{cmd}");
    }
    let rg = grep_command(true, "x", ".", "*.rs");
    assert!(rg.starts_with("rg "), "{rg}");
    assert!(rg.contains("--glob"), "{rg}");
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p superstate-agent --test test_bound_output`. Expected: compile errors, because `bound_output`, `grep_command`, `MAX_LINE_CHARS` and `MAX_OUTPUT_CHARS` do not exist.

- [ ] **Step 3: `bound_output`.** In `crates/superstate-agent/src/tools.rs`, directly after the `MAX_MATCHES` constant, add:

```rust
/// Longest line kept whole. Minified code puts megabytes on one line, which a line budget alone
/// never catches: a single grep over node_modules overflowed a 1,048,576-token request.
pub const MAX_LINE_CHARS: usize = 500;

/// Most characters a single tool result may carry (about 15K tokens), after line clipping and
/// line distillation. The final guarantee that one result cannot swamp the context.
pub const MAX_OUTPUT_CHARS: usize = 60_000;

/// Bound a tool's output by size: clip over-long lines, distil to `OUTPUT_LINE_BUDGET` lines,
/// then cap the total, keeping head and tail. Ordinary output passes through unchanged.
pub fn bound_output(text: &str) -> String {
    let clipped: Vec<String> = text
        .lines()
        .map(|l| {
            let n = l.chars().count();
            if n > MAX_LINE_CHARS {
                let head: String = l.chars().take(MAX_LINE_CHARS).collect();
                format!("{head} …[{} more chars clipped by superstate]", n - MAX_LINE_CHARS)
            } else {
                l.to_string()
            }
        })
        .collect();
    let mut joined = clipped.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    let distilled = distil(&joined, OUTPUT_LINE_BUDGET);
    let total = distilled.chars().count();
    if total <= MAX_OUTPUT_CHARS {
        return distilled;
    }
    let head_n = MAX_OUTPUT_CHARS * 2 / 3;
    let tail_n = MAX_OUTPUT_CHARS - head_n;
    let head: String = distilled.chars().take(head_n).collect();
    let tail: String = distilled.chars().skip(total - tail_n).collect();
    format!(
        "{head}\n\n[... {} chars elided by superstate ...]\n\n{tail}",
        total - head_n - tail_n
    )
}
```

- [ ] **Step 4: Apply it to every tool result.** In `tools.rs`:
  1. Rename the existing `pub fn execute(` to `fn execute_unbounded(` (keep its doc comment where it is).
  2. Directly above `fn execute_unbounded(`, insert a new public entry point, and move the existing doc comment onto it (the doc comment above `execute_unbounded` becomes the doc comment of `execute`; give `execute_unbounded` the one-line comment `/// The tools themselves; `execute` bounds whatever they return.`):

```rust
pub fn execute(
    root: &Path,
    shell: &dyn Shell,
    fs: &dyn Fs,
    name: &str,
    args: &Value,
) -> ToolOutcome {
    let mut outcome = execute_unbounded(root, shell, fs, name, args);
    outcome.content = bound_output(&outcome.content);
    outcome
}
```

- [ ] **Step 5: The grep command.** In `tools.rs`, add this function directly above `pub fn execute(`:

```rust
/// The search command. ripgrep honours .gitignore, which keeps dependency and build trees out of
/// results; the grep fallback (used when a container has no rg) excludes them explicitly, or a
/// search over node_modules returns megabyte-long minified lines.
pub fn grep_command(use_rg: bool, pattern: &str, sub: &str, glob: &str) -> String {
    if use_rg {
        let globarg = if glob.is_empty() {
            String::new()
        } else {
            format!("--glob {}", shell_quote(glob))
        };
        format!(
            "rg --line-number --no-heading --color never {globarg} -e {} {} | head -n {}",
            shell_quote(pattern),
            shell_quote(sub),
            MAX_MATCHES + 1
        )
    } else {
        let globarg = if glob.is_empty() {
            String::new()
        } else {
            format!("--include={}", shell_quote(glob))
        };
        let excludes = ["node_modules", ".git", "target", ".venv", "dist", "build"]
            .iter()
            .map(|d| format!("--exclude-dir={d}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "grep -rnI {excludes} {globarg} -e {} {} | head -n {}",
            shell_quote(pattern),
            shell_quote(sub),
            MAX_MATCHES + 1
        )
    }
}
```

  Then, in the `"grep" =>` arm of `execute_unbounded`, replace everything from `let globarg_rg = if glob_str.is_empty() {` through the end of the `let cmd = if have(shell, root, "rg") { … } else { … };` statement (keep the comment lines about ripgrep, or drop them since `grep_command` documents it) with:

```rust
            let cmd = grep_command(have(shell, root, "rg"), &pattern, &sub, &glob_str);
```

  Keep `let glob_str = s("glob");` above it.

- [ ] **Step 6: Bound the gate's output too.** In `crates/superstate-agent/src/run.rs`, in `fn run_gate`, replace `let body = tools::distil(o.merged().trim_end(), tools::OUTPUT_LINE_BUDGET);` with `let body = tools::bound_output(o.merged().trim_end());`.

- [ ] **Step 7: Run the new tests.** Run: `cargo test -p superstate-agent --test test_bound_output --test test_tools`. Expected: all pass (5 new).

- [ ] **Step 8: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 9: Commit.**

```bash
git add crates/superstate-agent/
git commit --no-gpg-sign -m "fix(agent): bound every tool result by size; grep fallback skips dependency trees"
```
