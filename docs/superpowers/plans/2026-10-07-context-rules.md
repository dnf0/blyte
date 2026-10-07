# Context rules in CLAUDE.md, kept in place by blyte

An orchestrating Claude session grew to 822K tokens in a day (2026-10-07): hundreds of unfiltered
`sed`/`cat` results and gate logs, and whole task files written out. The session had no rules about
context. blyte already manages a `<!-- blyte:default -->` block in `~/.claude/CLAUDE.md`; it now also
manages a `<!-- blyte:context -->` block with context rules, writes it on `blyte skill install`, and
puts it back (or brings it up to date) on every `blyte dispatch`, so every orchestrating session from
then on has it.

### Task CR1: managed context block in the global CLAUDE.md

**Goal:** blyte keeps a `<!-- blyte:context -->` block with context rules in `~/.claude/CLAUDE.md`:
written by `blyte skill install`, removed by `blyte skill uninstall`, and restored or updated on every
`blyte dispatch`. Why: rules only help if every orchestrating session has them, and users never
re-run `skill install`.

**Files:** `crates/blyte-cli/src/skill.rs`, `crates/blyte-cli/src/dispatch.rs`,
new `crates/blyte-cli/tests/test_context_block.rs`.

**What to build** (in `skill.rs`):

1. A constant, exactly:

```rust
pub const CONTEXT_BLOCK: &str = "\
<!-- blyte:context -->
## Context: keep this session small

Everything a tool returns stays in the conversation and is re-read on every turn, so it is most of
what a session costs.
- Trim command output at the source: `| tail -n 20`, `grep` for the lines you need, `sed -n`
  ranges, `--quiet`/`-q`; `git diff --stat` before any full diff.
- Read files by range (offset and limit), not whole, once you know where to look.
- For a dispatched run, read `blyte review <id>` and the JSON fields you need
  (`jq -r .outcome .wt/<id>.json`), not the whole summary or log; search `.wt/<id>.log` with
  `grep`/`tail` only for what `== checks` flags.
- Write tasks as `### Task <ID>` sections of the plan and dispatch with `--plan`/`--plan-task`;
  do not write task files that copy the plan.
- An investigation needing more than about five file reads goes to `blyte dispatch --research`;
  read its report, not the files.
- One session per piece of work. When the status line shows the context past about 60% of its
  window, tell the user to `/compact` or start a new session before going on.
<!-- /blyte:context -->";
```

2. Generalise the block helpers: `set_block(text: &str, block: &str) -> String` and
   `remove_block(text: &str, block: &str) -> String`, taking the start and end markers from the
   block's first and last lines. Same behaviour as today's `set_default_block`/`remove_default_block`
   (replace in place when both markers exist, otherwise append after one blank line; trailing
   newline normalised). Keep `set_default_block`/`remove_default_block` as one-line wrappers so their
   callers and tests are unchanged.

3. `pub fn ensure_context_block(home: &Path) -> Result<Option<PathBuf>, String>`:
   - if `home/.claude` does not exist, do nothing and return `Ok(None)` (the user does not use
     Claude Code; never create the directory);
   - otherwise read `home/.claude/CLAUDE.md` (empty if missing), apply `set_block(.., CONTEXT_BLOCK)`,
     and write it only if the text changed; return `Ok(Some(path))` when it wrote, `Ok(None)` when the
     file was already current.

4. `install_for`: for `Harness::Claude` and `!project`, call `ensure_context_block(&home_dir()?)` and
   push the path to `written` when it returns `Some`. Independent of `no_default` (the context rules
   are not the delegation default). `uninstall_for`: for `Harness::Claude` and `!project`, remove the
   context block from `~/.claude/CLAUDE.md` if present, and push the path to `changed` when it changed.

5. In `dispatch.rs`, at the start of `run_dispatch` (before worktree preparation), call
   `skill::ensure_context_block` with `skill::home_dir()`. If it wrote, print one line to **stderr**:
   `blyte: added context rules to <path> (they apply from the next Claude session)`. Any error is
   ignored (print nothing, carry on): it must never fail a dispatch. stdout stays exactly the JSON
   summary.

**Tests** (`crates/blyte-cli/tests/test_context_block.rs`, run with
`cargo test -p blyte-cli --test test_context_block`; each test uses a fresh temp dir as `home`, calling
`blyte_cli::skill` functions directly except the last two, which run the binary with `HOME` set, like
`tests/test_skill.rs` does):

- `adds_block_to_existing_claude_md`: `home/.claude/CLAUDE.md` = `"# Mine\n\nkeep this\n"`; after
  `ensure_context_block`, the file starts with that text unchanged, contains `CONTEXT_BLOCK` once, and
  ends with `"<!-- /blyte:context -->\n"`; the return is `Some(path)`.
- `creates_claude_md_when_only_the_dir_exists`: `home/.claude` exists, no CLAUDE.md; afterwards the
  file is exactly `CONTEXT_BLOCK + "\n"`.
- `does_nothing_without_dot_claude`: no `home/.claude`; returns `Ok(None)` and `home/.claude` still
  does not exist.
- `second_call_is_a_no_op`: call twice; the second returns `Ok(None)` and the file bytes are identical.
- `updates_a_stale_block_in_place`: the file has text before, a
  `"<!-- blyte:context -->\nold rules\n<!-- /blyte:context -->"` block, and text after; afterwards
  "old rules" is gone, `CONTEXT_BLOCK` appears once at the old block's position, and the text before
  and after is unchanged.
- `leaves_default_block_alone`: the file has the `DEFAULT_BLOCK`; after `ensure_context_block` both
  blocks are present once each, and `remove_block(.., CONTEXT_BLOCK)` gives back a text that still
  contains `DEFAULT_BLOCK` and not `<!-- blyte:context -->`.
- `install_and_uninstall_manage_the_block`: run `blyte skill install` with `HOME=<tmp>` (and
  `--no-statusline` if needed to keep the test hermetic) after creating `<tmp>/.claude`; CLAUDE.md
  contains `CONTEXT_BLOCK`. Then `blyte skill uninstall`; CLAUDE.md no longer contains
  `<!-- blyte:context -->`.
- `install_with_no_default_still_writes_context_block`: `blyte skill install --no-default` (with the
  same hermetic flags); CLAUDE.md contains `CONTEXT_BLOCK` and not `<!-- blyte:default -->`.

Check the exact `skill install`/`uninstall` flag names in `crates/blyte-cli/src/cli.rs` before writing
the last two tests.

**Constraints:** existing tests pass unmodified; `blyte dispatch` stdout is unchanged; no new
dependencies.

**Depends on:** none.

**Commit message:** `feat(skill): keep context rules in the global CLAUDE.md`
