# Plan: independent surfaces that can message each other

Spec: `docs/specs/2026-11-25-independent-surfaces.md` (its last section overrides the rest).
Facts checked against v0.63.2 by research run RS11 and by reading `serve.rs` `run_inner`,
`surface_notify.rs`, `tools.rs` `resolve_in_workspace` and the `Surface` struct.

## Stages

| Stage | Tasks | Ships as |
|---|---|---|
| A. Several surfaces per checkout, each owning its runs | S1, S2, S3, S4, S5 | one PR |
| B. Registry, `blyte surfaces`, `blyte open` | S6 (written when A has merged) | one PR |
| C. Message credential, peer endpoint, `tell`, peer turns | S7, S8 (written when B has merged) | one PR |

Stage B and C tasks are not written yet: their contracts name code that stage A creates.

Order in stage A: S1, then S2 and S5 together (different files), then S3, then S4.
S3 and S4 both change `serve.rs`, so they run one after the other.

Every run is cherry-picked (`git cherry-pick -x`) onto `feat/independent-surfaces`.

### Task S1: surface ids, names and directories

**Goal.** Give surfaces an id, a name and a directory of their own, as a library with no
caller yet. Later tasks use it so that several surfaces can run in one checkout.

**Depends on:** none.

**Siblings.** S2 (start records), S3 (`serve.rs` uses this module), S4 (ownership) and S5
(`blyte clean`) are separate tasks and are **not** in this worktree. Do not change
`serve.rs`, `surface_notify.rs`, `clean.rs`, `dispatch.rs` or `tools.rs`.

**Files.**
- `crates/blyte-agent/src/sandbox.rs` (add two functions; change nothing else)
- `crates/blyte-cli/src/surface_dir.rs` (new)
- `crates/blyte-cli/src/lib.rs` (add `pub mod surface_dir;`)
- `crates/blyte-cli/tests/test_surface_dir.rs` (new)
- `crates/blyte-agent/tests/test_sandbox.rs` (add tests)

**What to build.**

In `crates/blyte-agent/src/sandbox.rs`, next to `surface_token_path`:

```rust
/// A surface id is `s` followed by exactly 16 lowercase hex digits.
pub fn validate_surface_id(id: &str) -> Result<(), String>;
/// `<surface_token_dir()>/<id>.token`. Errors if the id is invalid or the directory is unknown.
pub fn surface_token_path_for_id(surface_id: &str) -> Result<PathBuf, String>;
```

`validate_surface_id` error text: `invalid surface id: <id>`. Leave `surface_token_path`
(keyed by main checkout) in place; S3 and S5 still need it.

In `crates/blyte-cli/src/surface_dir.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceRecord {
    pub id: String,
    pub name: String,
    pub pid: u32,
    pub port: u16,
    pub session_id: String,
    pub started_at: u64,
}

pub fn new_surface_id() -> Result<String, String>;          // "s" + 16 hex digits read from /dev/urandom
pub fn validate_surface_name(name: &str) -> Result<(), String>;
pub fn surfaces_dir(main_checkout: &Path) -> PathBuf;        // <main_checkout>/.wt/surfaces
pub fn surface_dir(main_checkout: &Path, id: &str) -> PathBuf;   // .../surfaces/<id>
pub fn record_path(main_checkout: &Path, id: &str) -> PathBuf;   // .../<id>/surface.json
pub fn entries_path(main_checkout: &Path, id: &str) -> PathBuf;  // .../<id>/entries.jsonl
pub fn write_record(main_checkout: &Path, record: &SurfaceRecord) -> Result<(), String>;
pub fn read_record(main_checkout: &Path, id: &str) -> Result<SurfaceRecord, String>;
pub fn list_records(main_checkout: &Path) -> Result<Vec<SurfaceRecord>, String>;
pub fn is_running(record: &SurfaceRecord) -> bool;
pub fn pick_name(default_base: &str, requested: Option<&str>, running_names: &[String]) -> Result<String, String>;
```

Behaviour:
- `validate_surface_name`: 1 to 64 characters, each one of `A-Z a-z 0-9 . _ -`, and the first
  character a letter or digit. Error: `invalid surface name: <name> (use letters, digits, '.', '_' and '-', starting with a letter or digit)`.
