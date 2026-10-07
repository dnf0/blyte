//! Gemini via the Generative Language API (AI Studio key auth) or Vertex AI.
//!
//! Two access paths share the wire shape (contents, systemInstruction, tools, generationConfig):
//! 1. AI Studio: uses an API key via `x-goog-api-key` header to `generativelanguage.googleapis.com`.
//! 2. Vertex AI: uses Application Default Credentials (`Authorization: Bearer <token>`) to
//!    `<location>-aiplatform.googleapis.com` (or `aiplatform.googleapis.com` when global).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::{Message, ModelInfo, Provider, ProviderError, Request, Response, ToolCall, Usage};

/// High effort: every measured run (the A/B pairs and the evals, 2026-09) used `-high`.
const DEFAULT_MODEL: &str = "gemini-3.8-flash-high";

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

/// Pure function computing the Vertex AI generateContent endpoint URL.
///
/// Host is `aiplatform.googleapis.com` when location is `global`, else `<location>-aiplatform.googleapis.com`.
/// The model identifier has any effort suffix stripped.
pub fn vertex_url(project: &str, location: &str, model: &str) -> String {
    let (base_model, _) = split_effort(model);
    let host = if location == "global" {
        "aiplatform.googleapis.com".to_string()
    } else {
        format!("{location}-aiplatform.googleapis.com")
    };
    format!("https://{host}/v1/projects/{project}/locations/{location}/publishers/google/models/{base_model}:generateContent")
}

/// Provider of OAuth2 access tokens for Vertex AI requests.
pub trait TokenSource: Send + Sync {
    fn token(&self) -> Result<String, ProviderError>;
}

impl<F> TokenSource for F
where
    F: Fn() -> Result<String, ProviderError> + Send + Sync,
{
    fn token(&self) -> Result<String, ProviderError> {
        self()
    }
}

/// Default token source running `gcloud auth application-default print-access-token`.
pub struct GcloudTokenSource;

impl TokenSource for GcloudTokenSource {
    fn token(&self) -> Result<String, ProviderError> {
        let output = std::process::Command::new("gcloud")
            .args(["auth", "application-default", "print-access-token"])
            .output()
            .map_err(|e| {
                ProviderError::Auth(format!(
                    "failed to execute `gcloud`: {e}; run `gcloud auth application-default login`"
                ))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ProviderError::Auth(format!(
                "gcloud failed with exit status {}: {}; run `gcloud auth application-default login`",
                output.status,
                stderr.trim()
            )));
        }

        let token = String::from_utf8(output.stdout)
            .map_err(|_| {
                ProviderError::Auth(
                    "invalid utf-8 in access token from gcloud; run `gcloud auth application-default login`"
                        .to_string(),
                )
            })?
            .trim()
            .to_string();

        if token.is_empty() {
            return Err(ProviderError::Auth(
                "gcloud returned empty access token; run `gcloud auth application-default login`"
                    .to_string(),
            ));
        }

        Ok(token)
    }
}

/// Vertex AI authentication settings and access token cache.
pub struct VertexAuth {
    pub project: String,
    pub location: String,
    token_source: Arc<dyn TokenSource>,
    token_ttl: Duration,
    clock: Arc<dyn Fn() -> Instant + Send + Sync>,
    cached_token: Mutex<Option<(String, Instant)>>,
}

impl VertexAuth {
    pub fn new(project: impl Into<String>, location: impl Into<String>) -> Self {
        Self {
            project: project.into(),
            location: location.into(),
            token_source: Arc::new(GcloudTokenSource),
            token_ttl: Duration::from_secs(45 * 60),
            clock: Arc::new(Instant::now),
            cached_token: Mutex::new(None),
        }
    }

    pub fn get_token(&self) -> Result<String, ProviderError> {
        let now = (self.clock)();
        {
            let lock = self.cached_token.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((ref token, fetched_at)) = *lock {
                if let Some(elapsed) = now.checked_duration_since(fetched_at) {
                    if elapsed < self.token_ttl {
                        return Ok(token.clone());
                    }
                }
            }
        }
        let token = self.token_source.token()?;
        let mut lock = self.cached_token.lock().unwrap_or_else(|e| e.into_inner());
        *lock = Some((token.clone(), now));
        Ok(token)
    }

