//! Gemini via the Generative Language API (AI Studio key auth).
//!
//! Vertex is the other access path and uses the same request body with ADC and a different
//! host; it slots in behind `Provider` when needed. AI Studio is first because it needs one
//! header and no cloud project, so the loop can be built before auth becomes a project.

use serde_json::{json, Value};

use crate::{Message, ModelInfo, Provider, ProviderError, Request, Response, ToolCall, Usage};

const DEFAULT_MODEL: &str = "gemini-3.8-flash";

/// Deadlines on the HTTP call itself.
///
/// The third instance of one bug in a single afternoon. A command with no deadline let
/// `find /` run for fifteen minutes; stdin left inherited let a command wait forever on a
/// terminal nobody was watching; and an HTTP client with no timeout blocked a run for
/// seventeen minutes on a single request, with no child process to show for it and nothing in
/// the log after "turn 2".
///
/// A harness is largely a collection of things that can wait forever. Every one of them needs
/// a number, and the absence of one is not a neutral default -- it is the longest possible
/// value.
const CONNECT_TIMEOUT_SECS: u64 = 20;
/// Generous: a thinking model on a long conversation legitimately takes minutes. Bounded all
/// the same, because "legitimately slow" and "never coming back" are indistinguishable without
/// a deadline.
const READ_TIMEOUT_SECS: u64 = 300;
const API_ROOT: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// agy's effort suffixes that map onto Gemini's `thinkingLevel`. `-xhigh` and `-max` are
/// deliberately absent: they have no Gemini equivalent, so they stay in the model id and the
/// API rejects the request instead of running at a silently different effort.
const EFFORT_LEVELS: [&str; 3] = ["low", "medium", "high"];

/// Split `gemini-3.8-flash-high` into `("gemini-3.8-flash", Some("high"))`.
///
/// The eval pins every driver to one model string, agy's, which carries the effort as a
/// suffix. Sending that string to the API verbatim either fails or silently drops the effort --
/// the "`--model` not pinned" parity defect in another form.
pub fn split_effort(model: &str) -> (String, Option<String>) {
    for level in EFFORT_LEVELS {
        if let Some(base) = model.strip_suffix(&format!("-{level}")) {
            if !base.is_empty() {
                return (base.to_string(), Some(level.to_string()));
            }
        }
    }
    (model.to_string(), None)
}

/// Append a part to the last content. Used to keep a parallel batch in one content.
fn push_part(contents: &mut [Value], part: Value) {
    if let Some(parts) = contents.last_mut().and_then(|c| c["parts"].as_array_mut()) {
        parts.push(part);
    }
}

pub struct Gemini {
    api_key: String,
    model: String,
    thinking_level: Option<String>,
}

impl Gemini {
    /// Reads `GEMINI_API_KEY`, falling back to `GOOGLE_API_KEY` as pillars does.
    pub fn from_env() -> Result<Self, ProviderError> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .map_err(|_| ProviderError::MissingKey("GEMINI_API_KEY"))?;
        let (model, thinking_level) = split_effort(
            &std::env::var("SUPERSTATE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
        );
        Ok(Self {
            api_key,
            model,
            thinking_level,
        })
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        let (model, thinking_level) = split_effort(&model.into());
        self.model = model;
        self.thinking_level = thinking_level;
        self
    }

    /// The `thinkingLevel` sent with every request, if the model string carried an effort.
    pub fn thinking_level(&self) -> Option<&str> {
        self.thinking_level.as_deref()
    }

