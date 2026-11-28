# A surface publishes without asking by default (2026-11-28)

Part 1 of `docs/specs/2026-11-28-publish-and-land-approval.md`. PT1 changes what PA1 adds and
starts from PA1's branch.

Rules for both tasks: no fallbacks (a missing or invalid value is an error that names it; the only
default is the documented `publish = "auto"`). Add no comments. Change only the files the task
names; if another file must change, say why in the final message.

### Task PA1: `[surface] publish` in `blyte.toml`, default `auto`

**Goal.** The surface's `publish` tool asks for a click on the page every time. The user decided
that by default it opens the pull request without asking, and that a setting restores the click.

**Files.** `crates/blyte-cli/src/config.rs`, `crates/blyte-cli/src/requests.rs`,
`crates/blyte-agent/src/tools.rs` (the `publish` tool's description only),
`crates/blyte-cli/src/conversation_text.rs`, `crates/blyte-cli/assets/conversation/reference.md`,
`crates/blyte-cli/assets/conversation/process.md`, `docs/usage.md`, `docs/security.md`,
`crates/blyte-cli/tests/test_publish_tool.rs`,
`crates/blyte-cli/tests/test_conversation_reference.rs`, and the test file that covers
`config::load` today.

**What to build.**
- `config.rs`: `pub enum PublishApproval { Auto, Ask }` (serde `rename_all = "lowercase"`,
  `Default` is `Auto`); `pub struct SurfaceConfig { pub publish: PublishApproval }` with
  `#[serde(deny_unknown_fields)]` and a serde default for the field; `RepoConfig` gains
  `#[serde(default)] pub surface: SurfaceConfig`. `publish = "never"` fails to load with serde's
  error, which names the value.
- `requests::publish_request_with_approval`: after the existing checks and before the approval
  step, read the setting with the function the surface already uses to read `blyte.toml` for this
  root (`config::load_with_fallback`; an error from it is returned as the tool's error). A folder
  that is not a repository cannot publish today and that does not change.
  - `Auto`: do not call `approver.ask_publish` and do not apply the "a publish approval is already
    pending" refusal; go on to the branch check, push and pull request as after an approval.
  - `Ask`: exactly today's behaviour.
  - In both cases, after the pull request is opened, record a notice entry in the surface's
    session: `Published <request>: branch <branch>, <pull request URL>` followed, for `Auto`, by
    ` (not asked: [surface] publish = "auto")`. Use the session's existing call for a notice entry.
- Model-facing text, so the model is not told a click always comes: the `publish` description in
  `tools.rs` `specs()`, `SANDBOX_ON_PARAGRAPH` in `conversation_text.rs`, the publish sections of
  `reference.md` and `process.md`. New wording, adapted to each place: publishing pushes the
  request's branch and opens a pull request; it asks the user first only when `blyte.toml` has
  `[surface] publish = "ask"`; merging is never done by the surface. Keep every other sentence.
- `docs/usage.md`: document `[surface] publish` beside where `fix_round` is documented.
  `docs/security.md`: a short paragraph with the trade-off from the spec's Part 1 ("What this
  gives up").

**Tests** (`cargo test -p blyte --test test_publish_tool --test test_conversation_reference`, the
config test file, and `cargo test -p blyte-agent --test test_publish_tool`):
1. Config: no `[surface]` table loads `PublishApproval::Auto`; `publish = "ask"` loads `Ask`;
   `publish = "never"` is an error containing `never`; `[surface] other = 1` is an error.
2. With no `[surface]` table, `SurfacePublishTool::publish` on a combined request returns the pull
   request URL with no approval entry recorded and no answer posted, and the session holds a
   notice containing `not asked`.
3. With `publish = "ask"` the existing approval tests pass: put that setting in their fixture's
   `blyte.toml` and change no assertion in them.
4. With `Auto`, each refusal still holds: a request of another surface, a request that is not
   combined, a turn started by another surface, a branch that moved.
5. The reference test that checks the publish wording asserts the new wording (`publish = "ask"`
   appears in `reference.md` and in `SANDBOX_ON_PARAGRAPH`).

**Depends on:** none. **Siblings (not in this worktree):** PT1 starts after this.

**Commit message:** `feat(surface): publish opens the pull request without asking unless blyte.toml says ask`

### Task PT1: a terminal surface can publish

**Not built (2026-11-28).** The run stopped as told: `blyte --terminal` builds only a
`ReplSession` and no `serve::Surface`, so it has no requests state for `SurfacePublishTool` to use.
Publishing from a terminal needs a design for that first. The text below is kept as written.

**Goal.** `blyte --terminal` registers no `publish` tool, and `TtyApprover::ask_publish` returns
`Approval::Unavailable`, so a terminal surface cannot publish under either setting.

**Files.** `crates/blyte-cli/src/repl.rs`, `crates/blyte-cli/src/tty_approver.rs`,
`crates/blyte-cli/tests/test_publish_tool.rs`, and the test file that covers the terminal surface
today if a registration test fits there better.

**What to build.**
- `repl.rs`: where the terminal surface is built (`ReplSession::try_new_with_approver`,
  `run_repl`), install the publish tool as `serve::run` does for a page surface
  (`SurfacePublishTool`, `PublishToolGuard::install`), with the terminal's approver. If the
  terminal surface has no `Surface` value to give `SurfacePublishTool`, stop and say exactly what
  is missing in the final message; do not build a second publish path.
- `TtyApprover::ask_publish`: print the details (request, branch, base, remote, title, the commit
  count) and ask `Publish? [y/N]` through the same input path `TtyApprover` uses for its other
  question; `y` or `yes` is `Approved`, anything else `Denied`; when standard input is not a
  terminal, `Unavailable`.

**Tests.**
1. Replace `tty_approver_returns_unavailable_for_ask_publish`: with input that is not a terminal
   the result is `Unavailable`; with the approver's test input seam (the one its other question
   uses) answering `y` it is `Approved`, and answering an empty line it is `Denied`.
2. A terminal surface's tool list contains `publish` when the folder is a repository.

**Depends on:** PA1 (dispatch with `--base wip/PA1`).

**Commit message:** `feat(surface): a terminal surface can publish`
