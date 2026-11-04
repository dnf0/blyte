# Status line: show the real auto-compact window, not an assumed 200k

## Problem

`blyte statusline` shows `context Xk of Yk` and suggests `/compact` at 60% of `Y`. It takes `Y` from
the top-level `autoCompactWindow` in `~/.claude/settings.json`, and when that key is missing it
assumes 200k (`base_line` in `crates/blyte-cli/src/statusline.rs`). blyte never sets that key.
A user without it (reported 2026-11-04) sees "of 200k", while Claude Code actually compacts at the
window tuned for the model: about 967k on 1M-context models such as `claude-opus-5-5`. The line
claims a limit that does not exist.

## Facts (Claude Code docs, checked 2026-11-04)

- `autoCompactWindow`: tokens, 100000 to 1000000, capped at the model's window; any settings file.
- `modelSettings.<canonical model id>.autoCompactWindow`: the same, or `"auto"` for the tuned window.
  `/autocompact` saves here. For that model it beats the top-level key in the same file. Claude Code
  matches the model's alias, date-suffixed and `[1m]` spellings to the canonical entry.
- `CLAUDE_CODE_AUTO_COMPACT_WINDOW` (environment): beats every setting. Accepts `k`/`M` suffixes.
- Settings scopes, highest first: managed, local (`.claude/settings.local.json`), project
  (`.claude/settings.json`), user (`~/.claude/settings.json`).
- The status line's input JSON has no compaction field; it has `model.id`,
  `context_window.context_window_size`, `context_window.used_percentage` and
  `workspace.project_dir`.
- The `--autocompact` launch flag is invisible to a status line; this change cannot see it.

### Task AC1

Make `blyte statusline` use the auto-compact window Claude Code uses, and the model's own window
when none is set, instead of assuming 200k.

Files: `crates/blyte-cli/src/statusline.rs`, `crates/blyte-cli/tests/test_statusline.rs`.

1. Replace `compact_window_from_settings()` with a resolver that returns one of:
   `Window(u64)` (an explicit window), `Tuned` (nothing set, or `"auto"`), or
   `Invalid(String)` (a message naming the variable or file and what is wrong). Make the inputs
   explicit so tests can drive it: the model id, the environment variable's value, and the list of
   settings files in scope order. Resolution:
   - `CLAUDE_CODE_AUTO_COMPACT_WINDOW`, if set and non-empty: parse a plain integer or a number with
     a `k` (×1000) or `M` (×1000000) suffix; it must be in 100000..=1000000, otherwise `Invalid`.
   - Otherwise go through the settings files in this order, highest first:
     macOS managed settings `/Library/Application Support/ClaudeCode/managed-settings.json`,
     `<project_dir>/.claude/settings.local.json`, `<project_dir>/.claude/settings.json`,
     `~/.claude/settings.json`. `<project_dir>` is `workspace.project_dir` from the input JSON; if
     it is missing, skip the two project files. A file that does not exist is skipped. A file that
     exists but cannot be read or parsed as JSON gives `Invalid` naming it.
   - In each file, first `modelSettings.<key>.autoCompactWindow`, then top-level
     `autoCompactWindow`. The first file that has either decides: a number in 100000..=1000000 is
     `Window`, `"auto"` is `Tuned`, anything else is `Invalid`.
   - `<key>` matching: take `model.id` from the input JSON; compare each `modelSettings` key with
     it after removing from both a trailing `[...]` suffix (such as `[1m]`) and a trailing
     `-YYYYMMDD` date. Without a `model.id`, skip the per-model lookup.
   - Nothing found: `Tuned`.
2. In `base_line`, take that result instead of `Option<u64>`:
   - `Window(w)`: limit = `min(size, w)`, the text and the 60% `/compact` suggestion as today.
   - `Tuned`: limit = `size` (the model's window), the same text and suggestion. Never 200k unless
     the model's window is 200k.
   - `Invalid(msg)`: `context {used}k of {size}k (auto-compact window unknown: {msg})`, with no
     suggestion.
   Update the doc comment of `base_line` to match. Remove every `unwrap_or(200_000)`.
3. Keep `run` reading files only once per call; nothing is written.
4. Tests in `test_statusline.rs`: keep the existing ones passing, adjusting only the parameter type.
   Add:
   - no window anywhere, size 1000000, 30% used: `context 300k of 1000k`, no suggestion; 65%: the
     suggestion;
   - top-level `autoCompactWindow: 200000` in the user file, size 1000000, 30%: `of 200k` with the
     suggestion;
   - `modelSettings.claude-opus-5-5.autoCompactWindow: 500000` with `model.id`
     `claude-opus-5-5[1m]` beats a top-level 200000 in the same file;
   - a project-file window beats a user-file window;
   - `"auto"` in a higher file beats a number in a lower one, giving the model window;
   - the environment variable `400k` beats all files; `50k` and `abc` give the `Invalid` text naming
     the variable;
   - a settings file with broken JSON gives the `Invalid` text naming the file.
   Use temp folders for the files; do not read the real home folder in tests.

No fallbacks: never substitute 200k or any other guessed window; unknown or invalid values are shown
as such. Do not change `blyte skill install` or any settings file.

Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.
