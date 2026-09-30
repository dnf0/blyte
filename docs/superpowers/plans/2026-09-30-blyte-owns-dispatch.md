# blyte owns the whole dispatch workflow — Plan

The operator's machine ran blyte through local wrapper scripts (`~/.claude/skills/agy-orchestration`):
an A/B against agy, gate discovery from a sentence in `.claude/agy-rules.md`, a ledger, verdicts
and a host-command scan. The A/B has answered its question, so those scripts retire. What they did
that is worth keeping moves into blyte, so every user gets it: a recorded verdict, a saved summary,
a warning about commands that reached outside the worktree, per-repository environment settings,
and a `blyte init` that works out a repository's gate and proves it before writing it.

## Global Constraints

- Rust workspace; checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.
- Each task creates its own new test file exactly as given. Do not edit existing test files.
- No new crate dependencies.
- Keep every existing flag, output line and JSON field working as now; only add.
- Conventional commit, `--no-gpg-sign`, the message the task gives.
- Update `docs/reference/cli.md` (the `--help` text of changed commands) and `docs/usage.md` where
  the task says.

Tasks 1 and 2 are independent (Task 1: `review`; Task 2: `dispatch` and `blyte.toml`). Task 3 needs
Task 2 merged.

### Task 1: `blyte review` records a verdict and warns about host commands

**Files:** `crates/blyte-cli/src/cli.rs` (`ReviewArgs`), `crates/blyte-cli/src/dispatch.rs`
(`run_review`, a new `pub fn host_commands`), new `crates/blyte-cli/tests/test_review_verdict.rs`,
`docs/usage.md` (a short "Recording a verdict" paragraph after the review section, or in the
dispatch section if there is no review section), `docs/reference/cli.md`.

**Verdict.** `ReviewArgs` gains `--verdict <accepted|fixed|redone>` (a clap `ValueEnum`, lowercase
values; anything else is a usage error, exit 2) and `--notes <TEXT>`. With `--verdict`, `blyte
review <id>` only records: it writes `<root>/.wt/<id>.verdict.json` as
`{"verdict": "<v>", "notes": "<notes or empty>", "ts": <unix seconds>}` (creating `.wt/` if
needed; overwriting an earlier verdict), prints `recorded verdict <v> for <id>`, and exits 0. It
does **not** need the worktree to exist (it is often removed after the merge). `--notes` without
`--verdict` is a usage error.

**Host commands.** `pub fn host_commands(log: &str) -> Vec<String>` scans a run log for shell
commands that may have changed things outside the worktree. It strips ANSI escapes, takes every
`● bash <command> -> ` line's `<command>`, and keeps a command when any of these holds:
- it contains `git push`, `sudo `, `cache clean`, `cache prune` or `brew `;
- it names a path outside the worktree (`~/`, `$HOME`, `/Users/`, `/home/`, `/private/`, `/var/`,
  `/etc/`, `/opt/`, `/usr/`) **and** changes something: the words `rm`, `rmdir`, `mv`, `cp`,
  `chmod`, `chown`, `truncate`, `dd`, `tee`, `ln`, or an output redirect `>`/`>>` whose target is
  not `/dev/null` (`2>&1` and `> /dev/null` do not count).
It returns them in log order, without duplicates, at most 20.

`run_review` (without `--verdict`) reads `<root>/.wt/<id>.log` when it exists and, when
`host_commands` finds any, prints a section after `== report`:

```
== host commands (may have changed things outside the worktree)
<command>
<command>
```

Nothing is printed when there are none.

- [ ] **Step 1: test.** Create `crates/blyte-cli/tests/test_review_verdict.rs` exactly:

```rust
//! `blyte review` records the reviewer's verdict beside the run, and warns about commands that
//! reached outside the worktree (E5, 2026-09-27: a run deleted uv cache entries) (2026-09-30).

use std::path::Path;
use std::process::Command;

use blyte_cli::dispatch::host_commands;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    std::fs::write(d.path().join("f.txt"), "a\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "i", "--no-gpg-sign"]);
    d
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

#[test]
fn host_commands_flags_changes_outside_the_worktree_only() {
    let log = "\
[Turn 1] \n● bash rm -rf build/ -> exit 0
● bash cat /Users/x/.zshrc > /dev/null -> exit 0
● bash cargo test 2>&1 | tail -5 -> exit 0
● bash rm -rf ~/.cache/uv/wheels-v5/pypi/ruff -> exit 0
● bash uv cache clean ruff -> exit 0
\x1b[31m● bash echo 127.0.0.1 x >> /etc/hosts -> exit 1\x1b[0m
● bash git push origin main -> exit 1
● bash uv cache clean ruff -> exit 0
";
    assert_eq!(
        host_commands(log),
        vec![
            "rm -rf ~/.cache/uv/wheels-v5/pypi/ruff",
            "uv cache clean ruff",
            "echo 127.0.0.1 x >> /etc/hosts",
            "git push origin main",
        ]
    );
    assert!(host_commands("● bash ls -> exit 0\n").is_empty());
}

#[test]
fn a_verdict_is_recorded_without_the_worktree() {
    let r = repo();
    let out = blyte(
        r.path(),
        &["review", "T1", "--verdict", "fixed", "--notes", "tightened a test"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("recorded verdict fixed for T1"));
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(r.path().join(".wt/T1.verdict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(v["verdict"], "fixed");
    assert_eq!(v["notes"], "tightened a test");
    assert!(v["ts"].as_u64().unwrap() > 1_700_000_000);

    assert_eq!(blyte(r.path(), &["review", "T1", "--verdict", "great"]).status.code(), Some(2));
    assert_eq!(blyte(r.path(), &["review", "T1", "--notes", "x"]).status.code(), Some(2));
}

#[test]
fn review_lists_host_commands_from_the_log() {
    let r = repo();
    git(r.path(), &["worktree", "add", "-q", "-b", "wip/T2", ".wt/T2"]);
    std::fs::write(
        r.path().join(".wt/T2.log"),
        "[Turn 1] \n● bash rm -rf ~/.cache/uv/x -> exit 0\n● bash ls -> exit 0\n",
    )
    .unwrap();
    let out = blyte(r.path(), &["review", "T2"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("== host commands"), "{text}");
    assert!(text.contains("rm -rf ~/.cache/uv/x"), "{text}");

    std::fs::write(r.path().join(".wt/T2.log"), "[Turn 1] \n● bash ls -> exit 0\n").unwrap();
    let text = String::from_utf8_lossy(&blyte(r.path(), &["review", "T2"]).stdout).to_string();
    assert!(!text.contains("== host commands"), "{text}");
}
```

- [ ] **Step 2:** implement until the check passes.
- [ ] **Step 3: commit** `feat(cli): blyte review records a verdict and flags host commands`.

### Task 2: `blyte dispatch` saves its summary; `blyte.toml` sets env and fold budget

**Files:** `crates/blyte-cli/src/config.rs` (`DispatchConfig`, `load`, `render`),
`crates/blyte-cli/src/dispatch.rs` (`run_dispatch`, a new `pub fn quick_steps`), new
`crates/blyte-cli/tests/test_dispatch_env.rs`, `docs/usage.md` (the `blyte.toml` keys),
`docs/reference/cli.md` if help text changes.

1. **Saved summary.** `run_dispatch` writes the JSON summary it prints to
   `<root>/.wt/<id>.json` too (pretty JSON plus a newline), on every path that prints one.
2. **`fold_budget` in `blyte.toml`.** `DispatchConfig` gains `fold_budget: Option<usize>`; the
   `--fold-budget` flag wins over it, as with the other settings.
3. **`[dispatch.env]`.** `DispatchConfig` gains `env: BTreeMap<String, String>` (serde default
   empty; TOML table `[dispatch.env]`). `load` rejects any name for which
   `blyte_agent::shell::credential_like(name, value)` is true, with an error naming the file and
   the variable and saying credentials are never set from `blyte.toml`; the error also rejects
   names starting with `BLYTE_`. `run_dispatch` sets each variable on the child `blyte` process
   (`cmd.env`), so the model's commands and the gate both see it, and records the names (not the
   values) in the start file as `"env": [names]`.
4. **Quick steps in the prompt.** `pub fn quick_steps(gate: &str) -> Option<String>`: for a gate of
   two or more non-empty lines, `Some(format!("The check's quick steps, to run yourself before you
   stop: {}. The last step, the full run, happens at the gate.", steps))` where `steps` is every
   line but the last, each in backticks, joined with `, `; `None` for a one-line gate.
   `run_dispatch` appends it, after a blank line, to `wrap_task(...)` when it is `Some`.
   `wrap_task` itself is unchanged.