- `write_record`: validates id and name, creates the surface directory, writes pretty JSON plus
  a newline to a temporary file in the same directory (name includes the process id), then
  renames it to `surface.json`.
- `read_record`: validates the id first. Errors name the path. The `id` inside the file must
  equal the directory name; if not, error `surface record <path> has id <x>, expected <id>`.
- `list_records`: if `.wt/surfaces` does not exist, returns an empty list (no surface has
  started here yet; this is the documented meaning, not a stand-in). Entries that are not
  directories are ignored. A directory whose name is not a valid surface id, or that has no
  readable, valid `surface.json`, is an error naming the directory. Result sorted by
  `started_at`, then `id`.
- `is_running`: connect to `127.0.0.1:<port>` with a 300 ms connect, read and write timeout,
  send `GET /api/health HTTP/1.1` with `Host` and `Connection: close`, read the whole
  response, parse the JSON body, and return true only if its `"surface_id"` string equals
  `record.id`. Any failure returns false. (`serve.rs` has a similar private
  `probe_existing_surface`; do not change or call it.)
- `pick_name`: with `requested = Some(n)`: validate it; if `n` is in `running_names`, error
  `a running surface is already named <n>; pass a different --name`; else return `n`.
  With `None`: validate `default_base` (if invalid, error
  `the repository directory name <default_base> cannot be a surface name; pass --name`);
  return `default_base` if not in `running_names`, else the first of `<default_base>-2`,
  `<default_base>-3`, ... that is not.

No fallbacks: a missing or malformed value is an error that names it, never a default. Not a
long-running job, so "resumable" does not apply. Do not write home-directory paths, private
repository names or company names anywhere (a test named `test_public_names` enforces it).

**Tests** (`cargo test -p blyte --test test_surface_dir`, `cargo test -p blyte-agent --test test_sandbox`). Use `tempfile` directories as the checkout.
1. `new_surface_id` twice gives two different ids, each passing `validate_surface_id`.
2. `validate_surface_id` rejects: empty, `s123`, `S0123456789abcdef`, `s0123456789ABCDEF`,
   `s0123456789abcde/`, `../s0123456789abcdef`, a 17-digit id.
3. `surface_token_path_for_id("s0123456789abcdef")` ends with `surface/s0123456789abcdef.token`
   (set `XDG_STATE_HOME` to a temp dir for the test, the way existing tests in that file do);
   an invalid id is an error.
4. `validate_surface_name` accepts `blyte`, `blyte-2`, `a.b_c`; rejects empty, `-x`, `.x`,
   `a b`, `a/b`, a 65-character name.
5. `write_record` then `read_record` returns the same record; no temporary file remains.
6. `read_record` on a file whose inner id differs gives the "expected" error.
7. `list_records`: no `.wt/surfaces` gives `Ok(vec![])`; two records come back sorted by
   `started_at`; a directory with no `surface.json` is an error naming it; a stray regular
   file in `.wt/surfaces` is ignored.
8. `is_running`: start a `TcpListener` on port 0 in a thread that answers one request with
   `HTTP/1.1 200 OK` and body `{"surface_id":"s0123456789abcdef","session_id":"x"}`: true for
   a record with that id and port; false for a record with another id (start a fresh
   listener); false for a port nothing listens on.
9. `pick_name`: `("blyte", None, [])` gives `blyte`; `("blyte", None, ["blyte","blyte-2"])`
   gives `blyte-3`; `("blyte", Some("x"), ["x"])` is the "already named" error;
   `("my repo", None, [])` is the "cannot be a surface name" error; `("my repo", Some("ok"), [])` gives `ok`.

**Constraints.** Existing tests pass unmodified.

**Commit message.** `feat: surface ids, names and per-surface directories`

### Task S2: runs record their surface

**Goal.** Record in each run's start record which surface started it, so a later task can
report a finished run to that surface only.

**Depends on:** S1 (uses `blyte_agent::sandbox::validate_surface_id`).

