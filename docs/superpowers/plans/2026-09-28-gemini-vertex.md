# Gemini on Vertex AI — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API or real Google endpoint (tests use local stub servers); commit locally only. Never print, log or commit a token, refresh token or key. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** `--model vertex/<gemini model>` runs Gemini through Vertex AI instead of the Gemini API
(AI Studio key), authenticated with the user's own Application Default Credentials. blyte ships no
account and assumes no project: the user's environment supplies both.

**Why (2026-09-28):** on the Gemini API, about 23% of blyte's turns miss the implicit prompt cache
(scored block-aware; about 10% of Gemini cost). A probe on the same growing conversations had 8 of
28 turns miss on the Gemini API and 1 of 28 on Vertex, and agy (which calls Vertex) had none in 16.
The difference measured so far is mostly at the start of a conversation (the Gemini API cached
nothing below about 16K tokens; Vertex from about 8K). Whether Vertex also removes blyte's mid-run
misses is measured after this lands, on a real task. See `2026-09-28-gemini-cache-hits.md`.

**Tech stack:** Rust, `ureq` (blocking, the existing pooled client), `serde_json`.

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**. The Gemini API path (`gemini-…` model ids, `GEMINI_API_KEY`) is unchanged.
- The request body is the same on both backends: reuse `Gemini::body` and `Gemini::parse`.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: the Vertex backend for the Gemini provider

**Files:** `crates/blyte-provider/src/vertex_auth.rs` (new), `crates/blyte-provider/src/gemini.rs`,
`crates/blyte-provider/src/registry.rs`, `crates/blyte-provider/src/lib.rs`,
`crates/blyte-provider/tests/test_gemini_vertex.rs` (new), `docs/providers.md`.

**Selection.** A model id with the prefix `vertex/` selects Vertex: `vertex/gemini-3.8-flash-high`.
`detect_provider_kind` maps it to `ProviderKind::Gemini`; `resolve_provider` builds
`Gemini::vertex_from_env()?` and applies the rest of the id with `with_model` (so effort suffixes
work as today). `model_id()` returns `vertex/<model>` on Vertex (pricing matches on the
`gemini-3.8-flash` substring, so cost accounting is unchanged) and `gemini/<model>` as today on the
Gemini API.

**Configuration** (read by `Gemini::vertex_from_env`):
- project: `GOOGLE_CLOUD_PROJECT`, else the ADC file's `quota_project_id`, else
  `ProviderError::MissingKey("GOOGLE_CLOUD_PROJECT")`;
- location: `GOOGLE_CLOUD_LOCATION`, else `global`.

**Endpoint** (`pub fn vertex_url(project, location, model) -> String`):
- `global`: `https://aiplatform.googleapis.com/v1/projects/{project}/locations/global/publishers/google/models/{model}:generateContent`
- any other location: `https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/publishers/google/models/{model}:generateContent`

Headers on Vertex: `Authorization: Bearer <token>`, `x-goog-user-project: <project>`,
`content-type: application/json`. For tests, `Gemini::with_vertex_root(url)` replaces the
`https://…aiplatform.googleapis.com` part so requests go to a local stub.

**Auth** (`vertex_auth.rs`, `pub struct VertexAuth`, `pub fn token(&self) -> Result<String, ProviderError>`):
1. `GOOGLE_OAUTH_ACCESS_TOKEN`, if set: used as is, never refreshed.
2. Otherwise the ADC file: `GOOGLE_APPLICATION_CREDENTIALS`, else
   `$HOME/.config/gcloud/application_default_credentials.json` (what `gcloud auth application-default login` writes).
   - `"type": "authorized_user"`: POST `application/x-www-form-urlencoded` to the token endpoint
     (default `https://oauth2.googleapis.com/token`) with `grant_type=refresh_token`, `client_id`,
     `client_secret`, `refresh_token`; read `access_token` and `expires_in`. Cache the token (behind
     a `Mutex`) and refresh it when fewer than 300 s remain.
   - `"type": "service_account"`: `ProviderError::Auth("service-account credentials are not supported yet; set GOOGLE_OAUTH_ACCESS_TOKEN, for example from gcloud auth print-access-token")`.
   - no file: `ProviderError::MissingKey("GOOGLE_APPLICATION_CREDENTIALS")`.
3. `VertexAuth::from_adc_file(path, token_url)` and `VertexAuth::from_token(token)` constructors
   exist for tests.
4. A 401 from Vertex clears the cached token and retries the request once.
5. Tokens, refresh tokens and client secrets never appear in errors, logs or `Debug` output.

**Commands never see the credentials:** `GOOGLE_OAUTH_ACCESS_TOKEN` is withheld from the commands
blyte runs by the existing environment filter (assert `credential_like` in a test; extend the rule
only if it fails). The ADC file sits under `~/.config/gcloud`, which the sandbox already denies
reading.

**`list_models` on Vertex:** returns just the configured model (no listing call).

**Docs:** `docs/providers.md` gains a "Gemini on Vertex AI" section: `--model vertex/gemini-3.8-flash-high`,
`gcloud auth application-default login` (or `GOOGLE_OAUTH_ACCESS_TOKEN`), `GOOGLE_CLOUD_PROJECT`,
`GOOGLE_CLOUD_LOCATION`, that service-account key files are not supported yet, and that blyte uses
the user's own credentials and project.

- [ ] **Step 1: tests first**, in `crates/blyte-provider/tests/test_gemini_vertex.rs`, using a local
  `std::net::TcpListener` stub server (one thread, fixed responses) for the token endpoint and the
  Vertex endpoint. They cover:
  1. `vertex_url` for `global` and for `europe-west1`.
  2. `detect_provider_kind(Some("vertex/gemini-3.8-flash-high"))` is `Gemini`; the resolved
     provider's `model_id()` is `vertex/gemini-3.8-flash` and its thinking level is `high`.
  3. An `authorized_user` ADC file: the first `token()` POSTs the four form fields and returns the
     stub's token; a second call within the lifetime does not contact the stub again; a token
     with `expires_in: 10` is refreshed on the next call.
  4. A `service_account` ADC file gives the `Auth` error above; no file gives `MissingKey`.
  5. `complete()` against a stub Vertex endpoint: the request carries `Authorization: Bearer <token>`
     and `x-goog-user-project`, its JSON body equals `Gemini::body` for the same request, and the
     response is parsed as on the Gemini API.
  6. A 401 then 200 from the stub: one refresh, one retry, success.
  7. `credential_like("GOOGLE_OAUTH_ACCESS_TOKEN", "x")` is true.
- [ ] **Step 2:** run them; they fail. **Step 3:** implement. **Step 4:** full checks.
- [ ] **Step 5: Commit.**

```bash
git add -A crates docs/providers.md
git commit --no-gpg-sign -m "feat(provider): Gemini on Vertex AI with the user's Application Default Credentials (vertex/ model prefix)"
```

## After it lands (not part of the task)

Measure on a real task: the same small plan task dispatched twice on each backend
(`gemini-3.8-flash-high` against `vertex/gemini-3.8-flash-high`), scoring true cache misses
block-aware, fresh input against new content, cost and time. Make Vertex the recommended Gemini path
only if its true-miss rate is clearly lower on real turns, not only at warm-up.
