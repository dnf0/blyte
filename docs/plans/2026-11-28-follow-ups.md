# Follow-ups after the Linux shell read protection (2026-11-28)

Four independent tasks. Each is one commit. None may add a fallback: when something is missing,
unknown or does not match, return an error that names it; never substitute a default, a guess, a
silent skip or an empty result. Do not add explanatory comments (the gate refuses them).

### Task PH1

**Goal.** `blyte plan <plan.md>` prints "error: plan has no ### Task sections" for a plan whose
task headings have no colon, such as `### Task LRP1` (see
`docs/plans/2026-11-28-linux-shell-read-protection.md`). Such headings are valid: `blyte dispatch
--plan-task "Task LRP1"` already accepts them. Make the parser accept them.

**Cause.** `parse_task_heading_id_title` in `crates/blyte-cli/src/plans.rs` does
`after_task.split_once(':')?`, so a heading without a colon is not a task.

**Files.** `crates/blyte-cli/src/plans.rs` only.

**What to build.** In `parse_task_heading_id_title`, when the text after `Task ` has no colon:
the first whitespace-separated word is the id (digits only become `T<digits>`, as now), and the
rest of the heading, trimmed, is the title. When there is no rest, the title is the id itself.
A heading that is exactly `Task` or `Task ` followed by nothing is still not a task (`None`).
Headings with a colon behave exactly as now.

**Tests** (unit tests in `plans.rs`, beside the existing ones):
- `parse_task_heading_id_title("Task LRP1")` gives id `LRP1`; the title it gives is `LRP1`.
- `"Task 3"` gives id `T3`.
- `"Task LRP1 the grant rule"` gives id `LRP1`, title `the grant rule`.
- `"Task A: one"` still gives id `A`, title `one`.
- `"Task "` gives `None`.
- `parse_plan` on a text with `### Task TT1` and `### Task TT2` (each followed by a line of body
  and a `**Depends on:** none` line) returns two tasks with ids `TT1` and `TT2`.

If the title is passed through further cleaning in that function, assert on what the function
returns after cleaning; the ids above are fixed.

Run: `cargo test -p blyte-cli plans`.

**Constraints.** Existing tests pass unmodified.

**Depends on:** none.

**Siblings.** VC1, DP1 and UP1 run separately and are not in this worktree; do not touch
`crates/blyte-agent` or `crates/blyte-cli/src/update.rs`.

**Commit message:** `fix(plan): a task heading without a colon is a task`

### Task VC1

**Goal.** On Linux a run is refused when `$HOME` (or another protected path) is under `/var` or
`/opt`, because those are read grants and Landlock cannot deny inside a grant. Instead, grant
the parts of `/var` and `/opt` that do not hold a protected path, so such a machine works and
the protected paths stay unreadable.

**Files.** `crates/blyte-agent/src/sandbox/grants.rs`, `crates/blyte-agent/src/sandbox.rs`
(only `confine_linux_sandbox`), new `crates/blyte-agent/tests/test_sandbox_linux_var_home.rs`,
`docs/security.md`.

**What to build.**

1. `GrantRules` gets two new fields:
   - `descend_dirs: &'a [PathBuf]`: read grants that may be replaced by their parts;
   - `descend_stop: &'a [PathBuf]`: folders that are never granted and never listed while
     descending.
   Resolve both the same way the other path lists are resolved.

2. A new rule in `confine_grants`, applied to **read** grants only, placed after the carve rule
   and before the final error: a read grant `G` that is in `descend_dirs` and strictly contains a
   protected path is replaced by the result of `descend(G)`:
   - list the direct children of the folder, sorted; a listing error is an `Err` naming the
     folder (`cannot read <folder>: <error>`), including `NotFound` for a nested folder; only a
     top-level `G` that does not exist yields nothing, as the carve rule does;
   - resolve each child with `resolve_protected_prefix`;
   - a child whose resolved path equals a `descend_stop` path, is inside one, equals a protected
     path or is inside one: not granted;
   - a child whose resolved path strictly contains a protected path or a `descend_stop` path: if
     the child is a symbolic link, `Err("on Linux the sandbox cannot protect <p>: <child> is a
     link to a folder that contains it")`; otherwise recurse into it;
   - any other child: granted (push the child path as listed).
   A write grant in `descend_dirs` is not descended: it reaches the existing final error.