**Siblings.** S3 (the surface calls `set_blyte_surface`), S4 (readers of the new field) and
S5 (`blyte clean`) are separate and **not** in this worktree. Do not change `serve.rs`,
`surface_notify.rs`, `requests.rs`, `run_results.rs` or `clean.rs`. Nothing reads the new
field yet.

**Files.**
- `crates/blyte-agent/src/tools.rs`
- `crates/blyte-cli/src/cli.rs`
- `crates/blyte-cli/src/dispatch.rs`
- `crates/blyte-cli/src/direct_edit.rs`
- `crates/blyte-agent/tests/test_blyte_tool.rs`
- `crates/blyte-cli/tests/test_dispatch_surface.rs` (new)

**What to build.**

1. `tools.rs`: next to `BLYTE_TOOL`, a process-wide `static BLYTE_SURFACE: std::sync::RwLock<Option<String>>`
   with `pub fn set_blyte_surface(surface: Option<String>)` and `pub fn blyte_surface() -> Option<String>`.
   (Process-wide, not thread-local like `BLYTE_REQUEST`: one process is one surface.)
2. In the `blyte` tool, where `--request` is handled for `dispatch` and `edit-finish`:
   - if the model's arguments contain `--surface` or an argument starting `--surface=`, return
     `ToolOutcome::err("do not pass --surface; blyte sets it automatically")`;
   - if `blyte_surface()` is `Some(id)`, insert `--surface <id>` the same way `--request` is
     inserted (before a `--` separator if there is one, else appended). This happens whether
     or not a request id is set.
   No other subcommand gets `--surface`.
3. `cli.rs`: add `pub surface: Option<String>` to `DispatchArgs` and `EditFinishArgs`,
   declared the same way as their `request` field (same visibility in `--help`), flag
   `--surface`, value name `ID`.
4. `dispatch.rs` and `direct_edit.rs`: if `--surface` is given, validate it with
   `blyte_agent::sandbox::validate_surface_id` before anything is created; on failure print
   `blyte: <error>` and exit non-zero. Every writer of `.wt/<id>.start.json` (in `dispatch.rs`
   the first start record and the one written for the spawned run; in `direct_edit.rs` the
   one `edit-finish` writes) adds `"surface"`: the id string, or JSON `null` when the flag was
   not given. The same value in every writer for one run. A fix round reuses the existing
   start record and must not drop the field.

No fallbacks: never invent a surface id; absent means `null`. Not a long-running job, so
"resumable" does not apply. Do not write home-directory paths, private repository names or
company names anywhere (`test_public_names` enforces it).

**Tests.**
- `crates/blyte-agent/tests/test_blyte_tool.rs` (follow the existing `--request` tests there):
  1. with `set_blyte_surface(Some("s0123456789abcdef"))`, a `dispatch` call runs the fake
     `blyte` with `--surface s0123456789abcdef` in its arguments;
  2. the same for `edit-finish`;
  3. with a `--` separator in the model's arguments, `--surface <id>` comes before `--`;
  4. `status` (or another subcommand) does not get `--surface`;
  5. the model passing `--surface x` or `--surface=x` to `dispatch` gets the "do not pass" error and nothing runs;
  6. with `set_blyte_surface(None)`, no `--surface` is added.
  These tests share a process-wide value: make each one set it explicitly and guard them with
  one shared `Mutex` in the test file so they do not interleave; reset to `None` at the end.
- `crates/blyte-cli/tests/test_dispatch_surface.rs` (follow `crates/blyte-cli/tests/test_dispatch_request.rs` for how a dispatch is run in a test):
  7. `blyte dispatch <ID> ... --surface s0123456789abcdef` writes a start record whose `"surface"` is that id;
  8. without `--surface`, the start record has `"surface": null`;
  9. `--surface bad/id` exits non-zero with `invalid surface id` on stderr and creates no start record, branch or worktree;
  10. `edit-finish --surface s0123456789abcdef` writes it too (follow the existing `edit-finish` tests for setup).

Commands: `cargo test -p blyte-agent --test test_blyte_tool` and `cargo test -p blyte --test test_dispatch_surface`.

**Constraints.** Existing tests pass unmodified, except a test that asserts the exact set of
keys in a start record, which may gain `"surface"`.

**Commit message.** `feat: runs record the surface that started them`

### Task S5: clean old surface files

