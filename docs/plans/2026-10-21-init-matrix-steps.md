# blyte init: expand CI matrix steps and recognise `make -C <dir> <target>`

**Context (2026-10-21):** reliability-orchestrator now runs `blyte init --print` on repositories without a
`blyte.toml` and uses the gate it derives. On spatial-service, a uv workspace, CI runs the unit tests once per service
through a matrix:

```yaml
jobs:
  test-unit:
    strategy:
      matrix:
        service: [spatial-service, shared-utils, portfolio-splitter, raster-reader, premise-reader]
    steps:
      - name: Run tests with coverage
        run: |
          make -C ${{ matrix.service }} test-coverage
```

`parse_workflow_commands` (`crates/blyte-cli/src/init.rs`) drops this step for two reasons: `make -C x test-coverage`
is not in `filter_and_clean_command`'s `check_cmds` (`make test` matches only when followed by whitespace), and
`${{ matrix.service }}` is never expanded. So init finds no tests for that repository.

### Task MX1: expand `${{ matrix.<key> }}` in run steps; recognise `make` check targets with flags

**Goal:** `detect_ci` returns one command per matrix value for a matrix step, and recognises `make` invocations with
directory flags and test-style targets, so `blyte init` derives per-service checks for workspace repositories.

**Files:**
- `crates/blyte-cli/src/init.rs`
- `crates/blyte-cli/tests/test_init_matrix.rs` (new)

**What to build:**

1. **Matrix expansion**, in `parse_workflow_commands` (stay line-based; do not add a YAML dependency):
   - A **job** is a key indented one level under the top-level `jobs:` key. Track the current job while scanning.
   - In a job, a `matrix:` key (normally under `strategy:`) introduces matrix entries: each key one level deeper than
     `matrix:` is a matrix key. Its values are either an inline flow list (`service: [a, b, "c"]`) or a block list on
     the following deeper-indented lines (`- a`). Strip surrounding single or double quotes from each value. Keys
     whose value is anything else (`include:`, `exclude:`, `${{ fromJSON(...) }}`, a mapping) define no values.
   - Each `run:` command (single-line or block, before `split_chain`) is expanded against its own job's matrix:
     every occurrence of `${{ matrix.<key> }}` (whitespace inside the braces optional: `${{matrix.key}}`) is replaced
     by each value of `<key>`, giving one command per value, in the matrix's order. A command using two matrix keys
     gives the cartesian product, first key outermost.
   - A command still containing `${{ matrix.` after expansion (unknown key, or a key with no values) is dropped.
   - Commands without matrix expressions are unchanged. Matrices in one job never affect another job.
   - Identical resulting commands keep only their first occurrence.
2. **`make` checks**, in `filter_and_clean_command`: after the existing `check_cmds` test fails, also accept a
   command whose first token is `make` when:
   - the remaining tokens are any of `-C <dir>`, `-C<dir>`, `--directory=<dir>`, `--directory <dir>`, `-s`,
     `--no-print-directory`, `-k`, plus exactly one target token not starting with `-`; and
   - the target is one of `check`, `lint`, `test`, `typecheck`, `validate`, or starts with `test-` or `test_`.
   `lint-fix`, `format`, `install`, `build` and any other target are not checks. A `make` command with variable
   assignments (`FOO=bar`) or two or more targets is not accepted by this new rule (the existing `check_cmds` rule
   still applies to it as today). Cleaning (`| tee` removal and reporting-option stripping) applies as for other
   checks.

**Tests** (new file `crates/blyte-cli/tests/test_init_matrix.rs`, in the style of `tests/test_init_real_ci.rs`,
calling `blyte_cli::init::detect_ci` on a tempdir with a workflow file; run with
`cargo test -p blyte-cli --test test_init_matrix`):
- `matrix_make_step_expands_per_service`: the spatial-service shape above (include the `strategy:` block with
  `fail-fast: false`, other steps such as `uses: actions/checkout@v4`, a `run: |` block of `sudo apt-get` setup, a
  `run: | uv sync` step, and a step `ls -la ${{ matrix.service }}/tests/coverage/`): `detect_ci` returns exactly the
  five `make -C <service> test-coverage` commands, in matrix order.
- `block_list_matrix_with_quotes`: `python: ["3.11", '3.12']` written as a block list (`- "3.11"` / `- '3.12'`) and a
  step `uv run --python ${{ matrix.python }} pytest -q`: two commands, `uv run --python 3.11 pytest -q` and
  `uv run --python 3.12 pytest -q`.
- `two_keys_give_cartesian_product`: keys `a: [x, y]`, `b: [1, 2]`, step `make -C ${{ matrix.a }}${{matrix.b}} test`:
  `make -C x1 test`, `make -C x2 test`, `make -C y1 test`, `make -C y2 test`.
- `unknown_or_dynamic_matrix_key_is_dropped`: a job with `matrix: { include: [...] }` style
  (`include:` followed by `- service: a`) and a step `make -C ${{ matrix.service }} test`, plus a plain
  `uv run ruff check .` step: only `uv run ruff check .` is returned.
- `matrix_is_scoped_to_its_job`: job one has `service: [a]` and no steps using it; job two has no matrix and a step
  `make -C ${{ matrix.service }} test`: nothing from job two is returned.
- `make_targets`: steps `make -C svc test-coverage`, `make --directory=svc test_unit`, `make -s lint`,
  `make -C svc lint-fix`, `make install`, `make -C a test lint`, `make FOO=1 test-x`: only the first three are
  returned.
- `duplicates_are_dropped`: two jobs with the same matrix and step give each command once.
- Existing init tests pass unmodified.
- Full gate: the repository's `blyte.toml` gate (`cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test --workspace --all-targets`).

**Constraints:** no other files change; no new dependencies; existing tests pass unmodified; `pub` items keep their
signatures.

**Depends on:** none.

**Commit message:**
```
feat(init): expand CI matrix steps and recognise make check targets

Workspace repositories run their tests per package through a CI matrix
(`make -C ${{ matrix.service }} test-coverage`). init dropped such steps:
the matrix expression was never expanded and `make -C dir test-coverage`
was not a recognised check. Expand matrix expressions from the job's
matrix, one command per value, and accept make with directory flags and
test-style targets.
```
