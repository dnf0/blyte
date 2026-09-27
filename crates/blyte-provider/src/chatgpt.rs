//! ChatGPT-login adapter (`chatgpt/`), reusing a Codex CLI login without refreshing it.
//!
//! # Protocol and Wire Format
//!
//! Reuses the local Codex CLI login credentials stored in `$CODEX_HOME/auth.json`
//! (defaulting to `~/.codex/auth.json`). Never performs login or refreshes tokens,
//! avoiding token rotation conflicts with Codex CLI.
//!
//! Communicates with the OpenAI Responses API at `https://chatgpt.com/backend-api/codex/responses`
//! streaming over Server-Sent Events (SSE).

use serde_json::{json, Value};
use std::path::Path;

use crate::{Message, ModelInfo, Provider, ProviderError, Request, Response, ToolCall, Usage};

/// ChatGPT-login provider adapter reusing Codex CLI authentication.
#[derive(Debug, Clone)]
pub struct ChatGpt {
    access_token: String,
    account_id: String,
    model: String,
}

impl ChatGpt {
    /// Loads credentials from the specified Codex home directory.
    ///
    /// Reads `<home>/auth.json`. Returns [`ProviderError::MissingKey`] with `"CODEX_HOME"`
    /// if the file is missing or unreadable. Returns [`ProviderError::Parse`] if the
    /// access token is missing, the account ID cannot be determined from either the JSON
    /// or the JWT payload claims, or if the token expires within 300 seconds of `now_unix`.
    pub fn from_codex_home(home: &Path, now_unix: u64) -> Result<Self, ProviderError> {
        let auth_path = home.join("auth.json");
        let content = std::fs::read_to_string(&auth_path)
            .map_err(|_| ProviderError::MissingKey("CODEX_HOME"))?;

        let auth_json: Value = serde_json::from_str(&content)
            .map_err(|e| ProviderError::Parse(format!("failed to parse auth.json: {e}")))?;

        let tokens = auth_json.get("tokens").ok_or_else(|| {
            ProviderError::Parse("missing tokens object in auth.json".to_string())
        })?;

        let access_token = tokens
            .get("access_token")
            .and_then(|t| t.as_str())
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| ProviderError::Parse("missing access_token in auth.json".to_string()))?;

        // Parse JWT payload: header.payload.signature
        let parts: Vec<&str> = access_token.split('.').collect();
        if parts.len() < 2 {
            return Err(ProviderError::Parse(
                "invalid access_token JWT format in auth.json".to_string(),
            ));
        }

        let payload_bytes = decode_b64url(parts[1])
            .map_err(|e| ProviderError::Parse(format!("failed to decode JWT payload: {e}")))?;

        let payload_json: Value = serde_json::from_slice(&payload_bytes)
            .map_err(|e| ProviderError::Parse(format!("failed to parse JWT payload JSON: {e}")))?;

        // Token expiry check: exp claim in JWT payload
        let exp = payload_json
            .get("exp")
            .and_then(|e| e.as_u64())
            .ok_or_else(|| {
                ProviderError::Parse("missing exp claim in access_token JWT".to_string())
            })?;

        if exp <= now_unix.saturating_add(300) {
            return Err(ProviderError::Parse(
                "ChatGPT login token expired or expiring soon; run any codex command to refresh your ChatGPT login"
                    .to_string(),
            ));
        }

        // Account ID: tokens.account_id, or JWT claim "https://api.openai.com/auth" -> "chatgpt_account_id"
        let mut account_id = tokens
            .get("account_id")
            .and_then(|a| a.as_str())
            .filter(|a| !a.trim().is_empty())
            .map(String::from);

        if account_id.is_none() {
            account_id = payload_json
                .get("https://api.openai.com/auth")
                .and_then(|a| a.get("chatgpt_account_id"))
                .and_then(|id| id.as_str())
                .filter(|id| !id.trim().is_empty())
                .map(String::from);
        }

        let account_id = account_id.ok_or_else(|| {
            ProviderError::Parse(
                "missing account_id in auth.json tokens and JWT claims".to_string(),
            )
        })?;