3. In `confine_linux_sandbox`: `descend_dirs` is `/var` and `/opt`; `descend_stop` is `$HOME`
   when `HOME` is set and not empty, otherwise empty. Every other existing `GrantRules { .. }`
   literal (the unit tests) gets `descend_dirs: &[]` and `descend_stop: &[]`.

4. `docs/security.md`: the sentence near line 88 that lists `$HOME` under `/var` or `/opt` as
   refused now says that for `/var` and `/opt` the run is granted everything in them except the
   home folder and the protected paths; keep the rest of that paragraph true.

**Tests.**

Unit tests in `grants.rs` (they use temp folders and run on every platform):
- `descend_grants_siblings_and_skips_home`: tree `v/lib/home/.aws`, `v/lib/other`, `v/log`;
  read grant `v`, `descend_dirs = [v]`, `descend_stop = [v/lib/home]`,
  `protected = [v/lib/home/.aws]`. Result read grants are exactly `v/lib/other` and `v/log`
  (resolved or as listed, compare after canonicalising).
- `descend_skips_a_protected_child_outside_home`: tree `v/tokens`, `v/a`; `protected =
  [v/tokens]`, no stop. Result is exactly `v/a`.
- `descend_dir_without_protected_path_is_kept_whole`: read grant `v` with nothing protected in
  it stays `v`.
- `descend_is_not_applied_to_a_write_grant`: write grant `v` containing a protected path is an
  `Err` whose text contains `cannot protect`.
- `descend_refuses_a_link_to_a_folder_holding_a_protected_path` (`#[cfg(unix)]`): two temp
  folders `v` and `outside`; `outside/secret` exists and is the only protected path; `v/link`
  is a symbolic link to `outside`; `v/a` is a plain folder. Read grant `v`,
  `descend_dirs = [v]`, and `protected` also holds `v/tokens` (which exists) so that `v` is
  descended. The result is an `Err` containing `is a link to a folder`.

Linux integration test, new file `crates/blyte-agent/tests/test_sandbox_linux_var_home.rs`,
`#![cfg(target_os = "linux")]`, following `test_sandbox_linux.rs` (copy its `landlock_or_skip`
helper and the way it builds a sandbox and runs a shell command with `HOME` set; if that test
sets `HOME` with a process-wide lock or a child process, do the same):
- `home_under_var_tmp_is_protected_and_var_stays_readable`: make a home folder under `/var/tmp`
  (a unique temp folder there) with `.aws/credentials` holding `SECRET-VAR-HOME`, a sibling
  file `/var/tmp/<unique>-sibling.txt` holding `sibling-ok`, and a workspace under the home
  folder at `<home>/repo`. Build the sandbox the way `test_sandbox_linux.rs` does for its
  "HOME under /tmp gets a private tmp" test. Assert: the sandbox is built (`Ok`); `cat
  <home>/.aws/credentials` fails and its output does not contain `SECRET-VAR-HOME`; `cat` of
  the sibling file prints `sibling-ok`. Remove the sibling file at the end.

Run: `cargo test -p blyte-agent grants` and `cargo test -p blyte-agent --test
test_sandbox_linux_var_home` (the second runs nothing on macOS; it must still compile there,
which the `cfg` attribute gives).

**Constraints.** Existing tests pass unmodified apart from the two new fields in `GrantRules`
literals. Do not change `default_profile`, `default_profile_net` or any other test file.

**Depends on:** none.

**Siblings.** DP1 changes the visibility of `default_profile` and `default_profile_net` in
`sandbox.rs` and the test files that call them; it is not in this worktree. PH1 and UP1 are
elsewhere in `crates/blyte-cli`.

**Commit message:** `feat(sandbox): on Linux a home folder under /var or /opt is supported`

### Task DP1

**Goal.** `blyte_agent::sandbox::default_profile` and `default_profile_net` build a Linux
profile without checking its grants against the protected paths. Since 2026-11-28 every caller
outside tests uses `default_sandbox_net`, which checks. Make the unchecked pair unreachable from
other crates and tests, so nothing new can use them by mistake.

