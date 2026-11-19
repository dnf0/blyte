# Environment guard: three defects left in main

Date: 2026-11-19. Found by the blind comparison of four models' work on the environment-guard
tasks; the merged version (#362) carries them. Each task is independent and touches its own files.

Rules for every task: no fallbacks (a missing or unreadable value is an error that names it, never
a default or a silent skip). Add no comments. Existing tests pass unmodified unless the task says
otherwise. Nothing here is a long-running job.

### Task ED1: outside_env resolves `..` and quoted arguments correctly

**Depends on:** none. **Siblings:** ED2 and ED3 change `dispatch.rs` and other test files
separately; they are not in this worktree; do not touch those files.

**Files:** `crates/blyte-cli/src/outside_env.rs`, `crates/blyte-cli/tests/test_outside_env.rs`.

**Why:** two defects make `borrowed_env_commands` miss commands that copy a dependency environment
from outside the worktree.

1. `normalize_path` pops a preceding `..` when it meets another `..`, so the relative path `../..`
   normalises to the empty path and `a/../../b` loses a level. Correct behaviour, with no file
   system access:
   - `..` after a normal component removes that component;
   - `..` directly under a root (`/..`) is dropped;
   - `..` at the start of a relative path, or after another kept `..`, is kept.
   So `../..` stays `../..`, `a/../../b` becomes `../b`, `/a/../../b` becomes `/b`, `a/./b/..` becomes `a`.
   Make `normalize_path` `pub` so the tests can call it.
2. `part_borrows_env` splits a command on whitespace and then trims quote characters from each
   token, so a quoted argument containing a space is cut in two: `cp -r "/Users/me/my repo/.venv" .`
   yields the tokens `"/Users/me/my` and `repo/.venv"`, and the second looks like a path inside the
   worktree. Replace the whitespace split with a small tokenizer in this module (no new dependency),
   `pub fn shell_words(part: &str) -> Vec<String>`:
   - words are separated by unquoted whitespace;
   - inside single quotes everything is literal up to the closing quote;
   - inside double quotes everything is literal up to the closing quote, except that a backslash
     before `"` or `\` yields that character;
   - outside quotes a backslash makes the next character literal (so `my\ repo` is one word);
   - adjacent quoted and unquoted pieces join into one word (`--from="/a b/.venv"` is the one word
     `--from=/a b/.venv`);
   - an unterminated quote runs to the end of the input.
   `part_borrows_env` then uses these words and no longer trims quote characters itself. Its other
   behaviour is unchanged.

**Tests** (add to `test_outside_env.rs`; run `cargo test -p blyte --test test_outside_env`):
- `normalize_path` for each of the four examples above, plus `./a` giving `a` and `/..` giving `/`.
- `shell_words` for: `cp -r "/a b/.venv" .` giving `["cp", "-r", "/a b/.venv", "."]`;
  `cp '/a b/.venv' x`; `cp /a\ b/.venv x`; `--from="/a b/.venv"`; `"a \"b\" c"` giving `a "b" c`;
  the empty string giving no words.
- `borrowed_env_commands` with the worktree `/work/repo/.wt/T1` reports
  `cp -r "/Users/me/my repo/.venv" .` and `cp -r /Users/me/my\ repo/.venv .`, and does not report
  `cp -r "sub dir/.venv" backup` (inside the worktree). Build the log the way the existing tests in
  this file do.
- `borrowed_env_commands` with the relative worktree `.wt/T1` reports `cp -r ../../../.venv .`
  (three levels up leaves the repository) and does not report `cp -r ../T1/.venv x`.

**Commit message:** `fix(review): outside_env keeps leading .. and honours shell quotes`

### Task ED2: only a blocked run's error is cut to its first line

**Depends on:** none. **Siblings:** ED1 changes `outside_env.rs`, ED3 changes
`tests/test_dispatch_base_check.rs`; neither is in this worktree; do not touch those files.

**Files:** `crates/blyte-cli/src/dispatch.rs`, new `crates/blyte-cli/tests/test_dispatch_summary_error.rs`.

**Why:** when dispatch builds its summary from the run's report it takes `error` as
`rec.get("error")...and_then(|s| s.lines().next())`, for every outcome. The first-line cut was meant
only for `blocked` (whose error repeats the gate output already in `final_message`). For every other
outcome the later lines of a multi-line error are lost from the summary.

**What to build:** in `dispatch.rs`, `pub fn summary_error(outcome: &str, error: Option<&str>) -> Option<String>`:
- `None` gives `None`;
- outcome `blocked`: the first line of the error (an empty error gives `None`, as `lines().next()` does now);
- any other outcome: the whole error unchanged.
Use it where the summary's error is taken from the report record (the tuple built in the `Ok(rec)`
arm that reads `rec.get("error")`), passing the same outcome string that arm puts in the tuple.
Change nothing else in that arm.

**Tests** (new file; run `cargo test -p blyte --test test_dispatch_summary_error`):
- `summary_error("blocked", Some("gate failed at base\nline two"))` is `Some("gate failed at base")`;
- `summary_error("error", Some("provider failed\ncaused by: timeout"))` is the whole string;
- the same whole-string result for `gate_failed` and `budget_exhausted`;
- `summary_error("error", None)` and `summary_error("blocked", None)` are `None`;
- `summary_error("blocked", Some(""))` is `None`.

**Commit message:** `fix(dispatch): keep the whole error in the summary unless the run is blocked`

### Task ED3: the skip-base-check tests reach the model

**Depends on:** none. **Siblings:** ED1 changes `outside_env.rs`, ED2 changes `dispatch.rs`; neither
is in this worktree. Change only the test file below; if a test cannot pass without changing
`src/`, stop and say so in the final message.

**Files:** `crates/blyte-cli/tests/test_dispatch_base_check.rs` only.

**Why:** `test_dispatch_no_base_check_flag_skips_base_check` and
`test_dispatch_config_base_check_false_skips_base_check` remove every API key and assert only that
the outcome is not `blocked`. Without a key the dispatch fails at provider setup, so the tests pass
whether or not the base check was skipped.

**What to build:** make both tests run against a local fake model so the run reaches the model.
- Add a helper to this file modelled on `model_server()` in
  `crates/blyte-cli/tests/test_watchdog_progress.rs` (an OpenAI-compatible server on
  `127.0.0.1:0` whose model answers "done"), which also counts the requests it receives: return the
  port and an `Arc<AtomicUsize>`.
- Run `blyte dispatch` in both tests with the environment that test uses for a fake model:
  `HOME` set to a fresh temporary directory, the API keys and `OLLAMA_*` variables removed,
  `OPENAI_BASE_URL=http://127.0.0.1:<port>/v1`, `OPENAI_API_KEY=test`,
  `BLYTE_MODEL=openai/gemini-3.8-flash`, plus the `BLYTE_USAGE_LOG=off` and
  `BLYTE_NO_UPDATE_CHECK=1` already there. Keep `--gate "exit 3"` and add `--max-turns 2` so the
  run ends quickly (the gate fails after each "done").
- Assert in both: the outcome is not `blocked`; `summary["turns"]` is at least 1; the server
  received at least one request; `start_json["base_check"]` is `false` (already there).
- In `test_dispatch_failing_gate_stops_as_blocked_before_model`, give the run the same fake model
  and assert the server received zero requests, so "before the model" is proved rather than
  implied by missing keys. Keep its existing assertions.

Before committing, check the two skip tests are decisive: temporarily remove `--no-base-check` from
the first one, see it fail, and restore it. Say in the final message that you did.

Run `cargo test -p blyte --test test_dispatch_base_check`.

**Commit message:** `test(dispatch): the skip-base-check tests reach the model`
