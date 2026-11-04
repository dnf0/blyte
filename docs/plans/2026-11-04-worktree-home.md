# Plan: one home for worktrees

Spec: `docs/specs/2026-11-04-worktree-home.md` (read it first; it is the authority). The three
tasks are independent. WT1 and WT2 both touch `crates/blyte-cli/src/clean.rs`, but in different
functions: WT1 changes only `find_repo_root`, and WT2 doesn't touch it.

### Task WT1

Spec decision 1: every command uses the main checkout as its root.

Files:
- `crates/blyte-cli/src/clean.rs` (`find_repo_root` only)
- `crates/blyte-cli/src/dispatch.rs`
- `crates/blyte-cli/src/wait.rs`
- any other command that reads or writes `<root>/.wt/` or calls `registry::register`. Find them
  with `grep -rn 'show-toplevel' crates/blyte-cli/src`. Leave `skill.rs` and `init.rs` alone:
  they write tracked files in the checkout they run in.
- tests in a new `crates/blyte-cli/tests/test_root_main_checkout.rs`.

1. Make `clean::find_repo_root` return the main checkout for any linked worktree, not only ones
   under `.wt/`. Use `config::main_checkout` on the `--show-toplevel` result. In a main checkout,
   return it unchanged. If the folder is a linked worktree but `main_checkout` returns `None`,
   return an `Err` naming the folder. No fallback to the worktree's own root.
2. Make `run_dispatch`, `run_review`, `run_status` (`dispatch.rs`) and `wait::run` take their root
   from `find_repo_root` instead of their own `git rev-parse --show-toplevel`. Do the same for any
   other command found by the grep that uses `.wt/` or the registry.
3. Keep the run's base exactly as today. With `--base`, nothing changes. Without it, dispatch must
   still use what it resolves today from the folder it was started in, not the main checkout's
   HEAD. If base resolution currently relies on the root folder, give it the starting folder
   explicitly. A test must prove this.
4. Tests, in temp git repos (never the real home or state dir; set `HOME` and `BLYTE_STATE_DIR`
   to temp folders):
   - `find_repo_root` gives the main checkout from:
     - the main checkout itself;
     - `<main>/.wt/plan-x`;
     - a sibling worktree `<parent>/<repo>--topic`;
     - `<main>/.claude/worktrees/x`.
   - dispatching from a sibling worktree: if a full dispatch is impractical in tests, test the
     function that computes the run's folder and registry root. The run's folder is
     `<main>/.wt/<id>`, the registry gets `<main>`, and a run's base defaults to the sibling's
     branch.
   - `review`, `status` and `wait` started from a linked worktree find a run stored under
     `<main>/.wt/`.

No fallbacks.

Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.

### Task WT2

Spec decision 2: `blyte clean` removes merged session worktrees inside the repository.

Files:
- `crates/blyte-cli/src/clean.rs` (not `find_repo_root`)
- `crates/blyte-cli/src/main.rs`, only if output wiring needs it
- `docs/` pages that describe `blyte clean`; find them with `grep -rln 'blyte clean' docs README.md`
- tests in a new `crates/blyte-cli/tests/test_clean_session_worktrees.rs`.

1. In `clean_repo`, after runs, also consider the main checkout's linked worktrees (from
   `parse_worktree_list`) whose path is directly under `<root>/.wt/` but which are not runs (no
   `<root>/.wt/<name>.start.json`), or under `<root>/.claude/worktrees/`. Compare canonical paths.
2. Remove one only when every condition in the spec holds:
   - the branch is merged (`is_branch_merged`, or `is_commit_merged` for a detached HEAD);
   - `is_worktree_clean`;
   - no `<worktree>/.wt/*.start.json`;
   - not in use: no running process has its working directory inside it. Get the working
     directories once per `clean` run with `lsof -a -d cwd -Fn` (lines starting with `n`), and
     compare canonical paths: a process in the worktree or any folder below it counts. If `lsof`
     fails or is missing, keep every such worktree with the reason `cannot tell whether it is in
     use: <error>`. Put the lookup behind a small function so tests can pass in a list.
   - idle for 7 days: `index` and `HEAD` in the worktree's git admin folder (`git -C <wt>
     rev-parse --absolute-git-dir`) are both more than 7 days old.

   Remove it with `git worktree remove <path>` (never `--force`). Then delete its local branch the
   way `clean` deletes a merged `wip/` branch, but only if no other worktree has it checked out.
   With `--dry-run`, only print.
3. Add `RemovalReason`/`KeptReason` variants in the existing style. Output, alongside the run
   lines:
   - `removed .wt/plan-AC (fix/x merged, 2.3 GB)`, `deleted branch fix/x (merged)`;
   - `kept .wt/plan-RB (not merged yet)`, `kept … (uncommitted changes)`, `kept … (in use by a running process)`,
     `kept … (used in the last 7 days)`;
   - `kept … (holds runs: review them from <path>)`.

   Count freed bytes in the existing total.
4. `report_other_worktrees` must no longer list the worktrees handled here. It goes on reporting
   worktrees outside the repository as today.
5. If `git worktree remove` fails, print an error naming the path and git's message, keep going
   with the other worktrees, and make `clean` exit non-zero at the end. This is not a silent skip.
6. Tests, in temp git repos with linked worktrees, cover each removal condition and each kept
   reason; `--dry-run`; a squash-merged branch; and a `.claude/worktrees/x` worktree. For the
   7-day rule, set the admin files' mtimes with `filetime` if it is already a dependency.
   Otherwise put the threshold behind a parameter that the tests set to zero, and add no new
   dependency.

No fallbacks.

Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.

### Task WT3

Spec decision 3: blyte's installed rule says where session worktrees go.

Files:
- `crates/blyte-cli/src/skill.rs` (`DEFAULT_BLOCK` only)
- `crates/blyte-cli/assets/blyte-implementer/SKILL.md`, only if it tells the reader where to
  create worktrees
- any test that compares `DEFAULT_BLOCK` text.

1. Add this paragraph to `DEFAULT_BLOCK`, before its closing marker, keeping lines under 100
   characters:

   ```
   Worktrees of your own (not blyte runs) go inside the repository at `.wt/plan-<topic>`,
   branched from the default branch: `git worktree add .wt/plan-<topic> -b <branch> origin/<default>`.
   Never next to the repository or in a temp or scratchpad folder. Remove it after its PR merges
   if you are still running; otherwise `blyte clean` removes it once the branch is merged and it
   has no changes.
   ```
2. Make sure `blyte skill install --force` replaces an existing block with the new text. That is
   the path `blyte update` uses; check it already does, and add a test if none covers it.
3. If `SKILL.md` says anything about where to create worktrees, make it agree.

Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.
