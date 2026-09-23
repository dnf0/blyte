# Task 2 Implementation Report: Anthropic Claude Provider Adapter

## Summary
- **Status:** DONE
- **Commit:** `8e8e4e6` ("feat(provider): implement Anthropic Claude provider adapter and model discovery")
- **Worktree:** `/Users/danielfisher/repos/superstate/.wt/feat-multi-provider-model-discovery`

## Changes Implemented

1. **`crates/superstate-provider/src/lib.rs`**:
   - Registered and exposed `pub mod anthropic;`.

2. **`crates/superstate-provider/src/anthropic.rs`**:
   - Implemented `Anthropic` adapter struct with `api_key` and `model`.
   - Defined protocol constants:
     - `DEFAULT_MODEL`: `"claude-3-7-sonnet-latest"`
     - `API_ROOT`: `"https://api.anthropic.com/v1"`
     - `ANTHROPIC_VERSION`: `"2023-06-01"`
     - `CONNECT_TIMEOUT_SECS`: 20s
     - `READ_TIMEOUT_SECS`: 300s
   - Added constructors:
     - `from_env() -> Result<Self, ProviderError>`: Reads `ANTHROPIC_API_KEY`, returns `ProviderError::MissingKey("ANTHROPIC_API_KEY")` if absent; reads `SUPERSTATE_MODEL` if set or defaults to `DEFAULT_MODEL`.
     - `with_model(mut self, model: impl Into<String>) -> Self`.
   - Implemented request body serialization `pub fn body(&self, req: &Request) -> Value`:
     - Serializes model, max tokens (defaults to 8192).
     - Renders top-level `system` instruction string (omitted if empty).
     - Maps `Message::User` and `Message::Assistant` into text content blocks.
     - Maps `Message::ToolCall` into assistant `tool_use` content blocks (`id`, `name`, `input`).
     - Maps `Message::ToolResult` into user `tool_result` content blocks (`tool_use_id`, `content`, `is_error`).
     - Serializes `req.tools` into `input_schema` function declarations (omitted if empty).
   - Implemented response parser `pub fn parse(v: &Value) -> Result<Response, ProviderError>`:
     - Enforces presence of a valid `content` array (rejecting error payloads and malformed responses).
     - Extracts text and `tool_use` blocks into normalized `Response`.
     - Extracts `stop_reason` into `finish_reason`.
     - Normalizes token usage, deducting `cache_read_input_tokens` from `input_tokens` to prevent double-counting cached tokens against fresh input costs.
   - Implemented model listing parser `pub fn parse_models_list(v: &Value) -> Result<Vec<ModelInfo>, ProviderError>`:
     - Parses `data` array of model objects from `GET /v1/models` (`id`, `display_name`, `description`).
     - Injects standard convenience aliases (`claude-3-7-sonnet-latest`, `claude-3-5-sonnet-latest`, `claude-3-5-haiku-latest`) if not already returned by the API.
     - Sets `provider: "anthropic"`.
   - Implemented `Provider` trait for `Anthropic`:
     - `complete(&self, req: &Request)`: POST to `/v1/messages` with `x-api-key`, `anthropic-version`, and timeout-bounded `ureq` client.
     - `model_id(&self)`: Returns `anthropic/{model}`.
     - `list_models(&self)`: GET to `/v1/models` with `x-api-key`, `anthropic-version`, and timeout-bounded `ureq` client.

3. **`crates/superstate-provider/tests/test_anthropic_wire.rs`**:
   - Followed strict TDD (RED phase first, observed compilation failure for unresolved module before implementing adapter).
   - Added 13 unit tests verifying offline wire behavior:
     - `system_instruction_is_placed_in_top_level_system_field`
     - `empty_system_instruction_is_omitted`
     - `custom_max_output_tokens_is_respected`
     - `user_and_assistant_message_content_blocks`
     - `tool_call_and_tool_result_wire_shape`
     - `tools_omitted_when_empty`
     - `parses_text_and_tool_use_response`
     - `response_with_no_usage_block_does_not_fail`
     - `response_without_content_fails_parsing`
     - `parse_models_list_extracts_models_and_adds_standard_aliases`
     - `parse_models_list_handles_empty_or_missing_data`
     - `from_env_requires_api_key_and_honors_superstate_model`
     - `with_model_updates_model_and_model_id`

## Verification Evidence
- `cargo test -p superstate-provider --test test_anthropic_wire`: 13 passed, 0 failed.
- `cargo clippy -p superstate-provider --all-targets -- -D warnings`: Clean, 0 warnings.
- `cargo check --workspace --all-targets`: Clean, 0 errors.
- `cargo fmt --all -- --check`: Clean formatting verified.
- `cargo test --workspace`: All tests passed across all workspace crates.
- `roborev status; roborev show HEAD`: Executed.
