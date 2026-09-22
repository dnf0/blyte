//! Gemini via the Generative Language API (AI Studio key auth).
//!
//! Vertex is the other access path and uses the same request body with ADC and a different
//! host; it slots in behind `Provider` when needed. AI Studio is first because it needs one
//! header and no cloud project, so the loop can be built before auth becomes a project.

use serde_json::{json, Value};

use crate::{Message, Provider, ProviderError, Request, Response, ToolCall, Usage};

const DEFAULT_MODEL: &str = "gemini-2.5-flash";
const API_ROOT: &str = "https://generativelanguage.googleapis.com/v1beta/models";

pub struct Gemini {
    api_key: String,
    model: String,
}

impl Gemini {
    /// Reads `GEMINI_API_KEY`, falling back to `GOOGLE_API_KEY` as pillars does.
    pub fn from_env() -> Result<Self, ProviderError> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .map_err(|_| ProviderError::MissingKey("GEMINI_API_KEY"))?;
        Ok(Self {
            api_key,
            model: std::env::var("SUPERSTATE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
        })
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Build the request body. Separated from the HTTP call so the wire shape is testable
    /// without a network or a key -- the mapping is where the bugs live, not the POST.
    pub fn body(&self, req: &Request) -> Value {
        let mut contents: Vec<Value> = Vec::new();
        for m in &req.messages {
            match m {
                Message::User(t) => contents.push(json!({
                    "role": "user", "parts": [{"text": t}]
                })),
                Message::Assistant(t) => contents.push(json!({
                    "role": "model", "parts": [{"text": t}]
                })),
                Message::ToolCall { name, arguments, .. } => contents.push(json!({
                    "role": "model",
                    "parts": [{"functionCall": {"name": name, "args": arguments}}]
                })),
                // Gemini carries no tool-call id: a functionResponse is matched to its call by
                // NAME and position. Ids are kept in our own Message type anyway, because the
                // loop needs them and other providers require them.
                Message::ToolResult { name, content, is_error, .. } => contents.push(json!({
                    "role": "user",
                    "parts": [{"functionResponse": {
                        "name": name,
                        "response": {"output": content, "error": is_error}
                    }}]
                })),
            }
        }

        let mut body = json!({ "contents": contents });
        if !req.system.trim().is_empty() {
            body["systemInstruction"] = json!({"parts": [{"text": req.system}]});
        }
        if !req.tools.is_empty() {
            let decls: Vec<Value> = req
                .tools
                .iter()
                .map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters
                }))
                .collect();
            body["tools"] = json!([{ "functionDeclarations": decls }]);
        }
        if let Some(max) = req.max_output_tokens {
            body["generationConfig"] = json!({"maxOutputTokens": max});
        }
        body
    }

    /// Parse a response body. Also separated for testing: this is where a provider's shape
    /// changes silently under you.
    pub fn parse(v: &Value) -> Result<Response, ProviderError> {
        let cand = v
            .get("candidates")
            .and_then(|c| c.get(0))
            .ok_or_else(|| ProviderError::Parse(format!("no candidates in {v}")))?;

        let mut out = Response {
            finish_reason: cand.get("finishReason").and_then(|f| f.as_str()).map(String::from),
            ..Default::default()
        };

        if let Some(parts) = cand.pointer("/content/parts").and_then(|p| p.as_array()) {
            for (i, part) in parts.iter().enumerate() {
                if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                    out.text.push_str(t);
                }
                if let Some(fc) = part.get("functionCall") {
                    out.tool_calls.push(ToolCall {
                        // Synthesised: Gemini does not issue ids. Stable within a turn, which
                        // is all the loop needs to pair a result with its call.
                        id: format!("call_{i}"),
                        name: fc.get("name").and_then(|n| n.as_str()).unwrap_or_default().to_string(),
                        arguments: fc.get("args").cloned().unwrap_or(json!({})),
                    });
                }
            }
        }

        if let Some(u) = v.get("usageMetadata") {
            let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
            out.usage = Usage {
                // promptTokenCount INCLUDES cached tokens, so subtract to get what was actually
                // fresh. Reporting the raw figure would overstate the cost of a well-cached
                // turn, which is the number this harness exists to reduce.
                input: n("promptTokenCount").saturating_sub(n("cachedContentTokenCount")),
                cached: n("cachedContentTokenCount"),
                output: n("candidatesTokenCount"),
                thinking: n("thoughtsTokenCount"),
            };
        }
        Ok(out)
    }
}

impl Provider for Gemini {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        let url = format!("{API_ROOT}/{}:generateContent", self.model);
        let resp = ureq::post(&url)
            .set("x-goog-api-key", &self.api_key)
            .set("content-type", "application/json")
            .send_json(self.body(req));

        match resp {
            Ok(r) => {
                let v: Value = r.into_json().map_err(|e| ProviderError::Parse(e.to_string()))?;
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
        format!("gemini/{}", self.model)
    }
}
