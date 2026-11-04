# Plan: `blyte skill install` sets the auto-compact window to 200k

Spec: `docs/specs/2026-11-04-install-autocompact-window.md` (read it first; it is the authority).

### Task AW1

Implement the spec in full. Files:
- `crates/blyte-cli/src/skill.rs`
- `crates/blyte-cli/src/cli.rs` and `crates/blyte-cli/src/main.rs` (the flag)
- `crates/blyte-cli/src/update.rs`
- `README.md` and `docs/getting-started.md`
- a new `crates/blyte-cli/tests/test_skill_autocompact.rs`
- `crates/blyte-cli/tests/test_skill_statusline.rs`, but only where the broken-settings behaviour changes.

1. **Flag.** Add `--no-autocompact` to `skill install`, next to `--no-statusline` (help text:
   "Do not set Claude Code's auto-compact window"). Pass it through `install_for` and `install`
   like `no_statusline`, and update every caller.
2. **Settings logic in `skill.rs`.** Write pure functions over the settings text, like
   `set_status_line`, so they can be tested without files:
   - `set_auto_compact_window(settings: &str) -> Result<AutoCompact, String>`. `AutoCompact` is an
     enum with two variants:
     - `Kept { key: String, value: String }`. Return it when the JSON already has a top-level
       `autoCompactWindow`, a `modelSettings.<any>.autoCompactWindow`, or
       `env.CLAUDE_CODE_AUTO_COMPACT_WINDOW`. Check them in that order, and the first one found
       is reported. `key` is its JSON path, for example `modelSettings.claude-opus-5-5.autoCompactWindow`.
     - `Set(String)`: the new text, with top-level `"autoCompactWindow": 200000` added, pretty
       printed with a trailing newline.

     Empty text is treated as `{}`. Text that is not a JSON object is an `Err`.
   - `remove_auto_compact_window(settings: &str) -> Result<Option<String>, String>`. It removes the
     top-level key only when its value is the number `200000`. It returns `None` when nothing
     changed.
   - Use a constant `AUTO_COMPACT_WINDOW: u64 = 200_000`.
3. **`install_for`** (Claude, global):
   - Read `~/.claude/settings.json` once. Apply the status line step, unless `--no-statusline`.
     Then apply the auto-compact step, unless `--no-autocompact` or the marker
     `registry::state_dir()/autocompact-window` exists.
   - Write once, with the existing one-time backup, and only if something changed.
   - Then write the marker containing `200000` and print the spec's `auto-compact: set …` line.
     For `Kept`, print `auto-compact: keeping your <key> (<value>)`. With the marker, print nothing.
   - **Errors, no fallbacks:** if `state_dir()` is `None`, that is an error naming the missing
     variables. An unreadable or unparseable settings file is an `Err` naming the file, for both
     steps; replace today's `eprintln!("warning: could not update …")` path.

     If the marker write fails after the settings were written, the error names the marker path
     and says the key was set but uninstall will not remove it.
4. **`uninstall_for`** (Claude, global):
   - If the marker exists, apply `remove_auto_compact_window` to the settings and write it if it
     changed, then delete the marker.
   - Never touch the key without the marker.
   - An unreadable or unparseable settings file is an `Err` naming the file. Today it is ignored
     silently; change that.
5. **`update::refresh_skills`.** It currently discards the child's output.
   - On success, add every stdout line that starts with `auto-compact:` to `lines`, after
     `skill refreshed for …`.
   - On failure, append the child's last non-empty stderr line to the existing
     `skill not refreshed …` message, so the cause is shown.
6. **Docs.** In `README.md` and `docs/getting-started.md`, where install's changes to
   `settings.json` are described (or next to the status line if they aren't), say that install sets
   `autoCompactWindow` to 200000 unless one is already set. Also describe `--no-autocompact` and
   what uninstall removes.
7. **Tests.** Every test sets `HOME` and `BLYTE_STATE_DIR` to temp folders; never touch the real
   ones. Cover:
   - the pure functions: empty input; the top-level key; a `modelSettings` key; the `env` key; a
     non-object; other keys preserved; remove with 200000; remove with another value; remove with
     no key.
   - running the binary `skill install` in a temp home with no settings file: the key and marker
     are written and the `auto-compact: set` line is printed.
   - a second install doesn't write again: it prints nothing, and a key removed by hand between
     installs stays removed.
   - an existing `modelSettings` window is kept and printed, and no marker is written.
   - `--no-autocompact` leaves both the key and the marker absent.
   - uninstall removes the key and the marker. When the key was changed to 300000 before
     uninstall, the key stays and the marker goes.
   - broken JSON in `settings.json` makes install exit non-zero, naming the file.
   - `refresh_skills` passing the `auto-compact:` line through, if the existing update tests make
     that practical; otherwise test the line filtering as a small pure helper.

No fallbacks: no default state dir, no silent skips, no warnings in place of errors.

Check: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`.
