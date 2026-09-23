# Multi-Provider Cloud Model Discovery & Interactive Selection Implementation Plan

> **User Goal:** Enable dynamic model discovery and execution across Google Gemini, Anthropic Claude, and Ollama Cloud, with interactive `/model` switching in the REPL and default model updated to `gemini-3.8-flash`.

**Architecture Reference:** [`docs/superpowers/specs/2026-09-23-multi-provider-model-discovery-design.md`](file:///Users/danielfisher/repos/superstate/docs/superpowers/specs/2026-09-23-multi-provider-model-discovery-design.md)

---

## Proposed Changes & File Layout

```
crates/superstate-provider/
  src/
    lib.rs                    # ModelInfo, Provider trait extension (list_models), ProviderError additions
    gemini.rs                 # Gemini::list_models, DEFAULT_MODEL -> gemini-3.8-flash
    anthropic.rs              # Anthropic provider implementation (Messages API + models.list)
    ollama.rs                 # Ollama Cloud provider implementation (chat API + tags)
    registry.rs               # Universal provider resolver and multi-provider discovery engine
  tests/
    test_gemini_models.rs     # Gemini models.list wire parsing and generation filter
    test_anthropic_wire.rs    # Anthropic Messages & model list wire serialization
    test_ollama_wire.rs       # Ollama Cloud chat and tag list wire serialization
    test_registry.rs          # Provider resolution and multi-provider discovery tests

crates/superstate-cli/
  src/
    repl.rs                   # /model command, multi-provider listing, index/name switching, /help, /doctor
    task_runner.rs            # Universal provider resolution integration
  tests/
    test_repl_model_cmd.rs    # REPL /model listing and switching integration tests
README.md                     # Documentation for multi-provider support, /model command, and env vars
```

---

## Tasks

### Task 1: Provider Trait Extension & Gemini Discovery (`superstate-provider`)
**Goal:** Extend the core `Provider` trait with `ModelInfo` and `list_models(&self)`, implement `Gemini::list_models`, and update `DEFAULT_MODEL` to `gemini-3.8-flash`.

- **Files to Modify:**
  - `crates/superstate-provider/src/lib.rs`
  - `crates/superstate-provider/src/gemini.rs`
- **Files to Create:**
  - `crates/superstate-provider/tests/test_gemini_models.rs`
- **Steps:**
  1. Define `ModelInfo` in `crates/superstate-provider/src/lib.rs`:
     ```rust
     #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
     pub struct ModelInfo {
         pub id: String,
         pub display_name: String,
         pub description: Option<String>,
         pub provider: String,
     }
     ```
  2. Add `fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>` to `Provider` trait with default implementation returning `Ok(Vec::new())`.
  3. Write failing unit test in `test_gemini_models.rs` asserting parsing of Google GenAI `models.list` JSON payload, filtering out models without `"generateContent"`, and stripping the `"models/"` prefix.
  4. Implement `Gemini::list_models(&self)` and `Gemini::parse_models_list(v: &Value)` in `gemini.rs`.
  5. Update `DEFAULT_MODEL` constant from `"gemini-2.5-flash"` to `"gemini-3.8-flash"` in `gemini.rs`.
  6. Run `cargo test -p superstate-provider --test test_gemini_models`.
  7. Commit: `feat(provider): add ModelInfo and Gemini list_models discovery with gemini-3.8-flash default`.

---

### Task 2: Anthropic Claude Provider Adapter (`superstate-provider::anthropic`)
**Goal:** Implement the Anthropic Claude provider adapter supporting the Messages API (`POST /v1/messages`), model listing (`GET /v1/models`), and bidirectional tool calling.

- **Files to Create:**
  - `crates/superstate-provider/src/anthropic.rs`
  - `crates/superstate-provider/tests/test_anthropic_wire.rs`
- **Files to Modify:**
  - `crates/superstate-provider/src/lib.rs`
- **Steps:**
  1. Expose `pub mod anthropic;` in `crates/superstate-provider/src/lib.rs`.
  2. Write unit tests in `tests/test_anthropic_wire.rs`:
     - Test request body generation (`body(&self, req: &Request)`): checks `system`, user/assistant messages, `tool_use`, `tool_result`, and `tools` schema mapping.
     - Test response body parsing (`parse(v: &Value)`): text extraction, `tool_use` extraction, stop reason, and token usage accounting (`input_tokens`, `output_tokens`, `cache_read_input_tokens`).
     - Test model list parsing (`parse_models_list(v: &Value)`).
  3. Implement `Anthropic` in `crates/superstate-provider/src/anthropic.rs`:
     - Reads `ANTHROPIC_API_KEY`.
     - Supports `complete(&self, req: &Request)` using `ureq` against `https://api.anthropic.com/v1/messages`.
     - Supports `list_models(&self)` against `https://api.anthropic.com/v1/models`.
  4. Run `cargo test -p superstate-provider --test test_anthropic_wire`.
  5. Commit: `feat(provider): implement Anthropic Claude provider adapter and model discovery`.

---

### Task 3: Ollama Cloud Provider Adapter (`superstate-provider::ollama`)
**Goal:** Implement the Ollama Cloud provider adapter targeting `https://ollama.com/api` (or `$OLLAMA_HOST`) with Bearer token authentication and function calling.

- **Files to Create:**
  - `crates/superstate-provider/src/ollama.rs`
  - `crates/superstate-provider/tests/test_ollama_wire.rs`
- **Files to Modify:**
  - `crates/superstate-provider/src/lib.rs`
- **Steps:**
  1. Expose `pub mod ollama;` in `crates/superstate-provider/src/lib.rs`.
  2. Write unit tests in `tests/test_ollama_wire.rs`:
     - Test request body construction (`POST /api/chat` with `stream: false`, messages, and tool definitions).
     - Test response parsing: text, `tool_calls` (`function.name`, `function.arguments`), and token usage (`prompt_eval_count`, `eval_count`).
     - Test model list parsing (`GET /api/tags` -> list of `ModelInfo`).
  3. Implement `Ollama` in `crates/superstate-provider/src/ollama.rs`:
     - Reads `OLLAMA_API_KEY` and optional `OLLAMA_HOST` (defaulting to `https://ollama.com/api`).
     - Implements `Provider` trait for Ollama Cloud.
  4. Run `cargo test -p superstate-provider --test test_ollama_wire`.
  5. Commit: `feat(provider): implement Ollama Cloud provider adapter with Bearer auth and discovery`.

---

### Task 4: Universal Provider Registry & Discovery Engine (`superstate-provider::registry`)
**Goal:** Implement universal model routing and multi-provider credential inspection.

- **Files to Create:**
  - `crates/superstate-provider/src/registry.rs`
  - `crates/superstate-provider/tests/test_registry.rs`
- **Files to Modify:**
  - `crates/superstate-provider/src/lib.rs`
  - `crates/superstate-cli/src/task_runner.rs`
  - `crates/superstate-cli/tests/test_task_runner.rs`
- **Steps:**
  1. Author unit tests in `test_registry.rs`:
     - Test model string routing: `claude-3-7-sonnet` -> Anthropic, `gemini-3.8-flash` -> Gemini, `qwen2.5-coder:32b` -> Ollama.
     - Test discovery engine aggregating results across all configured providers.
  2. Implement `resolve_provider(model: Option<&str>) -> Result<Box<dyn Provider>, ProviderError>` in `registry.rs`.
  3. Implement `discover_all_models() -> Vec<(String, Result<Vec<ModelInfo>, ProviderError>)>` in `registry.rs`.
  4. Update `crates/superstate-cli/src/task_runner.rs` to initialize provider via `resolve_provider(config.model.as_deref())`.
  5. Update `test_task_runner.rs` to reflect the updated default model `gemini-3.8-flash`.
  6. Run `cargo test -p superstate-provider` and `cargo test -p superstate --test test_task_runner`.
  7. Commit: `feat(provider): add universal provider resolver and multi-provider discovery engine`.

---

### Task 5: REPL `/model` Command, `/doctor` Multi-Provider Audit, & Documentation
**Goal:** Wire the `/model` slash command in the interactive REPL, update `/doctor` diagnostics to verify all configured cloud providers, and update docs.

- **Files to Modify:**
  - `crates/superstate-cli/src/repl.rs`
  - `crates/superstate-cli/tests/test_repl_commands.rs`
  - `crates/superstate-cli/tests/test_action_ledger_e2e.rs`
  - `README.md`
- **Files to Create:**
  - `crates/superstate-cli/tests/test_repl_model_cmd.rs`
- **Steps:**
  1. In `crates/superstate-cli/src/repl.rs`:
     - Add `cached_models: Vec<ModelInfo>` to `ReplSession`.
     - Implement `/model` handler:
       - No args: calls `discover_all_models()`, displays grouped models with numeric indices, marks active model.
       - With arg: switches active model by number (e.g. `/model 1`) or string (e.g. `/model claude-3-7-sonnet-latest`), updates `session.config.model`, and prints confirmation.
     - Update `/help` to include `/model [name|index]`.
     - Update `/doctor` step 4 to audit and report status for Google Gemini, Anthropic Claude, and Ollama Cloud.
  2. Write integration tests in `crates/superstate-cli/tests/test_repl_model_cmd.rs` verifying:
     - `/model` lists discovered models and marks the active model.
     - `/model 2` switches model by numeric index.
     - `/model <name>` switches model directly.
  3. Update `README.md` documenting multi-provider setup (`GEMINI_API_KEY`, `ANTHROPIC_API_KEY`, `OLLAMA_API_KEY`), the `/model` command, and `gemini-3.8-flash` as default.
  4. Run `cargo check --workspace --all-targets`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`.
  5. Run `cargo fmt --all -- --check`.
  6. Commit: `feat(cli): add REPL /model discovery selector and multi-provider doctor audit`.

---

## Verification Checklist
- [ ] `cargo check --workspace --all-targets` (0 warnings)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` (0 warnings)
- [ ] `cargo test --workspace` (100% tests passing)
- [ ] `cargo fmt --all -- --check`
