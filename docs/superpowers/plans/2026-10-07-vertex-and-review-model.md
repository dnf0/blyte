# Vertex route

Goal: blyte can use models that are only on Vertex AI, such as Gemini 4 Argon (GA in the Vertex
catalogue in `global` and `us-central1`, not in the Gemini API model list), so blyte can orchestrate
with Argon while 3.8 Flash implements. Access for our projects is still pending (countTokens returns
404 "not found or no access" on 2026-10-07); the task builds and tests without it.

### Task VX1: `vertex/<model>` reaches Gemini through Vertex AI with application-default credentials

**Goal and why.** Models only on Vertex (Gemini 4 Argon) must be reachable. Vertex takes the same
request body as the Generative Language API; only the URL and auth differ.

**Files.** `crates/blyte-provider/src/gemini.rs`, `crates/blyte-provider/src/registry.rs`,
`crates/blyte-provider/src/lib.rs` (pricing only). Unit tests in those modules.

**What to build.**
- `detect_provider_kind`: a model starting with `vertex/` is Gemini (check it before the other
  prefixes). `resolve_provider` strips `vertex/` and builds the Gemini provider in Vertex mode; a bare
  `vertex` means Vertex mode with the default model.
- Gemini Vertex mode (e.g. an enum field `Auth::ApiKey(String) | Auth::Vertex { project, location }`):
  - project from `GOOGLE_CLOUD_PROJECT`, else `CLOUDSDK_CORE_PROJECT`; neither set →
    `ProviderError::MissingKey("GOOGLE_CLOUD_PROJECT")`. Location from `GOOGLE_CLOUD_LOCATION`, default `global`.
    No API key is needed in this mode.
  - URL: a pure function `vertex_url(project, location, model) -> String`:
    host `aiplatform.googleapis.com` when location is `global`, else `<location>-aiplatform.googleapis.com`;
    path `/v1/projects/<project>/locations/<location>/publishers/google/models/<model>:generateContent`.
    `<model>` is the model with any effort suffix removed, exactly as the API-key path does.
  - Auth header `Authorization: Bearer <token>`. The token comes from running
    `gcloud auth application-default print-access-token` (no new crates). Cache it in the provider
    and reuse it for 45 minutes; fetch a new one after that or after an HTTP 401. A failure to get a token is
    a `ProviderError` whose message says to run `gcloud auth application-default login`.
    The token is never logged or printed. Put the command behind a small trait or a function
    pointer so tests can inject a fake token source.
  - Request body, response parsing, effort/thinking handling: unchanged, shared with the API-key path.
  - `model_id()` returns `vertex/<model>` in Vertex mode.
- `pricing`: add `gemini-4-argon`: input 2.00, cached 0.10, output 10.00 (USD per million; introductory
  price announced 2026-09-30, cached input 95% off).

**Tests.**
- `detect_provider_kind(Some("vertex/gemini-4-argon"))` is Gemini; `resolve_provider` with
  `GOOGLE_CLOUD_PROJECT` unset and `CLOUDSDK_CORE_PROJECT` unset returns `MissingKey("GOOGLE_CLOUD_PROJECT")`
  (serialise env-var tests the way existing registry tests do).
- `vertex_url("p","global","gemini-4-argon")` and `vertex_url("p","us-central1","gemini-4-argon")` give the exact URLs above.
- Effort suffix: Vertex mode with `gemini-4-argon-high` puts `gemini-4-argon` in the URL and sets the thinking level as today.
- Token cache: with a fake token source counting calls, two requests within 45 minutes fetch once; a request after
  the cache has expired (inject the clock or the expiry) fetches again.
- `model_id()` is `vertex/gemini-4-argon`.
- `pricing("vertex/gemini-4-argon")` returns the prices above.
No network in tests. Full gate: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test --workspace --all-targets`.

**Constraints.** The API-key path behaves exactly as before. No new dependencies. Document `vertex/` in
`docs/cli.md` where models are described (one short paragraph: prefix, env vars, the ADC login command).

**Depends on:** none.

**Commit message:** `feat(provider): vertex/<model> reaches Gemini through Vertex AI with application-default credentials`

### Task DB1: the Plans view answers quickly

**Goal and why.** `/api/plans` takes 8.5 s on this machine (110 plans from 7 days of runs, with `git`
merge checks per run), and the page polls it every 5 s, so requests overlap and the whole dashboard
lags (`/api/jobs` 0.7 s). The page shows only plans with a running task, so the server builds 100+
plans that the page discards.

**Files.** `crates/blyte-cli/src/plans.rs`, `crates/blyte-cli/src/dashboard.rs` (the `/api/plans` route
and the server only), `crates/blyte-cli/assets/dashboard/index.html` (the plans fetch only). Tests in
`crates/blyte-cli/tests/` in the style of `test_plans_multi_repo.rs`.

**What to build.**
1. `/api/plans?running=1` returns only plans with at least one task in state `running`, and builds only
   those: decide which plans have a running job from the job list first (cheap: start files and PIDs),
   then read, parse and run `git` checks only for those plans. Without the parameter, the response is
   unchanged (all plans).
2. The page requests `/api/plans?running=1`.
3. The page never has two plans requests in flight: replace `setInterval(fetchPlans, 5000)` with a
   `setTimeout` scheduled 5 s after the previous fetch completes (success or failure). Do the same for
   `fetchJobs`.
4. Cache `is_merged` results per (repository root, branch, branch head commit) for the life of the server
   process, so a finished run's merge state is computed once, not on every request.

**Tests.**
- With three plans in temp repos, one with a running job (live PID, as existing tests fake it) and two
  finished, `collect_plans` with the running-only option returns exactly the running one, and its
  JSON equals that plan's entry from the unfiltered call.
- The unfiltered call still returns all three.
- The `is_merged` cache: two calls for the same (root, branch, head) run `git` once (count through an
  injectable runner or a counter you add for tests); a changed head runs it again.
- `index.html`: a test that reads the asset and asserts it contains `/api/plans?running=1` and does not
  contain `setInterval(fetchPlans` or `setInterval(fetchJobs`.
Full gate: `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace --all-targets`.

**Constraints.** Plan contents, states and lanes unchanged. No new dependencies.

**Depends on:** none. **Siblings:** VX1 changes `blyte-provider` and is not in this worktree.

**Commit message:** `perf(dashboard): the Plans view builds only running plans and never overlaps requests`
