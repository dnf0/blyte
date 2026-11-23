# A faster local check

Date: 2026-11-23

## Why

The local check takes about 9 minutes, and about 8.5 of them are `cargo test --workspace
--all-targets` (measured 2026-11-23 on a 12-core Mac with a warm build: format 3 s, clippy 7 to
40 s, tests 512 to 533 s). Compiling is not the cost: the test files run one after another and sum
to 421 s (blyte-agent 224 s, blyte-cli 131 s, blyte-provider 12 s). CI runs the whole suite on
every pull request anyway.

Two changes, independent of each other:

- **T1**: the local check tests only the crates a change affects. The whole suite stays in CI.
- **T2**: the slowest test files are made fast.

After T1 merges, this repository's local `blyte.toml` gate changes its last step from
`cargo test --workspace --all-targets` to `python3 scripts/test-changed.py` (done by hand; the
file is not tracked).

### Task T1: a script that tests only the crates a change affects

**Depends on:** none.

**Siblings:** T2 changes slow tests in `crates/blyte-agent/tests/` and `crates/blyte-cli/tests/`
separately. Its work is not in this worktree; do not touch those files.

**Goal.** `python3 scripts/test-changed.py` runs `cargo test` for the workspace packages a change
affects, and no others, so the local check does not run 421 s of tests for a change to one crate.

**Files.**

- Create `scripts/test-changed.py` (Python 3, standard library only).
- Create `crates/blyte-cli/tests/test_test_changed_script.rs`.
- Change `CLAUDE.md`: under "Repo basics", after the "Checks (same as CI)" line, add one line:
  `- Faster local test step: python3 scripts/test-changed.py tests only the crates a change affects; CI runs the whole suite.`

**What to build.**

Command line:

- `--base <ref>`: the ref the change is measured from. Default `origin/main` (the documented
  value, this repository's default branch).
- `--files-from <path>`: read the changed files from this file (one repository-relative path per
  line, blank lines ignored) and do not call git. For tests.
- `--print`: print the decision and exit 0 without running cargo.

Changed files, when `--files-from` is not given: the union of
`git diff --name-only <merge-base>` (working tree against `git merge-base HEAD <base>`, so
committed, staged and unstaged changes all count) and
`git ls-files --others --exclude-standard` (untracked files).

Packages: from `cargo metadata --format-version 1 --no-deps`, run in the repository root (found
with `git rev-parse --show-toplevel`): each workspace package's name, its directory (the directory
of its `manifest_path`) and which other workspace packages it depends on (a dependency with a
`path`, of any kind, including dev-dependencies). Do not hard-code package names, directories or
dependencies in the script.

Decision, for each changed file, first rule that matches:

1. The file is inside a package's directory: that package is affected (whatever the file type; a
   Markdown file inside a crate can be compiled in with `include_str!`).
2. The file is under `docs/`, or is a `*.md` file in the repository root: affects nothing.
3. Anything else (`Cargo.toml`, `Cargo.lock`, `.github/`, `scripts/`, any other path): every
   package is affected.

Then add every workspace package that depends, directly or through others, on an affected package.

Output, always the first line printed to stdout, package names sorted and separated by one space:

- `packages: (none)` when nothing is affected. Then, without `--print`, print
  `no package affected; no tests run` and exit 0.
- `packages: (all)` when every workspace package is affected. Without `--print`, run
  `cargo test --workspace --all-targets`.
- `packages: blyte blyte-agent` otherwise. Without `--print`, run
  `cargo test -p blyte -p blyte-agent --all-targets`.

Without `--print` the script's exit code is cargo's exit code, and cargo's output goes straight to
the terminal (inherit stdout and stderr; flush the `packages:` line first).

**No fallbacks.** If `git` or `cargo metadata` fails or is missing, if the merge base cannot be
found, if `--files-from` names a file that cannot be read, or if `cargo metadata` output lacks a
field the script needs: print an error to stderr that names what failed (the command and its
stderr, or the path, or the field) and exit 2. Never fall back to testing everything, to testing
nothing, or to a different base. An unknown command-line argument is an error (exit 2).

