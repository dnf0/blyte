# Let tests read `.env` files they create in temp folders

## Problem

The macOS sandbox refuses to read any file named `.env`, anywhere
(`profile_with` in `crates/blyte-agent/src/sandbox.rs` adds
`(deny file-read-data (regex #"/\.env$"))`). The rule protects real secrets: the `.env` in a
repository's main checkout, in a worktree, or in the home folder.

It also catches `.env` files that a repository's own tests create in pytest's temp folder
(`tmp_path_factory.mktemp("env") / ".env"`, which lands under `/private/var/folders/...`). In one
repository that made 48 settings tests fail in every run's full gate, although the code was right.

## Decision

`.env` files under the system temp folders `/private/tmp` and `/private/var/folders` may be read,
except under the workspace root. Everywhere else the `.env` deny stays as it is. Grants from
`allow_read` still come last and override everything.

The workspace exception matters because a workspace can itself sit in a temp folder (blyte's own
sandbox tests use `tempfile::tempdir()` as the workspace), and a workspace's `.env` is the
repository's secret.

`$TMPDIR` is not added separately: on macOS it resolves under `/private/var/folders`. If it points
elsewhere, `.env` files there stay denied, as now.

### Task TE1

Make the macOS sandbox profile allow reading `.env` files under `/private/tmp` and
`/private/var/folders`, except under the workspace root.

Files: `crates/blyte-agent/src/sandbox.rs`, `crates/blyte-agent/tests/test_sandbox_read_deny.rs`.

1. Give `profile_with` the workspace root as a new first parameter, `workspace: &Path`, and
   update every caller (`default_profile` passes its `root`; update the calls in
   `crates/blyte-agent/tests/` and anywhere else `git grep -n profile_with` finds).
2. In `profile_with`, keep the existing `.env` deny. After it, and before the `allow_read`
   grants, add rules so that a path ending in `/.env` is readable when it is under
   `/private/tmp` or `/private/var/folders` and not under `workspace`. SBPL's last matching rule
   wins; either use `require-all`/`require-not` filters, or an allow followed by a re-deny for
   `.env` under `workspace` (escape the workspace path the way `escape_subpath` does, and for a
   regex escape every regex metacharacter in it). Keep the two temp folders as a named constant.
3. Update the doc comment on `profile_with` to say which `.env` files are readable and why
   (tests create them in temp folders).
4. Tests in `test_sandbox_read_deny.rs`:
   - Extend the profile-string test so it checks the new rules appear after the `.env` deny and
     before the `allow_read` grants.
   - In the sandboxed tests (the ones that skip when nested sandboxing is not permitted), add one
     where the workspace is one `tempfile::tempdir()` and a `.env` in a second, separate
     `tempfile::tempdir()` can be read (`cat` succeeds and prints its contents), while a `.env`
     in the workspace still cannot. The existing test that a workspace `.env` is denied must keep
     passing unchanged apart from the new parameter.
   - Add one where a `.env` outside the temp folders and outside the workspace is still denied,
     if you can create such a file without writing outside the test's temp folders; if you
     cannot, check it in the profile string instead.

No fallbacks: do not add `$TMPDIR` or any guessed location; do not catch and ignore errors. Do
not change the Linux (Landlock) path.

Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.
