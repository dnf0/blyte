# Comment lint: a run may not add comments

Date: 2026-11-19

## Decisions

- A comment a run adds is a defect. The code and its tests carry the meaning; a comment is where
  an implementer records an assumption or a shortcut instead of fixing it, and nothing checks it.
- blyte enforces this as a gate step, so the implementer is told which lines to remove and cannot
  finish until they are gone. The same finding appears in `blyte review`.
- Only lines the run added count. Existing comments are left alone.
- Languages in this version: Rust (`rs`), Python (`py`), JavaScript and TypeScript (`js`, `jsx`,
  `mjs`, `cjs`, `ts`, `tsx`). Other files are not checked. Shell, YAML and TOML are left out until
  the lexer is shown to be reliable on them (heredocs, block scalars).
- Not counted, because a tool reads them:
  - documentation comments: Rust `///`, `//!`, `/** */`, `/*! */`; JavaScript and TypeScript
    `/** */`. (Python docstrings are strings, not comments, so they are never counted.)
  - a `#!` line that is the first line of the file;
  - directives: a comment whose text, after the marker and any spaces, starts with one of the
    words in `DIRECTIVES` below.
- Setting: `[dispatch] comments = "gate" | "review" | "off"` in `blyte.toml`. `gate` is the
  default when the key is absent: it is the intended behaviour, not a stand-in. Any other value
  is an error that names the value.
  - `gate`: the step runs in the gate, and review flags.
  - `review`: review flags only.
  - `off`: neither.
- Research runs are never checked.

## Contract shared by the tasks

`crates/blyte-cli/src/comment_lint.rs` (Task CL1) exposes exactly:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Comment {
    pub file: String,
    pub line: usize,
    pub text: String,
}

pub const DIRECTIVES: &[&str] = &[
    "noqa", "type:", "pragma", "pylint:", "mypy:", "pyright:", "ruff:", "flake8:", "fmt:",
    "isort:", "yapf:", "nosec", "coding:", "-*-", "eslint", "@ts-", "prettier-", "biome-",
    "istanbul", "c8 ", "v8 ", "webpack", "@vite-ignore", "@jsx", "<reference", "jshint",
    "global ", "globals ", "rustfmt::", "clippy::", "SAFETY:", "SPDX-", "Copyright",
];

