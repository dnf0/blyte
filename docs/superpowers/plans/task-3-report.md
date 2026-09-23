# Task 3 Implementation Report: Ollama Cloud Provider Adapter

## Summary
- **Status:** DONE
- **Commit:** `0219ac3` ("feat(provider): implement Ollama Cloud provider adapter with Bearer auth and discovery")
- **Worktree:** `/Users/danielfisher/repos/superstate/.wt/feat-multi-provider-model-discovery`

## Changes Implemented

1. **`crates/superstate-provider/src/lib.rs`**:
   - Registered and exposed `pub mod ollama;`.

2. **`crates/superstate-provider/src/ollama.rs`**:
   - Implemented `Ollama` adapter struct with `api_key` (`Option<String>`), `api_root` (`String`), and `model` (`String`).
   - Defined protocol constants:
     - `DEFAULT_MODEL`: `"qwen2.5-coder:32b"`
     - `DEFAULT_API_ROOT`: `"https://ollama.com/api"`
     - `CONNECT_TIMEOUT_SECS`: 20s
     - `READ_TIMEOUT_SECS`: 300s
   - Added constructors and builders:
     - `from_env() -> Result<Self, ProviderError>`: Reads `OLLAMA_API_KEY` and `OLLAMA_HOST`. If neither is configured, returns `ProviderError::MissingKey("OLLAMA_API_KEY")`. Normalizes trailing slashes on custom hosts, defaulting to `DEFAULT_API_ROOT`. Reads `SUPERSTATE_MODEL` if present or defaults to `DEFAULT_MODEL` (stripping optional `ollama/` prefix).
     - `with_model(mut self, model: impl Into<String>) -> Self`.
     - `with_api_root(mut self, api_root: impl Into<String>) -> Self`.
     - `with_api_key(mut self, key: impl Into<String>) -> Self`.
     - Accessors `api_root()`, `api_key()`, and `model()`.
   - Implemented request body serialization `pub fn body(&self, req: &Request) -> Value`:
     - Embeds system prompt into `messages` as `{"role": "system", "content": ...}` when non-empty.
     - Serializes `Message::User` and `Message::Assistant` into role/content messages.
     - Serializes `Message::ToolCall` into `assistant` message with `tool_calls: [{"id": ..., "type": "function", "function": {"name": ..., "arguments": ...}}]`.
     - Serializes `Message::ToolResult` into `tool` role message with `content`.
     - Attaches OpenAI-compatible `tools` array with function declarations if tools are provided (omitted if empty).
     - Configures `"stream": false` and `"model": self.model`.
   - Implemented response parser `pub fn parse(v: &Value) -> Result<Response, ProviderError>`:
     - Checks for `error` field in JSON response and fails fast.
     - Extracts text from `message.content`.
     - Parses `message.tool_calls` handling both pre-parsed JSON objects and serialized string arguments with fallback IDs (`call_{i}`).
     - Maps `done_reason` into `finish_reason`.
     - Normalizes usage metrics from `prompt_eval_count` and `eval_count`.
   - Implemented model listing parser `pub fn parse_models_list(v: &Value) -> Result<Vec<ModelInfo>, ProviderError>`:
     - Parses `models` array from `GET /tags`.
     - Extracts model tag name as `id` and `display_name`.
     - Formats details (e.g. `parameter_size` and `quantization_level`) into `description`.
     - Sets `provider: "ollama"`.
   - Implemented `Provider` trait for `Ollama`:
     - `complete(&self, req: &Request)`: POST to `{api_root}/chat` with Bearer auth header (if `api_key` present) and bounded timeouts.
     - `model_id(&self)`: Returns `ollama/{model}`.
     - `list_models(&self)`: GET to `{api_root}/tags` with Bearer auth header (if `api_key` present) and bounded timeouts.

3. **`crates/superstate-provider/tests/test_ollama_wire.rs`**:
   - Followed strict TDD (RED phase verified compilation failure before implementing adapter).
   - Added 14 unit tests verifying offline wire behavior:
     - `system_instruction_is_placed_in_messages_array_when_present`
     - `empty_system_instruction_is_omitted_from_messages`
     - `user_and_assistant_messages_wire_formatting`
     - `tool_call_and_tool_result_wire_shape`
     - `tools_omitted_when_empty`
     - `parses_text_response_and_usage`
     - `parses_tool_calls_with_object_arguments`
     - `parses_tool_calls_with_string_arguments_and_fallback_id`
     - `parses_error_response_fails`
     - `parse_models_list_extracts_models_and_details`
     - `parse_models_list_handles_empty_or_missing_models`
     - `from_env_requires_api_key_or_host`
     - `from_env_honors_superstate_model`
     - `builder_methods_work`

## Verification Evidence
- `cargo test -p superstate-provider --test test_ollama_wire`: 14 passed, 0 failed.
- `cargo clippy -p superstate-provider --all-targets -- -D warnings`: Clean, 0 warnings.
- `cargo check --workspace --all-targets`: Clean, 0 errors.
- `cargo fmt --all -- --check`: Clean formatting verified.
- `cargo test --workspace`: All workspace tests passing.
- `roborev status; roborev show HEAD`: Executed.
