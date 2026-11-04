# `blyte skill install` sets Claude Code's auto-compact window to 200k

## Why

Most of what a Claude Code session costs is re-reading its context on every turn. On 1M-context
models (Opus 4.7 and later, including `claude-opus-5-5`) Claude Code compacts at about 967k tokens
unless `autoCompactWindow` is set, so a long session re-reads up to about 967k each turn. blyte's
context rules ("suggest `/compact` or a new session at 60%") assume a 200k window. blyte never
set that window, so users without their own setting (reported 2026-11-04) compact at about 967k and
the rules don't hold. #323 makes the status line show the real window; this change sets 200k for
users who have not chosen a window themselves.

## What it does

Only for `blyte skill install` with the Claude harness, globally (not with `--project`), and not with
the new `--no-autocompact` flag. These steps work on `~/.claude/settings.json` and run in the same
read, change and write as the status line step, with the same one-time backup to
`settings.json.blyte-backup`.

1. **Already handled:** if the marker file `<state_dir>/autocompact-window` exists, do nothing.
   `<state_dir>` is the existing `registry::state_dir()`. The marker means blyte has already made
   this change once. Whatever the file holds now, including the key removed by hand, is the user's
   choice, and later installs and `blyte update` don't change it.
2. **The user's own setting:** if the file already sets any of these, leave it alone and print
   `auto-compact: keeping your <key> (<value>)`:
   - a top-level `autoCompactWindow`;
   - `autoCompactWindow` in any `modelSettings.<model>` entry;
   - `CLAUDE_CODE_AUTO_COMPACT_WINDOW` in its `env` block.

   No marker is written, so if the user later removes their setting, a later install sets 200k.
3. **Otherwise:** add top-level `"autoCompactWindow": 200000`, then write the marker containing
   `200000`. Print:
   `auto-compact: set autoCompactWindow to 200000 in ~/.claude/settings.json (Claude Code compacts at 200k tokens; delete the key to use the model's full window)`.

`blyte skill uninstall` (Claude, global): if the marker exists and the file's top-level
`autoCompactWindow` is still `200000`, remove that key. Then delete the marker. Any other value
means the user changed it, so it is kept. Without the marker, the key is never touched.

## Not covered

- Project and managed settings files, and `CLAUDE_CODE_AUTO_COMPACT_WINDOW` in the shell. These
  override the user file, and blyte cannot see them all at install time. Writing the user file is
  still correct: it is the lowest scope, so anything the user set elsewhere wins.
- Codex and other harnesses.
- A per-model value. A single top-level 200000 is used, because Claude Code caps it at each model's
  own window.

## Errors (no fallbacks)

- `~/.claude/settings.json` exists but cannot be read or is not a JSON object: install fails with an
  error naming the file. Today the status line step only prints a warning and goes on; this change
  makes both steps fail. A broken settings file also breaks Claude Code, so carrying on would hide
  it.
- The marker cannot be written: install fails naming the marker path. The settings file has
  already been written by then; the error says so, so the user knows the key is set but uninstall
  won't remove it.

## Reach: existing users

`blyte update` re-runs `blyte skill install --force` (`update::refresh_skills`). So existing users
get the setting on their next update, unless they already have their own setting or a marker.
`refresh_skills` must show the `auto-compact:` line in `blyte update`'s output. Release notes for the
version say so.

## Docs

The README and `docs/getting-started.md` say what install changes in `settings.json`: the status
line, and now the auto-compact window, along with `--no-autocompact`.

## Decided (with the user, 2026-11-04)

- **A. Existing users:** applied on `blyte update`, as described under "Reach".
- **B. Broken settings file:** the install fails, for both the status line and auto-compact steps.
