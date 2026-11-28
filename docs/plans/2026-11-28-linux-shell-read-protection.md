# Plan: Linux shell read protection

Spec: `docs/specs/2026-11-28-linux-shell-read-protection.md`. Two tasks, one after the other
(LRP2 calls what LRP1 adds).

### Task LRP1

**Goal.** On Linux, never return a Landlock profile that grants a folder holding a protected path.
Landlock can only allow paths, so today `~/.cargo/credentials*` is readable in every run and all of
`$HOME` is readable when it is under `/tmp`. Write the path rule as plain functions that compile
and are tested on every platform (this machine is macOS; the Linux-only code cannot be run here).

**Depends on:** none. **Siblings:** LRP2 switches the `blyte-cli` callers afterwards; do not change
`crates/blyte-cli` here.

**Files.**
- Create `crates/blyte-agent/src/sandbox/grants.rs` (declared in `sandbox.rs` as `pub mod grants;`).
- Change `crates/blyte-agent/src/sandbox.rs`.
- Add tests to `crates/blyte-agent/tests/test_sandbox_linux.rs` (Linux-only, cannot run here).

**What to build.**

In `sandbox/grants.rs`, compiled on all platforms, no `cfg`:

```rust
pub struct GrantRules<'a> {
    /// Paths the shell must not read (from `protected_read_paths()`).
    pub protected: &'a [PathBuf],
    /// The subset blyte never grants, even with `--allow-read`: `$HOME/.config/blyte/env` and the surface token folder.
    pub own_protected: &'a [PathBuf],
    /// Temp folders that may be dropped and replaced by a private one.
    pub temp_dirs: &'a [PathBuf],
    /// Folders granted by their children when they contain a protected path (`$HOME/.cargo`).
    pub carve_dirs: &'a [PathBuf],
    /// Child names of a carved folder that are not granted for writes.
    pub carve_write_excluded: &'a [&'a str],
    /// The user's `--allow-read` paths.
    pub user_allow_read: &'a [PathBuf],
}

pub struct ConfinedGrants {
    pub write: Vec<PathBuf>,
    pub read: Vec<PathBuf>,
    /// Temp folders removed from the lists, in the order first met.
    pub dropped_temp: Vec<PathBuf>,
}

pub fn confine_grants(
    write: Vec<PathBuf>,
    read: Vec<PathBuf>,
    rules: &GrantRules<'_>,
) -> Result<ConfinedGrants, String>;

pub fn choose_private_tmp(
    candidates: &[PathBuf],
    dropped: &[PathBuf],
    protected: &[PathBuf],
) -> Result<PathBuf, String>;
```

All comparisons are on resolved paths: resolve each side with `super::resolve_protected_prefix`
(it resolves symlinks on the existing prefix and works for paths that do not exist) and compare
with `Path::starts_with`. An error from resolving is returned, not ignored. Say `g` *contains*
`p` when resolved `p` starts with resolved `g` (equal counts), and `g` is *inside* `p` when
resolved `g` starts with resolved `p` (equal counts).

`confine_grants` handles each granted path `g` of each list, keeping order, by the first rule that
applies:

1. `g` neither contains nor is inside any protected path: keep it.
2. `g` is in `user_allow_read` and is inside a protected path: if it is inside one of
   `own_protected`, return
   `Err("--allow-read <g> is refused: it is inside <o>, which a run may never read")`; otherwise
   keep it.
3. `g` is in `temp_dirs` and contains a protected path: drop it from the list and record it in
   `dropped_temp` (once).
4. `g` is in `carve_dirs` and contains a protected path that is not `g` itself: replace it with its
   direct children (`std::fs::read_dir`, sorted by path), skipping any child that contains or is
   inside a protected path, and, in the write list only, any child whose file name is in
   `carve_write_excluded`. If `g` does not exist (`ErrorKind::NotFound`) it is replaced by nothing.
   Any other `read_dir` or entry error returns `Err` naming `g` and the error.