**Goal.** `blyte clean` removes the files the old single-surface design left behind, and the
directories of surfaces that stopped long ago.

**Depends on:** S1 (uses `crate::surface_dir` and `blyte_agent::sandbox::surface_token_path_for_id`).

**Siblings.** S2 (start records), S3 (`serve.rs` starts per-surface) and S4 (ownership) are
separate and **not** in this worktree. Do not change `serve.rs`, `surface_notify.rs`,
`surface_dir.rs` or `sandbox.rs`.

**Files.**
- `crates/blyte-cli/src/clean.rs`
- `crates/blyte-cli/tests/test_clean_surfaces.rs` (new)

**What to build.** In `clean.rs`:

```rust
/// Returns one line per thing removed (or, with dry_run, that would be removed).
pub fn clean_surfaces(main_checkout: &Path, older_than_days: u64, dry_run: bool, now_secs: u64)
    -> Result<Vec<String>, String>;
```

called once per repository from the same place `clean_repo_with_options` does its per-repository
work, with the same `older_than` and `dry_run` values and its lines printed the same way the
other removals are (and suppressed the same way under `--quiet`). An `Err` is printed as
`blyte: <error>` and makes `blyte clean` exit non-zero, after the rest of the clean has run.

Behaviour:
1. **Old surface files.** If `<main_checkout>/.wt/surface.json` exists: parse it (fields
   `port`, `session_id`); ask `http://127.0.0.1:<port>/api/health` (300 ms timeouts) and treat
   the old surface as running only if the JSON body's `"session_id"` equals the file's. If
   it is running, leave everything in this step alone. Otherwise remove `.wt/surface.json`,
   the directory `.wt/sessions/`, and the file
   `blyte_agent::sandbox::surface_token_path(main_checkout)` if present. If `.wt/surface.json`
   does not exist, remove `.wt/sessions/` if present (nothing reads it any more). An
   unparsable `.wt/surface.json` is an error naming the file; nothing in this step is removed.
2. **Stopped surfaces.** For each record from `surface_dir::list_records(main_checkout)`:
   if `surface_dir::is_running(&record)`, never touch it. Otherwise take the newest
   modification time among the files in its directory; if that is more than `older_than_days`
   days before `now_secs`, remove the directory and the file
   `surface_token_path_for_id(&record.id)` if present. A younger stopped surface is left
   alone, token included (a surface that is still starting looks stopped).
3. An error from `list_records` (a directory under `.wt/surfaces` with no valid record) is
   returned as the error; such a directory is never removed.

No fallbacks: an unreadable or malformed file is an error that names it, not a skip. Not a
long-running job, so "resumable" does not apply. Do not write home-directory paths, private
repository names or company names anywhere (`test_public_names` enforces it).

**Tests** (`cargo test -p blyte --test test_clean_surfaces`; a temp git repository as the
checkout, `XDG_STATE_HOME` set to a temp dir, files' modification times set with
`std::fs::File::set_modified` or `filetime` if the crate already depends on it):
1. old `.wt/surface.json` naming a port nothing listens on, plus `.wt/sessions/entries.jsonl`
   and the old token file: all three are removed, and three lines are returned;
2. the same with a listener answering health with the matching `session_id`: nothing removed;
3. `dry_run = true` on case 1: lines returned, nothing removed;
4. a stopped surface directory whose files are 15 days old, `older_than_days = 14`: directory
   and its token removed;
5. the same at 13 days old: kept, token kept;
6. a surface directory 15 days old whose port answers health with its `surface_id`: kept;
7. a directory `.wt/surfaces/s0123456789abcdef/` with no `surface.json`: `Err` naming it, directory still there;
8. unparsable old `.wt/surface.json`: `Err` naming it, `.wt/sessions/` still there.

**Constraints.** Existing tests pass unmodified.

**Commit message.** `feat: blyte clean removes the old surface files and long-stopped surfaces`

### Task S3: every start is its own surface

**Goal.** Every `blyte` start is its own surface: its own id, name, directory, token, port
and transcript. A second start in a checkout no longer opens the first one's page. A
finished run tells the surface that started it.

**Depends on:** S1, S2.

