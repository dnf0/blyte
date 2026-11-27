# Runs outside a git repository, working in place

## Why

A surface started in a folder that is not a git repository has no runs, so the conversation does
every piece of work itself and the chat is blocked while it does. Runs must be available there so
the conversation can hand work off. The user chose the in-place model: the run edits the folder
itself; there is no copy and no worktree.

## Decisions

1. **Where records go.** A run in a non-repository folder writes nothing of its own into that
   folder. Its records (start, log, report, summary, saved originals) go to the folder's store,
   `<state dir>/folders/<key>/runs/`, next to the surface records already kept there. Task files go
   to `<state dir>/folders/<key>/tasks/`. In a git repository nothing changes (`<root>/.wt`).
2. **One writer per folder.** An in-place run holds an exclusive lock on
   `<state dir>/folders/<key>/in-place.lock` (`flock`, released by the kernel if the process dies).
   A second in-place run, or an approved write from the chat, fails with an error naming the
   holder. Research runs only read the folder and do not take the lock.
3. **Edits go through the write and edit tools only.** In an in-place run the shell cannot write
   the folder (it keeps temporary directories and tool caches, as the conversation's shell does).
   This is what makes every change recorded: the original of each file is saved before its first
   overwrite, so review shows a real diff and a run can be undone. It also means the folder is
   never in the shell sandbox's writable list, so starting in the home folder does not open the
   home folder to shell commands. Cost: a formatter or generator run from the shell cannot change
   files in the folder; the run must write the result with the tools.
4. **Protected paths are refused for writing in every run**, git or not: credential stores, any
   `.env`, shell start-up files, blyte's own configuration and state, git configuration, places
   that run things at login, and the personal `bin` folders on `PATH`. No option turns this off.
5. **No gate is needed.** A non-repository folder usually has no check. An in-place run with no
   gate ends as `finished` (files changed) or `no_changes`; it is never called `verified`. A run
   with a gate keeps today's outcomes.
6. **Research runs work in non-repository folders too**, with their scratch folder in the store.
7. **The conversation hands off.** In a non-repository folder the `blyte` tool allows `dispatch`,
   `wait`, `status`, `review` and `clean`; direct edits, plans, combining and publishing still
   need a repository. The prompt tells the model to hand off anything longer than a quick look-up.

No fallbacks anywhere: a missing state directory, an unreadable manifest, a snapshot that cannot
be saved or a lock that cannot be opened is an error that names what is missing, never a silent
skip.

## Order

IC1, IC2 and IC3 are independent and run in parallel. IC4a (the child run) needs all three; IC4b (dispatch) needs IC4a; IC4c (research runs) needs
IC4b. IC5 (review, the surface, the `blyte` tool and the prompts) needs IC4. IC4 and IC5 are outlined
here and get their full task text once IC1 to IC3 have merged, against the code as it then stands.

---

### Task IC1: where a folder's runs, tasks and lock live

**Depends on:** none. **Siblings:** IC2 adds `crates/blyte-agent/src/write_guard.rs`; IC3 adds
`snapshot_fs.rs` and `folder_lock.rs` in the same crate. They are not in this worktree; do not add
them.

**Files:** `crates/blyte-cli/src/surface_dir.rs`, `crates/blyte-agent/src/sandbox.rs`,
`crates/blyte-cli/tests/test_surface_dir.rs` (extend it if it exists, else create it),
`crates/blyte-agent/tests/test_sandbox.rs`.

**What to build**

1. In `crates/blyte-agent/src/sandbox.rs`, `state_blyte_dir()` returns `$BLYTE_STATE_DIR` when it
   is set and not empty, before looking at `XDG_STATE_HOME` and `HOME` (the same order
   `crates/blyte-cli/src/registry.rs` uses). Today it ignores `BLYTE_STATE_DIR`, so the macOS
   sandbox does not protect a state directory set that way.
2. In `crates/blyte-cli/src/surface_dir.rs`, next to `folder_store`:
   - `pub fn run_store(state_dir: &Path, root: &Path) -> Result<PathBuf, String>`: when
     `blyte_agent::sandbox::is_git_repository(root)` it returns `root.join(".wt")` and creates
     nothing; otherwise `folder_store(state_dir, root)?.join("runs")`, created with mode 0700.
   - `pub fn tasks_dir(state_dir: &Path, root: &Path) -> Result<PathBuf, String>`: `.wt/tasks`
     under `root` for a repository (creates nothing); otherwise `<folder store>/tasks`, created
     0700.
   - `pub fn in_place_lock_path(state_dir: &Path, root: &Path) -> Result<PathBuf, String>`:
     `<folder store>/in-place.lock` (the file is not created). For a git repository it returns
     `Err` saying that in-place runs are for folders that are not git repositories, naming `root`.
   Every error names the path it could not create or resolve. No function writes anything inside a
   non-repository `root`.

**Tests** (`cargo test -p blyte --test test_surface_dir`, `cargo test -p blyte-agent --test test_sandbox`)

- `run_store_of_a_repository_is_its_wt_folder`: `git init` a temp dir; result is `<root>/.wt`;
  `.wt` was not created; the state dir is still empty.
- `run_store_of_a_plain_folder_is_in_the_state_directory`: result is under
  `<state>/folders/<key>/runs`, exists, mode 0700; the plain folder's listing is unchanged.
- `two_plain_folders_get_different_run_stores`.
- `tasks_dir_of_a_plain_folder_is_in_the_state_directory` and
  `tasks_dir_of_a_repository_is_wt_tasks`.
- `in_place_lock_path_is_refused_for_a_repository` (error names the root) and
  `in_place_lock_path_of_a_plain_folder_is_in_its_store` (file does not exist afterwards).
- `run_store_of_a_missing_folder_is_an_error_naming_it`.
- In blyte-agent: `state_blyte_dir_prefers_blyte_state_dir` (set `BLYTE_STATE_DIR`,
  `XDG_STATE_HOME` and `HOME` to three different temp dirs; the result is the first), and
  `an_empty_blyte_state_dir_is_ignored`. Serialise environment changes the way the file's existing
  tests do.

**Constraints:** existing tests pass unmodified. No fallbacks.

**Commit message:** `feat(runs): a folder that is not a repository has a run store in the state directory`

---

### Task IC2: protected paths are refused for writing in every run

**Depends on:** none. **Siblings:** IC1 changes `sandbox.rs` and `surface_dir.rs`; IC3 adds
`snapshot_fs.rs` and `folder_lock.rs`. Not in this worktree; do not add them.

**Files:** `crates/blyte-agent/src/write_guard.rs` (new), `crates/blyte-agent/src/lib.rs` (one
`pub mod write_guard;` line), `crates/blyte-agent/src/tools.rs`,
`crates/blyte-agent/tests/test_write_guard.rs` (new).

**Why:** `check_write_scope` returns `Ok` at once when there is no write scope, which is every
background run. So the `write` and `edit` tools, which run outside the command sandbox, can today
write anywhere under the run's root, including blyte's own state when the root contains it.

**What to build**

1. `write_guard.rs`:
   `pub fn protected_write(real_target: &Path, home: &Path, state_dir: Option<&Path>) -> Option<&'static str>`.
   It returns a short reason when `real_target` (already resolved through symlinks, absolute) is
   one of these, or lies under one that is a folder, and `None` otherwise:
   - every path of `crate::sandbox::read_denied(home)` → `"a credential store"`;
   - any path whose file name is exactly `.env` → `"an environment file"`;
   - under `home`: `.bashrc`, `.bash_profile`, `.bash_login`, `.bash_logout`, `.profile`, `.zshrc`,
     `.zprofile`, `.zshenv`, `.zlogin`, `.zlogout`, `.cshrc`, `.tcshrc`, `.kshrc`, `.config/fish`
     → `"a shell start-up file"`;
   - `home/.config/blyte`, and `state_dir` when given → `"blyte's own files"`;
   - `home/.gitconfig`, `home/.config/git` → `"git configuration"`;
   - `home/Library/LaunchAgents`, `home/.config/systemd`, `home/.config/autostart` →
     `"a place that runs programs at login"`;
   - `home/.cargo/bin`, `home/.local/bin`, `home/go/bin`, `home/.bun/bin`, `home/.deno/bin` →
     `"a folder of programs on PATH"`;
   - `home/.npmrc`, `home/.pgpass`, `home/.my.cnf`, `home/.vault-token`, `home/.claude` →
     `"a credential store"`.
   Matching is by path components (`starts_with`), so `home/.sshx` is not protected.
2. In `tools.rs`, the `write` and `edit` tools call the guard before `check_write_scope`, in every
   mode, only when `fs.is_local()`. The target is resolved the way `check_write_scope` resolves it
   (`resolve_real_target`), so a symlink in the workspace that points at a protected path is
   refused. `home` comes from `$HOME`; when `HOME` is unset or empty the tool returns an error
   saying it cannot check protected paths without `HOME` (no fallback). `state_dir` is
   `crate::sandbox::state_blyte_dir()`. The refusal is a tool error, exactly:
   `<tool>: <path as the model gave it> is protected (<reason>); blyte does not write there`.
   Nothing is written.

**Tests** (`cargo test -p blyte-agent --test test_write_guard`), using a temp dir as `HOME` and a
workspace root equal to that home (the worst case):

- `each_protected_kind_is_refused`: one path per bullet above; the reason matches.
- `a_file_under_a_protected_folder_is_refused` (`.ssh/config`, `.config/blyte/allow-unsandboxed`).
- `a_similar_name_is_not_protected` (`.sshx/a`, `.envrc`, `notes/.bashrc.txt`, `bin/tool`).
- `the_state_directory_is_protected_when_given_and_not_when_absent`.
- `write_tool_refuses_a_protected_path_in_a_background_run`: no write scope set; `write` to
  `.zshrc` is a tool error with the exact text; the file does not exist afterwards.
- `edit_tool_refuses_a_protected_path`: an existing `.gitconfig` is unchanged.
- `a_symlink_to_a_protected_path_is_refused`: `link -> .ssh/authorized_keys`; `write` to `link`
  is refused and the target is unchanged.
- `an_ordinary_file_is_still_written` (`notes.txt` under the home root).
- `a_dotenv_in_a_repository_worktree_is_refused` (root is a different temp dir; target `.env`).
- `without_home_the_write_is_an_error_naming_home`.

**Constraints:** existing tests pass unmodified (if one writes a now-protected path, say so in the
final message and do not weaken the guard). No fallbacks.

**Commit message:** `fix(tools): the write and edit tools refuse protected paths in every run`

---

### Task IC3: saved originals and the folder lock

**Depends on:** none. **Siblings:** IC1 changes `sandbox.rs` and `surface_dir.rs`; IC2 adds
`write_guard.rs` and changes `tools.rs`. Not in this worktree; do not add them.

**Files:** `crates/blyte-agent/src/snapshot_fs.rs` (new), `crates/blyte-agent/src/folder_lock.rs`
(new), `crates/blyte-agent/src/lib.rs` (two `pub mod` lines),
`crates/blyte-agent/tests/test_snapshot_fs.rs` (new),
`crates/blyte-agent/tests/test_folder_lock.rs` (new). Nothing uses these yet; IC4 wires them in.

**What to build**

1. `snapshot_fs.rs`: `pub struct SnapshotFs<F: Fs>` with
   `pub fn new(inner: F, root: PathBuf, before_dir: PathBuf, manifest: PathBuf) -> Result<Self, String>`
   (`root` is canonicalised; `before_dir` is created; an existing manifest is read so a resumed
   run does not snapshot a path twice). It implements `Fs` by delegating, except `write`:
   - the target must be inside `root` after resolving symlinks in its existing ancestors;
     otherwise `Err` naming the path;
   - on the first write to a path in this manifest: if the file exists, its bytes (read as bytes,
     so binary files work) are copied to `before_dir/<path relative to root>`, and one line
     `{"path":"<relative>","existed":true}` is appended to the manifest and flushed; if it does
     not exist, `{"path":"<relative>","existed":false}` is appended. Only then is the inner
     `write` called. If saving the original or appending the line fails, the write is refused with
     an error naming the path and the cause, and the file is left untouched;
   - later writes to the same path add nothing.
   `is_local`, `read` and `canonicalize` delegate.
   Also:
   - `pub struct Change { pub path: String, pub existed: bool }` and
     `pub fn read_manifest(manifest: &Path) -> Result<Vec<Change>, String>` (a missing manifest is
     an empty list; a line that does not parse is an error naming the line number);
   - `pub fn changed_files(root: &Path, before_dir: &Path, manifest: &Path) -> Result<Vec<String>, String>`:
     the manifest's paths whose current bytes differ from the saved original (or that were created
     and exist now, or existed and are gone now), sorted; a rewrite with identical content is not
     listed; a saved original that is missing is an error;
   - `pub fn unified_diff(root: &Path, before_dir: &Path, manifest: &Path) -> Result<String, String>`:
     for each changed file, a unified diff with headers `--- a/<path>` and `+++ b/<path>`
     (`/dev/null` for the missing side), produced by running `diff -u` with `--label`; exit
     status 1 from `diff` is success; any other failure is an error. Files that are not valid
     UTF-8 on either side appear as the single line `Binary files a/<path> and b/<path> differ`.