**Files.** `crates/blyte-agent/src/sandbox.rs`, and the test files that call the pair:
`crates/blyte-agent/tests/test_approval.rs`, `test_network_off.rs`, `test_sandbox_linux.rs`,
`test_sandbox_program_folders.rs`, `test_sandbox_read_deny.rs`, `test_ssh_agent_pass_env.rs`,
`test_ssh_agent_sandbox.rs`, `test_write_guard.rs`, and
`crates/blyte-cli/tests/test_conversation_sandbox.rs`. Find any other with
`grep -rn 'default_profile' crates`.

**What to build.**
- `default_profile` and `default_profile_net` become private (`fn`). Remove the doc sentence
  that says they are unchecked on Linux only if it is no longer true; keep it otherwise. If a
  private function is then unused on one platform, give it the matching `#[cfg(...)]`; do not
  use `#[allow(dead_code)]`.
- Add `pub fn default_sandbox(root: &Path, allow_read: &[PathBuf]) -> Result<ConversationSandbox,
  String>` that returns `default_sandbox_net(root, allow_read, true)`.
- In the tests, `default_profile(root, allow)` becomes `default_sandbox(root, allow)` and
  `default_profile_net(root, allow, net)` becomes `default_sandbox_net(root, allow, net)`, each
  unwrapped with `.expect("sandbox")`. Where the test only needs the profile, take `.profile`.
  Where the test passes the profile to a shell or runner that has a way to set the temp folder
  (search the file for `with_tmpdir` or `tmpdir`), pass the sandbox's `tmpdir` too when it is
  `Some`. Test names that contain `default_profile` stay as they are.
- Do the call-site edits by hand or with one `sed` per pattern, then build; there are about
  thirty call sites in nine files.

**Tests.** No new tests: the existing suites are the check. Run
`cargo test -p blyte-agent` and `cargo test -p blyte-cli --test test_conversation_sandbox`.
Then confirm `grep -rn 'default_profile(' crates --include='*.rs' | grep -v 'sandbox.rs'`
prints nothing but test function names.

**Constraints.** No test's assertions change. No behaviour change on macOS.

**Depends on:** none.

**Siblings.** VC1 changes `confine_linux_sandbox` and `sandbox/grants.rs` and adds a new test
file; it is not in this worktree, so do not touch those. PH1 and UP1 are in `crates/blyte-cli`.

**Commit message:** `refactor(sandbox): the unchecked profile builders are private`

### Task UP1

**Goal.** `blyte update` needs the GitHub CLI (`gh`) today, for both reading the latest release
and downloading it, so someone who installed blyte with cargo and has no `gh` cannot update.
Make it update the way blyte was installed, with no GitHub CLI.

**Files.** `crates/blyte-cli/src/update.rs`, `crates/blyte-cli/tests/test_update.rs`,
and any user documentation that says `blyte update` needs
`gh` (`grep -rn 'gh auth\|GitHub CLI' README.md docs --include='*.md'`, ignoring
`docs/superpowers/`, `docs/plans/` and `docs/specs/`). Do not touch `publish.rs`: it uses `gh`
for something else.

**What to build.**

1. **Latest release over HTTPS.** `latest_tag` does `GET <api>/repos/<repo>/releases/latest`
   with `ureq` (already a dependency, with the `json` feature), header `User-Agent: blyte`,
   and reads the string field `tag_name`. `<api>` is `https://api.github.com`, or the value of
   the environment variable `BLYTE_UPDATE_API` when set (the tests use it). Errors:
   a failed request is `could not read the latest release of <repo>: <error>`; a response with
   no string `tag_name` is `the latest release of <repo> has no tag_name`; the existing
   `unexpected release tag` check stays. Use a 30 second timeout.

2. **How blyte was installed.**
   `pub enum InstallMethod { Cargo, Binary }` and
   `pub fn install_method(exe: &Path, cargo_home: Option<&Path>) -> Result<InstallMethod, String>`:
   - `cargo_home` is `None`, or `exe` (canonicalised) is not `<cargo_home>/bin/blyte`
     (canonicalised; a `bin/blyte` that does not exist means "not it"): `Binary`.
   - otherwise read `<cargo_home>/.crates2.json`. Missing file: `Err("blyte is in
     <cargo_home>/bin but <file> is missing, so how it was installed is unknown")`. Unparseable,
     or no `installs` object: an `Err` naming the file.
   - find the key of `installs` that starts with `blyte ` (name, space). None:
     `Err("<file> does not list blyte, so how it was installed is unknown")`.
   - the key ends with `(registry+https://github.com/rust-lang/crates.io-index)` or
     `(sparse+https://index.crates.io/)`: `Cargo`. Any other source:
     `Err("blyte was installed by cargo from <source>; update it with the cargo install command
     you used")`.
   The caller passes `cargo_home` as `$CARGO_HOME` when set and not empty, else `$HOME/.cargo`
   when `HOME` is set and not empty, else `None`.

