# blyte init: take per-directory setup steps from CI

**Context (2026-10-21):** after MX2, `blyte init --print` on a repository holding a uv workspace at the root and a
second, separate uv project in `worker-service/` derives the right gate, but the `worker-service` step fails on HEAD:
its CI job runs `uv sync --frozen --all-groups` with `defaults: run: working-directory: worker-service` before
pytest, and init's setup is only `detect_setup`'s root `uv sync`. The tests then fail on import.

### Task MX3: per-directory `uv sync` steps from CI become part of `setup`

**Goal:** `blyte init` adds each CI `uv sync` step that runs in a non-root working directory to `setup`, so every uv
project the gate tests has its environment.

**Files:**
- `crates/blyte-cli/src/init.rs`
- `crates/blyte-cli/tests/test_init_ci_setup.rs` (new)

**What to build:**

1. Add `pub fn detect_ci_setup(root: &Path) -> Vec<String>`: from the same workflow scan as `detect_ci` (same files,
   same order, same matrix expansion, working-directory and env rules as MX2), every `run:` command part (after
   `split_chain`) whose first tokens are `uv sync` (optionally after a `uv run`-style prefix is **not** allowed: only
   a literal `uv sync ...`), with a working directory other than the root. Return each as
   `(cd <dir> && [env K=V ]uv sync <flags as in CI>)`, with the directory and values quoted as MX2 quotes them,
   de-duplicated, in file and line order. Root-level `uv sync` steps are not returned (the root is covered by
   `detect_setup`). A `uv sync` step whose working directory or env is a CI expression is not returned; report it in
   `detect_ci_left_out` with the same reasons MX2 uses.
2. In `run`, the setup becomes the root setup (`detect_setup`, when present) followed by `detect_ci_setup`'s
   commands, joined with ` && ` into one line. When `detect_setup` is None and `detect_ci_setup` is non-empty, the
   setup is the CI commands alone. Verification runs this combined setup exactly as it runs the single command today,
   and the rendering stays `setup = "<one line>"` (escape `"` and `\` as a TOML basic string requires).
   `detect_setup` keeps its signature and behaviour.
3. In `--print --no-verify` mode, the rendered `setup` line is the combined setup too.

**Tests** (new file `crates/blyte-cli/tests/test_init_ci_setup.rs`, in the style of
`tests/test_init_step_context.rs`; run with `cargo test -p blyte --test test_init_ci_setup`):
- `per_directory_uv_sync_is_setup`: a workflow with `defaults: run: working-directory: worker-service` and steps
  `run: uv sync --frozen --all-groups` and `run: uv run --frozen --no-sync pytest`: `detect_ci_setup` gives
  `["(cd worker-service && uv sync --frozen --all-groups)"]`, and `detect_ci` still gives only the pytest command.
- `root_uv_sync_is_not_ci_setup`: a root-level `run: uv sync`: `detect_ci_setup` is empty.
- `matrix_directories`: `matrix: svc: [a, b]`, `working-directory: ${{ matrix.svc }}`, `run: uv sync`: two commands, for
  `a` then `b`.
- `secret_env_setup_left_out`: a non-root `uv sync` step with `env: { TOKEN: ${{ secrets.T }} }`: not in
  `detect_ci_setup`; present in `detect_ci_left_out` with reason containing `env TOKEN is set from a CI expression`.
- `init_print_combines_setup` (runs the built binary like `tests/test_init_step_context.rs` does, in a temp git
  repository with a root `uv.lock`, the workflow from the first test, `--print --no-verify`): stdout contains
  `setup = "uv sync && (cd worker-service && uv sync --frozen --all-groups)"`.
- Existing init tests pass unmodified.
- Full gate: the repository's `blyte.toml` gate.

**Constraints:** no other files change; no new dependencies; no fallbacks (nothing dropped silently); existing tests
pass unmodified; public items keep their signatures.

**Depends on:** MX2 (merged on this branch).

**Commit message:**
```
feat(init): add per-directory uv sync steps from CI to setup

A repository with a second uv project in a subdirectory syncs it in CI
before testing it; init only set up the root, so that project's tests
failed on HEAD. Add each CI `uv sync` that runs in a non-root working
directory to setup, after the root sync.
```
