# Linux: a run's shell is never granted a folder that holds credentials

Date: 2026-11-28. Source: the research report LXR1 (why
`research_run_refuses_reading_aws_credentials` fails on Linux), checked against `main` at 338d83a4.

## Problem

On macOS the shell sandbox (Seatbelt) denies reads of the credential paths in `read_denied(home)`,
blyte's own key file and the surface token folder. On Linux the sandbox (Landlock) can only allow
paths: everything beneath an allowed folder is readable, and nothing can be denied inside it. The
Linux profile builders (`default_profile`, `default_profile_net`, `conversation_sandbox_with_edits`
in `crates/blyte-agent/src/sandbox.rs`) never compare what they allow with what must stay
unreadable. So today, on Linux:

1. `~/.cargo/credentials` and `~/.cargo/credentials.toml` are readable and writable by the shell in
   every run, because `~/.cargo` is granted whole for reads and for writes.
2. When `$HOME` is under `/tmp`, `/var/tmp` or `$TMPDIR` (containers, CI accounts, the test suite),
   all of `$HOME` is readable and writable, because those folders are granted whole. This is why
   `research_run_refuses_reading_aws_credentials` has failed on Linux since it was added (#448).
3. A run whose workspace is `$HOME` or another folder containing a protected path (an in-place
   dispatch, a headless run, a surface in a git repository at `$HOME`) is granted that folder.
   Only a surface in a folder that is not a git repository checks for this
   (`check_linux_non_repo_root`).
4. When `$HOME` is under `/var` or `/opt`, it is readable, because those are granted for reads.

## Decisions

1. **One rule, checked where the Linux profile is built.** Before a Landlock profile is returned,
   every granted path (write and read) is compared with the protected paths
   (`protected_read_paths()`). A profile that would grant a protected path is never returned.
2. **`~/.cargo` is granted by its children.** For reads: every existing child except
   `credentials` and `credentials.toml`. For writes: additionally not `bin`, `config`,
   `config.toml`, `env`, `env.fish`, `env.nu` (programs on `PATH` and tool configuration, which
   macOS already keeps read-only since #457). `~/.cargo/registry` and `~/.cargo/git` are created
   first when `~/.cargo` exists, so a first `cargo fetch` still works.
3. **A temp folder that contains a protected path is not granted; the run gets a private one.**
   `/tmp`, `/var/tmp` and `$TMPDIR` are dropped from both lists when a protected path is beneath
   them, and the shell gets a private `TMPDIR` (mode 0700) in a temp folder that does not contain
   one. If there is none, the run fails with an error naming the folder and the protected path.
4. **Anything else that contains a protected path is an error, not a guess.** The run does not
   start: `on Linux the sandbox cannot protect <protected>: granted path <granted> contains it`.
   This covers a workspace at `$HOME`, `$HOME` under `/var` or `/opt`, and `--allow-read` on an
   ancestor of a protected path. Granting such folders child by child is not done: it would leave
   the unlisted private files readable.
5. **`--allow-read` on a protected path itself still works**, as on macOS (the user asked for it),
   except blyte's own key file and surface token folder, which are never granted.
6. **An in-place dispatch checks its folder first**, with the existing
   `check_linux_non_repo_root_for_env`, so the error comes before the folder is locked.

## Limits that stay

- **A workspace `.env` stays readable by the Linux shell.** macOS denies it by file name; Landlock
  cannot deny a file inside the granted workspace. blyte's own `read`, `glob` and `grep` tools
  still refuse it on both platforms. Documented in `docs/security.md`.
- A new top-level file in `~/.cargo` created during a run is not writable until the next run.
- `default_profile` and `default_profile_net` stay for tests; on Linux they are not checked and no
  code path of the `blyte` binary uses them after this change. Removing them is a follow-up once
  the Linux tests are green.

## Checking

Nothing on this machine runs Linux. The path rule is written as plain functions that compile and
are unit-tested on every platform, so the gate here covers it. Kernel enforcement, the private
`TMPDIR` in real runs and `research_run_refuses_reading_aws_credentials` can only be confirmed by
the Linux test run on `main` after the merge. One assumption to confirm there: cargo tolerates not
being able to create `~/.cargo/.package-cache` when it does not exist yet.
