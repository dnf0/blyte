# Self-hosting gaps, round 2 (2026-11-28)

Five tasks from the 2026-11-28 code check (research runs SHR1 and SOR1). QP1, WK1, PB1 and CK1
touch different files and run in parallel. RD1 changes `run.rs` like QP1 and starts from QP1's
branch. Spec for RD1: `docs/specs/2026-11-28-redact-known-secrets.md`.

Rules for every task: no fallbacks (when something is missing or unreadable, return an error that
names it; never a default, an empty result or a silent skip). Add no comments. Existing tests pass
without changing their assertions. Change only the files the task names; if another file must
change, say why in the final message.

### Task QP1: the research quote check does not read protected files

**Goal.** A research report's quote directives and its checked citations are read from disk by
`Runner::check_report_quotes` and `Runner::run_claim_check` in `crates/blyte-agent/src/run.rs`
with `self.fs.read`, without the protected-path test the `read` tool applies
(`crate::sandbox::is_protected_read`, used in `crates/blyte-agent/src/tools.rs` in the `"read"`
arm). A directive that names a protected file inside the workspace or a read root is pasted into
the report. Apply the same test.

**Files.** `crates/blyte-agent/src/run.rs`; new `crates/blyte-agent/tests/test_quote_check_protected.rs`.

**What to build.**
- In `run.rs`, `pub fn read_unless_protected(fs: &dyn Fs, path: &Path) -> Result<String, String>`:
  when `fs.is_local()`, call `crate::sandbox::is_protected_read(path)`; `Ok(true)` returns
  `Err(format!("{} is a protected path and is not read", path.display()))`; `Err(e)` returns
  `Err(e)`; otherwise `fs.read(path)`. When the file system is not local, `fs.read(path)`.
- Use it in the three closures: `expand_read` in `check_report_quotes` (after
  `tools::resolve_for_read`), and the `read` closures in `check_report_quotes` and
  `run_claim_check`.

**Tests** (`cargo test -p blyte-agent --test test_quote_check_protected`). See
`crates/blyte-agent/tests/test_protected_paths.rs` for how a test arranges a protected path
(a temporary `HOME`); tests that set `HOME` must not run in parallel with each other (one test
function, or a shared lock).
1. `read_unless_protected` on a file under the temporary home's `.ssh` returns an error containing
   `is a protected path`, and the error does not contain the file's content.
2. `read_unless_protected` on an ordinary file returns its content.
3. Through `quote_expand::expand` with a reader built on `read_unless_protected`: a report whose
   directive names the protected file is returned without the file's content and with a problem
   that names the path.

**Depends on:** none. **Siblings (not in this worktree):** WK1, PB1, CK1; RD1 starts after this.

**Commit message:** `fix(research): the quote check does not read protected files`

### Task WK1: the surface is woken when its request is combined, and when a passing run is flagged

**Goal.** A surface's model gets a queued message when a run fails, but not when (a) its request
has been combined, or (b) a run passed its check yet review flagged it, which holds combining
until a verdict. In both cases nothing wakes it, so it never asks to publish or records a verdict.

**Files.** `crates/blyte-cli/src/requests.rs`, `crates/blyte-cli/src/run_results.rs`,
`crates/blyte-cli/tests/test_requests.rs`, `crates/blyte-cli/tests/test_run_results.rs`.

**What to build.**
- (a) In `requests::maybe_combine`, the `0 =>` arm after `combine::combine_in`: after writing the
  `AttemptRecord` with `AttemptResult::Combined`, queue a message with
  `session.queue_for_request(request, &msg)` and, when that succeeds, write the record again with
  `reported: true` (the pattern the failure arms use). The message, built by a new
  `pub fn format_combined_message(rec: &crate::combine::CombineRecord) -> String`:

  ```
  Request <request> is combined on branch <branch> (<n> commit(s): <short hashes, comma separated>).
  Its check passed. Tell the user what changed, and ask to publish it if they want a pull request.
  ```

  A short hash is the first 8 characters. If the combine record is missing or unreadable after
  exit 0, queue nothing and call `session.record_error` naming the request and the record path
  (the existing error for an unreadable record stays; add one for a missing record).
- In the block near the top of `maybe_combine` that retries an unreported attempt
  (`attempt.result != AttemptResult::Combined && !attempt.reported`): an unreported `Combined`
  attempt is retried too, with `format_combined_message` from the stored combine record.
- (b) In `run_results::report_single_run`, for a run that is not research and passed with outcome
  `verified`: read `<records dir>/<run id>.checks.json` with `crate::replay::read_checks` and
  compute `crate::replay::card_for_run(false, "verified", checks_recorded, &flags)` as
  `requests.rs` does for readiness. When the checks are recorded and the card is not `"clean"`,
  queue, via a new `fn format_flagged_queue_message(run_id: &str, flags: &[String]) -> String`:

  ```
  Run <id> passed its check but review flagged it: <flags joined with "; ">.
  Combining waits for a verdict. Read blyte review <id>, then record blyte review <id> --verdict accepted, fixed or redone.
  ```

  A read error from `read_checks` is returned as this function's error, naming the file. When the
  checks file is not there yet, queue nothing for this case. If you find that the checks file is
  always written after `report_single_run` runs, stop and say so in the final message instead of
  moving the call.