**Siblings.** S4 (a surface lists, reports and acts on only its own runs) is separate and
**not** in this worktree: after this task every surface in a checkout still sees every run.
S5 (`blyte clean`) is done separately. Do not change `requests.rs`, `run_results.rs` or `clean.rs`.

**Files.**
- `crates/blyte-cli/src/serve.rs`
- `crates/blyte-cli/src/cli.rs` (`SurfaceArgs`)
- `crates/blyte-cli/src/surface_notify.rs`
- `crates/blyte-cli/assets/surface/index.html` (page title only)
- tests: `crates/blyte-cli/tests/test_serve.rs`, `test_surface_notify.rs`, `test_dispatch_notify.rs`, `test_review_notify.rs`, `test_surface_page.rs`, and any other test that builds a `Surface` (they need the two new fields)

**What to build.**

`cli.rs`, `SurfaceArgs`:
- `--name <NAME>`: `pub name: Option<String>`, help "Name for this surface (default: the repository directory name)".
- `--resume <NAME>`: `pub resume: Option<String>`, help "Restart a stopped surface, by name or id, with its transcript". `conflicts_with = "name"`.
- `--port` help becomes "Port to listen on (default: a free port)". It stays `u16` with default 0.

`serve.rs`, `Surface` gains `pub surface_id: String` and `pub surface_name: String`.
`health_response` adds `"surface_id"` and `"surface_name"` to its JSON (keeps `"session_id"`).
Whichever API response already gives the page the repository name also gives it
`"surface_name"`; the page sets `document.title` to `<surface_name> - blyte`.

`serve.rs`, `run_inner`, in this order:
1. `worktree_root = find_git_toplevel(&config.repo_root)`.
   `main_checkout = crate::config::main_checkout(&worktree_root)`; if that is `None`, print
   `blyte: cannot find the main checkout of <worktree_root>` and return 1 (remove the
   `unwrap_or_else(|| root.clone())`). If the two differ after canonicalising, print to `err`
   `blyte: started in a linked worktree (<worktree_root>); this surface works in the main checkout <main_checkout>`.
   From here on the surface's root is `main_checkout` for everything: `config.repo_root`,
   `Surface.root`, the repository name, the surface directory.
2. Delete all the code that reads `<root>/.wt/surface.json`, probes an existing surface,
   prints "surface already running" and returns, and the line removing `.wt/surface.token`.
   This task neither reads nor writes `.wt/surface.json`, `.wt/sessions/entries.jsonl` or
   `sandbox::surface_token_path(main_checkout)`.
3. `running`: the records from `surface_dir::list_records(&main_checkout)` for which
   `surface_dir::is_running` is true. An `Err` from `list_records` is printed and returns 1.
4. Without `--resume`: `id = surface_dir::new_surface_id()`,
   `name = surface_dir::pick_name(&repo, args.name.as_deref(), &running_names)`; errors print
   `blyte: <error>` and return 1.
   With `--resume <v>`: among all records, those whose `id == v` or `name == v`.
   - if one of them is running: `blyte: surface <v> is already running (port <port>)`, return 1;
   - if none: `blyte: no stopped surface named <v> in <main_checkout>`, return 1;
   - if more than one stopped: `blyte: more than one stopped surface is named <v>; resume by id: <id>, <id>`, return 1;
   - else reuse its `id` and `name`. If a running surface has that name:
     `blyte: a running surface is already named <name>; stop it or resume by another name`, return 1.
5. Bind `127.0.0.1:<args.port>` as now (a taken port is an error, as now).
6. Token at `sandbox::surface_token_path_for_id(&id)`, written with the existing
   `write_token_file`, removed on exit and on every error path after it is written, as now.
7. Transcript at `surface_dir::entries_path(&main_checkout, &id)` (create the surface
   directory before `Session::start_with_log_io`). On resume the existing file is loaded, as
   the session code already does for an existing log.
8. Write the record with `surface_dir::write_record` (id, name, pid, actual port, session id,
   started_at). It is **not** removed on exit: a stopped surface's directory stays so it can
   be resumed. Remove the private `SurfaceFile` struct, `probe_existing_surface` and
   `read_token_file` if nothing uses them any more.
