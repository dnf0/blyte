# Task 2 Brief: Anthropic Claude Provider Adapter

## Objective
Implement the `Anthropic` provider adapter in `crates/superstate-provider/src/anthropic.rs` supporting the Messages API (`POST /v1/messages`), model listing (`GET /v1/models`), and bidirectional tool calling.

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-multi-provider-model-discovery`

## Detailed Instructions
1. **Expose Module in `crates/superstate-provider/src/lib.rs`**:
   - Add `pub mod anthropic;`.
2. **Implement `crates/superstate-provider/src/anthropic.rs`**:
   - Constants:
     ```rust
     const DEFAULT_MODEL: &str = "claude-3-7-sonnet-latest";
     const API_ROOT: &str = "https://api.anthropic.com/v1";
     const ANTHROPIC_VERSION: &str = "2023-06-01";
     const CONNECT_TIMEOUT_SECS: u64 = 20;
     const READ_TIMEOUT_SECS: u64 = 300;
     ```
   - Struct:
     ```rust
     pub struct Anthropic {
         api_key: String,
         model: String,
     }
     ```
   - Constructors:
     - `from_env() -> Result<Self, ProviderError>`:
       Reads `ANTHROPIC_API_KEY`, returns `ProviderError::MissingKey("ANTHROPIC_API_KEY")` if absent.
       Reads `SUPERSTATE_MODEL` if set, else `DEFAULT_MODEL`.
     - `with_model(mut self, model: impl Into<String>) -> Self`.
   - Request serialization:
     - `pub fn body(&self, req: &Request) -> Value`:
       - `model`: `self.model`
       - `max_tokens`: `req.max_output_tokens.unwrap_or(8192)`
       - If `!req.system.is_empty()`: `"system": req.system`
       - `"messages"`: Vec of converted messages:
         - `Message::User(t)` -> `{"role": "user", "content": [{"type": "text", "text": t}]}`
         - `Message::Assistant(t)` -> `{"role": "assistant", "content": [{"type": "text", "text": t}]}`
         - `Message::ToolCall { id, name, arguments, .. }` ->
           `{"role": "assistant", "content": [{"type": "tool_use", "id": id, "name": name, "input": arguments}]}`
         - `Message::ToolResult { id, content, is_error }` ->
           `{"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "content": content, "is_error": is_error}]}`
       - If `!req.tools.is_empty()`:
         `"tools"`: array of `{"name": t.name, "description": t.description, "input_schema": t.parameters}`.
   - Response parsing:
     - `pub fn parse(v: &Value) -> Result<Response, ProviderError>`:
       - `finish_reason`: `v.get("stop_reason").and_then(|s| s.as_str()).map(String::from)`
       - `content`: array of blocks:
         - `type == "text"`: append string to `out.text`
         - `type == "tool_use"`: `out.tool_calls.push(ToolCall { id, name, arguments: input, signature: None })`
       - `usage`:
         - `input`: `input_tokens.saturating_sub(cache_read_input_tokens)`
         - `cached`: `cache_read_input_tokens`
         - `output`: `output_tokens`
         - `thinking`: 0
   - Model listing parsing:
     - `pub fn parse_models_list(v: &Value) -> Result<Vec<ModelInfo>, ProviderError>`:
       - Parses `v["data"]` array of model objects (`id`, `display_name`).
       - If `v["data"]` is empty or missing, or to ensure convenience aliases are always selectable, include standard aliases (`claude-3-7-sonnet-latest`, `claude-3-5-sonnet-latest`, `claude-3-5-haiku-latest`) if not already present.
       - Sets `provider: "anthropic".to_string()`.
   - `Provider` trait implementation for `Anthropic`:
     - `complete(&self, req: &Request) -> Result<Response, ProviderError>`:
       POST `{API_ROOT}/messages`
       Headers: `x-api-key: self.api_key`, `anthropic-version: ANTHROPIC_VERSION`, `content-type: application/json`.
     - `model_id(&self) -> String`:
       `format!("anthropic/{}", self.model)`
     - `list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>`:
       GET `{API_ROOT}/models`
       Headers: `x-api-key: self.api_key`, `anthropic-version: ANTHROPIC_VERSION`.
3. **Write Unit Tests in `crates/superstate-provider/tests/test_anthropic_wire.rs`**:
   - Test request body formatting (text messages, system prompt, tool definitions, tool_use, tool_result).
   - Test response body parsing (text extraction, tool_use blocks, token usage, stop reason).
   - Test model list parsing.
4. **Verification**:
   - Run `cargo test -p superstate-provider --test test_anthropic_wire`.
   - Run `cargo check --workspace --all-targets`.
   - Run `cargo clippy -p superstate-provider --all-targets -- -D warnings`.
5. **Commit**:
   - `git add crates/superstate-provider/src/lib.rs crates/superstate-provider/src/anthropic.rs crates/superstate-provider/tests/test_anthropic_wire.rs`
   - `git commit --no-gpg-sign -m "feat(provider): implement Anthropic Claude provider adapter and model discovery"`
6. **Roborev**:
   - Run `roborev status; roborev show HEAD`.
7. **Report**:
   - Write report to `docs/superpowers/plans/task-2-report.md`.
