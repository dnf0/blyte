# Plan: recognise wip branches merged through a GitHub squash merge (2026-10-21)

## Why

Reported by a data-platform session (2026-10-21): `blyte clean --dry-run` kept 11
worktrees as "not merged yet", and 28 `wip/*` branches had all their work in main, yet blyte
detected none. `blyte dispatch A1` then failed with `fatal: a branch named 'wip/A1' already exists`
for a task merged two weeks earlier. The workflow that causes it: `git merge --ff-only wip/<id>`
onto a feature branch, then the feature branch is squash-merged on GitHub and deleted. The commits
on main are new objects with no `(cherry picked from commit ...)` line, and the feature branch is
gone. `merged_into` (`crates/blyte-cli/src/clean.rs`, `fn merged_into`) tries ancestry,
`git cherry` and `git merge-tree`; none sees a squash merge whose lines main later edited.

GitHub's default squash-commit body lists every squashed commit's subject (`* <subject>`), and the
PR title becomes the squash subject. The reporter checked a subject match on 91 branches: it found
all 28 merged ones and none of the unmerged ones.

Safety rule (Claude, 2026-10-21): only main's commits made **after the branch forked** count. A
match against older history (`fix: typo` from last month) would mark unmerged work as merged, and
`blyte clean` deletes merged worktrees and branches.

### Task S1: subject-match fallback in `merged_into`

Files: `crates/blyte-cli/src/clean.rs` (only `fn merged_into` and the doc comment above
`pub fn is_branch_merged`, about line 1250), new test `crates/blyte-cli/tests/test_clean_squash.rs`,
`docs/usage.md` (the sentence that lists how `blyte clean` decides a branch is merged;
`grep -n "merge-tree\|cherry" docs/usage.md`).

**`clean.rs` is 1,970 lines: never read it whole.** Read `fn merged_into` (`grep -n "fn merged_into"`,
about 60 lines) and use `grep -n` for anything else. Check with `grep -n "merged_into\|is_commit_merged_with_targets" crates/blyte-cli/src/clean.rs`
that the `wip/<id>` path (`check_run_merged` → `decide_run_merged`) reaches `merged_into`; if it does
not, call the new check from wherever it decides a run's commit is merged, and say so in the commit
message.

1. Add `fn is_squash_merged_into_target(root: &Path, commit_sha: &str, target: &str) -> bool` and
   call it as step 4 at the end of `merged_into`, after the merge-tree check (only reached when the
   first three fail):
   - `base = git merge-base <target> <commit_sha>`; no merge base → false.
   - `subjects` = `git log --no-merges --format=%s <target>..<commit_sha>`; trim each line, drop empty
     ones. Empty list → false (nothing to match; the earlier checks own that case).
   - `haystack` = `git log --format=%B <base>..<target>` (commits on the target since the fork
     only). Build a `HashSet<String>` of its lines, each trimmed, then with a leading `* ` or `- `
     removed (keep both the stripped and unstripped forms).
   - True only when every subject is in the set as a **whole line** (no substring matching).
2. Doc comment on `is_branch_merged`: add the fourth bullet:
   `every commit's subject on <branch> appears as a whole line in the messages of <default>'s commits since the branch forked (a GitHub squash merge lists them).`
3. **Tests** (`test_clean_squash.rs`; copy the `git`, `write_file` and `init_repo` helpers from
   `crates/blyte-cli/tests/test_clean.rs`, first 33 lines). Call `blyte_cli::clean::is_branch_merged`
   (or `is_branch_merged_with_targets` with a non-`wip/` branch name, whichever the crate exports;
   `grep -n "pub mod clean" crates/blyte-cli/src/lib.rs`):
   - squash merged: branch `feat/x` with commits "feat: add a" and "fix: tweak a"; on main, a commit
     whose message is `feat: x (#1)\n\n* feat: add a\n\n* fix: tweak a`, with the branch's changes,
     then a later commit on main that edits the same lines (so merge-tree does not match) → merged.
   - partly merged: the same, but the squash body lists only "feat: add a" → not merged.
   - old subject only: main has a commit "fix: typo" **before** the branch forks; the branch has
     one commit "fix: typo" with different content, never merged → not merged.
   - substring only: main's squash body has `* feat: add a and more`; branch subject "feat: add a"
     → not merged.

Gate: the repository checks. Commit: `fix(clean): treat a wip branch as merged when a squash merge on main lists every commit subject`

### Task S2: clearer dispatch error when `wip/<id>` already exists (parallel with S1)

Files: `crates/blyte-cli/src/dispatch.rs` (only the `git worktree add` block, about lines 360-385:
`grep -n "git worktree add failed" crates/blyte-cli/src/dispatch.rs`, read 30 lines above it), new
test `crates/blyte-cli/tests/test_dispatch_branch_exists.rs`. **`dispatch.rs` is 1,800 lines: never
read it whole.**

1. Before running `git worktree add -b <branch> ...`, when `git rev-parse --verify --quiet refs/heads/<branch>`
   succeeds, do not run it. Instead print one of the following and return 2:
   - when `crate::clean::is_branch_merged(&root, &branch)` is true:
     `error: branch {branch} already exists from an earlier run, and its work is already merged. Run \`blyte clean\` to remove it, or dispatch with a new ID.`
   - otherwise:
     `error: branch {branch} already exists from an earlier run and is not merged. Review it with \`blyte review {id}\`, delete it (\`git branch -D {branch}\`) if it is not needed, or dispatch with a new ID.`
2. Test: in a temporary repository with a `blyte.toml` (copy setup from an existing dispatch test
   that runs the binary before any model call: `grep -ln "already exists\|worktree" crates/blyte-cli/tests/*.rs`),
   create branch `wip/A1` with an unmerged commit, run `blyte dispatch A1` with a task, and assert
   exit code 2 and that stderr contains `already exists from an earlier run and is not merged`.
   If no existing test runs dispatch this far without a model, put the message selection in a small
   `pub fn branch_exists_message(root, branch, id) -> String` and unit-test that for both cases
   instead.

Gate: the repository checks. Commit: `fix(dispatch): explain an existing wip branch instead of git's error`
