# Surface switcher: waiting on you, start in another folder, own app window

Three follow-ups to the surface switcher (`crates/blyte-cli/src/serve.rs`, `session.rs`,
`surface_registry.rs`, `surfaces_cmd.rs`, `assets/surface/index.html`). Facts checked against
`main` at `2567cc8b` by a research run (SWR1).

## Decisions

**Waiting on you.** A running surface is in exactly one state, kept in its registry entry
(`<state_dir>/surfaces/<id>.json`) beside `doing`:

| `state` | Meaning | Waiting on you? |
| --- | --- | --- |
| `working` | A request is running. | no |
| `approval` | A request is blocked on an approval the user has not answered. | yes |
| `replied` | The last request finished (reply or error) and no visible page has shown it. | yes |
| `idle` | Nothing is running and the user has seen the last reply. | no |

"Seen" is reported by the page: it tells its surface when it is visible and the session is not
busy. Nothing guesses it from open connections.

**Start in another folder.** The list can start a surface in any existing folder: the running
surface starts another `blyte` process there, detached, and the page goes to it. A folder that
does not exist is an error; nothing is created. A surface started this way has no terminal, so
the list also gets a Stop control for the current surface.

**Own app window.** `blyte` and `blyte open` open the surface in a Chrome-family app window (no
tabs or address bar) when one of Google Chrome, Chromium, Microsoft Edge or Brave Browser is
installed. A program's arguments are visible to other users of the machine and an app window's
browser may keep them for as long as it runs, so the address handed to the browser carries a
code that works once and for 60 seconds, never the surface's token.

## Rules for every task

- No fallbacks: a missing folder, an unreadable registry entry, a bad setting, an unknown state
  or a failed start is an error that names what is wrong. Nothing is guessed or skipped.
- Nothing here is a long-running job, so "resumable" does not apply.
- The model must gain no new ability: no new route may be callable with the message credential,
  and nothing new is offered to the model as a tool.
- Never print or log a token, a message credential or an open code, except the surface's own
  start line as today.
- Tests wait on conditions with a 30 s deadline and a failure message; no fixed sleeps standing
  in for a wait. Page tests follow `crates/blyte-cli/tests/test_surface_page.rs` (the page's
  script run under `node` with a mock DOM).
- User-facing wording is plain: "waiting on you", "needs your approval", "replied".

### Task SWA1