**Tests** (`crates/blyte-cli/tests/test_test_changed_script.rs`). Each test writes a file list to a
temporary file and runs
`python3 <repo>/scripts/test-changed.py --print --files-from <list>` with the repository root as
the working directory (`env!("CARGO_MANIFEST_DIR")/../..`), then checks the exit code and the first
line of stdout exactly:

- `crates/blyte-cli/src/main.rs` gives `packages: blyte`.
- `crates/blyte-cli/assets/orchestrator/process.md` gives `packages: blyte`.
- `crates/blyte-agent/src/lib.rs` gives `packages: blyte blyte-agent`.
- `crates/blyte-provider/src/lib.rs` gives `packages: (all)`.
- `docs/usage.md` and `README.md` together give `packages: (none)`.
- An empty list gives `packages: (none)`.
- `Cargo.lock` gives `packages: (all)`.
- `scripts/anything.sh` gives `packages: (all)`.
- `docs/usage.md` and `crates/blyte-cli/src/main.rs` together give `packages: blyte`.
- `--files-from` naming a file that does not exist: exit code 2, stderr contains that path.
- An unknown argument `--nope`: exit code 2.

Run them with `cargo test -p blyte --test test_test_changed_script`. Also run the script once for
real, `python3 scripts/test-changed.py --print`, and give its output in your report.

**Constraints.** Existing tests pass unmodified. No change to CI workflows. No new dependencies.

**Commit message (exact):**

```
feat(check): add scripts/test-changed.py to test only the crates a change affects
```

### Task T2: make the slowest test files fast

**Depends on:** none.

**Siblings:** T1 adds `scripts/test-changed.py` and
`crates/blyte-cli/tests/test_test_changed_script.rs` separately. They are not in this worktree; do
not add them.

**Goal.** These five test files take 136 s of the 421 s the suite runs for (measured 2026-11-23,
one run, each file's own `finished in` time). Make each fast without weakening it:

| File | Now | Target |
|---|---|---|
| `crates/blyte-agent/tests/test_gate_failures.rs` (6 tests, trivial gate commands) | 80.6 s | under 10 s |
| `crates/blyte-cli/tests/test_fix_round.rs` | 15.0 s | under 5 s |
| `crates/blyte-agent/tests/test_no_progress.rs` | 14.9 s | under 5 s |
| `crates/blyte-agent/tests/test_research_reminder.rs` | 13.9 s | under 5 s |
| `crates/blyte-cli/tests/test_run_results.rs` | 11.3 s | under 5 s |

**How.** First find where the time goes in each file: run it alone
(`cargo test -p blyte-agent --test test_gate_failures -- --test-threads=1 --nocapture`, and
`-p blyte` for the two in `crates/blyte-cli`), time each test, and read the code path it drives.
Six tests with trivial gate commands should not take 80 s, so look for a fixed wait, a retry delay,
a poll interval, a timeout that is waited out in full, or a slow process start. Then remove the
cause:

- If the time is a wait in the test itself (a `sleep`, an over-long timeout), shorten or remove it.
- If the time is a fixed wait or interval in production code, do not change the production value.
  Make it a field of the configuration struct the code already takes (keeping the present value as
  that field's default) and have the tests set a short value.
- If a test truly needs its time (it measures a real timeout), leave it and say so in the report.

**Constraints.**

- Every existing assertion stays. Do not delete, ignore (`#[ignore]`), merge or weaken a test.
- Production behaviour does not change: any value made configurable keeps its present value as the
  default, and no command-line flag or `blyte.toml` key is added.
- No fallbacks: do not replace a failing wait with a default or a skipped check.
- If you change production code, the tests of both crates must still pass.

**Tests.** The five files themselves, each run alone, with its `finished in` time given in your
report next to the cause you found and what you changed. If a file misses its target, say why.

**Commit message (exact):**

```
test: make the five slowest test files fast
```
