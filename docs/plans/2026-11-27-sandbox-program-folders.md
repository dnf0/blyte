# A run's shell cannot write to folders of programs or to tool configuration

Commands the model runs are sandboxed (`crates/blyte-agent/src/sandbox.rs`). The sandbox allows
writes to whole tool folders under the home folder so caches work: `.cargo`, `.rustup`, `.local`,
`go`, `.gradle`, `.m2` and others, in both `default_allowed` (dispatch, research, init) and
`conversation_paths` (surface). That also makes these writable from every run:

- folders of programs on PATH: `~/.cargo/bin`, `~/.local/bin`, `~/go/bin`, and the toolchains
  the cargo and rustc on PATH hand over to (`~/.rustup/toolchains/*/bin`);
- files that make a tool run a program of the writer's choice: `~/.cargo/config.toml`,
  `~/.gradle/init.d`, `~/.m2/settings.xml`, Python's `~/.local/lib`, installed tools under
  `~/.local/share`;
- in a dispatch, the main checkout's `.git/hooks` and `.git/config`.

A run could replace a program, or plant a hook, that the user later runs outside the sandbox.

## Decisions

- macOS (Seatbelt): a later `deny file-write*` overrides an earlier `allow file-write*`, so the
  tool folders stay allowed and the protected paths are denied after them.
- Linux (Landlock) cannot deny beneath an allowed folder. On Linux this change stops granting
  `.rustup`, grants `go/pkg` instead of `go`, and leaves `bin` and `lib` out of the `.local`
  grants. `~/.cargo` and the git folder stay granted whole on Linux; `docs/security.md` says so.
- `~/.rustup` is read-only in every run. A run that needs a toolchain or component that is not
  installed fails with rustup's own permission error; the user installs it first. Builds with an
  installed toolchain do not write there.

### Task SB1

**Goal:** make the paths above read-only to a run's shell, keeping the caches writable.
**Depends on:** none. No sibling tasks.

**Files:** `crates/blyte-agent/src/sandbox.rs`; `crates/blyte-cli/tests/test_conversation_sandbox.rs`
(one existing test asserts a write to `.local/bin` succeeds: change it); a new test file
`crates/blyte-agent/tests/test_sandbox_program_folders.rs`; `docs/security.md`. Other existing
tests may need their expectations updated where they assert the old grants
(`grep -rn '"go"\|\.rustup' crates/*/tests`).

**What to build**

1. One helper for the home-folder grants, used by both `default_allowed` and `conversation_paths`
   (today the list is written twice). It grants, under `$HOME`: `.cargo`, `.cache`, `.npm`,
   `Library/Caches`, `.gradle`, `.m2`, `go/pkg`, `.docker/buildx`, then `push_home_local_grants`.
   `.rustup` is no longer granted. `go` is no longer granted whole.
2. `push_home_local_grants`, Linux branch: leave out `bin` and `lib` as well as `state`.
3. macOS: a function that appends `(deny file-write* (subpath "<path>"))` for each protected
   path, each made canonical with `canonicalize_existing_prefix` and escaped as
   `deny_state_blyte_write` does. Call it wherever `deny_state_blyte_write` is called, so the
   denies come after every home-folder allow. Protected paths, under `$HOME`:
   `.cargo/bin`, `.cargo/config`, `.cargo/config.toml`, `.cargo/credentials`,
   `.cargo/credentials.toml`, `.local/bin`, `.local/lib`, `.local/state/blyte`,
   `.local/share/uv/tools`, `.local/share/mise`, `.local/share/pipx`, `.local/share/keyrings`,
   `.gradle/init.d`, `.m2/settings.xml`.
   - The workspace root must stay writable in a dispatch: if the root is inside a protected path
     or equal to it, that one path is not denied. Any later re-allow of a scratch root keeps
     working (see `default_profile_base`).
   - If `HOME` is unset or empty, no home grants are made today and none of these denies are
     added; keep that.
4. macOS, `default_profile_base` only (dispatch): also deny `<git common dir>/hooks` and
   `<git common dir>/config`, using the same git common dir `default_allowed` computes. The rest
   of the git folder stays writable so the run can commit.
5. `docs/security.md`: what a run can and cannot write under the home folder after this change;
   that installing programs (`cargo install`, `go install`, `uv tool install`, `pip install
   --user`, rustup toolchain or component installs) fails inside a run; and the Linux limits
   named under "Decisions".

No fallbacks: nothing here may silently widen a grant when a path is missing or cannot be made
canonical; follow how `deny_state_blyte_write` treats a path that does not exist yet. Nothing
here is a long-running job, so "resumable" does not apply.

**Tests** (`crates/blyte-agent/tests/test_sandbox_program_folders.rs` unless stated). Live tests
follow the existing ones: a temporary `HOME` set the way
`sandboxed_shell_cannot_write_the_usage_ledger_directory` in
`crates/blyte-cli/tests/test_conversation_sandbox.rs` sets it, `LocalShell::sandboxed`, and the
same skip when the test itself runs inside a sandbox.

- macOS, live, for both `conversation_profile` and `default_profile`: with the folders created
  first, a write FAILS and creates nothing at `.cargo/bin/x`, `.cargo/config.toml`,
  `.local/bin/x`, `.local/lib/x`, `.rustup/x`, `go/bin/x`, `.local/share/uv/tools/x`,
  `.gradle/init.d/x`, `.m2/settings.xml`, `.local/state/blyte/x`; and SUCCEEDS at
  `.cargo/registry/x`, `.cargo/.package-cache`, `.cargo/git/x`, `go/pkg/x`, `.cache/x`,
  `.local/share/uv/python/x`, `.gradle/caches/x`, `.m2/repository/x`.
- macOS, live, `default_profile` in a git repository: writing `<repo>/.git/hooks/pre-commit` and
  appending to `<repo>/.git/config` both fail and change nothing; `git commit --allow-empty -m x`
  succeeds (set `user.name` and `user.email` before the sandboxed part).
- macOS, live, `default_profile` with a workspace root at `$HOME/.local/lib/proj`: a write inside
  the root succeeds; a write to `$HOME/.local/lib/other` fails.
- macOS, live, with the real `HOME` (do not change it): in a temporary cargo project with no
  dependencies, `cargo build --offline` succeeds under `default_profile`. This proves an
  installed toolchain builds with `~/.rustup` read-only. Skip with the same nested-sandbox probe.
- Profile text, macOS: each protected path appears in a `deny file-write*` rule after the
  `allow file-write*` rule for its parent; the git denies appear in `default_profile` and not in
  `conversation_profile`.
- Linux (`#[cfg(target_os = "linux")]`, allowlist check with a temporary `HOME` that has
  `.local/bin`, `.local/lib`, `.local/share`, `.rustup`, `go/bin`, `go/pkg`): the write list has
  `.local/share` and `go/pkg`, and has none of `.local/bin`, `.local/lib`, `.rustup`, `go`,
  `go/bin`.
- Update `sandboxed_shell_cannot_write_the_usage_ledger_directory`: its positive control becomes
  `.cache/x`, and it asserts `.local/bin/x` fails.

Run: `cargo test -p blyte-agent --test test_sandbox_program_folders --test test_sandbox --test test_sandbox_linux --test test_write_guard && cargo test -p blyte --test test_conversation_sandbox`

**Commit message:** `fix(sandbox): a run's shell cannot write to folders of programs on PATH or to tool configuration`