        Ok(Self {
            access_token: access_token.to_string(),
            account_id,
            model: "gpt-5.5".to_string(),
        })
    }

    /// Constructs a ChatGPT provider instance from the ambient environment.
    ///
    /// Reads credentials from `$CODEX_HOME` or falls back to `~/.codex`.
    /// Reads `BLYTE_MODEL` (optional, defaults to `gpt-5.5` with any `chatgpt/` prefix stripped).
    pub fn from_env() -> Result<Self, ProviderError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let home = if let Ok(h) = std::env::var("CODEX_HOME").map(|h| h.trim().to_string()) {
            if !h.is_empty() {
                std::path::PathBuf::from(h)
            } else if let Ok(home_dir) = std::env::var("HOME") {
                std::path::PathBuf::from(home_dir).join(".codex")
            } else {
                std::path::PathBuf::from(".codex")
            }
        } else if let Ok(home_dir) = std::env::var("HOME") {
            std::path::PathBuf::from(home_dir).join(".codex")
        } else {
            std::path::PathBuf::from(".codex")
        };

        let raw_model = std::env::var("BLYTE_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| "gpt-5.5".to_string());
        let model = raw_model
            .strip_prefix("chatgpt/")
            .unwrap_or(&raw_model)
            .to_string();

        Self::from_codex_home(&home, now).map(|p| p.with_model(model))
    }

    /// Sets the model identifier to use for requests.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        let m = model.into();
        self.model = m.strip_prefix("chatgpt/").unwrap_or(&m).to_string();
        self
    }

    /// Returns the ChatGPT account identifier.
    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// Returns the stripped wire model identifier.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Serializes a normalised [`Request`] into an OpenAI Responses API streaming request body.
    pub fn body(&self, req: &Request) -> Value {
        let instructions = if req.system.trim().is_empty() {
            "You are a helpful assistant.".to_string()
        } else {
            req.system.clone()
        };

        let mut input: Vec<Value> = Vec::new();
        for msg in &req.messages {
            match msg {
                Message::User(text) => {
                    input.push(json!({
                        "type": "message",
                        "role": "user",
                        "content": [
                            {
                                "type": "input_text",
                                "text": text,
                            }
                        ]
                    }));
                }
                Message::Assistant(text) => {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [
                            {
                                "type": "output_text",
                                "text": text,
                            }
                        ]
                    }));
                }
                Message::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } => {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": match arguments {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        },
                    }));
                }
                Message::ToolResult { id, content, .. } => {
                    input.push(json!({
                        "type": "function_call_output",
                        "call_id": id,
                        "output": content,
                    }));
                }
            }
        }

        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                    "strict": false,
                })
            })
            .collect();

        let mut body = json!({
            "model": self.model,
            "instructions": instructions,
            "input": input,
            "tools": tools,
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "store": false,
            "stream": true,
        });

        if let Some(tokens) = req.max_output_tokens {
            body["max_output_tokens"] = json!(tokens);
        }

        body
    }

    /// Parses an SSE text payload into a normalised [`Response`].
    pub fn parse_sse(text: &str) -> Result<Response, ProviderError> {
        let mut tool_calls = Vec::new();
        let mut response_text = String::new();
        let mut usage = Usage::default();
        let mut finish_reason = None;
        let mut is_error_event = false;

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with(':') {
                continue;
            }

            if let Some(ev) = trimmed.strip_prefix("event:") {
                if ev.trim() == "error" {
                    is_error_event = true;
                }
                continue;
            }

            if let Some(rest) = trimmed.strip_prefix("data:") {
                let data = rest.trim();
                if data == "[DONE]" || data.is_empty() {
                    continue;
                }

                let v: Value = serde_json::from_str(data)
                    .map_err(|e| ProviderError::Parse(format!("failed to parse SSE data: {e}")))?;

                if is_error_event {
                    let msg = v
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or(data)
                        .to_string();
                    return Err(ProviderError::Parse(msg));
                }

                let event_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

                if event_type == "response.failed" {
                    let msg = v
                        .get("response")
                        .and_then(|r| r.get("error"))
                        .map(|e| {
                            if let Some(m) = e.get("message").and_then(|m| m.as_str()) {
                                m.to_string()
                            } else if let Some(s) = e.as_str() {
                                s.to_string()
                            } else {
                                e.to_string()
                            }
                        })
                        .unwrap_or_else(|| "response.failed".to_string());
                    return Err(ProviderError::Parse(msg));
                }

                if event_type == "error" || (v.get("error").is_some() && !v["error"].is_null()) {
                    let err_obj = v.get("error");
                    let msg = err_obj
                        .map(|e| {
                            if let Some(m) = e.get("message").and_then(|m| m.as_str()) {
                                m.to_string()
                            } else if let Some(s) = e.as_str() {
                                s.to_string()
                            } else {
                                e.to_string()
                            }
                        })
                        .unwrap_or_else(|| "error event".to_string());
                    return Err(ProviderError::Parse(msg));
                }

                if event_type == "response.output_item.done" {
                    if let Some(item) = v.get("item") {
                        let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        match item_type {
                            "function_call" => {
                                let id = item
                                    .get("call_id")
                                    .and_then(|id| id.as_str())
                                    .unwrap_or_default()
                                    .to_string();
                                let name = item
                                    .get("name")
                                    .and_then(|n| n.as_str())
                                    .unwrap_or_default()
                                    .to_string();
                                let arguments = match item.get("arguments") {
                                    Some(Value::String(s)) => {
                                        serde_json::from_str(s).unwrap_or_else(|_| json!({}))
                                    }
                                    Some(val) => val.clone(),
                                    None => json!({}),
                                };
                                tool_calls.push(ToolCall {
                                    id,
                                    name,
                                    arguments,
                                    signature: None,
                                });
                            }
                            "message" => {
                                if let Some(content) =
                                    item.get("content").and_then(|c| c.as_array())
                                {
                                    for part in content {
                                        if let Some(t) = part.get("text").and_then(|x| x.as_str()) {
                                            response_text.push_str(t);
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                } else if event_type == "response.completed" {
                    if let Some(resp) = v.get("response") {
                        if let Some(status) = resp.get("status").and_then(|s| s.as_str()) {
                            finish_reason = Some(status.to_string());
                        }
                        if let Some(u) = resp.get("usage") {
                            let input_tokens =
                                u.get("input_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                            let cached = u
                                .get("input_tokens_details")
                                .and_then(|d| d.get("cached_tokens"))
                                .and_then(|x| x.as_u64())
                                .unwrap_or(0);
                            let input = input_tokens.saturating_sub(cached);

                            let output_tokens =
                                u.get("output_tokens").and_then(|x| x.as_u64()).unwrap_or(0);
                            let thinking = u
                                .get("output_tokens_details")
                                .and_then(|d| d.get("reasoning_tokens"))
                                .and_then(|x| x.as_u64())
                                .unwrap_or(0);
                            let output = output_tokens.saturating_sub(thinking);

                            usage = Usage {
                                input,
                                cached,
                                output,
                                thinking,
                            };
                        }
                    }
                }
            }
        }

        Ok(Response {
            text: response_text,
            tool_calls,
            usage,
            finish_reason,
        })
    }
}

impl Provider for ChatGpt {
    /// Executes a synchronous streaming Responses API request and aggregates the SSE body.
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        let url = "https://chatgpt.com/backend-api/codex/responses";
        let agent = crate::http_client();
        let user_agent = format!("blyte/{}", env!("CARGO_PKG_VERSION"));

        let resp = agent
            .post(url)
            .set("Authorization", &format!("Bearer {}", self.access_token))
            .set("ChatGPT-Account-ID", &self.account_id)
            .set("Accept", "text/event-stream")
            .set("Content-Type", "application/json")
            .set("originator", "blyte")
            .set("User-Agent", &user_agent)
            .send_json(self.body(req));

        match resp {
            Ok(r) => {
                let text = r
                    .into_string()
                    .map_err(|e| ProviderError::Parse(e.to_string()))?;
                Self::parse_sse(&text)
            }
            Err(ureq::Error::Status(status, r)) => Err(ProviderError::Api {
                status,
                body: r.into_string().unwrap_or_default(),
            }),
            Err(e) => Err(ProviderError::Transport(e.to_string())),
        }
    }

    /// Returns the provider telemetry identifier (e.g. `chatgpt/gpt-5.5`).
    fn model_id(&self) -> String {
        format!("chatgpt/{}", self.model)
    }

    /// Lists supported models without making network calls.
    fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(vec![ModelInfo {
            id: "gpt-5.5".to_string(),
            display_name: "gpt-5.5".to_string(),
            description: None,
            provider: "chatgpt".to_string(),
        }])
    }
}

/// Decodes an unpadded or padded base64url string.
fn decode_b64url(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for &b in s.as_bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return Err(format!("invalid base64url byte: {}", b as char)),
        };
        buf = (buf << 6) | (val as u32);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}
