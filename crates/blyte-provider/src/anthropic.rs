//! Anthropic Claude via the Messages API (`POST /v1/messages`) and Models API (`GET /v1/models`).
//!
//! # Protocol and Wire Format
//!
//! Anthropic uses content blocks for multimodal and tool-use turns. Unlike Gemini (which expects
//! functionCall and functionResponse in `model` and `user` turns without explicit tool call IDs),
//! Anthropic requires explicit IDs:
//! - An assistant turn issues tool calls via `{"type": "tool_use", "id": "...", "name": "...", "input": {...}}`.
//! - The next user turn responds via `{"type": "tool_result", "tool_use_id": "...", "content": "...", "is_error": bool}`.
//! - The system prompt is passed as a top-level string field `"system"`, outside the `messages` array.
//! - Prompt caching is tracked via `cache_read_input_tokens`, which must be deducted from `input_tokens`
//!   to avoid double-counting cached tokens against fresh input costs.

use serde_json::{json, Value};

use crate::{Message, ModelInfo, Provider, ProviderError, Request, Response, ToolCall, Usage};

/// Default model identifier when `BLYTE_MODEL` is not specified in the environment.
const DEFAULT_MODEL: &str = "claude-3-7-sonnet-latest";

/// Base URL for Anthropic's REST API endpoints.
const API_ROOT: &str = "https://api.anthropic.com/v1";

/// Anthropic API version header required by all requests.
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Connection timeout in seconds.
///
/// Bounded to prevent silent indefinite hanging if the network or gateway is unresponsive.
const CONNECT_TIMEOUT_SECS: u64 = 20;

/// Read timeout in seconds.
///
/// Long conversations or extended model reasoning runs can take several minutes.
const READ_TIMEOUT_SECS: u64 = 300;

/// Standard convenience aliases always made available for interactive selection.
const STANDARD_ALIASES: &[(&str, &str)] = &[
    ("claude-3-7-sonnet-latest", "Claude 3.7 Sonnet (Latest)"),
    ("claude-3-5-sonnet-latest", "Claude 3.5 Sonnet (Latest)"),
    ("claude-3-5-haiku-latest", "Claude 3.5 Haiku (Latest)"),
];

/// Anthropic Claude model provider adapter.
pub struct Anthropic {
    api_key: String,
    model: String,
}