    /// Build the request body. Separated from the HTTP call so the wire shape is testable
    /// without a network or a key -- the mapping is where the bugs live, not the POST.
    pub fn body(&self, req: &Request) -> Value {
        let mut contents: Vec<Value> = Vec::new();
        let mut prev: Option<&Message> = None;
        for m in &req.messages {
            match m {
                Message::User(t) => contents.push(json!({
                    "role": "user", "parts": [{"text": t}]
                })),
                Message::Assistant(t) => contents.push(json!({
                    "role": "model", "parts": [{"text": t}]
                })),
                Message::ToolCall {
                    name,
                    arguments,
                    signature,
                    ..
                } => {
                    let mut part = json!({"functionCall": {"name": name, "args": arguments}});
                    // Echoed verbatim. Gemini 3.x binds the model's reasoning to the call and
                    // rejects the turn outright if it is absent.
                    if let Some(sig) = signature {
                        part["thoughtSignature"] = json!(sig);
                    }
                    // Calls from one turn are one model content: Gemini signs only the first call
                    // of a parallel batch, so a batch split into separate turns would be rejected.
                    if matches!(prev, Some(Message::ToolCall { .. })) {
                        push_part(&mut contents, part);
                    } else {
                        contents.push(json!({"role": "model", "parts": [part]}));
                    }
                }
                // Gemini carries no tool-call id: a functionResponse is matched to its call by
                // NAME and position. Ids are kept in our own Message type anyway, because the
                // loop needs them and other providers require them.
                Message::ToolResult {
                    name,
                    content,
                    is_error,
                    ..
                } => {
                    let part = json!({"functionResponse": {
                        "name": name,
                        "response": {"output": content, "error": is_error}
                    }});
                    if matches!(prev, Some(Message::ToolResult { .. })) {
                        push_part(&mut contents, part);
                    } else {
                        contents.push(json!({"role": "user", "parts": [part]}));
                    }
                }
            }
            prev = Some(m);
        }

        let mut body = json!({ "contents": contents });
        if !req.system.trim().is_empty() {
            body["systemInstruction"] = json!({"parts": [{"text": req.system}]});
        }
        if !req.tools.is_empty() {
            let decls: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    })
                })
                .collect();
            body["tools"] = json!([{ "functionDeclarations": decls }]);
        }
        let mut generation = serde_json::Map::new();
        if let Some(max) = req.max_output_tokens {
            generation.insert("maxOutputTokens".into(), json!(max));
        }
        if let Some(level) = &self.thinking_level {
            generation.insert("thinkingConfig".into(), json!({"thinkingLevel": level}));
        }
        if !generation.is_empty() {
            body["generationConfig"] = Value::Object(generation);
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
            finish_reason: cand
                .get("finishReason")
                .and_then(|f| f.as_str())
                .map(String::from),
            ..Default::default()
        };

        if let Some(parts) = cand.pointer("/content/parts").and_then(|p| p.as_array()) {
            for (i, part) in parts.iter().enumerate() {
                if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                    out.text.push_str(t);
                }
                if let Some(fc) = part.get("functionCall") {
                    out.tool_calls.push(ToolCall {
                        signature: part
                            .get("thoughtSignature")
                            .and_then(|s| s.as_str())
                            .map(String::from),
                        // Synthesised: Gemini does not issue ids. Stable within a turn, which
                        // is all the loop needs to pair a result with its call.
                        id: format!("call_{i}"),
                        name: fc
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or_default()
                            .to_string(),
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

    /// Parse a models list response body from Google GenAI `models.list`.
    ///
    /// # Parameters
    /// - `v`: The parsed JSON response object from the models endpoint.
    ///
    /// # Returns
    /// A vector of [`ModelInfo`] describing models that support `generateContent`.
    ///
    /// # Errors
    /// Returns [`ProviderError::Parse`] if the payload cannot be processed.
    pub fn parse_models_list(v: &Value) -> Result<Vec<ModelInfo>, ProviderError> {
        let mut out = Vec::new();
        // The models endpoint returns an array under the "models" key.
        if let Some(models) = v.get("models").and_then(|m| m.as_array()) {
            for m in models {
                // Models advertise supported operations; filter out models without content generation (e.g. embedContent only).
                let supports_generation = m
                    .get("supportedGenerationMethods")
                    .and_then(|g| g.as_array())
                    .map(|arr| {
                        arr.iter()
                            .any(|method| method.as_str() == Some("generateContent"))
                    })
                    .unwrap_or(false);

                if !supports_generation {
                    continue;
                }

                // Google prefixes model resource names with "models/"; strip it to yield the canonical model identifier.
                let raw_name = m.get("name").and_then(|n| n.as_str()).unwrap_or_default();
                let id = raw_name
                    .strip_prefix("models/")
                    .unwrap_or(raw_name)
                    .to_string();
                if id.is_empty() {
                    continue;
                }

                // Use the human-readable displayName if provided; fallback to the model identifier.
                let display_name = m
                    .get("displayName")
                    .and_then(|d| d.as_str())
                    .map(String::from)
                    .unwrap_or_else(|| id.clone());

                let description = m
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(String::from);

                out.push(ModelInfo {
                    id,
                    display_name,
                    description,
                    provider: "gemini".to_string(),
                });
            }
        }
        Ok(out)
    }
}

impl Provider for Gemini {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        let url = format!("{API_ROOT}/{}:generateContent", self.model);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout_read(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
            .build();
        let resp = agent
            .post(&url)
            .set("x-goog-api-key", &self.api_key)
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
        format!("gemini/{}", self.model)
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = API_ROOT;
        // Build an agent with bounded connection and read timeouts.
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout_read(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
            .build();
        // Query Google GenAI models list endpoint using AI Studio API key header.
        let resp = agent.get(url).set("x-goog-api-key", &self.api_key).call();

        match resp {
            Ok(r) => {
                // Parse JSON payload and delegate model filtering/extraction.
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