pub fn is_supported(path: &str) -> bool;
pub fn comments_in(path: &str, source: &str) -> Vec<Comment>;
pub fn added_lines(diff: &str) -> std::collections::BTreeMap<String, std::collections::BTreeSet<usize>>;
pub fn added_comments(
    diff: &str,
    read: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Vec<Comment>, String>;
pub fn flag(found: &[Comment]) -> Option<String>;
```

The gate step (Task CL2) is the command `blyte comments --base <sha>`, run in the worktree.

### Task CL1: the comment lexer and the added-comment finder

**Goal.** A module that finds the comments in a source file with a real lexer (not line
patterns), and the ones among them on lines a diff added. It is the base for a gate step and a
review flag that stop runs adding comments.

**Depends on:** none. **Siblings:** CL2 adds the `blyte comments` command and CL3 wires dispatch
and review; they are not in this worktree. Do not add a subcommand, a config key or any call to
this module here.

**Files.** Create `crates/blyte-cli/src/comment_lint.rs` and
`crates/blyte-cli/tests/test_comment_lint.rs`. In `crates/blyte-cli/src/lib.rs` add
`pub mod comment_lint;` in alphabetical position. No other file.

**What to build.** The items in "Contract shared by the tasks" above, with these behaviours.

`is_supported(path)`: true when the extension is one of `rs`, `py`, `js`, `jsx`, `mjs`, `cjs`,
`ts`, `tsx`.

`comments_in(path, source)`: lex the whole file once, character by character, and return one
`Comment` per comment that counts, in file order. `line` is the 1-based line where the comment
starts; `text` is the comment's first line, trimmed, cut to 120 characters. For an unsupported
path return an empty vector (the callers filter with `is_supported` first; this is the documented
behaviour, not a guess). The lexer must track strings so that comment markers inside them are not
comments:

- Rust, JavaScript, TypeScript: `//` to end of line; `/* ... */` (in Rust these nest). Strings:
  `"..."` with backslash escapes. JavaScript and TypeScript also `'...'` and `` `...` `` template
  literals (a template literal may span lines; treat `${ ... }` inside as part of the string).
  Rust also raw strings `r"..."`, `r#"..."#` with any number of `#`, with an optional `b` prefix,
  and character literals versus lifetimes: after a `'`, if the next character is `\` it is a
  character literal that ends at the next unescaped `'`; otherwise if the character after next
  is `'` it is a one-character literal; otherwise it is a lifetime and only the `'` is consumed.
- JavaScript and TypeScript regular-expression literals are not lexed. To keep `/[/]/`-style
  input from being read as a comment, a `//` counts only when it is not immediately preceded by
  a `\`.
- Python: `#` to end of line. Strings: `'...'`, `"..."`, and triple-quoted `'''...'''` and
  `"""..."""` (may span lines), with backslash escapes; prefixes (`r`, `b`, `f`, `u` and
  combinations) need no special handling beyond not treating the quote as anything else.

A comment does not count, and is not returned, when:

- it is documentation: in Rust it starts with `///` (but not `////`), `//!`, `/**` (but not
  `/***` and not the empty `/**/`), or `/*!`; in JavaScript and TypeScript it starts with `/**`
  (same exclusions);
- it starts with `#!` at byte 0 of the file;
- it is a directive: after removing the marker (`//`, `/*`, `#`) and leading spaces, the text
  starts with one of `DIRECTIVES`. The match is case-sensitive except that `Copyright` also
  matches `copyright` and `(c)`.

`added_lines(diff)`: parse unified diff text (`git diff` output, any context size). Map each new
path (from `+++ b/<path>`; skip `+++ /dev/null`) to the set of 1-based new-file line numbers of
its `+` lines, using the `@@ -a,b +c,d @@` headers to count.

`added_comments(diff, read)`: for each path in `added_lines(diff)` where `is_supported`, call
`read(path)`; on `Err(e)` return `Err(format!("cannot read {path}: {e}"))` (no skipping); keep the
comments of `comments_in` whose `line` is in the added set. Result ordered by path, then line.

`flag(found)`: `None` when empty; otherwise
`Some("comments added (N): <file>:<line>, <file>:<line>, <file>:<line> and M more: remove them, and check what each one was covering for")`,
listing at most three, with ` and M more` only when N > 3.

**Tests** (`crates/blyte-cli/tests/test_comment_lint.rs`, run with
`cargo test -p blyte --test test_comment_lint`). One test per line below; each asserts the exact
`line` values returned.

1. Rust: `// a` alone on a line and `let x = 1; // b` are both found, on their lines.
2. Rust: `let s = "http://x // y";` gives nothing.
3. Rust: a raw string `r#"a // b "# // c"#;` gives nothing; the same followed by ` // real`
   gives one.
4. Rust: `fn f<'a>(x: &'a str) {} // c` gives one (the lifetime does not open a string);
   `let c = '"'; // c` gives one; `let c = '\''; // c` gives one.
5. Rust: nested `/* a /* b */ c */ let x = 1;` gives one comment at its start line, and a
   following `// d` on the next line is found too.
6. Rust: `/// doc`, `//! doc`, `/** doc */`, `/*! doc */` give nothing; `//// four` and `/**/`
   give one each.
7. Rust: `// SAFETY: the pointer is valid` and `// clippy::foo` give nothing.
8. Python: `# a` and `x = 1  # b` are found; `s = "# no"` and `s = '# no'` give nothing.
9. Python: a triple-quoted string spanning three lines that contains `# no` gives nothing, and a
   `# yes` on the line after it is found with the right line number.
10. Python: `#!/usr/bin/env python` on line 1 gives nothing; the same text on line 3 gives one.
11. Python: `x = f()  # noqa: E501`, `# type: ignore`, `# pragma: no cover` give nothing.
12. TypeScript: `` const s = `a // ${b} // c`; `` gives nothing; a template literal spanning two
    lines with `//` on the second gives nothing; `// @ts-expect-error` and
    `// eslint-disable-next-line x` give nothing; `/** jsdoc */` gives nothing; `/* plain */`
    gives one.
13. JavaScript: `const r = /a\//; // c` gives exactly one, at that line.
14. `is_supported`: true for `a/b.rs`, `x.py`, `x.tsx`, `x.cjs`; false for `x.sh`, `x.yml`,
    `x.md`, `Makefile`.
15. `added_lines`: a diff with two files and two hunks in one of them gives the exact line sets;
    a deleted file (`+++ /dev/null`) is absent.
16. `added_comments`: with a diff that adds lines 2 and 5 of `a.rs`, and a `read` returning a
    file with comments on lines 1, 2 and 5, the result is lines 2 and 5 only. A path with
    extension `md` is never passed to `read` (the closure panics if it is).
17. `added_comments`: when `read` returns `Err("gone")` the result is
    `Err("cannot read a.rs: gone")`.
18. `flag`: empty gives `None`; two comments give the exact string with no "more"; five give
    three locations and ` and 2 more`.

**Constraints.** No new dependencies. No fallbacks: an unreadable file is an error, never a
skip. Existing tests pass unmodified. The module itself must contain no comments other than
documentation comments.

**Commit message:** `feat(lint): find the comments a diff adds`

### Task CL2: the `blyte comments` command

**Goal.** A command that fails when the working tree has comments that the base commit does not,
so it can run as a gate step and tell the implementer which lines to remove.

**Depends on:** CL1 (its module is in this worktree). **Siblings:** CL3 wires dispatch, config
and review separately; do not touch `dispatch.rs` or `config.rs` here.

**Files.** Create `crates/blyte-cli/src/comments_cmd.rs` and
`crates/blyte-cli/tests/test_comments_cmd.rs`; change `crates/blyte-cli/src/cli.rs` (a
`Comments(CommentsArgs)` subcommand with `--base <REV>`, required), `crates/blyte-cli/src/lib.rs`
(`pub mod comments_cmd;`) and `crates/blyte-cli/src/main.rs` (call it and exit with its code).
Add the command to `docs/reference/cli.md` in the style of the entries there.

**What to build.** `pub fn run(base: &str, dir: &std::path::Path) -> i32`:

1. Run `git diff --no-color --no-ext-diff --unified=0 <base>` in `dir` (working tree against the
   base, so committed and uncommitted changes both count). If git fails, print
   `blyte comments: git diff failed: <first line of stderr>` to stderr and return 2.
2. Run `git ls-files --others --exclude-standard` in `dir`; every untracked file listed is new:
   all of its lines count as added. Failure is handled as in step 1.
3. Find the added comments with `comment_lint` (read files from `dir`; a read error prints
   `blyte comments: <error>` to stderr and returns 2; a file that is not valid UTF-8 is such an
   error only when `is_supported` is true for it, and unsupported files are never read).
4. None found: print nothing, return 0.
5. Otherwise print to stdout, then return 1:

```
comments added (N). Remove each one. If it explained an assumption, a shortcut or a deviation from the task, fix that or say it in your final report.
<file>:<line>: <text>
...
Not counted: documentation comments (///, //!, /** */) and tool directives (noqa, eslint-disable, @ts-expect-error, SAFETY:).
```

The help text of the command is: `Fail when the working tree adds comments since a base commit`.

**Tests** (`cargo test -p blyte --test test_comments_cmd`). Each builds a temporary git
repository (the helpers in `crates/blyte-cli/tests/test_dispatch_base_check.rs` show how the
existing tests do it), commits a base file, and runs the built binary
(`env!("CARGO_BIN_EXE_blyte")`) with `comments --base <sha>` in it:

1. No change: exit 0, empty stdout.
2. A committed change adding `// x` to `a.rs`: exit 1; stdout contains `comments added (1)` and
   `a.rs:<line>: // x`.
3. The same change uncommitted (working tree only): exit 1, same line.
4. A new untracked `b.py` with `# y` on line 2: exit 1; stdout contains `b.py:2: # y`.
5. A change adding only `/// doc` and `// SAFETY: ok` to `a.rs`, and a new `notes.md` with
   `// z`: exit 0.
6. The base file already has `// old` and the change adds a code line below it: exit 0.
7. `--base` naming a revision that does not exist: exit 2; stderr starts with
   `blyte comments: git diff failed:`.

**Constraints.** No fallbacks: every git or read failure is exit 2 with the message, never a
pass. No new dependencies. Add no comments other than documentation comments. Existing tests
pass unmodified.

**Commit message:** `feat(cli): blyte comments fails when the working tree adds comments`

### Task CL3: run the comment lint in the gate and flag it in review

**Goal.** Dispatch adds the comment lint to the gate by default, tells the implementer the rule,
and `blyte review` flags comments a run added.

**Depends on:** CL1 (its module is in this worktree). **Siblings:** CL2 adds the
`blyte comments` command separately; it is **not** in this worktree. Do not add it, and do not
write a test that runs it: here the gate step is only a string.

**Files.** `crates/blyte-cli/src/config.rs`, `crates/blyte-cli/src/dispatch.rs`,
`crates/blyte-cli/tests/test_dispatch_comments.rs` (new). No change to `cli.rs`, `lib.rs` or
`main.rs`.

**What to build.**

1. `config.rs`: `DispatchConfig` gains `pub comments: Option<String>`. Add

   ```rust
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub enum CommentMode { Gate, Review, Off }
   pub fn comment_mode(value: Option<&str>) -> Result<CommentMode, String>;
   ```

   `None` → `Gate`; `"gate"`, `"review"`, `"off"` → those; anything else →
   `Err(format!("blyte.toml: [dispatch] comments must be \"gate\", \"review\" or \"off\", not \"{value}\""))`.
2. `dispatch.rs`: add

   ```rust
   pub fn comment_step(exe: &std::path::Path, base_sha: &str) -> String;
   pub fn gate_with_comment_step(gate: &str, step: &str) -> String;
   ```

   `comment_step` returns `'<exe>' comments --base <base_sha>`, the path in single quotes with
   any `'` inside written as `'\''`. `gate_with_comment_step` returns the step as the first
   line followed by the gate unchanged (`format!("{step}\n{gate}")`), or the gate unchanged when
   it has no non-blank line.
3. In the dispatch function: resolve the mode with `comment_mode(dispatch_cfg.comments.as_deref())`
   as soon as the configuration is loaded; on `Err` print `error: <message>` to stderr and exit
   the way the other configuration errors in that function do. When the run is not research and
   the mode is `Gate`, replace the gate with
   `gate_with_comment_step(&gate, &comment_step(&current_exe, &base_sha))` after `base_sha` is
   known and before the gate is used for anything (the `--gate` argument, `quick_steps`, the
   start record). Add `"comments": "gate" | "review" | "off"` to the start record (`"off"` for a
   research run).
4. `DISPATCH_RULES`: add this line directly after the line that begins `- Stay within the task.`:

   `- Write no comments in code: not to explain it, not to mark a section, not to note an assumption or a TODO. The check fails on any comment you add. If something needs saying, say it in your final report. Documentation comments a tool reads (///, //!, /** */) and tool directives (noqa, eslint-disable, SAFETY:) are the only exceptions; do not use them to get around this rule.`

5. Review: where the review builds its flags from the run's diff with
   `crate::fallback_check::added_fallbacks` (the `Ok(diff)` arm), also compute the added
   comments, unless the start record's `comments` is `"off"` (a start record without the field
   is an older run: check it). Read each file's content at the run's head commit with
   `git show <head>:<path>` in the repository (the same revision the diff's new side is), so the
   check works after the worktree is removed; a failure to read becomes the flag
   `comment check not run: <first line of the error>`, cut to 200 characters. Push
   `crate::comment_lint::flag(..)` onto `checks.flags` when it is `Some`.

**Tests** (`cargo test -p blyte --test test_dispatch_comments`):

1. `comment_mode`: the four accepted inputs give the three modes; `Some("flag")` gives the exact
   error string.
2. `comment_step(Path::new("/opt/my tools/blyte"), "abc123")` is exactly
   `'/opt/my tools/blyte' comments --base abc123`; a path containing `'` is escaped as
   `'\''`.
3. `gate_with_comment_step("cargo fmt\ncargo test", "S")` is `"S\ncargo fmt\ncargo test"`;
   with `""` and with `"\n  \n"` the gate is returned unchanged.
4. `DISPATCH_RULES` contains `Write no comments in code`.
5. A `blyte.toml` with `[dispatch]\ncomments = "review"` parses, and one with an unknown key
   still fails as before (`deny_unknown_fields`).
6. End to end, using the fake-model helpers the existing dispatch tests use
   (`crates/blyte-cli/tests/test_dispatch_base_check.rs` is the closest example): a dispatch
   with `--no-base-check` and no `comments` key writes a start record whose `gate` first line
   ends with ` comments --base <base_sha>` and whose `comments` is `"gate"`; with
   `comments = "review"` the gate is the configured gate unchanged and `comments` is
   `"review"`; with `comments = "bogus"` the dispatch exits non-zero and stderr contains
   `comments must be`. (The gate may fail in this test because the sibling command does not
   exist here; assert on the start record only.)

**Constraints.** No fallbacks: an unknown `comments` value is an error; a failed read in review
is a flag that says the check did not run, never silence. No new dependencies. Add no comments
other than documentation comments. Existing tests pass unmodified, except that an existing test
asserting the exact `gate` text of a start record may need the new first line: change only that
assertion and say so in the report.

**Commit message:** `feat(dispatch): the gate fails on comments a run adds, and review flags them`