impl Anthropic {
    /// Constructs an Anthropic provider instance from environment variables.
    ///
    /// Reads `ANTHROPIC_API_KEY` for authentication credentials and optionally `BLYTE_MODEL`
    /// for overriding the default model identifier.
    ///
    /// # Returns
    /// An initialized [`Anthropic`] instance on success.
    ///
    /// # Errors
    /// Returns [`ProviderError::MissingKey`] if `ANTHROPIC_API_KEY` is unset or empty.
    pub fn from_env() -> Result<Self, ProviderError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| ProviderError::MissingKey("ANTHROPIC_API_KEY"))?;
        Ok(Self {
            api_key,
            model: std::env::var("BLYTE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
        })
    }

    /// Sets the model identifier to use for completion requests.
    ///
    /// # Parameters
    /// - `model`: A model identifier string or alias.
    ///
    /// # Returns
    /// The updated [`Anthropic`] instance.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Serializes a normalised [`Request`] into Anthropic's Messages API request body JSON.
    ///
    /// # Parameters
    /// - `req`: The generic turn request containing conversation messages, system prompt, and tools.
    ///
    /// # Returns
    /// A [`Value`] JSON object adhering to Anthropic's `POST /v1/messages` schema.
    pub fn body(&self, req: &Request) -> Value {
        // Convert messages into Anthropic's role and content block structures.
        let mut messages: Vec<Value> = Vec::new();

        let mut push_block = |role: &str, block: Value| {
            if let Some(last) = messages.last_mut() {
                if last.get("role").and_then(|r| r.as_str()) == Some(role) {
                    if let Some(content) = last.get_mut("content").and_then(|c| c.as_array_mut()) {
                        content.push(block);
                        return;
                    }
                }
            }
            messages.push(json!({
                "role": role,
                "content": [block],
            }));
        };

        for msg in &req.messages {
            match msg {
                Message::User(text) => {
                    push_block(
                        "user",
                        json!({
                            "type": "text",
                            "text": text,
                        }),
                    );
                }
                Message::Assistant(text) => {
                    push_block(
                        "assistant",
                        json!({
                            "type": "text",
                            "text": text,
                        }),
                    );
                }
                Message::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    // Assistant tool invocations are represented as tool_use content blocks.
                    push_block(
                        "assistant",
                        json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": arguments,
                        }),
                    );
                }
                Message::ToolResult {
                    id,
                    content,
                    is_error,
                    ..
                } => {
                    // Tool results must be returned as user turns containing tool_result content blocks.
                    push_block(
                        "user",
                        json!({
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": content,
                            "is_error": is_error,
                        }),
                    );
                }
            }
        }

        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_output_tokens.unwrap_or(8192),
            "messages": messages,
        });

        // Top-level system prompt is omitted if empty, preventing empty instructions.
        if !req.system.trim().is_empty() {
            body["system"] = json!(req.system);
        }

        // Tools are converted to input_schema objects, or omitted entirely if empty.
        if !req.tools.is_empty() {
            let tools_json: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = json!(tools_json);
        }

        body
    }

    /// Parses an Anthropic Messages API response JSON payload into a normalised [`Response`].
    ///
    /// # Parameters
    /// - `v`: The JSON response payload returned by the Messages endpoint.
    ///
    /// # Returns
    /// A normalised [`Response`] containing text output, tool invocations, and token usage.
    ///
    /// # Errors
    /// Returns [`ProviderError::Parse`] if the payload lacks a valid `content` array or cannot be parsed.
    pub fn parse(v: &Value) -> Result<Response, ProviderError> {
        // Ensure content array is present; error payloads or malformed responses fail fast.
        let content = v
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or_else(|| ProviderError::Parse(format!("no content array in response: {v}")))?;

        let mut out = Response {
            finish_reason: v
                .get("stop_reason")
                .and_then(|s| s.as_str())
                .map(String::from),
            ..Default::default()
        };

        // Iterate through content blocks extracting plain text and tool_use invocations.
        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        out.text.push_str(text);
                    }
                }
                Some("tool_use") => {
                    let id = block
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let arguments = block.get("input").cloned().unwrap_or(json!({}));
                    out.tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                        signature: None,
                    });
                }
                _ => {}
            }
        }

        // Account for token usage. In Anthropic's Messages API, input_tokens already captures
        // non-cached fresh tokens, while cache_read_input_tokens captures prompt cache reads.
        if let Some(u) = v.get("usage") {
            let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            let input_tokens = n("input_tokens");
            let cache_read = n("cache_read_input_tokens");
            out.usage = Usage {
                input: input_tokens,
                cached: cache_read,
                output: n("output_tokens"),
                thinking: 0,
            };
        }

        Ok(out)
    }

    /// Parses a model listing JSON payload from `GET /v1/models`.
    ///
    /// # Parameters
    /// - `v`: The JSON response payload returned by the Models endpoint.
    ///
    /// # Returns
    /// A vector of [`ModelInfo`] describing available models and standard aliases.
    ///
    /// # Errors
    /// Returns [`ProviderError::Parse`] if the payload cannot be processed.
    pub fn parse_models_list(v: &Value) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut out = Vec::new();

        // Anthropic returns models in an array under the "data" key.
        if let Some(data) = v.get("data").and_then(|d| d.as_array()) {
            for m in data {
                let id = m.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                if id.is_empty() {
                    continue;
                }

                let display_name = m
                    .get("display_name")
                    .and_then(|d| d.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| id.to_string());

                let description = m
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(String::from);

                out.push(ModelInfo {
                    id: id.to_string(),
                    display_name,
                    description,
                    provider: "anthropic".to_string(),
                });
            }
        }

        // Guarantee standard convenience aliases are selectable even if not listed directly.
        for &(alias_id, alias_name) in STANDARD_ALIASES {
            if !out.iter().any(|m| m.id == alias_id) {
                out.push(ModelInfo {
                    id: alias_id.to_string(),
                    display_name: alias_name.to_string(),
                    description: None,
                    provider: "anthropic".to_string(),
                });
            }
        }

        Ok(out)
    }
}

impl Provider for Anthropic {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        let url = format!("{API_ROOT}/messages");
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout_read(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
            .build();
        let resp = agent
            .post(&url)
            .set("x-api-key", &self.api_key)
            .set("anthropic-version", ANTHROPIC_VERSION)
            .set("content-type", "application/json")
            .send_json(self.body(req));

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

    fn model_id(&self) -> String {
        format!("anthropic/{}", self.model)
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = format!("{API_ROOT}/models");
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout_read(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
            .build();
        let resp = agent
            .get(&url)
            .set("x-api-key", &self.api_key)
            .set("anthropic-version", ANTHROPIC_VERSION)
            .call();

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
