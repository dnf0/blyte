# Task 3 Brief: Ollama Cloud Provider Adapter

## Objective
Implement the `Ollama` provider adapter in `crates/superstate-provider/src/ollama.rs` supporting Ollama Cloud (`https://ollama.com/api`) and custom `$OLLAMA_HOST` endpoints with Bearer token authentication, chat completions with function calling, and model discovery via `GET /api/tags`.

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-multi-provider-model-discovery`

## Detailed Instructions
1. **Expose Module in `crates/superstate-provider/src/lib.rs`**:
   - Add `pub mod ollama;`.
2. **Implement `crates/superstate-provider/src/ollama.rs`**:
   - Constants:
     ```rust
     const DEFAULT_MODEL: &str = "qwen2.5-coder:32b";
     const DEFAULT_API_ROOT: &str = "https://ollama.com/api";
     const CONNECT_TIMEOUT_SECS: u64 = 20;
     const READ_TIMEOUT_SECS: u64 = 300;
     ```
   - Struct:
     ```rust
     pub struct Ollama {
         api_key: Option<String>,
         api_root: String,
         model: String,
     }
     ```
   - Constructors:
     - `from_env() -> Result<Self, ProviderError>`:
       Reads `OLLAMA_API_KEY` (as `Option<String>`).
       Reads `OLLAMA_HOST`, normalizes trailing slashes. If unset, uses `DEFAULT_API_ROOT`.
       If neither `OLLAMA_API_KEY` nor `OLLAMA_HOST` is set, return `ProviderError::MissingKey("OLLAMA_API_KEY")`.
       Reads `SUPERSTATE_MODEL` if set, else `DEFAULT_MODEL`.
     - `with_model(mut self, model: impl Into<String>) -> Self`.
     - `with_api_root(mut self, api_root: impl Into<String>) -> Self`.
     - `with_api_key(mut self, key: impl Into<String>) -> Self`.
   - Request serialization:
     - `pub fn body(&self, req: &Request) -> Value`:
       - Construct `messages: Vec<Value>`:
         - If `!req.system.trim().is_empty()`: `{"role": "system", "content": req.system}`
         - `Message::User(t)` -> `{"role": "user", "content": t}`
         - `Message::Assistant(t)` -> `{"role": "assistant", "content": t}`
         - `Message::ToolCall { id, name, arguments, .. }` ->
           `{"role": "assistant", "content": "", "tool_calls": [{"id": id, "type": "function", "function": {"name": name, "arguments": arguments}}]}`
         - `Message::ToolResult { id, content, .. }` ->
           `{"role": "tool", "content": content}`
       - Top-level JSON:
         - `"model": self.model`
         - `"stream": false`
         - `"messages": messages`
         - If `!req.tools.is_empty()`:
           `"tools"`: array of `{"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.parameters}}`
   - Response parsing:
     - `pub fn parse(v: &Value) -> Result<Response, ProviderError>`:
       - Checks for error in `v["error"]`.
       - Extracts `message` object at `v["message"]`.
       - `out.text`: string at `message["content"]`.
       - `out.tool_calls`: parse array at `message["tool_calls"]`:
         For each item:
         - Extract `function["name"]`
         - Extract `function["arguments"]`: if string, parse as JSON; if object, use directly; fallback `{}`
         - Extract `id` (or fallback `format!("call_{i}")`)
         - `signature: None`
       - `out.finish_reason`: string at `v["done_reason"]`.
       - `out.usage`:
         - `input`: `v.get("prompt_eval_count").and_then(|x| x.as_u64()).unwrap_or(0)`
         - `cached`: 0
         - `output`: `v.get("eval_count").and_then(|x| x.as_u64()).unwrap_or(0)`
         - `thinking`: 0
   - Model listing parsing:
     - `pub fn parse_models_list(v: &Value) -> Result<Vec<ModelInfo>, ProviderError>`:
       - Parses `v["models"]` array.
       - Each entry:
         - `name`: string as `id` (e.g. `"qwen2.5-coder:32b"`)
         - `display_name`: `name`
         - `description`: format details if present (e.g. `details.parameter_size` and `details.quantization_level`)
         - `provider: "ollama".to_string()`
   - `Provider` trait implementation for `Ollama`:
     - `complete(&self, req: &Request) -> Result<Response, ProviderError>`:
       POST `{api_root}/chat`
       Header `Authorization: Bearer <key>` if `self.api_key` is Some.
       Header `content-type: application/json`.
       Bounded connect (20s) and read (300s) timeouts.
     - `model_id(&self) -> String`:
       `format!("ollama/{}", self.model)`
     - `list_models(&self) -> Result<Vec<ModelInfo>, ProviderError>`:
       GET `{api_root}/tags`
       Header `Authorization: Bearer <key>` if `self.api_key` is Some.
       Bounded timeouts.
3. **Write Unit Tests in `crates/superstate-provider/tests/test_ollama_wire.rs`**:
   - Test request body formatting (system prompt, user, assistant, tool_calls, tool results, tools schema).
   - Test response body parsing (text extraction, tool_calls with function arguments, done_reason, token usage).
   - Test model listing parsing from `/tags`.
   - Test environment constructors with `OLLAMA_API_KEY` and `OLLAMA_HOST`.
4. **Verification**:
   - Run `cargo test -p superstate-provider --test test_ollama_wire`.
   - Run `cargo check --workspace --all-targets`.
   - Run `cargo clippy -p superstate-provider --all-targets -- -D warnings`.
5. **Commit**:
   - `git add crates/superstate-provider/src/lib.rs crates/superstate-provider/src/ollama.rs crates/superstate-provider/tests/test_ollama_wire.rs`
   - `git commit --no-gpg-sign -m "feat(provider): implement Ollama Cloud provider adapter with Bearer auth and discovery"`
6. **Roborev**:
   - Run `roborev status; roborev show HEAD`.
7. **Report**:
   - Write report to `docs/superpowers/plans/task-3-report.md`.
