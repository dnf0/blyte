//! Ollama Cloud and custom endpoint provider adapter (`POST /chat` and `GET /tags`).
//!
//! # Protocol and Wire Format
//!
//! Ollama provides OpenAI-compatible chat endpoints with streaming, tool-calling, and model tag inspection.
//! This adapter supports both Ollama Cloud (`https://ollama.com/api`) with Bearer token authentication and
//! custom local/remote `$OLLAMA_HOST` endpoints:
//! - An assistant turn issues tool calls via `{"role": "assistant", "tool_calls": [{"id": "...", "type": "function", "function": {"name": "...", "arguments": {...}}}]}`.
//! - A tool response is sent via `{"role": "tool", "content": "..."}`.
//! - The system prompt is passed as an initial message with `{"role": "system", "content": "..."}` within the messages array.
//! - Model discovery is performed via `GET /api/tags` parsing the `models` array and details.

use serde_json::{json, Value};

use crate::{Message, ModelInfo, Provider, ProviderError, Request, Response, ToolCall, Usage};

/// Default model identifier when `SUPERSTATE_MODEL` is not specified in the environment.
const DEFAULT_MODEL: &str = "qwen2.5-coder:32b";

/// Default base URL for Ollama Cloud API.
const DEFAULT_API_ROOT: &str = "https://ollama.com/api";

/// Connection timeout in seconds.
///
/// Bounded to prevent silent indefinite hanging if the network or gateway is unresponsive.
const CONNECT_TIMEOUT_SECS: u64 = 20;

/// Read timeout in seconds.
///
/// Long conversations or local model generation runs on large models can take several minutes.
const READ_TIMEOUT_SECS: u64 = 300;

/// Normalizes the API root URL so that trailing slashes are removed and the `/api` prefix
/// is consistently present (e.g. `http://localhost:11434` -> `http://localhost:11434/api`).
fn normalize_api_root(raw: &str) -> String {
    let trimmed = raw.trim_end_matches('/');
    if trimmed.ends_with("/api") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/api")
    }
}

/// Ollama Cloud and local/remote endpoint provider adapter.
pub struct Ollama {
    api_key: Option<String>,
    api_root: String,
    model: String,
}