5. Otherwise return
   `Err("on Linux the sandbox cannot protect <p>: granted path <g> contains it")` when `g` contains
   `p`, or `... granted path <g> is inside it` when `g` is inside `p`. `<p>` and `<g>` are the
   paths as given (`display()`), not the resolved ones.

Membership tests (`g` is in `temp_dirs`, `carve_dirs`, `user_allow_read`) compare resolved paths.

`choose_private_tmp` returns `<c>/blyte-<pid>-<n>` for the first candidate `c` that is not in
`dropped` (resolved comparison), neither contains nor is inside a protected path, and is an
existing folder. `<pid>` is `std::process::id()` and `<n>` comes from a process-wide
`AtomicU64` counter, so two calls in one process never return the same path. It does not create the
folder. With no usable candidate it returns
`Err("cannot choose a private temp folder: <first dropped> contains <the protected path beneath it> and no other temp folder is usable")`.

In `sandbox.rs`:

- Add, for all platforms:

  ```rust
  /// Returns the default sandbox profile and optional private `TMPDIR`.
  pub fn default_sandbox_net(
      root: &Path,
      allow_read: &[PathBuf],
      network: bool,
  ) -> Result<ConversationSandbox, String>
  ```

  On macOS it returns `default_profile_net(root, allow_read, network)` with `tmpdir: None`.
  On other platforms it builds `write = default_allowed(root)` and
  `read = read_allowed(home, allow_read)` as `default_profile_net` does, then applies the Linux
  step below with no existing private temp folder.

- In `conversation_sandbox_with_edits`, non-macOS branch: apply the Linux step to its `write` and
  `read` lists with `paths.tmpdir` as the existing private temp folder. The user's `--allow-read`
  paths passed to the rules are `allow_read` only (not `root`, the git folder or `.wt`).

- The Linux step (one private function, `#[cfg(not(target_os = "macos"))]`):
  1. If `$HOME` is set and not empty and `$HOME/.cargo` is a folder, create `$HOME/.cargo/registry`
     and `$HOME/.cargo/git` (`create_dir_all`); a failure returns `Err` naming the folder.
  2. `protected = protected_read_paths()?`; `own_protected` = the resolved `$HOME/.config/blyte/env`
     and `surface_token_dir()` when each is known; `temp_dirs` = `/tmp`, `/var/tmp`, `/private/tmp`,
     `/private/var/folders` and `$TMPDIR` when set and not empty; `carve_dirs` = `$HOME/.cargo`;
     `carve_write_excluded` = `credentials`, `credentials.toml`, `bin`, `config`, `config.toml`,
     `env`, `env.fish`, `env.nu`.
  3. `confine_grants(write, read, &rules)?`.
  4. If `dropped_temp` is not empty and there is no existing private temp folder: choose one with
     `choose_private_tmp` (candidates in order: `$TMPDIR` when set, `/tmp`, `/var/tmp`), create it
     with the existing `create_private_temp_dir`, add it to `write`, and return it as `tmpdir`.
     If there is an existing private temp folder, keep it.
  5. Return `Profile::LandlockConfined { write, read, block_tcp: !network }` and the `tmpdir`.

- Leave `default_profile`, `default_profile_net`, `default_allowed`, `read_allowed` and
  `push_home_grants` as they are. Add one sentence to the doc comments of `default_profile` and
  `default_profile_net`: on Linux they do not check their grants against the protected paths; use
  `default_sandbox_net`.

No fallbacks: nothing here may fall back to granting a folder whole, skipping the check, or
continuing without a temp folder. Every failure is an `Err` naming the paths.

**Tests.** In a `#[cfg(test)] mod tests` in `sandbox/grants.rs`, using `tempfile::tempdir()` for
real folders (they run on macOS and Linux). One test each:

1. `keeps_grants_unrelated_to_protected_paths`: lists come back unchanged, `dropped_temp` empty.
2. `drops_temp_dir_containing_protected_path_from_both_lists`: `tmp` in both lists and in
   `temp_dirs`, protected `tmp/home/.aws` (not existing): `tmp` is in neither list, `dropped_temp`
   is `[tmp]`, other entries kept in order.