2. `folder_lock.rs`:
   - `pub struct Holder { pub pid: u32, pub kind: String, pub id: String }` (serde);
   - `pub fn acquire(lock_path: &Path, holder: &Holder) -> Result<FolderLock, String>`: opens (and
     creates) the file, takes `flock(LOCK_EX | LOCK_NB)` as `gate_slots.rs` does, then truncates
     the file and writes the holder as one line of JSON. If the lock is held it returns an error,
     exactly `<kind> <id> (pid <pid>) is already working in this folder`, from the holder written
     in the file; if the file cannot be read as a holder the error says the folder is locked and
     names the lock file. The lock is released when `FolderLock` is dropped or the process exits;
   - `pub fn holder(lock_path: &Path) -> Result<Option<Holder>, String>`: `None` when the file is
     missing or nobody holds the lock (it takes and releases the lock to find out, without
     changing the file's content); `Some` when it is held.

**Tests** (`cargo test -p blyte-agent --test test_snapshot_fs --test test_folder_lock`)

- `the_original_is_saved_before_the_first_overwrite`: bytes in `before_dir` equal the old content;
  the manifest has one line with `existed: true`.
- `a_new_file_is_recorded_as_not_existing` (nothing in `before_dir`).
- `a_second_write_keeps_the_first_original` (still one manifest line; saved bytes are the first).
- `a_binary_original_is_saved_byte_for_byte` (write the file with `std::fs` first, bytes
  `[0, 159, 146, 150]`).
- `a_write_outside_the_root_is_refused` and `a_write_through_a_symlinked_folder_out_of_the_root_is_refused`.
- `when_the_original_cannot_be_saved_the_file_is_untouched`: make `before_dir` read-only (mode
  0500); the write is `Err` naming the path; the file keeps its old content.
- `a_reopened_manifest_does_not_snapshot_again`: second `SnapshotFs::new` on the same manifest.
- `changed_files_leaves_out_an_identical_rewrite`, `changed_files_lists_created_and_deleted_files`,
  `a_missing_saved_original_is_an_error`, `a_corrupt_manifest_line_is_an_error_naming_the_line`.
- `the_diff_shows_old_and_new_lines`: contains `--- a/notes.txt`, `+++ b/notes.txt`, `-old`, `+new`.
- `the_diff_of_a_created_file_is_against_dev_null`.
- `a_binary_change_is_one_line`.
- Lock: `a_second_acquire_is_refused_naming_the_holder` (exact text),
  `the_lock_is_free_after_the_guard_is_dropped`, `holder_is_none_for_a_missing_file`,
  `holder_is_none_when_nobody_holds_it_and_the_file_is_unchanged`,
  `holder_reports_who_holds_it`, and `the_lock_is_released_when_the_holding_process_dies` (spawn a
  child with `std::process::Command` that takes the lock with `flock` from Python or a small
  helper and is then killed; `acquire` then succeeds).

**Constraints:** no new dependencies. No fallbacks. Existing tests pass unmodified.

**Commit message:** `feat(agent): saved originals of overwritten files and a one-writer folder lock`

---

### Task IC4a: a run that edits a folder in place (the child run)

**Depends on:** IC1, IC2, IC3 (all on this branch). **Siblings:** none running. IC4b (the
`dispatch` side) comes after this task and is **not** in this worktree: do not change
`dispatch.rs`, `wait.rs` or `clean.rs` here.

**Goal.** Give the child run (`blyte <prompt> --gate ...`, the process `blyte dispatch` spawns;
`crates/blyte-cli/src/task_runner.rs`) a mode in which it edits the folder it runs in, with every
change recorded and the shell unable to write that folder. **Why:** a folder that is not a git
repository has no worktree to isolate a run, so the record of saved originals is the only way to
review or undo what a run did.

**Files.**
- `crates/blyte-cli/src/cli.rs`: two hidden flags on the top-level `Cli` (next to `read_roots`,
  `no_gate_slot`): `--snapshot-dir <DIR>` (`snapshot_dir: Option<PathBuf>`) and
  `--snapshot-manifest <FILE>` (`snapshot_manifest: Option<PathBuf>`). Each `requires` the other;
  both `conflicts_with` `container` and `converse`.
- `crates/blyte-cli/src/task_runner.rs`: `TaskConfig` gains
  `in_place: Option<InPlace>` where `pub struct InPlace { pub before_dir: PathBuf, pub manifest: PathBuf }`,
  filled from the two flags.
- `crates/blyte-cli/tests/test_in_place_child.rs` (new).
- Any other place that builds a `TaskConfig` literal gets `in_place: None`.

**What to build.** When `config.in_place` is `Some`:
1. **File system.** The `Fs` the tools use is
   `blyte_agent::snapshot_fs::SnapshotFs::new(LocalFs, config.root.clone(), before_dir, manifest)`.
   If `new` fails, the run fails before the first model call with that error (no fallback to
   `LocalFs`).
2. **Shell sandbox.** `build_sandbox_for_root` returns the same profile as the `config.converse`
   branch (`blyte_agent::sandbox::conversation_sandbox_net(root, &config.allow_read, network)`),
   with its private `TMPDIR`: the shell can read the folder and cannot write it. If
   `config.sandbox != "on"` or `blyte_agent::sandbox::available()` is false, the run fails before
   the first model call with `in-place runs need the sandbox: <what is missing>`; an in-place run
   never runs with an unsandboxed shell.
3. **Rollback** is off whatever `--rollback` says (there is no commit to roll back to).
4. **Outcome with no gate.** With an empty gate the runner returns `Outcome::Finished`
   (`crates/blyte-agent/src/run.rs`). Make sure the report file written by `--report` then carries
   the outcome string `finished` (find how the report maps outcomes today; if `Finished` already
   maps to `finished`, change nothing and say so in your final message).
5. Nothing else changes: turn and time budgets, the gate when one is given, the log.

The write guard from IC2 (`write_guard::protected_write`) already applies to these runs through
`tools.rs`; do not add a second check.

**Tests** (`crates/blyte-cli/tests/test_in_place_child.rs`; use the scripted-model approach the
existing child-run tests use, for example `tests/test_report_timing.rs`; run with
`cargo test -p blyte --test test_in_place_child`):
- a run whose scripted model overwrites `a.txt` and creates `sub/b.txt` with the write tool:
  both files have the new content, `<before_dir>/a.txt` holds the old content, the manifest lists
  `a.txt` with `existed: true` and `sub/b.txt` with `existed: false`, and the report's outcome is
  `finished`;
- a run whose scripted model runs `echo x > a.txt` in the shell: `a.txt` is unchanged and the
  manifest is empty (skip this test, with a printed reason, only where
  `blyte_agent::sandbox::available()` is false);
- with `--sandbox off` and the two flags: the process exits non-zero and its error contains
  `in-place runs need the sandbox`, and no model call was made;
- `--snapshot-dir` without `--snapshot-manifest` is a usage error; so is either with `--converse`;
- a second run with the same `--snapshot-dir` and `--snapshot-manifest` that overwrites `a.txt`
  again leaves `<before_dir>/a.txt` holding the content from before the *first* run (resumable).

**Constraints.** No fallbacks: every missing or failing piece is an error that names it.
Resumable: nothing is held in memory until the end; the manifest and saved originals are written
as the run goes (IC3 does this; do not buffer). Existing tests pass unmodified; if one cannot,
stop and report it instead of editing it. No home-directory paths in code or tests.

**Commit message:** `feat(run): a run can edit its folder in place with every change recorded`

### Task IC4b: `blyte dispatch` in a folder that is not a git repository

**Depends on:** IC4a. **Siblings:** none running. IC5 (review, undo, the surface, the `blyte`
tool, prompts) and IC4c (research runs) come later and are **not** in this worktree: do not
change `queue_actions.rs`, `serve.rs`, `clean.rs`, `surface_notify.rs`, the agent crate or any
asset.

**Goal.** `blyte dispatch <id> <task.md>` works when the current directory is not inside a git
repository: the run edits that folder in place, one run at a time, and its records live in the
folder's run store. `blyte status <id>` and `blyte wait` find such a run. **Why:** the surface
can then hand work off in any folder instead of doing everything in the chat.

**Files.**
- `crates/blyte-cli/src/dispatch_in_place.rs` (new) and its `mod` line.
- `crates/blyte-cli/src/dispatch.rs`: only (a) the branch at the top of `run_dispatch`, (b)
  `exit_code`, (c) making existing private helpers `pub(crate)` so the new module can call them
  (`run_implementer_pass`, `ImplementerConfig`, the summary writer, the task-text loader for
  `--plan`/`--plan-task`), (d) `run_status`.
- `crates/blyte-cli/src/wait.rs`.
- `crates/blyte-cli/tests/test_dispatch_in_place.rs` (new).

**What to build.**

1. **The branch.** In `run_dispatch`, when `git rev-parse --show-toplevel` runs and exits
   non-zero, call `crate::dispatch_in_place::run(args)` and return its code, instead of printing
   `error: not a git repository`. `git` missing stays the error it is today. Inside a git
   repository nothing changes.

2. **`pub fn records_dir_for(cwd: &Path) -> Result<PathBuf, String>`** in the new module: inside
   a git repository, `<main checkout>/.wt` (from `crate::clean::find_repo_root`); otherwise
   `crate::surface_dir::run_store(&state_dir, &canonical cwd)`, with the state directory from
   `crate::surface_registry::state_dir()`. An unresolvable state directory is an error.
   `run_status` and `wait.rs` use it in place of their own `find_repo_root()` + `.wt`.

3. **`pub fn run(args: &DispatchArgs) -> i32`**, in this order. Every refusal prints
   `error: <reason>` to stderr and returns 2.
   - Root is the canonical current directory. Refuse `--research` with
     `research runs in a folder that is not a git repository are not supported yet`. Refuse
     `--base` other than its default `HEAD` with `--base needs a git repository`.
   - Store: `run_store`, created if absent. Records are `<store>/<id>.task.md`, `.start.json`,
     `.log`, `.report.json`, `.json`, `.changes.json`, `.diff` and the directory `<store>/<id>.before/`.
   - If `<store>/<id>.json` exists: refuse with `run <id> has already finished here; use a new id`.
     If `<store>/<id>.start.json` exists without it, this is a restart of an interrupted run:
     carry on with the same records (the manifest and saved originals are kept; the log is
     appended to).
   - Lock: `blyte_agent::folder_lock::acquire(&in_place_lock_path, &Holder { pid, kind: "run", id })`.
     On failure refuse with the lock's error, which names the holder. Hold it until the summary
     is written.
   - Gate: `--gate` when given; otherwise the `[dispatch].gate` of a `blyte.toml` in the root when
     that file exists; otherwise no gate (empty). A `blyte.toml` that exists and does not parse is
     an error. No comment step, base check, setup command, fix round, model review, passed-tree
     record or background clean.
   - Task text: as today (`<task.md>` or `--plan`/`--plan-task`), wrapped with in-place rules in
     place of `DISPATCH_RULES`. The rules say: you are working directly in the user's folder, not
     a copy; change files only with the write and edit tools, because shell commands cannot write
     this folder; there is no git here, so do not commit; change only what the task needs; finish
     with a short report of what you changed.
   - Start record `<id>.start.json`: the fields an ordinary run writes, with `"in_place": true`,
     `"worktree"` = the root, `"branch": ""`, `"base": null`, `"base_sha": null`.
   - Child: `run_implementer_pass` with the root as its working directory and
     `--snapshot-dir <store>/<id>.before --snapshot-manifest <store>/<id>.changes.json --rollback=false`.
   - After the child: `changed_files` from
     `blyte_agent::snapshot_fs::changed_files(root, before_dir, manifest)`; write
     `blyte_agent::snapshot_fs::unified_diff(...)` to `<store>/<id>.diff`. Either failing is an
     `error` outcome whose `error` field carries the reason.
   - Outcome, from the child report's outcome and `changed_files`:

     | report | gate | files changed | outcome |
     |---|---|---|---|
     | `finished` | none | yes | `finished` |
     | `finished` | none | no | `no_changes` |
     | `verified` | given | yes | `verified` |
     | `verified` | given | no | `no_changes` |
     | `gate_failed`, `budget_exhausted`, `error` | any | any | the same |
     | anything else | any | any | `error`, naming the unexpected value |

     `exit_code("finished")` is 0.
   - Summary `<id>.json`: a `DispatchSummary` with `worktree` = the root, `branch` = `""`,
     `commits` empty, `changed_files` as above, `gate_source` = `"flag"`, `"config"` or `"none"`
     (use the existing strings for the first two), printed to stdout as today.
   - Do not call `surface_notify` (IC5 does that once it can find a non-repository surface).

**Tests** (`crates/blyte-cli/tests/test_dispatch_in_place.rs`; scripted model as in
`tests/test_dispatch_research.rs`; every test sets `BLYTE_STATE_DIR` to its own temporary
directory and runs in its own temporary folder with no `.git`; run with
`cargo test -p blyte --test test_dispatch_in_place`):
- a run that overwrites `a.txt`: exit 0, summary outcome `finished`, `changed_files == ["a.txt"]`,
  `branch == ""`, `<store>/T1.diff` contains both the old and the new line, and the folder
  contains no `.wt` directory and no file the run did not write;
- a run that changes nothing: outcome `no_changes`, exit 4;
- `--gate 'test -f made.txt'` with a model that creates `made.txt`: outcome `verified`;
- a second dispatch while the lock is held by a live process (take the lock in the test with
  `folder_lock::acquire`): exit 2, stderr names the holder's id, and no start record is written;
- the same id after it finished: exit 2, `already finished here`;
- `--research`: exit 2 with the message above; `--base main`: exit 2;
- `blyte status T1` in that folder after the run prints `outcome: finished`;
- `blyte wait T1` in that folder prints the finished line for `T1`;
- in a git repository, `records_dir_for` returns `<root>/.wt` (and the existing dispatch tests
  still pass unmodified).

**Constraints.** No fallbacks. Resumable, as described under "a restart of an interrupted run".
Existing tests pass unmodified; if one cannot, stop and report it instead of editing it. No
home-directory paths in code or tests.

**Commit message:** `feat(dispatch): dispatch runs in place in a folder that is not a git repository`

### Task IC4c: research runs in a folder that is not a git repository

**Depends on:** IC4b (on this branch). **Siblings:** IC5a (review and undo) runs at the same time
and is **not** in this worktree: do not add `blyte review` or `blyte undo` behaviour, do not write
`<id>.after/`, and do not change `queue_actions.rs`. IC5b (the surface, the `blyte` tool, the
prompts) comes later: do not change `serve.rs`, the agent crate or any asset.

**Goal.** `blyte dispatch <id> <question.md> --research` works when the current directory is not
inside a git repository: the run reads that folder, cannot change it, and its report lands in the
folder's run store. **Why:** a surface in a plain folder needs to hand off questions about it, not
only edits.

**Files.**
- `crates/blyte-cli/src/dispatch_in_place.rs`.
- `crates/blyte-cli/src/dispatch.rs`: only making existing private research helpers `pub(crate)`
  (the scratch-directory creation, the research prompt wrapper and source block, the report copy,
  the research defaults for gate and budgets). If a helper is inline in `run_dispatch`, move it
  into a function without changing what it does.
- `crates/blyte-cli/tests/test_dispatch_in_place.rs` (add tests; change the existing
  `--research` refusal test, which this task makes obsolete, and no other existing test).

**What to build.** In `dispatch_in_place::run`, `--research` is no longer refused. A research run:
- takes **no folder lock** (it cannot write the folder), so it runs beside an in-place run and
  beside other research runs;
- still refuses `--base` other than `HEAD`, and refuses an id whose `<store>/<id>.json` exists;
- works in the scratch directory `<store>/<id>/`, created exactly as an ordinary research run
  creates `.wt/<id>` today (same helper);
- uses the research gate, gate source, time and turn defaults an ordinary research run uses;
- starts the child with the flags an ordinary research run passes (`--read-root <root>`,
  `--no-gate-slot`, `--research`, and the claim check as today), with **no** `--snapshot-dir` or
  `--snapshot-manifest`;
- wraps the question with the existing research wrapper and source block, plus one added
  sentence: `This folder is not a git repository: cite every file by its full path.` (the quote
  check finds files by the tail of a path only in a git repository);
- copies the report to `<store>/<id>.report.md` and to `--report-to` as an ordinary research run
  does, including the banner on a report that did not pass;
- writes the start record and summary as an ordinary research run does, with `worktree` = the
  scratch directory, `branch` = `""`, `"in_place": false`, `changed_files` and `commits` empty;
- takes its outcome from the child report directly, as an ordinary research run does;
- restarts as IC4b's in-place run does: a start record without a summary means carry on with the
  same records and scratch directory.

The run must not be able to read more of the folder than the surface's conversation can: the
credential read-deny list applies to the read tool and the shell as it does for an ordinary
research run. Do not widen it.

**Tests** (`cargo test -p blyte --test test_dispatch_in_place`; scripted model as the existing
tests in that file; each in its own temporary folder with no `.git` and its own
`BLYTE_STATE_DIR`):
- a research run whose scripted model reads `notes.txt` by full path and writes `report.md`
  quoting a line of it: exit 0, outcome `verified`, `<store>/R1.report.md` holds the report, the
  folder contains no new file or directory, and `changed_files` is empty;
- a research run whose scripted model tries to write `<root>/x.txt` with the write tool: the
  write is refused and `x.txt` does not exist afterwards;
- a research run starts and finishes while the folder lock is held by a live process;
- a research run whose scripted model reads `<root>/.aws/credentials`: the read is refused (skip
  with a printed reason only where the sandbox cannot be applied);
- `--research --base main`: exit 2;
- `--report-to <path>` receives the report.

**Constraints.** No fallbacks: a scratch directory that cannot be created, a report that is
missing, or a helper that fails is an error naming it. Resumable as described. Existing tests pass
unmodified apart from the one refusal test named above; if another cannot, stop and report it. No
home-directory paths in code or tests.

**Commit message:** `feat(dispatch): research runs in a folder that is not a git repository`

### Task IC5a: review and undo for runs in a folder that is not a git repository

**Depends on:** IC4b (on this branch). **Siblings:** IC4c (research runs there) runs at the same
time and is **not** in this worktree: leave the `--research` refusal in `dispatch_in_place.rs` as
it is. IC5b (the surface, the `blyte` tool, the prompts) comes later: do not change `serve.rs`,
the agent crate or any asset.

**Goal.** `blyte review <id>` shows what an in-place run changed and records a verdict, and
`blyte undo <id>` puts the folder back as it was before the run. **Why:** an in-place run has no
branch to inspect or throw away, so its saved originals are the only review and the only way back.

**Files.**
- `crates/blyte-cli/src/in_place_review.rs` (new) and its `mod` line: everything below except
  the small hooks named here.
- `crates/blyte-cli/src/dispatch_in_place.rs`: only writing `<id>.after/` (item 1).
- `crates/blyte-cli/src/dispatch.rs`: only the branch at the top of `run_review` (item 2).
- `crates/blyte-cli/src/cli.rs` and `crates/blyte-cli/src/main.rs`: the `undo` subcommand.
- `crates/blyte-cli/tests/test_in_place_review.rs` (new).

**What to build.**

1. **After-copies.** In `dispatch_in_place::run`, after the child ends and before the summary is
   written, copy each file in `changed_files` from the folder to `<store>/<id>.after/<path>`
   (created with its parent directories). A copy that fails makes the outcome `error` with the
   reason. On a restart the copies are written again.
2. **`blyte review <id>`.** At the top of `run_review`, when
   `crate::dispatch_in_place::records_dir_for(cwd)` is not a `.wt` directory of a git repository
   (the current directory is not in a repository), call `crate::in_place_review::review(args)`
   and return its code. Inside a repository nothing changes. `review`:
   - errors (`error: no run <id> in this folder`, exit 2) when `<store>/<id>.start.json` is absent;
     says `run <id> has not finished` (exit 2) when the summary is absent;
   - prints the sections an ordinary review prints, in the same order and with the same headings:
     `== checks` (a run that did not end `finished` or `verified`; host commands, outside reads
     and refused reads from the log, using the existing functions; a recorded undo), `== files`
     (one line per manifest entry: `created` or `modified`, then the path), `== report` (outcome,
     turns, cost, final message, as today);
   - with `--diff`, prints `<store>/<id>.diff`; a missing diff file is an error naming it;
   - with `--verdict`/`--notes`, records the verdict in `<store>/<id>.verdict.json` in the format
     `queue_actions::record_verdict` writes (reuse it if it can take the records directory
     without changing its behaviour for repositories; otherwise write the same JSON here);
   - for a research run (`"research": true` in the start record) prints the checks and the report
     and no files section.
3. **`blyte undo <id>`** (`in_place_review::undo`). Exit 0 on success; every refusal prints
   `error: <reason>` and exits 2.
   - Refuse when the current directory is in a git repository (`undo is for runs in a folder that
     is not a git repository`), when the run is unknown or unfinished, when it is a research run,
     and when `<store>/<id>.undone.json` exists (`run <id> was already undone`).
   - Take the folder lock (`folder_lock::acquire`, kind `"undo"`, id = the run id) and hold it to
     the end; a held lock is a refusal naming the holder.
   - For every manifest entry, the file in the folder must be byte-for-byte equal to its
     `<id>.after/` copy, or already equal to what undo would leave (its `<id>.before/` copy for a
     modified file; absent for a created file). If any file is neither, change nothing and refuse,
     listing every such path: `changed since run <id> ended: <paths>`.
   - Then restore: a modified file gets its `<id>.before/` content; a created file is removed
     (directories are left). A missing `<id>.before/` copy for a modified file is an error naming
     it, checked before anything is changed.
   - Write `<store>/<id>.undone.json` (`{"undone_at": <unix seconds>}`) last, and print one line
     per file restored or removed.
   - Interrupted and run again, it finishes the job: files already restored count as done.

**Tests** (`cargo test -p blyte --test test_in_place_review`; build the run's records in the
test, or run a scripted in-place dispatch as `tests/test_dispatch_in_place.rs` does; own temporary
folder with no `.git` and own `BLYTE_STATE_DIR`):
- after a run that modifies `a.txt` and creates `sub/b.txt`: `<store>/T1.after/` holds both new
  contents; `blyte review T1` prints `modified a.txt` and `created sub/b.txt`; `--diff` prints
  the old and new line of `a.txt`;
- `review T1 --verdict accepted --notes x` writes `T1.verdict.json` with that verdict;
- `review NOPE`: exit 2, `no run NOPE in this folder`;
- `undo T1`: `a.txt` has its old content, `sub/b.txt` is gone, `T1.undone.json` exists, exit 0;
  a second `undo T1`: exit 2, `already undone`; `review T1` then flags the undo under `== checks`;
- `undo T1` after `a.txt` was edited by hand: exit 2, the message names `a.txt`, and neither
  file is changed;
- `undo T1` while the lock is held by a live process: exit 2, naming the holder;
- an undo interrupted after restoring `a.txt` (set the folder up that way in the test, without
  the marker) completes when run again;
- `blyte undo T1` inside a git repository: exit 2;
- in a git repository `blyte review` behaves as before (the existing review tests pass
  unmodified).

**Constraints.** No fallbacks: a missing record, copy or diff is an error naming it, never an
empty section. Undo changes nothing unless it can change everything. Resumable as described.
Existing tests pass unmodified; if one cannot, stop and report it instead of editing it. No
home-directory paths in code or tests.

**Commit message:** `feat(review): review and undo a run made in a folder that is not a git repository`

### Task IC5b: the surface, the `blyte` tool and the prompts (outline)

**Depends on:** IC4c, IC5a. The surface lists and reconciles runs in a non-repository folder and
serves their diff from the saved diff file; `surface_notify` finds a non-repository surface; the
`blyte` tool allows `dispatch`, `wait`, `status`, `review`, `undo` and `clean` there and writes its
output file to the run store; an approved write from the chat is refused while a run holds the
folder lock; the start-up notice, the sandbox paragraph, `process.md` and `reference.md` describe
in-place runs and tell the model to hand off anything longer than a quick look-up. Written in
full once IC4c and IC5a are on the branch.
