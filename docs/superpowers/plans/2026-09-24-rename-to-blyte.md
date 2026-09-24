# Rename to blyte Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** The harness is renamed from `superstate` to `blyte`:
- crates `blyte-*`, with the package `blyte` providing the `blyte` binary and the `blyte_cli` library;
- Rust paths `blyte_*::`;
- environment variables `BLYTE_*`;
- user-facing text "Blyte".

**Not renamed:**
- repository names: the GitHub repo `climate-x-org/superstate`, the eval repo `superstate-eval` (and `superstate_eval`, `superstate_eval_runs`), and local checkout paths;
- historical records: `docs/superpowers/plans/`, `docs/superpowers/specs/`, and past changelog entries.

**Why:** The user chose `blyte`: blight, the rust fungus Rust is named after, plus "byte". The placeholder crates `blyte`, `blyte-agent`, `blyte-provider`, `blyte-core` and `blyte-cli` are already reserved on crates.io at 0.0.0, and the workspace stays `publish = false`.

**Method:** `scripts/rename_to_blyte.sh` (committed with this plan) does the whole rename deterministically:
1. `git mv` of the crate directories.
2. Perl substitution over tracked text files, with the repository names protected by placeholders.
3. The changelog header.
4. `cargo fmt`.

Claude dry-ran it on a throwaway worktree: clippy was clean, every test passed, and it built a `blyte` binary. The only remaining `superstate` references were the protected ones.

**Tech Stack:** Rust 2021, bash, perl.

## Global Constraints

- Base branch: `feat/rename-blyte` (from `main`). The task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back.
- Do NOT edit `~/.gemini`, `~/.cargo`, `~/.local/bin` or any file outside the worktree. Do not install binaries.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass after the rename. `blyte-mcp`'s `test_blyte_status_and_cancel_lifecycle` and `blyte-cli`'s `test_cli_daemon_detached_forwarding` are known to be timing-flaky: if one alone fails, re-run once and report both results.
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Run the rename

**Files:** everything the script touches (the whole repository except the historical records named above).

- [ ] **Step 1: Run the script** from the worktree root: `bash scripts/rename_to_blyte.sh`. Expected: it prints `renamed; now run: ...` and exits 0.

- [ ] **Step 2: Check nothing but protected names remain.** Run:

```bash
git ls-files | grep -v -E '^docs/superpowers/(plans|specs)/|^CHANGELOG\.md$|^scripts/rename_to_blyte\.sh$' \
  | xargs grep -oh -i "[a-z_./:-]*superstate[a-z_./:-]*" | sort | uniq -c
```

  Expected: only `superstate-eval`, `superstate_eval…` and `https://github.com/climate-x-org/superstate…` forms. If anything else remains, stop and report it.

- [ ] **Step 3: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Confirm `target/debug/blyte` exists.

- [ ] **Step 4: Commit.**

```bash
git add -A
git commit --no-gpg-sign -m "refactor!: rename superstate to blyte (crates, binary, env vars, text); repository names unchanged"
```
