# Multi-Provider Cloud Model Discovery & Interactive Selection Design

**Date:** 2026-09-23  
**Status:** Approved  
**Target Crates:** `crates/superstate-provider`, `crates/superstate-cli`

---

## 1. Executive Summary

Superstate currently supports Google Gemini via `superstate-provider::gemini::Gemini` with a hardcoded fallback default (`gemini-2.5-flash`). Frontier workflows require dynamic model discovery, multi-provider cloud support (Google Gemini, Anthropic Claude, and Ollama Cloud), and runtime model switching in the interactive REPL via `/model`.

This specification formalizes:
1. **Multi-Provider Cloud Adapters:** Adding first-class support for **Google Gemini**, **Anthropic Claude**, and **Ollama Cloud** normalized behind the `Provider` trait.
2. **Dynamic Model Discovery:** Querying provider endpoints at runtime to verify credentials and discover available models capable of text/code generation.
3. **Interactive REPL Model Selector (`/model`):** Discovering and displaying verified models across all configured cloud providers, and allowing immediate model/provider switching by name or numeric index (`/model 1`).
4. **Default Model Update:** Setting the new default model to `gemini-3.8-flash`.
5. **System Handshake Diagnostic Integration (`/doctor`):** Auditing all configured provider keys, connectivity, and model counts.

---

## 2. Architectural Design

```
+-----------------------------------------------------------------------------------------+
|                                  Superstate CLI / REPL                                  |
|   - /model (lists verified models across all configured cloud providers)                |
|   - /model <number> or <name> (switches active model and provider)                      |
|   - /doctor (audits API keys and model availability across all providers)               |
+-----------------------------------------------------------------------------------------+
                                             |
                                             v
+-----------------------------------------------------------------------------------------+
|                    Provider Registry & Model Discovery Service                          |
|   - Reads environment keys:                                                             |
|     * GEMINI_API_KEY / GOOGLE_API_KEY                                                   |
|     * ANTHROPIC_API_KEY                                                                 |
|     * OLLAMA_API_KEY (optional OLLAMA_HOST)                                             |
|   - Discovers models via authenticated GET calls                                        |
|   - Resolves model string -> appropriate Provider adapter                               |
+-----------------------------------------------------------------------------------------+
          |                                  |                                  |
          v                                  v                                  v
+---------------------+            +---------------------+            +---------------------+
|  Gemini Adapter     |            |  Anthropic Adapter  |            | Ollama Cloud Adapter|
|  - Google GenAI API |            |  - Messages API     |            |  - Ollama Cloud API |
|  - GET /v1beta/models            |  - GET /v1/models   |            |  - GET /api/tags    |
|  - POST generateContent          |  - POST /v1/messages|            |  - POST /api/chat   |
+---------------------+            +---------------------+            +---------------------+
```

---

## 3. Provider Specifications

### 3.1. Google Gemini
- **Credentials:** `GEMINI_API_KEY` (fallback `GOOGLE_API_KEY`).
- **Endpoint Root:** `https://generativelanguage.googleapis.com/v1beta`.
- **Discovery Endpoint:** `GET /models`.
  - Filter: `supportedGenerationMethods` contains `"generateContent"`.
  - Name format: strips `models/` prefix (e.g. `gemini-3.8-flash`, `gemini-2.5-pro`).
- **Completion Endpoint:** `POST /models/{id}:generateContent`.
- **Default Model:** `gemini-3.8-flash`.

### 3.2. Anthropic Claude
- **Credentials:** `ANTHROPIC_API_KEY`.
- **Endpoint Root:** `https://api.anthropic.com/v1`.
- **Discovery Endpoint:** `GET /models`.
  - Header: `x-api-key: <key>`, `anthropic-version: 2023-06-01`.
  - Parses `data` array into `ModelInfo`.
  - Augmented with aliases: `claude-3-7-sonnet-latest`, `claude-3-5-sonnet-latest`, `claude-3-5-haiku-latest`.
- **Completion Endpoint:** `POST /messages`.
  - Wire mappings:
    - User message: `{"role": "user", "content": [{"type": "text", "text": "..."}]}`
    - Tool call: `{"role": "assistant", "content": [{"type": "tool_use", "id": call_id, "name": name, "input": arguments}]}`
    - Tool result: `{"role": "user", "content": [{"type": "tool_result", "tool_use_id": call_id, "content": content, "is_error": is_error}]}`
    - Tool specs: `{"name": name, "description": desc, "input_schema": parameters}`
