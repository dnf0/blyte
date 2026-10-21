# blyte init: run CI steps in their working directory with their env, and report steps left out

**Context (2026-10-21):** after MX1 (matrix expansion), `blyte init --print` on a uv workspace derived the right
per-service test commands, but two failed on HEAD for reasons that are not test failures:

- A workflow with `defaults: run: working-directory: worker-service` runs `uv run pytest` inside that directory; init
  ran it at the repository root.
- A step with
  ```yaml
  env:
    ENVIRONMENT: staging
    AWS_REGION: eu-west-1
  run: make -C ${{ matrix.service }} test-coverage
  ```
  needs those plain values (one test raises "AWS_REGION environment variable is required"); init dropped them.

Also, `parse_workflow_commands` silently drops a command it cannot expand (an unknown matrix key). Rule for this
code: nothing is dropped silently. A step init cannot reproduce is **left out with a reason** that is printed and
written into the file, as verification already does for steps that fail for environment reasons.

### Task MX2: working directory and env for CI steps; report steps left out

**Goal:** each CI step is reproduced with its working directory and its literal environment values, and every step
init recognises as a check but cannot reproduce is reported with a reason instead of disappearing.

**Files:**
- `crates/blyte-cli/src/init.rs`
- `crates/blyte-cli/tests/test_init_step_context.rs` (new)

**What to build** (stay line-based; no YAML dependency):

1. **Working directory.** Track `defaults: run: working-directory:` at workflow level (top-level `defaults:`) and job
   level (`defaults:` one level under a job), and `working-directory:` on a step (a key at the same indentation as the
   step's `run:`, before or after it). The step's own value wins over the job's, which wins over the workflow's.
   Expand `${{ matrix.<key> }}` in it as MX1 does for commands (a working directory that expands to several values
   multiplies the commands, consistently with the command's own expansion: one value per matrix combination, so
   `working-directory: ${{ matrix.service }}` with `run: uv run pytest` gives one command per service).
   A directory of `.` or `./` means the root (no prefix).
2. **Env.** Collect `env:` mappings at workflow level, job level and step level (step wins, then job, then workflow;
   keys are scalars one level under `env:`). Strip quotes from values. Expand `${{ matrix.<key> }}` in values.
3. **Rendering.** For each command that `filter_and_clean_command` keeps (after `split_chain`, as today), build the
   final command as `[cd <dir> && ][env K1=V1 K2=V2 ]<cleaned command>`, env keys in sorted order. Quote a value with
   single quotes unless it matches `^[A-Za-z0-9_./,:@%+=-]+$` (escape an embedded `'` as `'\''`). Quote the directory
   the same way. Commands with neither keep exactly today's text.
4. **Left out, with a reason.** A recognised check command (one that `filter_and_clean_command` keeps) is left out,
   with a one-line reason, when:
   - it still contains `${{ matrix.` after expansion: `unknown matrix key <key>` (or `matrix key <key> has no plain
     values` when the key exists but has none);
   - an env value that applies to it contains `${{` after matrix expansion (secrets, vars, github context):
     `env <KEY> is set from a CI expression`;
   - its working directory contains `${{` after matrix expansion: `working-directory is a CI expression`.
   A command that is not a recognised check is ignored as today, whatever its env.
5. **API.** Add `pub fn detect_ci_left_out(root: &Path) -> Vec<(String, String, String)>` returning
   `(command, workflow file name, reason)` for each left-out check, in file and line order, de-duplicated by command.
   For the command text of a left-out step, use the cleaned command with whatever could not be expanded left as it was
   in the workflow. `detect_ci` and `detect_ci_with_sources` keep their signatures and return only reproducible
   commands.
6. **`run`** (the `blyte init` command): for each left-out step print to stderr
   `init: <command> ... left out: <reason>`, and render it in the file as a comment line in the same header block
   where verified steps are described: `# <command>: from <file>, left out: <reason>`. The exit status rules are
   unchanged.

**Tests** (new file `crates/blyte-cli/tests/test_init_step_context.rs`, in the style of
`tests/test_init_matrix.rs`; run with `cargo test -p blyte --test test_init_step_context`):
- `workflow_default_working_directory`: top-level `defaults: run: working-directory: svc` and a step
  `run: uv run --frozen pytest`: `detect_ci` gives `["cd svc && uv run --frozen pytest"]`.
- `step_working_directory_overrides_job_default`: job default `a`, step `working-directory: b`: `cd b && ...`; a
  second step in the same job without one: `cd a && ...`.
- `matrix_working_directory`: `matrix: service: [x, y]`, step `working-directory: ${{ matrix.service }}`,
  `run: make test`: `["cd x && make test", "cd y && make test"]`.
- `step_env_literals_are_prefixed`: workflow `env: { FORCE_COLOR: "1" }` written as a block mapping, step `env:` with
  `AWS_REGION: eu-west-1`, `ENVIRONMENT: staging`, `LIST: "a,b c"`, and
  `run: make -C ${{ matrix.service }} test-coverage` with `service: [s1]`: exactly
  `env AWS_REGION=eu-west-1 ENVIRONMENT=staging FORCE_COLOR=1 LIST='a,b c' make -C s1 test-coverage`.
- `env_from_secret_leaves_step_out`: a step with `env: { TOKEN: ${{ secrets.T }} }` (block form) and
  `run: uv run pytest`, plus another step `run: uv run ruff check .`: `detect_ci` gives only `uv run ruff check .`;
  `detect_ci_left_out` gives one entry with command `uv run pytest`, the workflow's file name, and a reason
  containing `env TOKEN is set from a CI expression`.
- `unknown_matrix_key_is_reported`: `make -C ${{ matrix.nope }} test` with no such key: absent from `detect_ci`;
  `detect_ci_left_out` reason contains `unknown matrix key nope`.
- `non_check_steps_ignored`: a step `run: sudo apt-get install -y gdal-bin` with secret env: not in either list.
- `init_print_reports_left_out` (runs the built binary like other CLI tests in `crates/blyte-cli/tests/`, in a temp
  git repository with one commit, `--print --no-verify`): stdout contains
  `# uv run pytest: from ci.yml, left out: env TOKEN is set from a CI expression` and stderr contains
  `left out: env TOKEN is set from a CI expression`.
- Existing init tests, including `tests/test_init_matrix.rs` and `tests/test_init_real_ci.rs`, pass unmodified.
- Full gate: the repository's `blyte.toml` gate.

**Constraints:** no other files change; no new dependencies; no fallbacks (nothing dropped silently, no guessed
directory or value); existing tests pass unmodified; public items keep their signatures.

**Depends on:** MX1 (merged on this branch).

**Commit message:**
```
feat(init): run CI steps in their working directory with their env

init ran every CI step at the repository root without the step's env,
so steps under `defaults.run.working-directory` and steps needing plain
values such as AWS_REGION failed on HEAD. Prefix each step with its
working directory and literal env values, and report steps that depend
on CI expressions (secrets, unknown matrix keys) as left out with a
reason instead of dropping them.
```
