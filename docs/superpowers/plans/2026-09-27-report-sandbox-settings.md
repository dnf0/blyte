# Sandbox Settings in the Run Report — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. Do not print the values of any real environment variable. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** Every `--report` records the security settings the run actually had: which sandbox (if
any), the network mode, the `--allow-read` grants, the `--pass-env` names, and the names (never
values) of the environment variables withheld from commands. Then any run can be audited afterwards.

**Why (2026-09-27):** #91–#97 added a write sandbox, read-deny, the env filter, Landlock and
`--network`; the report records only `sandboxed: bool`, which cannot tell a Seatbelt run from a
Landlock one, or show that a run was granted `--allow-read ~/.aws`.

**Tech Stack:** Rust, serde.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**. Existing report fields keep their names, types and values
  (`sandboxed` stays). New fields are additive; `SCHEMA_VERSION` stays 1.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Five new report fields

**What to build** (names are part of the contract; the tests below check them):

`crates/blyte-cli/src/report.rs`, `RunRecord` gains (each with a one-line doc comment), set in `base`:
- `sandbox: String`: `"container"` when `config.container` is set; otherwise `"off"` when
  `config.sandbox != "on"` or `!blyte_agent::sandbox::available()`; otherwise the kind of
  `blyte_agent::sandbox::default_profile_net(&config.root, &config.allow_read, config.network == "on")`:
  `Seatbelt` → `"seatbelt"`, `Landlock` → `"landlock"`, `LandlockOffline` → `"landlock-offline"`.
  Put this mapping in a `pub fn sandbox_kind(config: &TaskConfig) -> String` in `report.rs`.
- `network: String`: `config.network`.
- `allow_read: Vec<String>`: `config.allow_read` as display strings, in the given order.
- `pass_env: Vec<String>`: `config.pass_env`.
- `withheld_env: Vec<String>`: `blyte_agent::shell::withheld_env()` for a local run (empty when
  `config.container` is set, since container commands never inherit the host environment).

Document the five fields in the report schema spec
(`docs/superpowers/specs/2026-09-23-native-harness-eval-surface-design.md`), in the same style as
the existing field list there, under a short "Security settings (2026-09-27)" note.

- [ ] **Step 1: Write the failing test.** Create `crates/blyte-cli/tests/test_report_security.rs`:

```rust
//! The report records the run's security settings, so a run can be audited afterwards: `sandboxed`
//! alone could not tell Seatbelt from Landlock or show an `--allow-read` grant (2026-09-27).

use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;

use blyte_cli::cli::Cli;
use blyte_cli::report::{sandbox_kind, RunRecord};
use blyte_cli::task_runner::TaskConfig;

fn config(args: &[&str]) -> TaskConfig {
    let mut argv = vec!["blyte"];
    argv.extend_from_slice(args);
    TaskConfig::from(&Cli::try_parse_from(argv).unwrap())
}

#[test]
fn grants_and_modes_are_recorded() {
    let c = config(&[
        "--allow-read", "/a", "--allow-read", "/b",
        "--pass-env", "GH_TOKEN",
        "--network", "off",
    ]);
    let r = serde_json::to_value(RunRecord::from_error(&c, "x", Duration::from_secs(1))).unwrap();
    assert_eq!(r["allow_read"], serde_json::json!(["/a", "/b"]));
    assert_eq!(r["pass_env"], serde_json::json!(["GH_TOKEN"]));
    assert_eq!(r["network"], "off");
    assert!(r["withheld_env"].is_array(), "{r}");
    assert!(r["sandbox"].is_string(), "{r}");
}

#[test]
fn the_sandbox_kind_names_the_mechanism() {
    let mut c = config(&[]);
    c.root = PathBuf::from(".");
    let kind = sandbox_kind(&c);
    if !blyte_agent::sandbox::available() {
        assert_eq!(kind, "off");
    } else if cfg!(target_os = "macos") {
        assert_eq!(kind, "seatbelt");
    } else {
        assert_eq!(kind, "landlock");
    }
    let off = config(&["--sandbox", "off"]);
    assert_eq!(sandbox_kind(&off), "off");
    let ctr = config(&["--container", "abc"]);
    assert_eq!(sandbox_kind(&ctr), "container");
}

#[test]
fn a_container_run_withholds_nothing_because_it_inherits_nothing() {
    std::env::set_var("ZZ_REPORT_TEST_TOKEN", "v");
    let c = config(&["--container", "abc"]);
    let r = serde_json::to_value(RunRecord::from_error(&c, "x", Duration::from_secs(1))).unwrap();
    assert_eq!(r["withheld_env"], serde_json::json!([]));
    let local = serde_json::to_value(RunRecord::from_error(&config(&[]), "x", Duration::from_secs(1))).unwrap();
    assert!(
        local["withheld_env"].as_array().unwrap().iter().any(|n| n == "ZZ_REPORT_TEST_TOKEN"),
        "{local}"
    );
}
```

If `Cli` needs a prompt or other argument to parse, add the minimum to `config()`; change nothing else.

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte_cli --test test_report_security` (or the crate's actual package name): compile error.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Run the full checks (Global Constraints). All pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates docs
git commit --no-gpg-sign -m "feat(cli): the run report records the sandbox, network mode, read grants and withheld environment names"
```