3. `errors_when_non_temp_grant_contains_protected_path`: error text contains
   `cannot protect`, the protected path and the granted path, and ends `contains it`.
4. `errors_when_grant_equals_protected_path`: grant `home/.aws` not in `user_allow_read`: `Err`.
5. `errors_when_grant_is_inside_protected_path`: grant `home/.ssh/keys`: error ends `is inside it`.
6. `allow_read_of_protected_path_is_kept`: `home/.aws` in `read` and `user_allow_read`: kept.
7. `allow_read_inside_own_protected_path_is_refused`: `home/.config/blyte/env` in `read`,
   `user_allow_read` and `own_protected`: error contains `--allow-read` and `may never read`.
8. `allow_read_of_ancestor_of_protected_path_is_refused`: `home` in `read` and `user_allow_read`,
   protected `home/.aws`: `Err` ending `contains it`.
9. `carves_folder_into_children_without_protected_ones`: `cargo` with children `registry/`, `git/`,
   `bin/`, `config.toml`, `credentials.toml`, `credentials`; protected both credentials files;
   `cargo` in both lists and `carve_dirs`; `carve_write_excluded = ["bin", "config.toml"]`.
   Read list is exactly `[bin, config.toml, git, registry]` (full paths, sorted); write list is
   exactly `[git, registry]`.
10. `carved_folder_that_does_not_exist_is_dropped`: no error, no entry.
11. `carve_dir_not_containing_protected_path_is_kept_whole`.
12. `symlinked_grant_to_protected_folder_is_refused` (`#[cfg(unix)]`): `link -> home/.aws`, grant
    `link`: `Err`.
13. `choose_private_tmp_skips_dropped_and_protected_candidates`: candidates `[a, b, c]`, `a`
    dropped, `b` contains a protected path, `c` exists: result's parent is `c` and its name starts
    `blyte-<pid>-`.
14. `choose_private_tmp_returns_distinct_paths`: two calls differ.
15. `choose_private_tmp_errors_when_no_candidate_is_usable`: error names the dropped folder and the
    protected path.
16. `choose_private_tmp_skips_missing_candidate`.

In `crates/blyte-agent/tests/test_sandbox_linux.rs`, following that file's existing live Landlock
tests (`landlock_or_skip`, a fake `HOME` inside the real home folder where the file already does
that):

17. `landlock_default_and_conversation_sandboxes_cannot_read_cargo_credentials`: with
    `HOME/.cargo/credentials.toml`, `HOME/.cargo/credentials` and `HOME/.cargo/registry/x` present,
    a shell under `default_sandbox_net` and one under `conversation_sandbox_net` fail to `cat`
    either credentials file and succeed on `registry/x`.
18. `landlock_default_sandbox_with_home_under_tmp_gets_private_tmp`: fake `HOME` under `/tmp` with
    `.aws/credentials`: the result has `tmpdir: Some(_)` outside `/tmp`; with `TMPDIR` set to it,
    `mktemp -d` succeeds and `cat "$HOME/.aws/credentials"` fails. Remove the private folder at the
    end.
19. `default_and_conversation_sandboxes_refuse_root_containing_protected_path`: `root` = the fake
    `HOME`: both builders return `Err` containing `cannot protect`.

Run: `cargo test -p blyte-agent sandbox::grants` and the repository checks. Tests 17 to 19 are
compiled only on Linux; check them by reading against the file's existing tests.

**Constraints.** Existing tests pass unmodified. No change in behaviour on macOS. No fallbacks.

**Commit message:** `fix(sandbox): on Linux a run's shell is not granted folders that hold credentials`

### Task LRP2

**Goal.** Make every run of the `blyte` binary use the checked sandbox from LRP1, clean up its
private temp folder, refuse an in-place dispatch in a folder that holds credentials before locking
it, and document the result.

**Depends on:** LRP1 (`blyte_agent::sandbox::default_sandbox_net` returning
`Result<ConversationSandbox, String>` with fields `profile` and `tmpdir`).