5. **`render`** gains commented examples after `# network = "on"`:
   `# fold_budget = 60000` and
   `# [dispatch.env]` / `# UV_FROZEN = "1"   # settings for every command and the gate; never credentials`.

- [ ] **Step 1: test.** Create `crates/blyte-cli/tests/test_dispatch_env.rs` exactly:

```rust
//! blyte.toml carries a repository's environment settings and fold budget, the prompt names the
//! gate's quick steps, and dispatch keeps its summary beside the run (2026-09-30).

use std::path::Path;
use std::process::Command;

use blyte_cli::config::load;
use blyte_cli::dispatch::quick_steps;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

#[test]
fn env_and_fold_budget_load_and_credentials_are_refused() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("blyte.toml"),
        "[dispatch]\ngate = \"true\"\nfold_budget = 40000\n\n[dispatch.env]\nUV_FROZEN = \"1\"\nUV_OFFLINE = \"1\"\n",
    )
    .unwrap();
    let cfg = load(d.path()).unwrap().unwrap().dispatch;
    assert_eq!(cfg.fold_budget, Some(40000));
    assert_eq!(cfg.env.get("UV_FROZEN").map(String::as_str), Some("1"));
    assert_eq!(cfg.env.len(), 2);

    for bad in ["GITHUB_TOKEN = \"x\"", "UV_INDEX_PIGEONHOLE_PASSWORD = \"x\"", "BLYTE_MODEL = \"x\""] {
        std::fs::write(
            d.path().join("blyte.toml"),
            format!("[dispatch]\ngate = \"true\"\n[dispatch.env]\n{bad}\n"),
        )
        .unwrap();
        let err = load(d.path()).unwrap_err();
        let name = bad.split(' ').next().unwrap();
        assert!(err.contains(name) && err.contains("blyte.toml"), "{err}");
    }
}

#[test]
fn quick_steps_are_every_gate_line_but_the_last() {
    assert_eq!(
        quick_steps("cargo fmt --all --check\ncargo clippy -- -D warnings\n\ncargo test\n").as_deref(),
        Some(
            "The check's quick steps, to run yourself before you stop: `cargo fmt --all --check`, \
             `cargo clippy -- -D warnings`. The last step, the full run, happens at the gate."
        )
    );
    assert_eq!(quick_steps("pytest -q"), None);
}

#[test]
fn dispatch_saves_its_summary_and_records_env_names() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    std::fs::write(r.join("task.md"), "x").unwrap();
    std::fs::write(
        r.join("blyte.toml"),
        "[dispatch]\ngate = \"true\"\n[dispatch.env]\nUV_FROZEN = \"1\"\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["dispatch", "E1", "task.md", "--sandbox", "off"])
        .current_dir(r)
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap();
    let printed: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON summary");
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".wt/E1.json")).unwrap()).unwrap();
    assert_eq!(printed, saved);
    let start: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".wt/E1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start["env"], serde_json::json!(["UV_FROZEN"]));
}
```

- [ ] **Step 2:** implement until the check passes.
- [ ] **Step 3: commit** `feat(cli): blyte.toml sets env and fold budget; dispatch saves its summary`.

### Task 3: `blyte init` finds the gate the repository really uses, and proves it

Needs Task 2 (`DispatchConfig::env`).

**Files:** new `crates/blyte-cli/src/init.rs` (`pub mod init;` in `lib.rs`), `crates/blyte-cli/src/main.rs`
(the `Init` arm calls `blyte_cli::init::run`), `crates/blyte-cli/src/cli.rs` (`Init` flags),
new `crates/blyte-cli/tests/test_init_verified.rs`, `docs/usage.md` (rewrite the `blyte init`
paragraph: sources, environment, verification), `docs/reference/cli.md`. Keep
`config::detect` and `config::render` and their behaviour; `init` uses `detect` as its fallback.

**Flags.** `blyte init [--force] [--print] [--no-verify] [--sandbox <auto|off>] [--step-timeout <SECONDS, default 600>]`.
`--print` prints the file instead of writing it (verification still runs unless `--no-verify`).