9. Call `blyte_agent::tools::set_blyte_surface(Some(id.clone()))` before the session starts.
10. Print `blyte: surface <name> at http://127.0.0.1:<port>/#token=<token>` (was `blyte: surface at ...`).

`surface_notify.rs`, `notify_runs_changed(main_checkout, id)` (signature unchanged):
- read `<main_checkout>/.wt/<id>.start.json`; a missing or unparsable file is
  `Failed("cannot read ...")` / `Failed("cannot parse ...")`;
- `"surface"` absent or `null`: return `NoSurface` (a run no surface started);
- otherwise validate the id (invalid: `Failed`), then `surface_dir::read_record`. If the
  surface directory does not exist: `Failed("run <id> belongs to surface <sid>, which has no directory under <main_checkout>/.wt/surfaces")`. Other read errors: `Failed` with the error;
- token from `surface_token_path_for_id(sid)`; not found: `NoSurface` (the surface is
  stopped; the run stays unreported until it is resumed); the empty and newline checks stay;
- post to the record's port as now.

No fallbacks: nothing here may fall back to another surface, another root or a default name
when a value is missing; each such case is one of the errors above. Not a long-running job,
so "resumable" in the long-job sense does not apply. Do not write home-directory paths,
private repository names or company names anywhere (`test_public_names` enforces it).

**Tests.** Follow the existing tests in each file for how a surface is started and stopped.
In `test_serve.rs`:
1. two surfaces started in the same checkout both serve: different ports, different
   `surface_id` from `/api/health`, names `<dir>` and `<dir>-2`, two directories under
   `.wt/surfaces/`, two token files; each token is refused (403) by the other surface;
2. `--name x` twice while the first runs: the second start returns 1 with "already named x";
3. after a surface stops: its `surface.json` and `entries.jsonl` remain, its token file is gone;
4. `--resume <name>` after stopping: same `surface_id` and name, and the transcript entries
   written before the stop are returned by the session events API;
5. `--resume <name>` while it runs: returns 1 with "already running";
6. `--resume nosuch`: returns 1 with "no stopped surface named nosuch";
7. started inside a linked worktree (`git worktree add` in the test): stderr has the "linked
   worktree" line, the surface directory is under the **main** checkout's `.wt/surfaces/`, and
   nothing is written under the linked worktree's `.wt/`;
8. `--port <p>` with `p` already bound by the test: returns 1 with "failed to bind";
9. `/api/health` returns `surface_id`, `surface_name` and `session_id`;
10. no `.wt/surface.json` and no `.wt/sessions/` are created by a start.
In `test_surface_notify.rs` (and adjust `test_dispatch_notify.rs`, `test_review_notify.rs` to the new layout):
11. start record with `"surface": null`: `NoSurface`, nothing posted;
12. start record naming a surface with record and token and a listener: `Told`, and the
    post carries that surface's token;
13. two surfaces with listeners, run owned by the second: only the second is posted to;
14. start record naming a surface with no directory: `Failed` with "has no directory";
15. record present, token file absent: `NoSurface`;
16. no start record for the id: `Failed`.
In `test_surface_page.rs`: 17. the page source sets the title from `surface_name`.

Existing tests of the removed behaviour ("surface already running", `.wt/surface.json`
written or removed, the token keyed by main checkout) are rewritten to the behaviour above.
All other existing tests pass unmodified apart from adding the two new `Surface` fields where
a test builds one.

Commands: `cargo test -p blyte --test test_serve --test test_surface_notify --test test_dispatch_notify --test test_review_notify --test test_surface_page`.

**Commit message.** `feat: every blyte start is its own surface`

### Task S4: a surface owns its runs

**Goal.** A surface lists, checks, combines, reports and acts on only the runs and requests
it started, so several surfaces in one checkout do not each report every run or push each
other's work.

**Depends on:** S3.

**Siblings.** None running at the same time. The registry, `blyte surfaces`, `blyte open`
and messages between surfaces are later work: do not start on them.

**Files.**
- `crates/blyte-cli/src/requests.rs`
- `crates/blyte-cli/src/run_results.rs`
- `crates/blyte-cli/src/serve.rs`
- tests: `crates/blyte-cli/tests/test_requests.rs`, `test_run_results.rs`, `test_serve.rs`