3. **Update.** After the existing version comparison and `--check` handling:
   - `Cargo`: copy the running binary to `prev_path` as now, then run
     `cargo install blyte --locked` with inherited output. The program is `cargo`, or the value
     of `BLYTE_CARGO` when set (the tests use it). It cannot be started:
     `could not run cargo: <error>`. Non-zero exit: `cargo install blyte --locked failed`.
     Then print the new version with `version_of(&exe)` and continue with the skills refresh,
     as the binary path does.
   - `Binary`: as now, but download
     `<download>/<repo>/releases/download/<tag>/<asset>` with `ureq` into the same temporary
     file, where `<download>` is `https://github.com` or the value of `BLYTE_UPDATE_DOWNLOAD`
     when set. A failed request or a failed write removes the temporary file and is
     `download failed: <error>`. Use a 30 second connect timeout and no overall read timeout
     shorter than 10 minutes. Everything after the download (make executable, check it runs,
     keep the previous build, rename, refresh skills) is unchanged.
   - Remove `gh()` and `BLYTE_GH` from `update.rs`; the messages that tell the user to install
     `gh` go with them.

**Tests.**

Unit tests in `update.rs` for `install_method`, each with a temp folder as cargo home:
- exe elsewhere → `Binary`; `cargo_home` `None` → `Binary`;
- exe at `<home>/bin/blyte`, `.crates2.json` with
  `{"installs":{"blyte 0.74.1 (registry+https://github.com/rust-lang/crates.io-index)":{}}}` → `Cargo`;
- the same with `(sparse+https://index.crates.io/)` → `Cargo`;
- the same with `(git+https://github.com/dnf0/blyte#abc)` → `Err` containing `installed by cargo from`;
- `.crates2.json` missing → `Err` containing `is missing`;
- `.crates2.json` listing only `blyte-agent 0.1.0 (registry+...)` → `Err` containing `does not list blyte`.

`crates/blyte-cli/tests/test_update.rs`: replace the stand-in `gh` with a stand-in HTTP server
on `127.0.0.1:0` (a `std::net::TcpListener` on a thread, answering `GET
/repos/<repo>/releases/latest` with `{"tag_name":"<tag>"}` and `GET
/<repo>/releases/download/<tag>/<asset>` with the stand-in binary's bytes, `Content-Length`
set, `Connection: close`; anything else 404). Pass `BLYTE_UPDATE_API` and
`BLYTE_UPDATE_DOWNLOAD` as `http://127.0.0.1:<port>`. Every existing test in the file keeps its
name and its assertions. Set `CARGO_HOME` to an empty temp folder in those tests so the copy of
blyte under test is a `Binary` install. Add:
- `a_cargo_install_is_updated_with_cargo`: the binary under test is copied to
  `<cargo home>/bin/blyte` with a matching `.crates2.json`; `BLYTE_CARGO` is a script that
  writes its arguments to a file and exits 0; the latest tag is newer. Assert the arguments
  are `install blyte --locked`, the exit code is 0, and the server got no download request.
- `a_failing_cargo_install_is_an_error`: the same with a script that exits 1; the exit code is
  not 0 and the output contains `cargo install blyte --locked failed`.
- `an_unreachable_release_api_is_an_error`: `BLYTE_UPDATE_API` points at a closed port; the
  exit code is not 0 and the output contains `could not read the latest release`.

Run: `cargo test -p blyte-cli update`.

**Constraints.** `--check` and `--rollback` behave as now. Do not change `docs/roadmap.md`. No new dependencies. Other test
files pass unmodified; if another test file sets `BLYTE_GH` for `blyte update`, say so in your
report instead of changing it (`test_publish*.rs` use `gh` for publishing: leave them).

**Depends on:** none.

**Siblings.** PH1, VC1 and DP1 run separately and are not in this worktree.

**Commit message:** `feat(update): update the way blyte was installed, without the GitHub CLI`