impl Ollama {
    /// Constructs an Ollama provider instance from environment variables.
    ///
    /// Reads `OLLAMA_API_KEY` (optional for local hosts, required for Ollama Cloud),
    /// `OLLAMA_HOST` (optional, defaults to `https://ollama.com/api`), and `SUPERSTATE_MODEL`
    /// (optional, defaults to `qwen2.5-coder:32b`).
    ///
    /// # Returns
    /// An initialized [`Ollama`] instance on success.
    ///
    /// # Errors
    /// Returns [`ProviderError::MissingKey`] if neither `OLLAMA_API_KEY` nor `OLLAMA_HOST` is set.
    pub fn from_env() -> Result<Self, ProviderError> {
        // Read optional API key, treating whitespace-only values as unset.
        let api_key = std::env::var("OLLAMA_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty());

        // Read optional custom host root, stripping trailing slashes for clean path joining.
        let host = std::env::var("OLLAMA_HOST")
            .ok()
            .filter(|h| !h.trim().is_empty());

        // Require at least one configuration vector so callers receive clear diagnostics.
        if api_key.is_none() && host.is_none() {
            return Err(ProviderError::MissingKey("OLLAMA_API_KEY"));
        }

        // Default to Ollama Cloud API root when no host override is specified.
        let api_root = host
            .map(|h| normalize_api_root(&h))
            .unwrap_or_else(|| DEFAULT_API_ROOT.to_string());

        // Strip any provider prefix to keep wire requests clean while accepting uniform IDs.
        let raw_model =
            std::env::var("SUPERSTATE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let model = raw_model
            .strip_prefix("ollama/")
            .unwrap_or(&raw_model)
            .to_string();

        Ok(Self {
            api_key,
            api_root,
            model,
        })
    }

    /// Sets the model identifier to use for completion requests.
    ///
    /// # Parameters
    /// - `model`: A model identifier string or tag name.
    ///
    /// # Returns
    /// The updated [`Ollama`] instance.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        let m = model.into();
        self.model = m.strip_prefix("ollama/").unwrap_or(&m).to_string();
        self
    }

    /// Sets the API root URL for Ollama requests.
    ///
    /// Ensures trailing slashes are stripped and standard `/api` suffix is present
    /// (e.g. `http://localhost:11434` -> `http://localhost:11434/api`).
    ///
    /// # Parameters
    /// - `api_root`: Base URL string (e.g. `https://ollama.com/api` or `http://localhost:11434`).
    ///
    /// # Returns
    /// The updated [`Ollama`] instance.
    pub fn with_api_root(mut self, api_root: impl Into<String>) -> Self {
        self.api_root = normalize_api_root(&api_root.into());
        self
    }

    /// Sets or overrides the API key for Bearer authentication.
    ///
    /// # Parameters
    /// - `key`: API token string.
    ///
    /// # Returns
    /// The updated [`Ollama`] instance.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Returns the configured API root base URL.
    pub fn api_root(&self) -> &str {
        &self.api_root
    }

    /// Returns the configured API key if one was provided.
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Returns the active model identifier without provider prefix.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Serializes a normalised [`Request`] into Ollama's `POST /chat` JSON body.
    ///
    /// # Parameters
    /// - `req`: The generic turn request containing conversation messages, system prompt, and tools.
    ///
    /// # Returns
    /// A [`Value`] JSON object adhering to Ollama's chat completion schema.
    pub fn body(&self, req: &Request) -> Value {
        let mut messages: Vec<Value> = Vec::new();

        // System prompt is passed as the first message with role "system" when non-empty.
        if !req.system.trim().is_empty() {
            messages.push(json!({
                "role": "system",
                "content": req.system,
            }));
        }

        // Map conversation turns into Ollama wire message objects.
        for m in &req.messages {
            match m {
                Message::User(text) => {
                    messages.push(json!({
                        "role": "user",
                        "content": text,
                    }));
                }
                Message::Assistant(text) => {
                    messages.push(json!({
                        "role": "assistant",
                        "content": text,
                    }));
                }
                Message::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    // Assistant tool invocations are represented with role "assistant" and tool_calls array.
                    messages.push(json!({
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": arguments,
                            }
                        }]
                    }));
                }
                Message::ToolResult { id, content, .. } => {
                    // Tool execution results are returned with role "tool" and tool_call_id.
                    messages.push(json!({
                        "role": "tool",
                        "tool_call_id": id,
                        "content": content,
                    }));
                }
            }
        }

        let mut body = json!({
            "model": self.model,
            "stream": false,
            "messages": messages,
        });

        // Tools schema is attached under "tools" using standard OpenAI-compatible function descriptors.
        if !req.tools.is_empty() {
            let tools_json: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = json!(tools_json);
        }

        body
    }

    /// Parses an Ollama `POST /chat` response JSON payload into a normalised [`Response`].
    ///
    /// # Parameters
    /// - `v`: The JSON response payload returned by the chat completion endpoint.
    ///
    /// # Returns
    /// A normalised [`Response`] containing text output, tool invocations, and token usage.
    ///
    /// # Errors
    /// Returns [`ProviderError::Parse`] if the payload reports an error or lacks a valid `message` object.
    pub fn parse(v: &Value) -> Result<Response, ProviderError> {
        // Intercept provider error messages in the body and fail fast.
        if let Some(err) = v.get("error") {
            if !err.is_null() {
                let msg = if let Some(s) = err.as_str() {
                    s.to_string()
                } else if let Some(s) = err.get("message").and_then(|m| m.as_str()) {
                    s.to_string()
                } else {
                    err.to_string()
                };
                return Err(ProviderError::Parse(msg));
            }
        }

        // Ensure the assistant message object is present in the response.
        let message = v
            .get("message")
            .and_then(|m| m.as_object())
            .ok_or_else(|| ProviderError::Parse(format!("no message object in response: {v}")))?;

        // Extract primary textual output if available.
        let text = message
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string();

        // Parse tool calls from message.tool_calls array.
        let mut tool_calls = Vec::new();
        if let Some(calls) = message.get("tool_calls").and_then(|c| c.as_array()) {
            for (i, call) in calls.iter().enumerate() {
                let function = call.get("function");
                let name = function
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string();

                // Ollama tool call arguments can arrive as pre-parsed JSON objects or serialized strings.
                let arguments = match function.and_then(|f| f.get("arguments")) {
                    Some(Value::Object(map)) => Value::Object(map.clone()),
                    Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
                    _ => json!({}),
                };

                // Fall back to synthetic call ID if the provider did not supply one.
                let id = call
                    .get("id")
                    .and_then(|id_val| id_val.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| format!("call_{i}"));

                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                    signature: None,
                });
            }
        }

        // Finish reason is extracted from done_reason if present.
        let finish_reason = v
            .get("done_reason")
            .and_then(|r| r.as_str())
            .map(String::from);

        // Usage statistics from prompt_eval_count and eval_count.
        let usage = Usage {
            input: v
                .get("prompt_eval_count")
                .and_then(|x| x.as_u64())
                .unwrap_or(0),
            cached: 0,
            output: v.get("eval_count").and_then(|x| x.as_u64()).unwrap_or(0),
            thinking: 0,
        };

        Ok(Response {
            text,
            tool_calls,
            usage,
            finish_reason,
        })
    }

    /// Parses a model listing JSON payload from `GET /tags`.
    ///
    /// # Parameters
    /// - `v`: The JSON response payload returned by the tags endpoint.
    ///
    /// # Returns
    /// A vector of [`ModelInfo`] describing available models and their quantization/parameter details.
    ///
    /// # Errors
    /// Returns [`ProviderError::Parse`] if the payload cannot be parsed.
    pub fn parse_models_list(v: &Value) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut out = Vec::new();

        // Ollama returns models in an array under the "models" key.
        if let Some(models) = v.get("models").and_then(|m| m.as_array()) {
            for m in models {
                let name = m.get("name").and_then(|n| n.as_str()).unwrap_or_default();
                if name.is_empty() {
                    continue;
                }

                // Format model details (parameter size and quantization level) into a concise description.
                let description = m.get("details").and_then(|d| {
                    let param_size = d.get("parameter_size").and_then(|s| s.as_str());
                    let quant = d.get("quantization_level").and_then(|q| q.as_str());
                    match (param_size, quant) {
                        (Some(p), Some(q)) => Some(format!("{p}, {q}")),
                        (Some(p), None) => Some(p.to_string()),
                        (None, Some(q)) => Some(q.to_string()),
                        (None, None) => None,
                    }
                });

                out.push(ModelInfo {
                    id: name.to_string(),
                    display_name: name.to_string(),
                    description,
                    provider: "ollama".to_string(),
                });
            }
        }

        Ok(out)
    }
}

