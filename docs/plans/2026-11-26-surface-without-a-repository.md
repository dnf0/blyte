# A surface in a folder that is not a git repository

Date: 2026-11-26. Based on `feat/self-hosting-gaps` (PR #433, including its Task H6).

## Problem

`blyte` started in a folder that is not a git repository exits with
`blyte: not inside a git repository (<dir>)`. The user wants a surface to start anywhere, as a
place to talk, read files and use the sandboxed shell, and wants the model itself to be able to
turn the folder into a repository when the work needs one (with the user's approval, no button).

## Facts (checked against the code on `feat/self-hosting-gaps`)

- Surface start refuses in `serve.rs` (`find_git_toplevel`, then `resolve_main_checkout`,
  then `current_branch`). The conversation itself (`ReplSession`, `AgentRunner`,
  `conversation_paths`, the per-command sandbox) already works without git and is tested so.
- A surface writes its record and log under `<root>/.wt/surfaces/<id>/`, and the conversation
  creates `<root>/.wt/tasks` (and `<root>/.wt/tmp/<pid>` when the root is under a temp folder).
- The `read`, `glob` and `grep` tools allow any path inside the root (`resolve_for_read` in
  `blyte-agent/src/tools.rs`) and do not use the credential list the shell sandbox denies
  (`sandbox::read_denied`, files named `.env`, the surface token folder, `~/.config/blyte/env`).
  With the home folder as root they could read `~/.ssh`, `~/.aws` and blyte's own tokens. The
  same hole exists today for a git repository rooted at the home folder.
- `write`/`edit` treat `~/.local` (macOS) as writable without approval, which includes blyte's
  own state folder; only the shell sandbox excludes it.
- On Linux the shell sandbox grants reads beneath the root and has no deny rules, so a root
  that contains credential folders exposes them to the shell.
- The `blyte` tool starts a fresh `blyte` process per call, the write limits are recomputed
  every turn, and `publish` reads the root on demand: once the folder is a repository with a
  first commit, runs work without restarting the surface. Stale after `git init`: the branch
  in the registry entry.

## Decisions

1. **No repository, no refusal.** When the folder is not inside a git work tree the surface
   starts with the folder itself as its root. Inside a work tree nothing changes.
2. **Nothing is written into such a folder.** The surface's record and log go to
   `<state dir>/folders/<key>/surfaces/<id>/`; no `.wt/tasks` and no `.wt/tmp` are created.
   The folder stays read-only to the model without approval, like a main checkout.
3. **Runs, direct edits, combining and publishing are refused with one clear message** while
   the folder is not a repository, decided at the time of each call, not at start.
4. **The model can make it a repository**: it asks to run `git init` and a first commit
   through the existing approval for an unsandboxed command. No new tool and no button. After
   that, runs work in the same surface.
5. **Credentials stay unreadable whatever the root is**: the file tools refuse the same paths
   the shell sandbox denies, and never write blyte's own state or configuration without
   approval. On Linux a surface whose root contains one of those credential paths does not
   start, because the Linux sandbox cannot deny a path under a readable root.
6. No fallbacks: where a value does not exist without a repository (the branch) it is absent,
   not invented.

## Tasks

N1 and N2 are independent. N3 depends on N1 (same file).

### Task N1: file tools refuse credential paths, and blyte's own folders need approval to write

**Goal.** `read`, `glob` and `grep` must not return what the shell sandbox denies, whatever the
root is; `write`/`edit` must not change blyte's state or configuration without approval.

**Depends on:** none. **Siblings:** N2 changes `crates/blyte-cli` (surface start); it is not in
this worktree and nothing here needs it.

**Files.** `crates/blyte-agent/src/tools.rs`, `crates/blyte-agent/src/search.rs`,
`crates/blyte-agent/src/sandbox.rs` (one new public function only), new
`crates/blyte-agent/tests/test_protected_paths.rs`.

**What to build.**
- `sandbox::protected_read_paths() -> Result<Vec<PathBuf>, String>`: the paths the macOS
  profile denies for reading, from the same sources and in the same form: `read_denied(home)`,
  the surface token folder and `~/.config/blyte/env` (the paths `deny_state_blyte_write` and
  the profile's read-deny use; reuse the existing helper functions, do not copy the list). If
  `HOME` is unset or empty the result is the non-home entries only, exactly as the profile does.
- `sandbox::is_protected_read(path: &Path) -> Result<bool, String>`: true when the path, after
  resolving symlinks of its existing prefix (as `resolve_real_target` does for writes), equals
  or is inside a protected path, or its file name is exactly `.env`.
- `read`: after `resolve_for_read`, a protected path is refused with
  `read was not made: <path> is a credential path that this conversation cannot read`.
- `glob` and `grep` (local, in `tools.rs`/`search.rs`): a protected directory given as the
  search root is refused with the same message (verb `glob`/`grep`); during a walk, protected
  directories are not entered and protected files are skipped (not listed, not searched). Add
  one line to the result when anything was skipped: `(credential paths skipped: <n>)`.
- The rule applies to paths reached through `READ_ROOTS` too. It does not apply to the
  implementer in a run's own worktree differently: it is the same function everywhere.
- `check_write_scope`: a target inside blyte's state folder (`$XDG_STATE_HOME/blyte` or
  `~/.local/state/blyte`, and `$BLYTE_STATE_DIR` when set) or inside `~/.config/blyte` is
  never covered by `scope.allowed`; it goes to the approval path with the reason
  `inside blyte's own state or configuration`. An open edit worktree cannot be there, so that
  branch is unaffected.

No fallbacks: an error resolving a path is returned as an error, not treated as "not
protected". Nothing here runs for long.

**Tests** (`test_protected_paths.rs`; set `HOME` to a temp folder under a lock, as the other
sandbox tests do; each fails on the current code):
- `read_refuses_ssh_key_inside_root`: root = the fake home; `read .ssh/id_ed25519` is an error
  naming the path; `read notes.txt` succeeds.
- `read_refuses_dot_env_but_reads_dot_env_example`.
- `read_refuses_a_symlink_into_a_protected_folder`: `link -> .aws/credentials`.
- `grep_skips_protected_files_and_says_so`: the secret string in `.ssh/k` and in `a.txt`;
  the result has `a.txt`, not `.ssh/k`, and the skipped line.
- `glob_does_not_list_protected_files`.
- `grep_with_a_protected_search_root_is_refused`.
- `read_refuses_the_surface_token_folder`.
- `write_into_blyte_state_asks_for_approval`: with a write scope whose `allowed` contains
  `<home>/.local` and an approver that records and refuses, `write` to
  `<home>/.local/state/blyte/x` is refused and the approver was asked once; `write` to
  `<home>/.local/share/x` is made without asking.
- `write_into_blyte_config_asks_for_approval`.

Run `cargo test -p blyte-agent --test test_protected_paths`, then the repository's checks.

**Constraints.** Existing tests pass; where an existing test reads a `.env` through a file
tool and expected success, change it to expect the refusal and say so in the final message.
No new dependencies.

**Commit message.**
`fix: file tools refuse credential paths, and writes to blyte's own folders need approval`

### Task N2: a surface starts in a folder that is not a git repository

**Goal.** `blyte` (and `blyte --resume`) work in a folder outside any git work tree, writing
nothing into it.

**Depends on:** none (H6 is already in the base branch). **Siblings:** N1 and N3 change
`crates/blyte-agent` and the conversation instructions; they are not in this worktree.

**Files.** `crates/blyte-cli/src/serve.rs`, `surface_dir.rs`, `surface_registry.rs`,
`surfaces_cmd.rs`, `assets/surface/index.html`, any other reader of `RegistryEntry.branch`,
tests in `crates/blyte-cli/tests/` (new `test_surface_without_repository.rs`).

**What to build.**
1. Start: if `git rev-parse --show-toplevel` fails in the start folder because it is not
   inside a work tree, the root is the canonical start folder and the surface is "without a
   repository". Any other git failure (git missing, a broken repository) stays the error it is
   today. Print one line at start:
   `blyte: <folder> is not a git repository: runs, combining and publishing are off until it is one`.
2. Where the surface keeps its record and log:
   - `surface_dir::folder_store(state_dir, root) -> Result<PathBuf, String>` =
     `<state_dir>/folders/<key>` where `<key>` is 16 lower-case hex digits of FNV-1a 64-bit
     over the canonical root path's bytes (write the hash inline; no dependency). The folder is
     created `0700` and holds a file `root` containing the canonical path; if `root` exists
     with a different path, that is an error naming both paths (a hash collision), not a merge.
     `state_dir` is the one the surface registry uses.
   - A new surface started without a repository keeps `surface.json` and `entries.jsonl` in
     `<folder_store>/surfaces/<id>/`. One started in a repository keeps them where it does now.
   - Listing records for a root (for `--resume`, name conflicts and `blyte clean`) reads both
     places and returns each record with the directory it was found in; a resumed surface
     keeps using the directory its record is in. So a surface begun before `git init` can be
     resumed after it.
3. `RegistryEntry.branch` becomes `Option<String>` (`null` when the root is not a repository;
   an existing entry with a string still parses). The activity hook reads the branch again on
   each write, so it is right after `git init` or a checkout; a git failure other than "not a
   work tree" is reported on stderr as the hook reports write failures, and the entry is not
   rewritten. `blyte surfaces` and the `surfaces` tool print `-` for no branch.
4. `GET /api/session` gains `"repository": true|false`, computed at each request. The page
   shows, next to the folder name, the text `not a git repository` while it is false (through
   `escapeHtml` like the rest; no new button).
5. HTTP routes that need git (`/api/runs/{id}/diff`, `/decision`, `/api/requests/{id}/push`,
   `/pull-request`) answer `409` with body `{"error":"this folder is not a git repository"}`
   when the root is not a repository at the time of the call. `/api/runs` and `/api/requests`
   answer as they do today (empty lists). The background reconcile does nothing while the root
   is not a repository.
6. Linux only: if the root is not a repository and contains (is an ancestor of or equal to)
   any path of `blyte_agent::sandbox::read_denied(home)`, or the home folder itself, start
   fails with `blyte: on Linux a surface cannot start in <root>: it contains <path>, which the
   sandbox could not keep unreadable; start it in a subfolder`. (Use `read_denied` as it is;
   do not call functions N1 adds.)
7. A root with no file name (`/`) or a folder name that cannot be a surface name keeps today's
   errors.

No fallbacks. Nothing here runs for long.

**Tests** (`test_surface_without_repository.rs`, with an isolated state folder as
`test_serve.rs` does; each fails on the current code):
- `starts_in_a_plain_folder_and_writes_nothing_into_it`: start with hooks in a temp folder
  with one file; `/api/health` answers; `/api/session` has `"repository": false`; after stop
  the folder contains exactly that one file; `<state>/folders/*/surfaces/<id>/surface.json`
  and `entries.jsonl` exist and `<state>/folders/*/root` holds the canonical path.
- `registry_entry_has_no_branch_and_surfaces_prints_a_dash`.
- `resume_finds_a_surface_started_without_a_repository`: start, stop, resume by name; the same
  id and log are used.
- `resume_still_works_after_git_init`: start, stop, `git init` + empty commit in the folder,
  resume by name succeeds with the same id, and its log stays in the folder store.
- `branch_appears_after_git_init_without_restart`: start in a plain folder, `git init -b main`
  + commit, trigger the activity hook (post a message as existing tests do with a scripted
  model), and the registry entry's branch is `main`; `/api/session` says `"repository": true`.
- `push_route_answers_409_without_a_repository`.
- `a_broken_repository_is_still_an_error`: a folder with a `.git` file containing garbage does
  not start as "without a repository".
- `folder_store_refuses_a_key_collision`: a store whose `root` file names another path → error
  naming both.
- Linux only (`#[cfg(target_os = "linux")]`): `linux_refuses_a_root_that_contains_ssh`.
- Page test (`test_surface_page.rs`): the page source shows `not a git repository` from the
  `repository` field.

Run `cargo test -p blyte --test test_surface_without_repository --test test_serve --test test_surface_page`,
then the repository's checks.

**Constraints.** Existing tests pass unmodified except for the type of `branch`. No home
paths or private names in the repository. No new dependencies.

**Commit message.** `feat: a surface starts in a folder that is not a git repository`

### Task N3: without a repository the model is told so, and can ask to make one

**Goal.** In a folder that is not a repository the conversation creates nothing in it, the
`blyte` and `publish` tools refuse with one clear message, and the instructions say how to
turn the folder into a repository.

**Depends on:** N1 (both change `crates/blyte-agent/src/tools.rs`); dispatch with
`--base wip/<N1 run id>` or after N1 is on the branch. **Siblings:** N2 (surface start in
`crates/blyte-cli/src/serve.rs`) is not in this worktree.

**Files.** `crates/blyte-agent/src/sandbox.rs`, `crates/blyte-agent/src/tools.rs`,
`crates/blyte-cli/assets/conversation/process.md`, `reference.md`, tests that quote them, new
`crates/blyte-agent/tests/test_no_repository.rs`.

**What to build.**
- `sandbox::is_git_repository(root) -> bool`: true when `git_common_dir(root)` is some.
- `conversation_paths(root)` when the root is not a repository: no `.wt/tasks` entry and no
  directory created under the root. If a default writable folder contains the root it is
  removed as today, and the private temp folder is `<grant>/blyte-<pid>` (mode `0700`) inside
  the first remaining temp grant in the existing order (`$TMPDIR`, then the system temp
  folders); if none remains, return an error that names the root. `created_tmpdir` and removal
  on exit work as they do now. Callers that create the tasks folder skip it when the root is
  not a repository (`conversation_tasks_dir` callers in `crates/blyte-cli/src/task_runner.rs`
  may be changed for this alone).
- The `blyte` tool, for `dispatch`, `wait`, `status`, `review`, `plan`, `init`, `clean`,
  `edit-start`, `edit-finish`, checks first, before creating any file (today a background
  dispatch creates `.wt/<id>.dispatch.out`), and `publish` checks first too; when the root is
  not a repository each returns the error:
  `this folder is not a git repository, and runs, direct edits and publishing need one. If the user wants that here, ask to run "git init" and then a first commit with the bash tool and unsandboxed: true; the user approves each command. Otherwise keep working with read, grep, glob and the shell.`
  The check is made at every call, so the tools work after `git init` without a restart.
- Instructions: add one paragraph to `process.md` (always present, so it cannot go stale):
  the folder may not be a git repository; then there are no runs, direct edits or publishing;
  files in it are read-only without approval; to work with runs there, ask to run `git init`
  and a first commit (`git add -A && git commit -m "chore: first commit"`) as unsandboxed
  commands the user approves, and only when the user wants the folder to become a repository,
  never in the user's home folder without their saying so. One matching line in
  `reference.md` under the `blyte` tool.

No fallbacks. Nothing here runs for long.

**Tests** (`test_no_repository.rs`; each fails on the current code):
- `conversation_paths_creates_nothing_in_a_plain_folder`: a plain temp-free folder (create it
  under a fake `HOME` outside the temp grants if the test harness allows; otherwise assert on
  the under-temp case below) → no `.wt` exists afterwards and no allowed path is inside the root.
- `under_a_temp_folder_the_private_temp_is_outside_the_root`: root in a temp folder, not a
  repository → `tmpdir` is set, exists with mode `0700`, is not inside the root, the root is
  inside no allowed path, and the root still contains no `.wt`.
- `in_a_repository_nothing_changes`: `git init` → `.wt/tasks` is allowed as before.
- `blyte_tool_refuses_dispatch_without_a_repository_and_creates_nothing`: `dispatch T1 x.md`
  with `background: true` → error containing `not a git repository` and `git init`; the root
  has no `.wt`.
- `blyte_tool_works_again_after_git_init`: same root after `git init`: the tool no longer
  returns that message (it reaches the `blyte` binary; use the existing test seam for the
  binary path, as `test_blyte_tool` tests do).
- `publish_refuses_without_a_repository`: with a publish tool registered that records calls →
  the error above and no call recorded.
- Instruction tests: the assembled text contains `git init` and `not a git repository`.

Run `cargo test -p blyte-agent --test test_no_repository`, then the repository's checks.

**Constraints.** Existing tests pass unmodified apart from ones quoting the changed
instruction lines. No new dependencies.

**Commit message.**
`feat: without a repository the model says so and can ask to run git init`

## Real check before the pull request

Start the built binary in a plain folder and in a scratch home folder holding a fake
`.ssh/id_ed25519` and `.env`: the surface starts; nothing is written into the folder; the model
is asked to read both files with `read`, `grep` and `cat` (all refused or empty); asked to
dispatch (refused with the message); asked to make the folder a repository (an approval appears;
after approving, `blyte dispatch` reaches the gate setup instead of the refusal).