**Files.** `crates/blyte-cli/src/task_runner.rs`, `crates/blyte-cli/src/report.rs`,
`crates/blyte-cli/src/init.rs`, `crates/blyte-cli/src/dispatch_in_place.rs`, `docs/security.md`.

**What to build.**

1. `task_runner.rs`, `build_sandbox_for_root`: replace the `default_profile_net` call with
   `default_sandbox_net(...)?` and return its `profile` and `tmpdir` (today that branch returns
   `None`).
2. `task_runner.rs`, `run_task_with_event_callback`: the guard `_in_place_tmp_guard` takes the
   `tmpdir` whenever it is `Some`, not only when `config.in_place.is_some()`. Rename it
   `_private_tmp_guard`. Check every other caller of `build_sandbox_for_root` and
   `build_local_shell_with_tmpdir`: where a `tmpdir` is returned and the shell is built there, the
   shell gets it through `with_tmpdir`, and it is removed when the run ends.
3. `report.rs` (the `default_profile_net` call near line 299) and `init.rs` (near line 1667): call
   `default_sandbox_net`. An `Err` is reported the way that function already reports a sandbox
   failure (in `init.rs`, return the error to the caller with the function's existing error type);
   it is never swallowed. Where a shell is built from the profile, pass `tmpdir` with
   `with_tmpdir` and remove the folder afterwards. In `report.rs`, if the profile is only
   described and no shell is built, remove a returned `tmpdir` straight away.
4. `dispatch_in_place.rs`, in `run`, under `#[cfg(target_os = "linux")]`, for a run that is not
   `--research`: call
   `crate::serve::check_linux_non_repo_root_for_env(&root, std::env::var_os("HOME").as_deref())`
   before the folder lock is taken and before `.start.json` is written, and return its error
   through the function's existing error path. Make the function `pub(crate)` if it is not.
5. `docs/security.md`:
   - In "Credential reads", replace the description of the Linux allow-list so it says: `~/.cargo`
     is granted by its children, without `credentials` and `credentials.toml`; a temp folder
     (`/tmp`, `/var/tmp`, `$TMPDIR`) that contains a protected path is not granted and the run gets
     a private `TMPDIR`; a run whose workspace, `--allow-read` path or system folder contains a
     protected path does not start, with the error
     `on Linux the sandbox cannot protect <path>: granted path <path> contains it`
     (for example a workspace at `$HOME`, or `$HOME` under `/var` or `/opt`).
   - Add the remaining limit in the same section: on Linux a `.env` file in the workspace is
     readable by shell commands, because Landlock cannot deny a file inside the granted workspace;
     blyte's own `read`, `glob` and `grep` tools refuse it on both platforms.
   - In the writes section's list of subpaths that "remain writable on Linux", remove
     `~/.cargo/bin`, `~/.cargo/config*`, `~/.cargo/credentials*` and `~/.cargo/env*` and say they
     are not writable there since this change.

No fallbacks: no branch may fall back to `default_profile_net` or to an unsandboxed shell when
`default_sandbox_net` fails.

**Tests.** The behaviour is Linux-only; on Linux
`research_run_refuses_reading_aws_credentials` (in
`crates/blyte-cli/tests/test_dispatch_in_place.rs`, unmodified) is the test that must pass. Add to
that file, under `#[cfg(target_os = "linux")]`, following `TestEnv` and the existing tests there:

- `in_place_dispatch_in_home_is_refused_on_linux`: a non-research in-place dispatch whose folder
  is `HOME` exits non-zero, stderr contains `on Linux` and `.ssh` or another protected path's
  name, and the folder has no lock file and no `.start.json` afterwards.

After the change, `grep -rn 'default_profile' crates/blyte-cli/src` prints nothing.

**Constraints.** Existing tests pass unmodified. No change in behaviour on macOS.

**Commit message:** `fix(sandbox): every Linux run uses the checked sandbox, and an in-place run refuses a folder that holds credentials`
