# Plan: steer file searches to the glob and grep tools (2026-10-18)

## Why

Research run R-interrupt2 (568 saved run reports) found 28 failing `bash` turns, about 7,800 s or
2% of all run time, spent in `find` / `grep -rn` / `rg` walks over `/`, `/Users/...`, `~/repos` or
`.venv` trees; 22 were killed at the 300 s `COMMAND_TIMEOUT`. blyte has native `glob` and `grep`
tools that respect `.gitignore` and are capped, but:

- `glob` only searches the workspace (`crate::search::glob_paths(root, ...)`,
  `crates/blyte-agent/src/tools.rs` `"glob" =>` arm). In a research run the workspace is a scratch
  directory and the code is under a read root, so the model cannot glob the repository and falls
  back to `find`.
- The rules never tell the model to prefer the tools over the shell (`RESEARCH_RULES` mentions
  `read` and `grep` only; `DISPATCH_RULES` mentions neither).

Decision (user, 2026-10-18): the model should use its glob and grep tools for finding files and
searching contents.

### Task S1: glob takes a `path`, and the rules and tool descriptions point to glob and grep

Files: `crates/blyte-agent/src/tools.rs`, `crates/blyte-cli/src/dispatch.rs`,
`crates/blyte-agent/tests/test_native_glob.rs`, `docs/architecture.md`, `docs/how-it-works.md`
(and any other doc that describes the glob tool's arguments; grep `docs/` for it).

1. **`glob` gets an optional `path` argument**, like `grep`'s: "directory to search under; defaults
   to the workspace; may be an absolute path under a read root". In the `"glob" =>` arm:
   - validate it exactly as `grep` does, with `resolve_for_read(fs, root, &path)`, returning its
     error unchanged;
   - search under the resolved directory instead of `root` (local: `glob_paths(dir, ...)`;
     container: run the same `git ls-files` / shell fallback with the resolved directory as the
     working directory);
   - when `path` was given, print each hit joined to `path` as the model gave it (so an absolute
     `path` gives absolute hits the `read` tool accepts); with no `path`, output is unchanged.
   - Update the `glob` ToolSpec: add the `path` property and its description; keep `pattern`
     relative to `path` (or the workspace).
2. **Tool descriptions.** In the `bash` ToolSpec description add: to find files or search file
   contents use the `glob` and `grep` tools, not `find`, `grep -r` or `rg` in the shell; they
   respect .gitignore, are capped and cannot hang.
3. **Rules** in `crates/blyte-cli/src/dispatch.rs`:
   - `DISPATCH_RULES`: add a rule: "Find files with the glob tool and search file contents with
     the grep tool. Do not run `find`, `grep -r` or `rg` in the shell over `/`, your home directory,
     or any directory outside the worktree: they walk huge trees and are killed at the time limit."
   - `RESEARCH_RULES`: change "Read the code with the read and grep tools, using absolute paths
     under the repository" to name the read, grep and glob tools (glob and grep with `path` set to
     the repository's absolute path), and add the same "do not run find / grep -r / rg over ..."
     sentence, with "outside the repository" in place of "outside the worktree".
   - Keep each rule one line, in the style of the surrounding rules.
4. **Tests** in `test_native_glob.rs`:
   - `path` set to an absolute directory under a read root finds files there and prints absolute
     hits;
   - `path` outside the workspace and every read root is rejected with the same error `grep`
     gives;
   - no `path`: existing behaviour unchanged (existing tests keep passing).
   Use the existing fixtures and helpers in that file; set read roots the way other tests in
   `crates/blyte-agent/tests/` do (grep for `set_read_roots` or `READ_ROOTS`).
5. **Docs**: the `glob` rows in `docs/architecture.md` and `docs/how-it-works.md` mention the
   optional `path` (a read root).

Gate: the repository checks (`blyte.toml`). Commit message:
`feat(tools): glob searches under a read root, and the rules steer searches to glob and grep`