    pub fn invalidate_token(&self) {
        let mut lock = self.cached_token.lock().unwrap_or_else(|e| e.into_inner());
        *lock = None;
    }
}

/// Authentication credentials for Gemini: API key or Vertex AI.
pub enum Auth {
    ApiKey(String),
    Vertex(VertexAuth),
}

/// Append a part to the last content. Used to keep a parallel batch in one content.
fn push_part(contents: &mut [Value], part: Value) {
    if let Some(parts) = contents.last_mut().and_then(|c| c["parts"].as_array_mut()) {
        parts.push(part);
    }
}

pub struct Gemini {
    auth: Auth,
    model: String,
    thinking_level: Option<String>,
}

impl Gemini {
    /// Reads `GEMINI_API_KEY`, falling back to `GOOGLE_API_KEY` as pillars does.
    pub fn from_env() -> Result<Self, ProviderError> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .or_else(|_| std::env::var("GOOGLE_API_KEY"))
            .map_err(|_| ProviderError::MissingKey("GEMINI_API_KEY"))?;
        let (model, thinking_level) =
            split_effort(&std::env::var("BLYTE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()));
        Ok(Self {
            auth: Auth::ApiKey(api_key),
            model,
            thinking_level,
        })
    }

    /// Reads Vertex AI credentials from environment variables:
    /// project from `GOOGLE_CLOUD_PROJECT`, else `CLOUDSDK_CORE_PROJECT`;
    /// location from `GOOGLE_CLOUD_LOCATION`, default `"global"`.
    pub fn vertex_from_env() -> Result<Self, ProviderError> {
        let project = std::env::var("GOOGLE_CLOUD_PROJECT")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("CLOUDSDK_CORE_PROJECT")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .ok_or(ProviderError::MissingKey("GOOGLE_CLOUD_PROJECT"))?;

        let location = std::env::var("GOOGLE_CLOUD_LOCATION")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "global".to_string());

        let env_model = std::env::var("BLYTE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        let clean = env_model.strip_prefix("vertex/").unwrap_or(&env_model);
        let clean = if clean == "vertex" {
            DEFAULT_MODEL
        } else {
            clean
        };
        let (model, thinking_level) = split_effort(clean);

        Ok(Self {
            auth: Auth::Vertex(VertexAuth::new(project, location)),
            model,
            thinking_level,
        })
    }

    /// Explicit API key constructor.
    pub fn new_api_key(api_key: impl Into<String>) -> Self {
        let (model, thinking_level) = split_effort(DEFAULT_MODEL);
        Self {
            auth: Auth::ApiKey(api_key.into()),
            model,
            thinking_level,
        }
    }

    /// Explicit Vertex constructor with default model.
    pub fn new_vertex(project: impl Into<String>, location: impl Into<String>) -> Self {
        let (model, thinking_level) = split_effort(DEFAULT_MODEL);
        Self {
            auth: Auth::Vertex(VertexAuth::new(project, location)),
            model,
            thinking_level,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        let m = model.into();
        let clean = m.strip_prefix("vertex/").unwrap_or(&m);
        let (model, thinking_level) = split_effort(clean);
        self.model = model;
        self.thinking_level = thinking_level;
        self
    }

    pub fn with_token_source(mut self, source: impl TokenSource + 'static) -> Self {
        if let Auth::Vertex(ref mut v) = self.auth {
            v.token_source = Arc::new(source);
        }
        self
    }

    pub fn with_clock(mut self, clock: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        if let Auth::Vertex(ref mut v) = self.auth {
            v.clock = Arc::new(clock);
        }
        self
    }

    pub fn with_token_ttl(mut self, ttl: Duration) -> Self {
        if let Auth::Vertex(ref mut v) = self.auth {
            v.token_ttl = ttl;
        }
        self
    }

    pub fn auth(&self) -> &Auth {
        &self.auth
    }

    pub fn is_vertex(&self) -> bool {
        matches!(self.auth, Auth::Vertex(_))
    }

    pub fn project(&self) -> Option<&str> {
        match &self.auth {
            Auth::Vertex(v) => Some(&v.project),
            Auth::ApiKey(_) => None,
        }
    }

    pub fn location(&self) -> Option<&str> {
        match &self.auth {
            Auth::Vertex(v) => Some(&v.location),
            Auth::ApiKey(_) => None,
        }
    }

    pub fn endpoint_url(&self) -> String {
        match &self.auth {
            Auth::ApiKey(_) => format!("{API_ROOT}/{}:generateContent", self.model),
            Auth::Vertex(v) => vertex_url(&v.project, &v.location, &self.model),
        }
    }

    pub fn get_token(&self) -> Result<String, ProviderError> {
        match &self.auth {
            Auth::Vertex(v) => v.get_token(),
            Auth::ApiKey(k) => Ok(k.clone()),
        }
    }

    pub fn invalidate_token(&self) {
        if let Auth::Vertex(v) = &self.auth {
            v.invalidate_token();
        }
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
        let url = self.endpoint_url();
        let agent = crate::http_client();
        let body = self.body(req);

        let resp = match &self.auth {
            Auth::ApiKey(key) => agent
                .post(&url)
                .set("x-goog-api-key", key)
                .set("content-type", "application/json")
                .send_json(body),
            Auth::Vertex(vertex) => {
                let token = vertex.get_token()?;
                let resp = agent
                    .post(&url)
                    .set("Authorization", &format!("Bearer {token}"))
                    .set("content-type", "application/json")
                    .send_json(body.clone());
                if let Err(ureq::Error::Status(401, _)) = &resp {
                    vertex.invalidate_token();
                    let new_token = vertex.get_token()?;
                    agent
                        .post(&url)
                        .set("Authorization", &format!("Bearer {new_token}"))
                        .set("content-type", "application/json")
                        .send_json(body)
                } else {
                    resp
                }
            }
        };

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
        match self.auth {
            Auth::Vertex(_) => format!("vertex/{}", self.model),
            Auth::ApiKey(_) => format!("gemini/{}", self.model),
        }
    }

    fn effort(&self) -> Option<String> {
        self.thinking_level().map(String::from)
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let api_key = match &self.auth {
            Auth::ApiKey(key) => key,
            Auth::Vertex(_) => return Ok(Vec::new()),
        };
        let url = API_ROOT;
        let agent = crate::http_client();
        let resp = agent.get(url).set("x-goog-api-key", api_key).call();

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn test_vertex_url_cases() {
        assert_eq!(
            vertex_url("p", "global", "gemini-4-argon"),
            "https://aiplatform.googleapis.com/v1/projects/p/locations/global/publishers/google/models/gemini-4-argon:generateContent"
        );
        assert_eq!(
            vertex_url("p", "us-central1", "gemini-4-argon"),
            "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1/publishers/google/models/gemini-4-argon:generateContent"
        );
    }

    #[test]
    fn test_vertex_effort_suffix_url_and_thinking() {
        let g = Gemini::new_vertex("p", "global").with_model("gemini-4-argon-high");
        assert_eq!(
            g.endpoint_url(),
            "https://aiplatform.googleapis.com/v1/projects/p/locations/global/publishers/google/models/gemini-4-argon:generateContent"
        );
        assert_eq!(g.thinking_level(), Some("high"));
        assert_eq!(g.effort(), Some("high".to_string()));
    }

    #[test]
    fn test_vertex_model_id() {
        let g = Gemini::new_vertex("p", "global").with_model("gemini-4-argon");
        assert_eq!(g.model_id(), "vertex/gemini-4-argon");
    }

    #[test]
    fn test_vertex_token_cache_ttl() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();

        let source = move || {
            let n = calls_clone.fetch_add(1, Ordering::SeqCst);
            Ok(format!("tok-{n}"))
        };

        let now = Instant::now();
        let cur = Arc::new(Mutex::new(now));
        let cur_clone = cur.clone();
        let clock = move || *cur_clone.lock().unwrap();

        let g = Gemini::new_vertex("p", "global")
            .with_token_source(source)
            .with_clock(clock);

        assert_eq!(g.get_token().unwrap(), "tok-0");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Within 45 minutes: cached
        *cur.lock().unwrap() = now + Duration::from_secs(30 * 60);
        assert_eq!(g.get_token().unwrap(), "tok-0");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // After 45 minutes: fetches again
        *cur.lock().unwrap() = now + Duration::from_secs(46 * 60);
        assert_eq!(g.get_token().unwrap(), "tok-1");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
