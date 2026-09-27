# `blyte.toml` and `blyte init` — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** a repository can say, once, how its work is checked. `blyte init` detects the toolchain
and writes `blyte.toml` with the gate, model and budgets; `blyte dispatch` uses it when a flag is
not given. The orchestrator no longer invents a gate per dispatch.

**Why (2026-09-27):** in every skill eval Claude composed the gate itself (`ruff check .`,
`python -m pytest -q`, …). It guessed well on toy repositories; on a real one a wrong gate
either passes broken work or fails good work. The check command belongs to the repository.

**Tech Stack:** Rust, `toml` crate (new dependency of `blyte-cli`), serde.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**, with one named exception: in
  `crates/blyte-cli/tests/test_dispatch.rs`, test `subcommands_parse_and_a_plain_prompt_is_unchanged`:
  `assert_eq!(a.gate, "cargo test")` becomes `assert_eq!(a.gate.as_deref(), Some("cargo test"))`;
  `assert_eq!(a.max_turns, 300)` becomes `assert_eq!(a.max_turns, None)`;
  `assert_eq!(a.max_seconds, 3600)` becomes `assert_eq!(a.max_seconds, None)`; and the
  `--gate is required` assertion becomes
  `assert!(Cli::try_parse_from(["blyte", "dispatch", "T1", "task.md"]).is_ok(), "--gate is optional with blyte.toml")`.
  Nothing else in that file changes.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: `blyte.toml`, `blyte init`, and dispatch defaults

**Files:** `crates/blyte-cli/Cargo.toml` (add `toml = "0.8"`), `crates/blyte-cli/src/config.rs` (new),
`crates/blyte-cli/src/lib.rs`, `crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/main.rs`,
`crates/blyte-cli/src/dispatch.rs`, `crates/blyte-cli/assets/blyte-implementer/SKILL.md`,
`crates/blyte-cli/tests/test_blyte_toml.rs` (new), and the named change to `test_dispatch.rs`.

**The file format** (`<repo root>/blyte.toml`; every key optional):

```toml
# blyte.toml: how this repository's work is checked, for `blyte dispatch`.
[dispatch]
gate = """
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace --all-targets
"""
# model = "gemini-3.8-flash-high"
max_seconds = 3600
max_turns = 300
# max_cost = 2.0
# network = "on"
```

**What to build:**

