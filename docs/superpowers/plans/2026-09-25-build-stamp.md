# Build Stamp: Reports Say Which blyte Ran — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** The `blyte` binary knows the git commit it was built from. `blyte --version` prints it, and every `--report` JSON carries it as `harness_build`.

**Why:** the dispatch ledger records a blyte version for every run so improvements can be tracked by version. Today it records the source checkout's current HEAD, which drifts from the installed binary (a plan commit, or an un-reinstalled checkout). Only the binary itself can say what it is.

**Tech Stack:** Rust 2021, a Cargo build script.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass unmodified.
- No new dependencies.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Stamp the build

**Files:** new `crates/blyte-cli/build.rs`; `crates/blyte-cli/src/cli.rs`; `crates/blyte-cli/src/report.rs`; new test `crates/blyte-cli/tests/test_build_stamp.rs`.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-cli/tests/test_build_stamp.rs`:

```rust
//! The binary knows the commit it was built from: the ledger used to record the source
//! checkout's HEAD, which drifts from the installed binary.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use blyte_cli::report::RunRecord;
use blyte_cli::task_runner::TaskConfig;

fn head() -> String {
    let out = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn config() -> TaskConfig {
    TaskConfig {
        prompt: "p".into(),
        gate: None,
        context_strategy: "folded".into(),
        model: None,
        container: None,
        max_turns: 1,
        max_gates: 1,
        rollback: false,
        root: PathBuf::from("."),
        container_workdir: "/workspace".into(),
        max_seconds: None,
        report: None,
        fold_budget: None,
    }
}

#[test]
fn a_report_carries_the_commit_the_binary_was_built_from() {
    let r = RunRecord::from_error(&config(), "e", Duration::from_secs(0));
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["harness_build"], head());
}

#[test]
fn version_prints_the_build_commit() {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .arg("--version")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains(&format!("({})", head())), "{stdout}");
}
```

- [ ] **Step 2: Run to confirm it fails.** Run: `cargo test -p blyte --test test_build_stamp`. Expected: both fail (no `harness_build` field; `--version` prints `blyte 0.1.0`).

- [ ] **Step 3: Implement.**
  1. Create `crates/blyte-cli/build.rs`:

```rust
//! Stamps the build with the commit it was built from (`BLYTE_BUILD`), so `--version` and every
//! run report say which blyte ran. Outside a git checkout (a crates.io build) it is `unknown`.

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !text.is_empty()).then_some(text)
}

fn main() {
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=BLYTE_BUILD={commit}");
    // Rebuild the stamp when HEAD moves: a commit, checkout or pull rewrites these.
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={dir}/HEAD");
        println!("cargo:rerun-if-changed={dir}/logs/HEAD");
    }
}
```

  2. In `crates/blyte-cli/src/cli.rs`, change `version = "0.1.0",` in the `#[command(...)]` attribute to `version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("BLYTE_BUILD"), ")"),`.
  3. In `crates/blyte-cli/src/report.rs`, add a field to `RunRecord`, directly after `schema_version`:

```rust
    /// The commit this blyte was built from (`build.rs`), or `unknown`.
    pub harness_build: String,
```

  and in `RunRecord::base`, directly after `schema_version: SCHEMA_VERSION,`, add `harness_build: env!("BLYTE_BUILD").into(),`. Leave `SCHEMA_VERSION` at 1 (the change is additive).

- [ ] **Step 4: Run the tests.** Run: `cargo test -p blyte --test test_build_stamp --test test_run_report --test test_cli_smoke`. Expected: all pass (2 new).

- [ ] **Step 5: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "feat(cli): the binary knows the commit it was built from (--version, report harness_build)"
```
