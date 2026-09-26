//! OpenAI-compatible Chat Completions adapter (`openai/` and `openrouter/`).
//!
//! # Protocol and Wire Format
//!
//! OpenAI and compatible gateways (e.g. OpenRouter, Groq, vLLM, LM Studio) implement
//! the Chat Completions wire format (`POST /chat/completions` and `GET /models`):
//! - The system prompt is passed as the first message with `{"role": "system", "content": "..."}`.
//! - User and assistant messages use `role: "user"` and `role: "assistant"`.
//! - Consecutive tool invocations within a turn are merged into a single assistant message
//!   with `content: null` and an array of `tool_calls`.
//! - Tool results are passed with `{"role": "tool", "tool_call_id": "...", "content": "..."}`.
//! - Tool specifications are declared under `tools` with `type: "function"`.
//! - Output token limit is passed via `max_completion_tokens`.

use serde_json::{json, Value};

use crate::{Message, ModelInfo, Provider, ProviderError, Request, Response, ToolCall, Usage};

/// OpenAI-compatible provider adapter.
#[derive(Debug, Clone)]
pub struct OpenAi {
    prefix: String,
    api_key: Option<String>,
    base_url: String,
    model: String,
}

impl OpenAi {
    /// Constructs an OpenAI provider instance from environment variables.
    ///
    /// Reads `OPENAI_API_KEY` (optional if `OPENAI_BASE_URL` is set), `OPENAI_BASE_URL`
    /// (optional, defaults to `https://api.openai.com/v1`, trailing slashes trimmed),
    /// and `BLYTE_MODEL` (optional, defaults to `gpt-5.5` with any `openai/` prefix stripped).
    ///
    /// # Errors
    /// Returns [`ProviderError::MissingKey`] with `"OPENAI_API_KEY"` if neither
    /// `OPENAI_API_KEY` nor `OPENAI_BASE_URL` is configured.
    pub fn openai_from_env() -> Result<Self, ProviderError> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty());
        let base_url = std::env::var("OPENAI_BASE_URL")
            .ok()
            .filter(|u| !u.trim().is_empty());

        if api_key.is_none() && base_url.is_none() {
            return Err(ProviderError::MissingKey("OPENAI_API_KEY"));
        }

        let base_url = base_url
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string())
            .trim_end_matches('/')
            .to_string();

        let raw_model = std::env::var("BLYTE_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| "gpt-5.5".to_string());
        let model = raw_model
            .strip_prefix("openai/")
            .unwrap_or(&raw_model)
            .to_string();

        Ok(Self {
            prefix: "openai".to_string(),
            api_key,
            base_url,
            model,
        })
    }

    /// Constructs an OpenRouter provider instance from environment variables.
    ///
    /// Reads `OPENROUTER_API_KEY` (required) and connects to `https://openrouter.ai/api/v1`.
    /// Reads `BLYTE_MODEL` (optional, defaults to `openai/gpt-5.5` with any `openrouter/` prefix stripped).
    ///
    /// # Errors
    /// Returns [`ProviderError::MissingKey`] with `"OPENROUTER_API_KEY"` if the key is not set.
    pub fn openrouter_from_env() -> Result<Self, ProviderError> {
        let api_key = std::env::var("OPENROUTER_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or(ProviderError::MissingKey("OPENROUTER_API_KEY"))?;

        let base_url = "https://openrouter.ai/api/v1".to_string();

        let raw_model = std::env::var("BLYTE_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| "openai/gpt-5.5".to_string());
        let model = raw_model
            .strip_prefix("openrouter/")
            .unwrap_or(&raw_model)
            .to_string();

        Ok(Self {
            prefix: "openrouter".to_string(),
            api_key: Some(api_key),
            base_url,
            model,
        })
    }

    /// Sets the model identifier to use for requests.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        let m = model.into();
        let prefix_slash = format!("{}/", self.prefix);
        self.model = m.strip_prefix(&prefix_slash).unwrap_or(&m).to_string();
        self
    }

    /// Sets or overrides the base URL, trimming any trailing slashes.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    /// Sets or overrides the API key for Bearer authentication.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Returns the configured API key if present.
    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    /// Returns the configured base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Returns the stripped wire model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Serializes a normalised [`Request`] into an OpenAI Chat Completions JSON body.
    pub fn body(&self, req: &Request) -> Value {
        let mut messages: Vec<Value> = Vec::new();

        if !req.system.trim().is_empty() {
            messages.push(json!({
                "role": "system",
                "content": req.system,
            }));
        }

        let mut iter = req.messages.iter().peekable();
        while let Some(msg) = iter.next() {
            match msg {
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
                    let mut tool_calls = vec![json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": match arguments {
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            },
                        }
                    })];

                    while let Some(Message::ToolCall { .. }) = iter.peek() {
                        if let Some(Message::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        }) = iter.next()
                        {
                            tool_calls.push(json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": match arguments {
                                        Value::String(s) => s.clone(),
                                        other => other.to_string(),
                                    },
                                }
                            }));
                        }
                    }

                    messages.push(json!({
                        "role": "assistant",
                        "content": Value::Null,
                        "tool_calls": tool_calls,
                    }));
                }
                Message::ToolResult { id, content, .. } => {
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
            "messages": messages,
        });

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

        if let Some(tokens) = req.max_output_tokens {
            body["max_completion_tokens"] = json!(tokens);
        }

        body
    }

    /// Parses an OpenAI Chat Completions response JSON payload into a normalised [`Response`].
    pub fn parse(v: &Value) -> Result<Response, ProviderError> {
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

        let choice = v
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .ok_or_else(|| ProviderError::Parse(format!("no choices in response: {v}")))?;

        let message = choice.get("message");

        let text = message
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string();

        let mut tool_calls = Vec::new();
        if let Some(calls) = message
            .and_then(|m| m.get("tool_calls"))
            .and_then(|c| c.as_array())
        {
            for (i, call) in calls.iter().enumerate() {
                let function = call.get("function");
                let name = function
                    .and_then(|f| f.get("name"))
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string();

                let arguments = match function.and_then(|f| f.get("arguments")) {
                    Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({})),
                    Some(Value::Object(map)) => Value::Object(map.clone()),
                    _ => json!({}),
                };

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

        let finish_reason = choice
            .get("finish_reason")
            .and_then(|r| r.as_str())
            .map(String::from);

        let usage = if let Some(u) = v.get("usage") {
            let prompt_tokens = u.get("prompt_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
            let cached = u
                .get("prompt_tokens_details")
                .and_then(|d| d.get("cached_tokens"))
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let input = prompt_tokens.saturating_sub(cached);

            let completion_tokens = u
                .get("completion_tokens")
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let thinking = u
                .get("completion_tokens_details")
                .and_then(|d| d.get("reasoning_tokens"))
                .and_then(|x| x.as_u64())
                .unwrap_or(0);
            let output = completion_tokens.saturating_sub(thinking);

            Usage {
                input,
                cached,
                output,
                thinking,
            }
        } else {
            Usage::default()
        };

        Ok(Response {
            text,
            tool_calls,
            usage,
            finish_reason,
        })
    }

    /// Parses a model listing JSON payload from `GET /models`.
    pub fn parse_models_list(&self, v: &Value) -> Result<Vec<ModelInfo>, ProviderError> {
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

        let mut out = Vec::new();
        if let Some(data) = v.get("data").and_then(|d| d.as_array()) {
            for m in data {
                if let Some(id) = m.get("id").and_then(|i| i.as_str()) {
                    if id.is_empty() {
                        continue;
                    }
                    out.push(ModelInfo {
                        id: id.to_string(),
                        display_name: id.to_string(),
                        description: None,
                        provider: self.prefix.clone(),
                    });
                }
            }
        }

        Ok(out)
    }
}

impl Provider for OpenAi {
    /// Executes a synchronous completion request to `{base}/chat/completions`.
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        let url = format!("{}/chat/completions", self.base_url);
        let agent = crate::http_client();
        let mut req_builder = agent.post(&url).set("content-type", "application/json");

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

    /// Returns the provider telemetry identifier (e.g. `openai/gpt-5.5` or `openrouter/...`).
    fn model_id(&self) -> String {
        format!("{}/{}", self.prefix, self.model)
    }

    /// Discovers available models by querying `GET /models`.
    fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = format!("{}/models", self.base_url);
        let agent = crate::http_client();
        let mut req_builder = agent.get(&url);

        if let Some(ref key) = self.api_key {
            req_builder = req_builder.set("Authorization", &format!("Bearer {key}"));
        }

        let resp = req_builder.call();

        match resp {
            Ok(r) => {
                let v: Value = r
                    .into_json()
                    .map_err(|e| ProviderError::Parse(e.to_string()))?;
                self.parse_models_list(&v)
            }
            Err(ureq::Error::Status(status, r)) => Err(ProviderError::Api {
                status,
                body: r.into_string().unwrap_or_default(),
            }),
            Err(e) => Err(ProviderError::Transport(e.to_string())),
        }
    }
}
