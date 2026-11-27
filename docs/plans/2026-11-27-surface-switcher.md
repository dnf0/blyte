# Plan: switching between surfaces from inside a surface

Spec: `docs/specs/2026-11-27-surface-switcher.md`.

### Task SW1: the surface switcher

**Depends on:** none. **Siblings:** none.

**Goal.** The name box in a surface's header becomes a button that lists the surfaces running on
this machine and takes the window to the one picked; the window title says when the surface is
working. **Why:** with several surfaces open, each in its own browser tab, there is no way to get
from one to another or to tell them apart in the window list.

**Files.**
- `crates/blyte-cli/src/surfaces_cmd.rs`: move the code in the `open` command that reads a
  surface's token and builds its address into
  `pub fn surface_url(entry: &RegistryEntry) -> Result<String, String>`; the `open` command calls
  it and behaves as before (same messages, same exit codes).
- `crates/blyte-cli/src/serve.rs`: the route `GET /api/surfaces`.
- `crates/blyte-cli/assets/surface/index.html`: the button, the list, the title.
- `crates/blyte-cli/tests/test_surface_switcher.rs` (new); additions to
  `crates/blyte-cli/tests/test_surface_page.rs` for the page.

**What to build.**

1. **`pub fn surfaces_payload(registry_dir: &Path, current_id: &str) -> Result<serde_json::Value, String>`**
   (in `surfaces_cmd.rs`). It returns
   `{"surfaces": [{"id", "name", "root", "branch", "doing", "current", "url", "error"}]}` from
   `surface_registry::live_entries(registry_dir)`, sorted by `name`, then `started_at`.
   `root` is the folder's full path as a string; `branch` is a string or `null`; `current` is
   true for the entry whose id is `current_id`; `url` is `surface_url(entry)` and `error` is
   `null`, or, when `surface_url` fails, `url` is `null` and `error` is its message. A failing
   `live_entries` is an `Err`. If no entry has `current_id`, that is an `Err` naming the id.
2. **`GET /api/surfaces`** answers with that payload for this surface's registry directory and
   id. It needs the surface's token like the other `/api` routes (use the same check). A payload
   error is a 500 with the message as JSON `{"error": ...}`. The response is not written to any
   log, transcript or event stream.
3. **The page.**
   - `#repo-name` becomes `<button id="surface-switcher" class="repo-name" type="button"
     aria-haspopup="listbox" aria-expanded="false">`, still showing the text it shows today.
     Keep the element reachable as `repo-name` wherever the script sets its text (change those
     lookups to the new id).
   - Clicking it fetches `/api/surfaces` (with the token, as the page's other fetches do) and
     opens a list under the button. Each row: the name, the full `root` path, the branch when not
     null, and `doing`, cut to one line with CSS. The current surface's row is marked and is not
     a link. A row with `url: null` is disabled and shows its `error`. Any other row, when
     picked, sets `window.location.href` to its `url` and then calls `window.location.reload()`
     only if the new address has the same origin as the current one (a change of fragment alone
     does not load the page).
   - With no other surface running, the list shows the current row and the line
     `No other surfaces are running. Start one by running blyte in another folder.`
   - A failed fetch shows the error text in the list; it does not show an empty list.
   - The list closes on Escape, on a click outside it, and after a pick; `aria-expanded` follows.
     Rows are reachable with Tab and picked with Enter. Use the page's existing colours, borders
     and popover styling (look at how the model select and other menus are styled); no new
     colours.
   - Write the list's markup with a pure function
     `renderSurfaceList(surfaces)` that returns a string and escapes every value it inserts, so
     it can be tested without a browser.
   - **Title.** Where the script sets `document.title` from `surface_name`, set
     `<name>: working - blyte` while the session is busy and `<name> - blyte` otherwise, and
     update it wherever the working indicator is shown or hidden.

**Tests.**
- `crates/blyte-cli/tests/test_surface_switcher.rs`, isolated the way
  `tests/test_surfaces_cmd.rs` isolates the registry and token directories (nothing outside
  temporary directories; run with `cargo test -p blyte --test test_surface_switcher`):
  - two live entries with readable tokens: both listed, sorted by name, `current` true for
    exactly the one asked for, each `url` is `http://127.0.0.1:<port>/#token=<its token>`;
  - an entry whose token file is missing: listed with `url` null and a non-empty `error`;
  - an entry whose process is not alive is not listed;
  - a `current_id` that is not in the registry: `Err` naming it;
  - `blyte open <name>` still prints what it printed before (the existing tests in
    `test_surfaces_cmd.rs` pass unmodified).
- `crates/blyte-cli/tests/test_surface_page.rs` (the way its existing tests run the page's
  functions): `renderSurfaceList` marks the current row, disables a row with `url: null` and
  shows its error, escapes a name containing `<script>`, and shows the "No other surfaces" line
  for a single row; the title function returns `x: working - blyte` when busy and `x - blyte`
  when not.

**Constraints.** No fallbacks: a missing token, an unreadable registry or a failed fetch is
shown as the error it is, never as an empty or shortened list. Not a long-running job, so nothing
to make resumable. Existing tests pass unmodified; if one cannot, stop and report it instead of
editing it. No home-directory paths in code or tests. Do not change `surface_registry.rs` or the
registry entry's fields.

**Commit message:** `feat(surface): switch between running surfaces from the name in the header`