- `config.rs` (`pub mod config;`):
  - `#[derive(Debug, Default, Clone, PartialEq, serde::Deserialize)] pub struct DispatchConfig { pub gate: Option<String>, pub model: Option<String>, pub max_seconds: Option<u64>, pub max_turns: Option<u32>, pub max_cost: Option<f64>, pub network: Option<String> }`
    and `pub struct RepoConfig { #[serde(default)] pub dispatch: DispatchConfig }` (same derives).
    Unknown keys are an error (`#[serde(deny_unknown_fields)]` on both).
  - `pub fn load(root: &Path) -> Result<Option<RepoConfig>, String>`: `Ok(None)` when there is no
    `blyte.toml`; a parse error names the file and the problem.
  - `pub fn detect(root: &Path) -> Vec<String>`: the gate steps for the toolchains present, in this
    order, each only when detected:
    - **Rust** (`Cargo.toml`): `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
      `cargo test --workspace --all-targets`.
    - **Python** (`pyproject.toml`): the runner is `uv run ` when `uv.lock` exists, else
      `python3 -m `. Then `<runner>ruff check .` when `pyproject.toml` contains `[tool.ruff` or a
      `ruff.toml` exists; `<runner>pytest -q` when `pyproject.toml` contains `[tool.pytest` or a
      `tests/` directory exists.
    - **Node** (`package.json`): the runner is `pnpm` with `pnpm-lock.yaml`, `yarn` with
      `yarn.lock`, else `npm`. For each of the scripts `lint`, `typecheck`, `test` that
      `package.json`'s `"scripts"` object defines, in that order: `npm run lint` /
      `npm run typecheck` / `npm test` (`pnpm run lint`, `yarn lint`, and so on for the others;
      `test` is `<runner> test`).
    - **Go** (`go.mod`): `test -z "$(gofmt -l .)"`, `go vet ./...`, `go test ./...`.
  - `pub fn render(gate: &[String]) -> String`: the `blyte.toml` text above with the detected steps
    in `gate` (one per line inside the `"""` string) and the same other lines; valid TOML that
    `load` reads back with `dispatch.gate` equal to the steps joined by `\n` plus a final `\n`.
- `cli.rs`: new subcommand `Init { #[arg(long)] force: bool, #[arg(long)] print: bool }`
  ("Detect this repository's checks and write blyte.toml"). `DispatchArgs.gate` becomes
  `Option<String>`; `max_seconds` and `max_turns` become `Option<u64>` / `Option<u32>` with no clap
  default (doc comments say the defaults, 3600 and 300).
- `main.rs`: `blyte init` finds the repository root (`git rev-parse --show-toplevel`; not a
  repository: exit 2), runs `detect`; nothing detected: exit 1 with
  `no toolchain detected: write blyte.toml by hand (see blyte init --print for the format)`
  (with `--print`, print the template with an empty gate and exit 0). Otherwise `--print` writes
  `render(..)` to stdout; else write `<root>/blyte.toml`, refusing (exit 1, message naming
  `--force`) when it exists and `--force` is not given; print `wrote <path>` and the detected steps.
- `dispatch.rs`, `run_dispatch`: load the repository's `blyte.toml` (a parse error: exit 2 with the
  message). Resolve each setting as flag, else `blyte.toml`, else built-in default: gate (no
  default: exit 2 with `no gate: pass --gate, or run blyte init to write blyte.toml`), model
  (none: blyte's own default), `max_seconds` 3600, `max_turns` 300, `max_cost` (none), `network`
  (`on`). The JSON summary gains `"gate_source": "flag" | "blyte.toml"`.
- `SKILL.md`, in section 3 (Dispatch), replace the line
  `- Each line of the gate is a step; all must pass. Put the slow full test run last.` with exactly:
  ```markdown
  - **The gate:** if the repository has a `blyte.toml`, leave `--gate` out: blyte uses the
    repository's own checks. If it has none, run `blyte init` once, read the gate it detected,
    and fix it if it is wrong, or pass `--gate`. Each line is a step; all must pass; the slow full
    test run goes last.
  ```

- [ ] **Step 1: Write the failing tests.** Create `crates/blyte-cli/tests/test_blyte_toml.rs`:

```rust
//! A repository says once how its work is checked (blyte.toml), and `blyte init` detects it:
//! in every skill eval the orchestrator invented the gate itself (2026-09-27).

use std::path::Path;
use std::process::Command;

use blyte_cli::config::{detect, load, render};

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    assert!(Command::new("git").args(["init", "-q"]).current_dir(d.path()).status().unwrap().success());
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte")).args(args).current_dir(dir)
        .env_remove("GEMINI_API_KEY").env_remove("GOOGLE_API_KEY").env_remove("BLYTE_MODEL")
        .output().unwrap()
}

#[test]
fn rust_is_detected_with_fmt_clippy_and_tests() {
    let r = repo();
    std::fs::write(r.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    assert_eq!(detect(r.path()), vec![
        "cargo fmt --all --check",
        "cargo clippy --all-targets -- -D warnings",
        "cargo test --workspace --all-targets",
    ]);
}

#[test]
fn python_uses_uv_when_locked_and_only_the_tools_it_has() {
    let r = repo();
    std::fs::write(r.path().join("pyproject.toml"), "[project]\nname = \"x\"\n[tool.ruff]\nline-length = 100\n").unwrap();
    std::fs::write(r.path().join("uv.lock"), "").unwrap();
    std::fs::create_dir(r.path().join("tests")).unwrap();
    assert_eq!(detect(r.path()), vec!["uv run ruff check .", "uv run pytest -q"]);
    std::fs::remove_file(r.path().join("uv.lock")).unwrap();
    assert_eq!(detect(r.path()), vec!["python3 -m ruff check .", "python3 -m pytest -q"]);
}

#[test]
fn node_runs_the_scripts_it_defines_with_its_package_manager() {
    let r = repo();
    std::fs::write(r.path().join("package.json"), r#"{"scripts":{"test":"vitest","lint":"eslint ."}}"#).unwrap();
    std::fs::write(r.path().join("pnpm-lock.yaml"), "").unwrap();
    assert_eq!(detect(r.path()), vec!["pnpm run lint", "pnpm test"]);
}

#[test]
fn render_round_trips_through_load() {
    let r = repo();
    let steps = vec!["cargo fmt --all --check".to_string(), "cargo test".to_string()];
    std::fs::write(r.path().join("blyte.toml"), render(&steps)).unwrap();
    let cfg = load(r.path()).unwrap().unwrap();
    assert_eq!(cfg.dispatch.gate.as_deref(), Some("cargo fmt --all --check\ncargo test\n"));
    assert_eq!(cfg.dispatch.max_turns, Some(300));
}

#[test]
fn a_bad_key_is_an_error_that_names_the_file() {
    let r = repo();
    std::fs::write(r.path().join("blyte.toml"), "[dispatch]\ngaet = \"x\"\n").unwrap();
    let e = load(r.path()).unwrap_err();
    assert!(e.contains("blyte.toml"), "{e}");
}

#[test]
fn init_writes_once_and_refuses_to_overwrite() {
    let r = repo();
    std::fs::write(r.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    let out = blyte(r.path(), &["init"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(std::fs::read_to_string(r.path().join("blyte.toml")).unwrap().contains("cargo clippy"));
    assert_eq!(blyte(r.path(), &["init"]).status.code(), Some(1));
    assert_eq!(blyte(r.path(), &["init", "--force"]).status.code(), Some(0));
    let empty = repo();
    assert_eq!(blyte(empty.path(), &["init"]).status.code(), Some(1));
}

#[test]
fn dispatch_takes_its_gate_from_blyte_toml_and_says_so() {
    let r = repo();
    std::fs::write(r.path().join("f.txt"), "a\n").unwrap();
    assert!(Command::new("git").args(["add", "."]).current_dir(r.path()).status().unwrap().success());
    assert!(Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "i", "--no-gpg-sign"]).current_dir(r.path()).status().unwrap().success());
    std::fs::write(r.path().join("task.md"), "x").unwrap();
    // No gate anywhere: refused before any run.
    assert_eq!(blyte(r.path(), &["dispatch", "T1", "task.md", "--sandbox", "off"]).status.code(), Some(2));
    std::fs::write(r.path().join("blyte.toml"), "[dispatch]\ngate = \"true\"\n").unwrap();
    let out = blyte(r.path(), &["dispatch", "T2", "task.md", "--sandbox", "off"]);
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    assert_eq!(v["gate_source"], "blyte.toml");
}
```

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte --test test_blyte_toml`: compile error.

- [ ] **Step 3: Implement** the additions above, and make the named change to `test_dispatch.rs`.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(cli): blyte.toml records a repository's gate and budgets; blyte init detects them"
```