impl Provider for Ollama {
    /// Executes a synchronous completion request to Ollama's `POST /chat` endpoint.
    ///
    /// Attaches Bearer authentication header when `api_key` is present.
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        let url = format!("{}/chat", self.api_root);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout_read(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
            .build();
        let mut req_builder = agent.post(&url).set("content-type", "application/json");

        // Attach Authorization header if credentials are configured.
        if let Some(ref key) = self.api_key {
            req_builder = req_builder.set("Authorization", &format!("Bearer {key}"));
        }

        let resp = req_builder.send_json(self.body(req));

        match resp {
            Ok(r) => {
                let v: Value = r
                    .into_json()
                    .map_err(|e| ProviderError::Parse(e.to_string()))?;
                Self::parse(&v)
            }
            Err(ureq::Error::Status(status, r)) => Err(ProviderError::Api {
                status,
                body: r.into_string().unwrap_or_default(),
            }),
            Err(e) => Err(ProviderError::Transport(e.to_string())),
        }
    }

    /// Returns the provider telemetry identifier (e.g. `ollama/qwen2.5-coder:32b`).
    fn model_id(&self) -> String {
        format!("ollama/{}", self.model)
    }

    /// Discovers available models by querying `GET /tags`.
    ///
    /// Attaches Bearer authentication header when `api_key` is present.
    fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = format!("{}/tags", self.api_root);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout_read(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
            .build();
        let mut req_builder = agent.get(&url);

        // Attach Authorization header if credentials are configured.
        if let Some(ref key) = self.api_key {
            req_builder = req_builder.set("Authorization", &format!("Bearer {key}"));
        }

        let resp = req_builder.call();

        match resp {
            Ok(r) => {
                let v: Value = r
                    .into_json()
                    .map_err(|e| ProviderError::Parse(e.to_string()))?;
                Self::parse_models_list(&v)
            }
            Err(ureq::Error::Status(status, r)) => Err(ProviderError::Api {
                status,
                body: r.into_string().unwrap_or_default(),
            }),
            Err(e) => Err(ProviderError::Transport(e.to_string())),
        }
    }
}
