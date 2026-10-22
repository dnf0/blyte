# Plan: tell people when a newer blyte is out (2026-10-22)

## Why

Nothing tells a user that a newer blyte exists. `blyte update` installs the latest release, but
people run old versions for days. For example, a user ran out of disk space on 2026-10-22 with a
version from before the squash-merge clean fix. Decision (user, 2026-10-22): `blyte dispatch` and
`blyte review` print a one-line notice when a newer release is out. They check at most once a day.
There is no self-update and no Slack post.

Claude sessions see the notice through `blyte review`, which they run in the foreground. Dispatch
output usually goes to `.wt/<id>.out`. The `blyte-implementer` skill tells Claude to pass the line
on to the user.

Verified 2026-10-22:
- `https://api.github.com/repos/dnf0/blyte/releases/latest` answers without auth (HTTP 200). Its JSON has
  `tag_name` (`v0.36.0`) and `body`, the commitizen changelog: `### Feat` headings and `- **agent**: nudge, then stop, fix runs that make no progress (#304)` bullets.
- `update::REPO` (`crates/blyte-cli/src/update.rs:15`) holds `owner/name`. `update::parse_version`
  (line 114) parses `0.35.2` / `v0.35.2`. `registry::state_dir()` (`registry.rs:11`) returns the state
  directory, or `None` when it cannot be found.
- `blyte-provider` already depends on `ureq = { version = "2.12", features = ["json"] }`; `blyte-cli`
  does not.
- `main.rs` (around line 41) matches `Command::Dispatch` and `Command::Review`.

### Task UN1: daily release check with a one-line notice on dispatch and review

Files:
- new `crates/blyte-cli/src/update_notice.rs`, and `pub mod update_notice;` in `crates/blyte-cli/src/lib.rs`;
- `crates/blyte-cli/src/main.rs`: only the `Command::Dispatch` and `Command::Review` arms;
- `crates/blyte-cli/Cargo.toml`: add `ureq` with the same version and features as `blyte-provider`;
- new test `crates/blyte-cli/tests/test_update_notice.rs`;
- `docs/getting-started.md`: one sentence after line 24, where `blyte update` is described;
- `crates/blyte-cli/assets/blyte-implementer/SKILL.md`: one sentence in section 4, near `blyte review T1`.

No fallbacks. A failed check is reported, never silently skipped. The only default is the
documented opt-out.

1. **`update_notice.rs`, pure parts** (all `pub` so the test file can call them):
   - `summarize(body: &str) -> String`:
     - take the lines starting with `- ` (after trimming);
     - strip the `- ` and any `**`;
     - join them with `; `;
     - cut the result to 200 characters on a char boundary, ending in `…` when cut;
     - return an empty string when there are no bullets.
   - `notice_line(current: &str, tag: &str, summary: &str) -> Option<String>`:
     - `Some` only when `update::parse_version(tag)` is greater than `parse_version(current)`;
     - text: `blyte: {tag without v} is out (you have {current}): {summary}. Run \`blyte update\`.`;
     - with an empty summary, leave out the `: {summary}` part.
     - If either version does not parse, return `None` and let `check` report it (see 2).
   - `struct Cache { checked_at: u64, tag: Option<String>, summary: Option<String>, error: Option<String> }`, serde, stored as JSON.
2. **`check(state_dir: &Path, current: &str, now: u64, fetch: impl FnOnce() -> Result<(String, String), String>) -> Vec<String>`**:
   - Returns the lines to print. `fetch` returns `(tag_name, body)`.
   - Read `<state_dir>/latest-release.json`. A missing file means fetch. If it is unreadable, fetch and overwrite it.
   - **Fresh** (`now - checked_at < 86_400`): do not call `fetch`. Use the cached `tag`/`summary` and
     return the notice line, if any. Print the notice on every run until the user updates.
   - **Stale, fetch succeeds**: write `{checked_at: now, tag, summary: summarize(body), error: None}`
     and return the notice line, if any.
   - **Stale, fetch fails**: write `{checked_at: now, error: Some(e)}` and keep the previous `tag`/`summary`.
     Return `blyte: could not check for a newer release: {e}`, plus the notice line from the kept tag, if any.
     Because `checked_at` is updated, the error appears at most once a day.
   - If the fetched tag does not parse as a version, return `blyte: could not check for a newer release: unexpected tag {tag}`.
   - If the cache cannot be written, add `blyte: could not save the release check to {path}: {e}`.
3. **`print_notice()`**, called first in the `Dispatch` and `Review` arms of `main.rs`:
   - When `BLYTE_NO_UPDATE_CHECK` is set to anything non-empty, do nothing (the documented opt-out, for CI and tests).
   - When `registry::state_dir()` is `None`, print `blyte: could not check for a newer release: no state directory (set HOME or BLYTE_STATE_DIR)`.
   - Otherwise call `check` with `env!("CARGO_PKG_VERSION")`, the current unix time, and a fetch that:
     - GETs `https://api.github.com/repos/{update::REPO}/releases/latest` with `ureq`;
     - sends headers `User-Agent: blyte/{version}` and `Accept: application/vnd.github+json`;
     - uses a 3-second total timeout (`AgentBuilder::new().timeout(Duration::from_secs(3))`);
     - reads `tag_name` and `body` (a `null` body counts as empty);
     - turns a non-200 status into `Err("HTTP {code}")`.
   - Print each returned line to stderr.
4. **Tests** (`test_update_notice.rs`, temp dirs, no network):
   - `summarize` on the real body from "Verified" above (`### Feat\n\n- **agent**: nudge, then stop, fix runs that make no progress (#304)\n`)
     gives `agent: nudge, then stop, fix runs that make no progress (#304)`; and a body over 200 characters is cut.
   - `notice_line("0.35.0", "v0.36.0", "x")` is `Some(..."0.36.0 is out (you have 0.35.0): x...")`. Equal and older versions give `None`.
   - `check` with no cache and a fetch returning a newer tag gives the notice line and writes the cache.
   - `check` with a fresh cache does not call the fetch: pass a closure that panics. It still gives the cached notice.
   - `check` with a stale cache and a failing fetch gives the error line and keeps the old tag's notice. A second call at `now + 60` gives no error line.
   - `check` with the same version gives no lines.
   - Make sure the existing dispatch and review tests do not reach the network. Set `BLYTE_NO_UPDATE_CHECK=1` where those tests build their `Command`
     (`grep -rln "cargo_bin\|CARGO_BIN_EXE" crates/blyte-cli/tests`). If many files need it, a shared helper is fine.
     Alternatively, if the tests already set `BLYTE_STATE_DIR` to a temp dir, write a fresh cache there.
     Pick whichever touches fewer files and say which in the commit message.
5. **Docs**:
   - `getting-started.md`: `blyte dispatch` and `blyte review` check for a newer release once a day and print one line when there is one. Set `BLYTE_NO_UPDATE_CHECK=1` to turn this off.
   - `SKILL.md`, section 4: `If blyte review prints "blyte: <version> is out", tell the user, with that line.`

Gate: the repository checks. Commit: `feat(cli): tell the user once a day when a newer blyte release is out`