**What to build.** A run's start record `.wt/<id>.start.json` has `"surface"`: a surface id
string, or `null`/absent for a run no surface started. `Surface.surface_id` is this surface's id.

1. `requests.rs` and `run_results.rs`: each file's `parse_start_file` result gains
   `surface: Option<String>` (absent or `null` gives `None`; a non-string value is an error
   naming the file).
2. A run **belongs to this surface** when its `surface` is `Some(id)` with `id == surface.surface_id`.
   A request belongs to this surface when it has at least one start record and every start
   record with that request belongs to this surface. A request whose start records name
   more than one owner (counting "none" as an owner) is an error:
   `request <r> has runs from more than one surface`.
3. `requests.rs`: add
   `pub fn owned_run_ids(surface: &Surface) -> Result<HashSet<String>, String>` (ids of the
   start records that belong to this surface) and
   `pub fn request_owner_check(surface: &Surface, request: &str) -> Result<(), RequestActionError>`,
   which returns a new variant `RequestActionError::NotOwned(String)` with the message
   `request <r> belongs to another surface` (or `request <r> was not started by a surface`
   when its runs have no surface), `RequestActionError::BadRequest` for the mixed-owner error,
   and the existing not-found error when no start record has that request.
4. Filtering:
   - `run_results::report_finished_runs`: only runs that belong to this surface.
   - `requests::list_requests`: only requests that belong to this surface (a mixed-owner
     request is left out of the list and its error is returned the way this function already
     returns per-file errors; if it has no such channel, return `Err`).
   - `requests::maybe_combine`: returns `Ok(false)` without combining for a request that does
     not belong to this surface.
   - `serve.rs` `reconcile_runs`: runs `ensure_checks` and `maybe_combine` only for runs in `owned_run_ids`.
   - `serve.rs` `GET /api/runs`: only runs in `owned_run_ids`.
5. Refusals (HTTP 403, JSON error body with the message above; map `NotOwned` to 403 where
   `RequestActionError` is turned into a response):
   - `requests::push_request` and `requests::open_request_pull_request`: call
     `request_owner_check` first, before any other work.
   - `serve.rs` `POST /api/runs/{id}/decision` and `GET /api/runs/{id}/diff`: 403 with
     `run <id> belongs to another surface` (or `run <id> was not started by a surface`) unless
     the run belongs to this surface.

No fallbacks: a run with no surface is never treated as this surface's; an unreadable start
record is an error, not a skip. Not a long-running job, so "resumable" does not apply. Do not
write home-directory paths, private repository names or company names anywhere
(`test_public_names` enforces it).

**Tests.** Follow the existing tests in each file for building a `Surface` and writing start
records; give the test surface `surface_id = "s00000000000000aa"` and call the other
`"s00000000000000bb"`. Existing tests that write start records without `"surface"` and expect
the surface to see them must add `"surface": "s00000000000000aa"` to those records; that is
the only permitted change to existing tests.
1. `report_finished_runs` reports a finished run owned by `aa`; does not report (and writes no
   `.reported.json` for) one owned by `bb` or one with `"surface": null`.
2. `list_requests` returns the `aa` request and not the `bb` one or the unowned one.
3. `maybe_combine` on a ready `bb` request returns `Ok(false)` and writes nothing under `.wt/combined/`.
4. `push_request` on a combined `bb` request: `NotOwned`, and the test publisher saw no push.
5. `open_request_pull_request` on a `bb` request: `NotOwned`, publisher saw nothing.
6. `push_request` on a request with one `aa` run and one `bb` run: `BadRequest` with "more than one surface".
7. `GET /api/runs` lists only the `aa` run.
8. `POST /api/runs/<bb run>/decision` and `GET /api/runs/<bb run>/diff`: 403; no verdict or hold file written.
9. `reconcile_runs` writes `.wt/<id>.checks.json` for the `aa` run only.
10. a start record with `"surface": 5` makes `list_requests` return an error naming the file.

Commands: `cargo test -p blyte --test test_requests --test test_run_results --test test_serve`.

**Commit message.** `feat: a surface sees and acts on only its own runs and requests`