- **Usage Extraction:** `input_tokens`, `output_tokens`, `cache_read_input_tokens`, `cache_creation_input_tokens`.

### 3.3. Ollama Cloud
- **Credentials:** `OLLAMA_API_KEY`.
- **Endpoint Root:** `https://ollama.com/api` (or override via `OLLAMA_HOST`).
- **Headers:** `Authorization: Bearer <OLLAMA_API_KEY>`.
- **Discovery Endpoint:** `GET /tags`.
  - Parses `models` array containing model names (e.g. `qwen2.5-coder:32b`, `llama3.3:70b`, `gemma4:31b`).
- **Completion Endpoint:** `POST /chat`.
  - Formatted with `stream: false`.
  - Tool calls parsed from `message.tool_calls` (`function.name`, `function.arguments`).
  - Tool results sent with `role: "tool"`.

---

## 4. Superstate Provider Data Structures

In `crates/superstate-provider/src/lib.rs`:

```rust
/// Detailed metadata describing a discovered model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub provider: String,
}

pub trait Provider {
    /// Executes one blocking model turn.
    fn complete(&self, req: &Request) -> Result<Response, ProviderError>;

    /// Identifier recorded in telemetry, e.g. `gemini/gemini-3.8-flash`.
    fn model_id(&self) -> String;

    /// Discovers available models for this provider using configured credentials.
    fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(Vec::new())
    }
}
```

### 4.1. Universal Provider Resolution
A factory function in `superstate-provider`:
```rust
pub fn resolve_provider(model: Option<&str>) -> Result<Box<dyn Provider>, ProviderError>;
pub fn discover_all_models() -> Vec<(String, Result<Vec<ModelInfo>, ProviderError>)>;
```

Routing logic:
1. If `model` starts with `claude-` or prefix is `anthropic/` -> instantiate `Anthropic`.
2. If `model` is an Ollama tag, starts with `ollama/`, or contains `:` -> instantiate `Ollama`.
3. Default or `gemini-` -> instantiate `Gemini` with `gemini-3.8-flash`.

---

## 5. Interactive REPL UX (`/model`)

### 5.1. Listing Models (`/model`)
```text
superstate> /model
Discovering available cloud models...

[Anthropic Claude] (via ANTHROPIC_API_KEY)
  [1] claude-3-7-sonnet-latest   - Frontier hybrid reasoning & coding
  [2] claude-3-5-sonnet-latest   - Fast coding and analysis
  [3] claude-3-5-haiku-latest    - Rapid lightweight assistance

[Google Gemini] (via GEMINI_API_KEY)
  [4] gemini-3.8-flash (active)  - Next-generation multimodal fast model
  [5] gemini-2.5-flash           - Agentic reasoning model
  [6] gemini-2.5-pro             - Deep reasoning model

[Ollama Cloud] (via OLLAMA_API_KEY at https://ollama.com/api)
  [7] qwen2.5-coder:32b          - Cloud-hosted coding model
  [8] llama3.3:70b               - Cloud-hosted reasoning model

Active model: gemini-3.8-flash (Google Gemini)
To switch: /model <number> or /model <name> (e.g. /model 1 or /model claude-3-7-sonnet-latest)
```

### 5.2. Switching Models
- `/model 1` -> switches to `claude-3-7-sonnet-latest`
- `/model gemini-2.5-pro` -> switches to `gemini-2.5-pro`
- `/model qwen2.5-coder:32b` -> switches to `qwen2.5-coder:32b`

The REPL updates `session.config.model`, reinstantiates the provider on the next turn, and emits a confirmation:
`Switched active model to: claude-3-7-sonnet-latest (Anthropic Claude)`

---

## 6. Verification and Testing

1. **Unit Tests (`superstate-provider`):**
   - Wire formatting and parsing tests for Gemini, Anthropic, and Ollama Cloud.
   - Model listing mock payload parsing tests for all three providers.
   - Provider resolution tests verifying correct routing based on model strings and credentials.
2. **Integration Tests (`superstate-cli`):**
   - REPL command tests verifying `/model` display, numeric index resolution, and switching.
   - `/doctor` verification tests ensuring multi-provider credential checks report accurately.