**Tests** (`cargo test -p blyte --test test_requests --test test_run_results`), following the
existing tests in those files:
1. `format_combined_message` for a record with branch `combined/r1` and commits
   `["0123456789abcdef", "fedcba9876543210"]` contains `combined/r1`, `2 commit(s)`, `01234567`
   and `fedcba98`, and not `0123456789`.
2. After a `maybe_combine` that combines (extend an existing combining test), the session holds one
   queued message for the request that contains `is combined on branch`, and the attempt record has
   `reported: true`.
3. A verified run whose checks file holds one flag queues a message that contains
   `passed its check but review flagged it` and the flag text.
4. A verified run with a clean checks file queues nothing.

**Depends on:** none. **Siblings (not in this worktree):** QP1, PB1, CK1, RD1.

**Commit message:** `feat(surface): wake the surface when its request is combined or a passing run is flagged`

### Task PB1: `blyte dispatch --plan` reads a plan that exists only on `--base`

**Goal.** A plan written by a direct edit lives on `wip/<id>`, not in the checkout. Dispatching a
task from it with `--base wip/<id> --plan docs/plans/x.md` fails with "error reading plan". Read
the plan from the base branch in that case.

**Files.** `crates/blyte-cli/src/dispatch.rs`, `crates/blyte-cli/src/dispatch_in_place.rs` (call
site only), `crates/blyte-cli/assets/conversation/reference.md`, new
`crates/blyte-cli/tests/test_dispatch_plan_from_base.rs`.

**What to build.**
- `load_task_text` gains `root: &Path` and `base: Option<&str>`. For `--plan`:
  1. if the plan path exists as a file (as given, relative to the current directory as today),
     read it as today;
  2. else, if `base` is given and the path is relative, read
     `git -C <root> show <base>:<path>` (path with forward slashes, a leading `./` removed);
  3. else the error is
     `error reading plan <path>: not found in the checkout` followed, when a base was given, by
     ` or on <base>` and git's error line. An absolute path that does not exist is never looked up
     on the base.
  The text that precedes the section stays `From the plan <path>:` in case 1 and is
  `From the plan <path> (on <base>):` in case 2.
- `dispatch_in_place.rs` has no base: pass `None`.
- `reference.md`: in the list of dispatch options, after the `--base` item, add one item: a plan
  written by a direct edit is on `wip/<id>`; dispatch its tasks with `--base wip/<id>` and the
  plan's path, and blyte reads the plan from that branch.

**Tests** (`cargo test -p blyte --test test_dispatch_plan_from_base`): in a temporary git
repository with a commit on `main` and a branch `wip/p1` that adds `docs/plans/p.md` holding
`### Task A1: one` with a body line:
1. `load_task_text` with the plan path, task `Task A1` and base `wip/p1`, run where the file is
   absent, returns text that contains the body line and `(on wip/p1)`.
2. With no base: the error contains `not found in the checkout` and not `or on`.
3. With base `main`: the error contains `or on main`.
4. With the file present in the checkout and a different body on the base: the checkout's body is
   returned.
If `load_task_text` cannot be reached from an integration test, make it `pub` (the crate already
exposes modules for tests) and say so.

**Depends on:** none. **Siblings (not in this worktree):** QP1, WK1, CK1, RD1.

**Commit message:** `feat(dispatch): read a plan from the base branch when it is not in the checkout`

### Task CK1: `blyte update` checks the downloaded binary against a published SHA-256

**Goal.** `blyte update` (binary install) replaces the running binary with a download it only
checks by running it. Publish a checksum with every release and refuse a download that does not
match it.

**Files.** `crates/blyte-cli/src/update.rs`, `crates/blyte-cli/Cargo.toml`, `Cargo.lock`,
`.github/workflows/release.yml`, `crates/blyte-cli/tests/test_update.rs`, `docs/reference/cli.md`.

**What to build.**
- `release.yml`, in the build job after "Name binary for release": a step
  `shasum -a 256 blyte-${{ matrix.target }} > blyte-${{ matrix.target }}.sha256`, and attach both
  files to the release (`files:` takes one path per line).
- `update.rs`:
  - add the `sha2` crate (version 0.10) to `crates/blyte-cli/Cargo.toml`;
  - `pub fn parse_checksum(text: &str) -> Result<String, String>`: the first whitespace-separated
    token, lower-cased; an error naming what was found unless it is exactly 64 hexadecimal
    characters;
  - `pub fn sha256_hex(path: &Path) -> Result<String, String>`;
  - in the binary path of `update`, after `download_release` and before the file is made
    executable or run: fetch `<download base>/<repo>/releases/download/<tag>/<asset>.sha256` with
    the same client and error style as `download_release`, parse it, and compare with
    `sha256_hex(&tmp)`. On a missing checksum file, an unparseable one, or a mismatch: remove the
    temporary file and return an error that names the asset and, for a mismatch, both digests,
    ending with `the installed blyte is unchanged`. The download is never run before it matches.
