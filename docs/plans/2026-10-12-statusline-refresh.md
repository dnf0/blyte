# Status line refreshes while idle

Claude Code re-runs a `statusLine` command only when the session changes, so the blyte cost part
(`blyte $X (N runs)`) goes stale while a dispatch runs in the background and the conversation sits idle.
The `statusLine` field `refreshInterval` (seconds, minimum 1) re-runs it on a timer
(https://code.claude.com/docs/en/statusline.md). blyte should set it, including on machines where the
status line is already `blyte statusline`, so that `blyte update` (which runs `skill install --force`)
brings existing users up to date.

### Task SL1

**Goal:** `blyte skill install` writes `"refreshInterval": 30` into the Claude Code `statusLine`, and adds it
to an existing `blyte statusline` entry that lacks it.

**Files:** `crates/blyte-cli/src/skill.rs` (`set_status_line`, `remove_status_line`),
`crates/blyte-cli/tests/test_skill_statusline.rs`. Touch nothing else.

**Behaviour of `set_status_line(settings) -> Result<Option<String>, String>`:**
1. Add `pub const STATUS_LINE_REFRESH_SECS: u64 = 30;` in `skill.rs`.
2. No `statusLine`: write `{"type": "command", "command": "blyte statusline", "refreshInterval": 30}`.
3. `statusLine` whose command does not start with `blyte statusline`: wrap it as today
   (`blyte statusline --after '<old>'`). Keep every other key already in that `statusLine` object
   (for example `padding`), set `type` to `"command"`, and set `refreshInterval` to 30 only if the object has none
   (keep a user's own value).
4. `statusLine` whose command already starts with `blyte statusline`: if it has a `refreshInterval`, return
   `Ok(None)` as today; if not, add `"refreshInterval": 30` and return the new settings.
5. Key order in the rest of the file stays as it is (the existing test 2 checks this; keep it passing, updating
   only its expected `statusLine` object to include the new key).

**Behaviour of `remove_status_line`:** unchanged, except that when it restores a wrapped command it also removes
`refreshInterval` if its value is exactly 30 (blyte's value); any other value is the user's and stays.

**Tests to add** (in `test_skill_statusline.rs`, same style as the existing ones):
- empty settings give a `statusLine` with `"refreshInterval": 30`;
- an existing bare `blyte statusline` entry without `refreshInterval` gains it (`Some`), and running again is `None`;
- an existing `blyte statusline --after '...'` entry with `"refreshInterval": 5` is left alone (`None`);
- wrapping a user command that has `padding: 2` keeps `padding`;
- remove: a wrapped entry with `refreshInterval: 30` restores the old command without `refreshInterval`; with
  `refreshInterval: 5` it keeps 5.

**Check:** `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`

**Commit:** `feat(skill): status line refreshes every 30s so dispatch costs update while idle`