**Goal:** the list, the tab title and `blyte surfaces` show which surfaces are waiting on the
user. **Depends on:** none. **Siblings:** SWC1 runs at the same time in another worktree (app
window: `open_browser`, `cli.rs`, `main.rs`, the page's token start-up); SWB1 comes later
(start and stop from the list). Do not build those here.

**Files:** `crates/blyte-cli/src/session.rs`, `serve.rs`, `surface_registry.rs`,
`surfaces_cmd.rs`, `assets/surface/index.html`; tests `test_surface_registry.rs`,
`test_surface_switcher.rs`, `test_surfaces_cmd.rs`, `test_surface_page.rs`, and a new
`crates/blyte-cli/tests/test_surface_waiting.rs`. Other tests that build a registry entry need
the new field.

**What to build**

1. Registry entry: a required field `state` with the four values in the table (serialised as
   those lower-case words). An entry without it, or with another value, is treated exactly as
   `read_entries`/`live_entries` treat a malformed entry today; do not add a default.
2. The session keeps the state and writes it through the activity hook (extend the hook; today
   it carries only the `doing` text):
   - a request starts, or a queued one starts: `working`;
   - `SurfaceApprover::ask` or `ask_publish` records a pending approval: `approval`; when no
     approval is pending any more (answered, or `deny_all_pending`): `working`;
   - the worker finishes a request, with a reply or an error: `replied`, unless a page reported
     itself visible during that request's last 2 seconds or after it (see 3), in which case `idle`;
   - a surface that has just started or resumed: `idle`.
3. `POST /api/session/seen` (token required, like every other session route; empty body): if the
   state is `replied`, it becomes `idle` and the registry entry is rewritten; in every state the
   server records the time, for the rule in 2. Returns 204.
4. `GET /api/session` gains `state`. `surfaces_payload` rows gain `state`. The running-surface
   table of `blyte surfaces` gains a column for it, printing `waiting on you (approval)`,
   `waiting on you (replied)`, `working` or `idle`.
5. The page:
   - sends `POST /api/session/seen` when `document.visibilityState === "visible"` and the session
     is not busy: on load, on `visibilitychange`, and when an `idle` event arrives;
   - tab title: `<name>: waiting on you - blyte` while an approval is pending or while the last
     request has finished and the page has not been visible since; otherwise as today
     (`<name>: working - blyte`, `<name> - blyte`). `approval_requested` and `approval_answered`
     events update it;
   - list rows of running surfaces show a `waiting on you` badge with `needs your approval` or
     `replied` beside it, and `working` as a quieter badge; `idle` shows nothing. Rows that are
     waiting sort first, then the existing order;
   - the switcher button shows a dot (with `aria-label` text "another surface is waiting on
     you") when any OTHER surface is waiting. The page refreshes `/api/surfaces` every 15 s
     while visible, and when the switcher opens, as today;
   - `validateSurfaces` requires `state` to be one of the four values.

**Tests**

- `test_surface_waiting.rs`, a real surface with a scripted runner (follow
  `test_surface_start_order.rs` or `test_surface_stop_line.rs`): the registry entry's `state` is
  `idle` after start; `working` while a request runs (hold the runner on a gate); `replied` after
  it finishes; `idle` after `POST /api/session/seen`; `approval` while an approval is pending and
  `working` after it is answered; a request that ends in an error leaves `replied`; a `seen`
  sent while the request is running, less than 2 s before it ends, leaves `idle`; `seen` without
  a token is 403; `GET /api/session` reports the same state at each step.
- `test_surface_registry.rs`: an entry without `state`, and one with `state: "x"`, are handled
  as malformed entries are today (assert the existing behaviour, whatever it is).
- `test_surface_switcher.rs` / `test_surfaces_cmd.rs`: payload rows carry `state`; the table
  prints the four wordings.
- `test_surface_page.rs`: the title for each case; the badge texts; waiting rows sort first; the
  dot appears only for another surface; `seen` is posted when visible and not when hidden;
  `validateSurfaces` rejects a row with no `state`.

Run: `cargo test -p blyte --test test_surface_waiting --test test_surface_registry --test test_surface_switcher --test test_surfaces_cmd --test test_surface_page`

**Commit message:** `feat(surface): the list and the tab title show which surfaces are waiting on you`

### Task SWC1

**Goal:** the surface opens in its own app window, without its token in any program's
arguments. **Depends on:** none. **Siblings:** SWA1 runs at the same time in another worktree
(surface state, list rows, tab title); SWB1 comes later. Do not build those here, and keep
changes to `index.html` to the page's start-up and the leftover lookup named below.

**Files:** `crates/blyte-cli/src/serve.rs`, `cli.rs`, `main.rs`, `surfaces_cmd.rs` (`blyte
open`), `assets/surface/index.html`; a new module `crates/blyte-cli/src/open_window.rs`; tests in
a new `crates/blyte-cli/tests/test_open_window.rs` and `test_surface_page.rs`; docs where
`--no-open` is documented (`grep -rn -- '--no-open' docs/ README.md`).

**What to build**

1. Setting: `--open-in <app|browser>` on the surface (top-level, with the other surface flags,
   under the same rule that forbids them with `--terminal`, a prompt or a subcommand) and on
   `blyte open`. Without the flag, the environment variable `BLYTE_OPEN_IN` decides; without
   either, `app`. Any other value is an error naming the flag or variable and the value (exit 2).
   `--no-open` still opens nothing.
2. `open_window.rs`: `pub enum OpenIn { App, Browser }`, and
   `pub fn open_command(open_in, url, os, installed: &dyn Fn(&str) -> bool) -> Result<OpenPlan, String>`
   that returns the program and arguments to run and, when `app` was asked for and cannot be
   had, the notice to print. It is pure, so tests need no browser.
   - macOS, `app`: the first of `Google Chrome`, `Chromium`, `Microsoft Edge`, `Brave Browser`
     found as `/Applications/<name>.app` or `$HOME/Applications/<name>.app`; run
     `open -na "<name>" --args --app=<url>`.
   - Linux, `app`: the first of `google-chrome`, `google-chrome-stable`, `chromium`,
     `chromium-browser`, `microsoft-edge`, `brave-browser` found on `PATH`; run
     `<program> --app=<url>`.
   - `app` with none found: open in the default browser as today (`open <url>` / `xdg-open
     <url>`) and print once to stderr: `blyte: no Chrome, Chromium, Edge or Brave found for an
     app window; opening in the default browser (--open-in browser stops this notice)`. This is
     the documented behaviour of `app`, not a silent substitute.
   - `browser`: as today.
   `serve::open_browser` and the copy used by `blyte open` go through it. Leave
   `blyte dashboard --open` alone.
3. Open codes, so no token reaches a program's arguments:
   - the surface keeps a small set of open codes: 32 random bytes as hex from the same source as
     `generate_token`, each valid once and for 60 seconds, at most 16 held (creating a 17th
     removes the oldest);
   - `POST /api/session/open-codes` (token required) creates one and returns `{"code": "..."}`;
   - `POST /api/open` with body `{"code": "..."}` needs no token (add it to the exemption beside
     `GET /api/health`; the Host and Origin checks still apply): a valid code is removed and the
     reply is `{"token": "<the surface's token>"}`; an unknown, used or expired code is 403 with
     no detail. Compare with `constant_time_eq`;
   - the address given to the opener is `http://127.0.0.1:<port>/#open=<code>`. The surface
     makes its own code in-process at start. `blyte open <name>` reads the token file as today,
     asks that surface for a code over HTTP, and opens the `#open=` address; if that request
     fails it is an error naming the surface, with no fall back to the token address;
   - the start line printed to stdout stays `…/#token=<token>` as today (scripts and tests read
     it), and the token address keeps working in the page;
   - the page, on load with `#open=<code>`: posts it to `/api/open`, then continues exactly as
     if it had been loaded with `#token=<token>` (find how the page reads and keeps the token
     today and reuse it, including whatever it does to the address bar); if the exchange is
     refused it shows a plain message: `This link has been used or has expired. Run blyte open
     <name> to open this surface.` It must not loop or retry.
4. Remove the leftover lookup of an element with id `repo-name` in `applySessionState`
   (`index.html`, about line 3986; the element does not exist) and update the assertion in
   `test_surface_page.rs` (`header_shows_not_a_git_repository_when_session_repository_is_false`)
   to read the switcher button instead.
5. Docs: the new flag and variable, what an app window is, and the notice.

**Tests**

- `test_open_window.rs`, pure: for macOS and Linux, each browser in order of preference; `app`
  with none installed gives the default-browser command and the notice; `browser` gives today's
  command and no notice; the url is passed as one argument, unchanged.
- Setting: `--open-in x` and `BLYTE_OPEN_IN=x` fail with exit 2 and the message; the flag wins
  over the variable; `--open-in` with `--terminal` is refused like the other surface flags.
- Open codes, through `serve::route` or a real surface: creating one without the token is 403;
  a code exchanges for the token once; a second use is 403; a code older than 60 s is 403 (make
  the clock injectable; do not sleep 60 s); a 17th code removes the first; `POST /api/open`
  with an `Origin` of another port is refused as other POSTs are.
- The opener is called with an `#open=` address and never with the token: start a real surface
  with a recording opener (`serve::run_with_hooks` takes one) and assert the recorded address
  does not contain the token, while the start line does.
- `blyte open`: against a mock surface, it posts for a code with the token header and opens the
  `#open=` address; when the mock answers 500 it fails with the surface's name and opens nothing.
- `test_surface_page.rs`: load with `#open=abc` posts the code and then behaves as with a token;
  a 403 shows the message and makes exactly one request.

Run: `cargo test -p blyte --test test_open_window --test test_surface_page --test test_surface_switcher --test test_surfaces_cmd`

**Commit message:** `feat(surface): the surface opens in its own app window, by a one-time code`

### Task SWB1

**Goal:** start a surface in another folder from the list, and stop the current one.
**Depends on:** SWA1 and SWC1 (both are in this worktree's base).

**Files:** `crates/blyte-cli/src/serve.rs`, `surfaces_cmd.rs`, `assets/surface/index.html`; a
new module `crates/blyte-cli/src/surface_start.rs`; tests in a new
`crates/blyte-cli/tests/test_surface_start_from_list.rs` and `test_surface_page.rs`,
`test_surface_switcher.rs`.

**What to build**

1. `POST /api/surfaces/start` (token required), body `{"folder": "<path>"}`:
   - a leading `~/` is replaced with `$HOME` (unset or empty `HOME` is an error); after that the
     path must be absolute, exist and be a directory, else 400 with a message naming the path
     and which of those failed. Nothing is created;
   - spawns `std::env::current_exe()` with the working directory set to the folder, arguments
     `--no-open`, stdin null, stdout and stderr to a new file
     `<state_dir>/surface-starts/<16 hex>.log` (directory `0700`, file `0600`), in its own
     session (`setsid` in `pre_exec`) so it outlives this surface and its terminal. The child
     inherits the environment, minus any variable this surface sets to identify itself or its
     session (check what `serve.rs` and the session export, for example `BLYTE_SESSION`; list in
     your final message what you removed and why);
   - waits up to 30 s for the child's start line (`blyte: surface <name> at <url>`) in that
     file. On the line: asks the new surface for an open code (as `blyte open` does after SWC1)
     and replies `{"name": "<name>", "url": "http://127.0.0.1:<port>/#open=<code>"}`; the log
     file is removed. If the child exits first: 500 with the last 20 lines of the file, and the
     file is removed. If 30 s pass: the child is killed, 500 says so and gives the log's path.
     The token from the start line is never put in a reply or an error;
   - a thread reaps the child so it does not become a zombie while this surface runs.
2. `GET /api/surfaces` gains `"folders"`: the folders of repositories blyte knows
   (`<state_dir>/repos`, see `crates/blyte-cli/src/registry.rs`) that still exist, and the roots
   of running surfaces, without duplicates, sorted. Failing to read the registry is the same
   error `/api/surfaces` gives for its other parts today.
3. `POST /api/session/stop` (token required): replies 202, then stops the surface the way
   SIGINT does (the same flag and clean shutdown, including the `resume with:` line and the
   removal of its registry entry and token files).
4. The page, in the switcher:
   - a row "Start a surface in another folder": a text field for a path with the known folders
     offered as suggestions (`<datalist>`), and a Start button. While starting, the button is
     disabled and says `Starting…`. On success the page goes to the returned address the way a
     row click does (`navigateToSurface`). On failure the server's message is shown under the
     field, as text (never as HTML);
   - a Stop control on the current surface's row: one click asks "Stop this surface?" in place
     (no browser dialog), the second sends the request, and the page then shows `This surface
     has stopped. Resume it with: blyte --resume <name>`;
   - the empty-list text becomes `No other surfaces are running.`;
   - `validateSurfaces` requires `folders` to be a list of strings.

**Tests**

- `test_surface_start_from_list.rs`, with `BLYTE_STATE_DIR` in a temporary folder and a fake
  model server as other surface tests use: a start in a second temporary folder returns a name
  and an `#open=` address; the new surface answers `/api/health`; exchanging the code gives a
  token that works on its `/api/session`; the reply and the first surface's output contain no
  token; the new surface is still running after the first surface stops; stop the new one at the
  end of the test (`POST /api/session/stop`) and wait until its health check fails. A folder
  that does not exist, a file, and a relative path are each 400 with the message, and no process
  is left. A child that exits at once (point `HOME`/state so that start fails, or use a folder
  where start is refused) is 500 with the child's output. No token: 403.
- `POST /api/session/stop`: the surface prints the `resume with:` line and its registry entry
  and token file are gone; without the token it is 403 and the surface keeps running.
- `test_surface_switcher.rs`: `folders` lists known repositories and running roots once each,
  and leaves out one that no longer exists.
- `test_surface_page.rs`: the start row posts the folder and navigates on success; shows the
  message on failure, escaped; the Stop control needs two clicks; `validateSurfaces` rejects a
  payload without `folders`.

Run: `cargo test -p blyte --test test_surface_start_from_list --test test_surface_page --test test_surface_switcher`

**Commit message:** `feat(surface): start a surface in another folder from the list, and stop the current one`