- `docs/reference/cli.md`, in the `blyte update` text: one sentence saying the download is
  compared with the release's published SHA-256 and refused when they differ or none is published.

**Tests** (`cargo test -p blyte --test test_update` and the unit tests in `update.rs`), following
the local HTTP server the existing `test_update.rs` tests use for `BLYTE_UPDATE_API` and
`BLYTE_UPDATE_DOWNLOAD`:
1. `parse_checksum("ABCDEF…(64 hex)  blyte-x")` returns the lower-cased digest; `"abc  x"` and
   `""` are errors.
2. `sha256_hex` of a file holding `abc` is
   `ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad`.
3. An update whose server serves the asset and a matching `.sha256` succeeds as the existing
   happy-path test does (add the checksum to that test's server).
4. A served `.sha256` that does not match: exit is non-zero, the error contains `unchanged`, and
   the installed binary's bytes are the same as before.
5. No `.sha256` on the server (404): the same as 4, and the error names `<asset>.sha256`.

**Depends on:** none. **Siblings (not in this worktree):** QP1, WK1, PB1, RD1.

**Commit message:** `feat(update): check the downloaded binary against the release's SHA-256`

### Task RD1: known secret values are removed from tool results and gate output

**Goal and design.** Read `docs/specs/2026-11-28-redact-known-secrets.md` first; it is the
contract. Build exactly that.

**Files.** new `crates/blyte-agent/src/redact.rs`, `crates/blyte-agent/src/lib.rs` (module line),
`crates/blyte-agent/src/shell.rs` (visibility of `credential_like` only),
`crates/blyte-agent/src/tools.rs`, `crates/blyte-agent/src/run.rs` (`run_gate_step` only),
`docs/security.md`, new `crates/blyte-agent/tests/test_redact.rs`.

**What to build.**
- `redact.rs`:
  - `pub const MIN_SECRET_BYTES: usize = 8;`
  - `pub struct Redactor` holding `(name, value)` pairs sorted by value length, longest first,
    with duplicates of a value removed (the first name in sorted order is kept).
  - `pub fn from_pairs(pairs: Vec<(String, String)>) -> Redactor`: drops values shorter than
    `MIN_SECRET_BYTES`.
  - `pub fn apply(&self, text: &str) -> String`: each occurrence of each value becomes
    `[redacted NAME]`.
  - `pub fn parse_env_file(text: &str) -> Vec<(String, String)>`: lines `NAME=value`, an optional
    leading `export `, surrounding matching single or double quotes removed from the value, blank
    lines and lines starting with `#` ignored, a line with no `=` ignored.
  - `pub fn for_root(root: &Path) -> Result<Redactor, String>`: the three sources in the spec. A
    `.env` at `root` that does not exist contributes nothing; one that exists and cannot be read is
    `Err` naming the path.
- `tools::execute_with_timeout`: after bounding, `Redactor::for_root(root)`; on `Ok`, apply it to
  `outcome.content`; on `Err(e)`, return `ToolOutcome::err(e)` in place of the result.
- `run::run_gate_step`: apply the same to the output text it returns; on `Err(e)`, the step
  fails with `e` as its output.
- `docs/security.md`: a short section stating what is redacted, where, and the limits in the
  spec's "Not done" and "Where" sections (direct edits, setup steps and the second-model review
  are not covered).

**Tests** (`cargo test -p blyte-agent --test test_redact`); tests that set environment variables
run in one test function or under one lock:
1. `from_pairs([("A","supersecretvalue")]).apply("x supersecretvalue y")` is `x [redacted A] y`.
2. A 7-byte value is not redacted; an 8-byte one is.
3. With `("A","abcdefgh")` and `("B","abcdefghijkl")`, the text `abcdefghijkl` becomes
   `[redacted B]`.
4. `parse_env_file` on `export API_TOKEN="tok_12345678"\n# c\nPLAIN\nHOST=example` returns
   `[("API_TOKEN","tok_12345678"),("HOST","example")]`.
5. `for_root` on a directory whose `.env` holds `API_TOKEN=tok_12345678` and
   `PROJECT_NAME=demo-project-name`: `apply` redacts the first value and leaves the second.
6. `for_root` with a `.env` that is a directory returns an error naming the path.
7. Through `tools::execute_with_timeout` with the `read` tool on a workspace file that contains
   the `.env` token: the content holds `[redacted API_TOKEN]` and not the token.
8. With a `HARNESS_ENV` name set to `key-abcdefgh-123456`, `apply` redacts it under that name.

**Depends on:** QP1 (dispatch with `--base wip/QP1`). **Siblings (not in this worktree):** WK1,
PB1, CK1.

**Commit message:** `feat(sandbox): remove known secret values from tool results and gate output`