**1. Where the gate comes from, in order:**
- `pub fn detect_ci(root: &Path) -> Vec<String>`: every `*.yml`/`*.yaml` in
  `.github/workflows/`, sorted by file name, read line by line (no YAML library). A step's command
  is either `run: <cmd>` on one line, or the indented lines after `run: |` (or `run: >`) up to the
  first line indented no deeper than the `run:` key; a line ending in ` \` joins the next. A
  trailing `# comment` after `run: |` is ignored. From each command line keep only **check
  commands**: after an optional `uv run `, `poetry run `, `pdm run `, `npx ` or `pipenv run `
  prefix, it starts with `pytest`, `ruff`, `pyright`, `mypy`, `black`, `isort`, `pre-commit run`,
  `cargo fmt`, `cargo clippy`, `cargo test`, `go test`, `go vet`, `golangci-lint`,
  `npm test`, `npm run lint`, `npm run typecheck`, `pnpm test`, `pnpm run lint`, `yarn test`,
  `yarn lint`, `make check`, `make lint` or `make test`. Remove reporting arguments: any argument
  starting `--cov`, `--junitxml`, `--junit-xml` or `--cov-report`, and anything from a `| tee`
  onward. Collapse runs of spaces. Drop exact duplicates, keeping the first. Order: as found.
- If `detect_ci` finds nothing, `config::detect(root)` as now. Each step remembers its source
  (the workflow file name, or `detected`).

**2. Environment.** `pub fn detect_env(root: &Path) -> BTreeMap<String, String>`: when `uv.lock`
exists, `UV_FROZEN = "1"`; and when `pyproject.toml` also has a `[[tool.uv.index]]` whose `url` is
not on `pypi.org`, `UV_OFFLINE = "1"` as well (a private index needs a token the run does not
have; uv's cache has the locked packages).

**3. Verification** (unless `--no-verify`). Add a detached worktree of `HEAD` at
`.wt/init-<pid>` (and `.wt/` to `info/exclude`, as dispatch does). Run each step there, one after
another, through `blyte_agent::shell::LocalShell` — sandboxed with
`sandbox::default_profile_net(&worktree, &[], true)` unless `--sandbox off` — with the step's
environment set (`detect_env`) and the step timeout. Print `init: <step> ... passed (12s)` (or the
outcome) to stderr as it goes. Then remove the worktree (`git worktree remove --force`), even on
error. Classify each step with `pub fn classify(output: &str, exit_code: i32) -> Verdict` where
`#[derive(Debug, Clone, PartialEq)] pub enum Verdict { Passed, Environment(String), Failed }`:
- exit 0 → `Passed`;
- the output contains (case-insensitive) `401`, `403`, `unauthorized`, `forbidden`,
  `not found in the package registry`, `could not resolve host`, `network is unreachable`,
  `connection refused`, `operation not permitted`, `command not found`, `no such file or directory`
  (the last two only on the first line of stderr that mentions the command) → `Environment(<that
  line, trimmed, at most 120 characters>)`;
- otherwise → `Failed`.
Keep `Passed` and `Failed` steps in the gate; leave `Environment` steps out.

**4. The file.** `pub fn render_verified(steps: &[InitStep], env: &BTreeMap<String, String>) -> String`
(`pub struct InitStep { pub command: String, pub source: String, pub verdict: Option<Verdict> }`;
`None` when not verified) writes, in this order: the existing header comment; one comment line per
step, `# <command>: from <source>, <passed in 12s | failed on HEAD: fix it before dispatching | left
out: <reason> | not verified>`; the `[dispatch]` table with `gate` holding the kept steps (as
`render` does) and the other keys as `render` writes them; then, when `env` is not empty, a
`[dispatch.env]` table with a comment `# for every command and the gate; never credentials` and the
variables. The result must load with `config::load`.

Exit status: 0 when every kept step passed; 3 when a kept step failed (the file is still written,
and a warning names the step); 1 when no step was found or every step was left out (nothing is
written; stderr says what was tried and why each was left out). The existing refusal to overwrite
without `--force` stays.

- [ ] **Step 1: test.** Create `crates/blyte-cli/tests/test_init_verified.rs` exactly:

```rust
//! `blyte init` takes the gate from CI, sets what the environment needs, and proves each step in a
//! fresh worktree before writing it: it had picked `make check` (docs only) and a `make test`
//! that needed credentials (2026-09-30).

use std::path::Path;
use std::process::Command;

use blyte_cli::config::load;
use blyte_cli::init::{classify, detect_ci, detect_env, Verdict};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

const WORKFLOW: &str = "\
name: PR
jobs:
  test:
    steps:
      - uses: astral-sh/setup-uv@v5
      - name: Install
        run: uv sync --group dev
      - name: Lint
        run: uv run ruff check .
      - name: Tests
        run: | # pipefail so pytest's failure counts
          set -o pipefail
          uv run pytest -m \"not e2e\" --cov=pkg \\
            --cov-report=xml --junitxml=out.xml | tee out.txt
      - name: Lint again
        run: uv run ruff check .
";

#[test]
fn ci_steps_are_the_gate_without_setup_or_reporting() {
    let d = tempfile::tempdir().unwrap();
    write(&d.path().join(".github/workflows/pull_request.yaml"), WORKFLOW);
    assert_eq!(
        detect_ci(d.path()),
        vec!["uv run ruff check .", "uv run pytest -m \"not e2e\""]
    );
}

#[test]
fn uv_projects_run_frozen_and_private_indexes_offline() {
    let d = tempfile::tempdir().unwrap();
    write(&d.path().join("uv.lock"), "");
    write(&d.path().join("pyproject.toml"), "[project]\nname = \"x\"\n");
    let env = detect_env(d.path());
    assert_eq!(env.get("UV_FROZEN").map(String::as_str), Some("1"));
    assert!(!env.contains_key("UV_OFFLINE"));
    write(
        &d.path().join("pyproject.toml"),
        "[project]\nname = \"x\"\n[[tool.uv.index]]\nname = \"p\"\nurl = \"https://example.d.codeartifact.eu-west-2.amazonaws.com/pypi/p/simple/\"\n",
    );
    assert_eq!(detect_env(d.path()).get("UV_OFFLINE").map(String::as_str), Some("1"));
    assert!(detect_env(tempfile::tempdir().unwrap().path()).is_empty());
}

#[test]
fn failures_are_told_apart_from_environment_problems() {
    assert_eq!(classify("ok", 0), Verdict::Passed);
    assert!(matches!(
        classify("error: Failed to fetch: 401 Unauthorized\n", 1),
        Verdict::Environment(_)
    ));
    assert!(matches!(
        classify("× Because cursor-agents was not found in the package registry\n", 1),
        Verdict::Environment(_)
    ));
    assert_eq!(classify("FAILED tests/test_x.py::test_y - AssertionError\n1 failed\n", 1), Verdict::Failed);
}

#[test]
fn init_proves_each_step_and_leaves_out_the_ones_the_environment_breaks() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    write(
        &r.join(".github/workflows/ci.yml"),
        "jobs:\n  t:\n    steps:\n      - run: make check\n      - run: make lint\n      - run: make test\n",
    );
    write(
        &r.join("Makefile"),
        "check:\n\t@true\nlint:\n\t@echo 'error: 401 Unauthorized' >&2; exit 1\ntest:\n\t@echo '1 failed'; exit 1\n",
    );
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["init", "--sandbox", "off"])
        .current_dir(r)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{}", String::from_utf8_lossy(&out.stderr));
    let text = std::fs::read_to_string(r.join("blyte.toml")).unwrap();
    assert!(text.contains("# make check: from ci.yml, passed"), "{text}");
    assert!(text.contains("# make lint: from ci.yml, left out: "), "{text}");
    assert!(text.contains("# make test: from ci.yml, failed on HEAD"), "{text}");
    let gate = load(r).unwrap().unwrap().dispatch.gate.unwrap();
    assert_eq!(gate.lines().collect::<Vec<_>>(), vec!["make check", "make test"]);
    // The verification worktree is gone.
    let wts = Command::new("git").args(["worktree", "list"]).current_dir(r).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&wts.stdout).lines().count(), 1);
}
```

- [ ] **Step 2:** implement until the check passes.
- [ ] **Step 3: commit** `feat(cli): blyte init takes the gate from CI and proves it in a fresh worktree`.
