//! Model providers, normalised behind one trait.
//!
//! # Why a trait from the first commit
//!
//! Gemini came first, because that is where the credits were and it is the model the EXP-081
//! baseline ran on, which kept the comparison honest; Anthropic, OpenAI-compatible, Ollama and
//! ChatGPT followed. The wire formats
//! differ in shape, not just in spelling -- Anthropic sends `tool_use` content blocks and
//! expects `tool_result`; Gemini sends `functionCall` parts and expects `functionResponse` --
//! so anything above this layer that touches a provider's own vocabulary has to be rewritten
//! per provider. Normalising once, here, is what makes swapping models a config change.
//!
//! # Why `ureq` and no async runtime
//!
//! This harness executes tools **synchronously and one at a time**. That is a deliberate design
//! decision, not an omission: 47.6% of all input tokens in EXP-081 were spent on turns whose
//! entire output was "waiting for the background task to finish", because the previous harness
//! backgrounded slow commands and made the agent poll for them -- each poll costing a full turn
//! carrying the whole conversation (FINDINGS 13.29). A blocking client makes that failure mode
//! unrepresentable.

pub mod anthropic;
pub mod chatgpt;
pub mod gemini;
pub mod ollama;
pub mod openai;
pub mod registry;

pub use registry::{detect_provider_kind, discover_all_models, resolve_provider, ProviderKind};

/// Computes whole-call and read timeouts from an optional string value or defaults (180 s, 150 s).
pub fn model_timeouts_from(val: Option<&str>) -> (std::time::Duration, std::time::Duration) {
    let whole = val
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(180);
    let read = whole.saturating_sub(30).max(30);
    (
        std::time::Duration::from_secs(whole),
        std::time::Duration::from_secs(read),
    )
}

/// Computes the streaming idle timeout in seconds from an optional string value or default (300 s): Gemini can think silently for minutes before a long reply.
/// Invalid or zero values fall back to the default.
pub fn model_idle_timeout_secs_from(val: Option<&str>) -> u64 {
    val.and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(300)
}

/// Streaming idle timeout in seconds, derived from `BLYTE_MODEL_IDLE_TIMEOUT_SECS` or default 300 s, read once.
pub fn model_idle_timeout_secs() -> u64 {
    static IDLE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *IDLE.get_or_init(|| {
        model_idle_timeout_secs_from(
            std::env::var("BLYTE_MODEL_IDLE_TIMEOUT_SECS")
                .ok()
                .as_deref(),
        )
    })
}

/// Computes the streaming whole-call cap in seconds from an optional string value or default (1800 s).
/// Invalid or zero values fall back to the default.
pub fn model_stream_cap_secs_from(val: Option<&str>) -> u64 {
    val.and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(1800)
}

/// Streaming whole-call cap in seconds, derived from `BLYTE_MODEL_TIMEOUT_SECS` or default 1800 s, read once.
pub fn model_stream_cap_secs() -> u64 {
    static CAP: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *CAP.get_or_init(|| {
        model_stream_cap_secs_from(std::env::var("BLYTE_MODEL_TIMEOUT_SECS").ok().as_deref())
    })
}

/// Timeouts (whole-call, read) derived from `BLYTE_MODEL_TIMEOUT_SECS` or default (180 s, 150 s), read once.
pub fn model_timeouts() -> (std::time::Duration, std::time::Duration) {
    static TIMEOUTS: std::sync::OnceLock<(std::time::Duration, std::time::Duration)> =
        std::sync::OnceLock::new();
    *TIMEOUTS.get_or_init(|| {
        model_timeouts_from(std::env::var("BLYTE_MODEL_TIMEOUT_SECS").ok().as_deref())
    })
}

/// Whole-call timeout in seconds, derived from `BLYTE_MODEL_TIMEOUT_SECS` or default 180 s.
pub fn model_timeout_secs() -> u64 {
    model_timeouts().0.as_secs()
}

/// Builds an HTTP client with the specified whole-call and read timeouts.
pub fn build_http_client(
    timeout: std::time::Duration,
    timeout_read: std::time::Duration,
) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(20))
        .timeout_read(timeout_read)
        .timeout(timeout)
        .build()
}

/// Builds an HTTP client for streaming calls with a 20 s connect timeout and `timeout_read(idle)`.
///
/// No whole-call `.timeout(..)` is set: in ureq 2 a whole-call timeout replaces the read timeout.
pub fn build_streaming_http_client(idle: std::time::Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(20))
        .timeout_read(idle)
        .build()
}

/// The process's one HTTP client. `ureq::Agent` pools connections, so every model call after
/// the first reuses one instead of paying a TCP and TLS handshake (15-45 ms) each time.
pub fn http_client() -> &'static ureq::Agent {
    static CLIENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        let (whole, read) = model_timeouts();
        build_http_client(whole, read)
    })
}

/// The process's shared HTTP client for streaming calls, with a connect timeout and idle read timeout.
pub fn streaming_http_client() -> &'static ureq::Agent {
    static CLIENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        build_streaming_http_client(std::time::Duration::from_secs(model_idle_timeout_secs()))
    })
}

use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("no API key: set {0}")]
    MissingKey(&'static str),
    #[error("authentication: {0}")]
    Auth(String),
    #[error("transport: {0}")]
    Transport(String),
    #[error("provider returned {status}: {body}")]
    Api { status: u16, body: String },
    #[error("could not parse provider response: {0}")]
    Parse(String),
    #[error("{}", format_no_answer(.block_reason.as_deref(), .block_reason_message.as_deref()))]
    NoAnswer {
        block_reason: Option<String>,
        block_reason_message: Option<String>,
        usage: Usage,
    },
}

fn format_no_answer(reason: Option<&str>, message: Option<&str>) -> String {
    match (reason, message) {
        (Some(r), Some(m)) => format!("Gemini returned no answer: prompt blocked ({r}): {m}"),
        (Some(r), None) => format!("Gemini returned no answer: prompt blocked ({r})"),
        (None, Some(m)) => format!("Gemini returned no answer: {m}"),
        (None, None) => "Gemini returned no answer".to_string(),
    }
}

impl ProviderError {
    /// Whether this error represents a transport timeout.
    pub fn is_timeout(&self) -> bool {
        match self {
            ProviderError::Transport(s) => {
                let lower = s.to_ascii_lowercase();
                lower.contains("timed out") || lower.contains("timeout")
            }
            _ => false,
        }
    }
}

/// A tool the model may call. Provider-neutral; each adapter renders it into its own schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's parameters.
    pub parameters: serde_json::Value,
}

/// One model turn's request. `system` is held separately because every provider treats it
/// differently, and because it is the part that stays constant and therefore caches.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Message {
    /// Text from the user or runtime.
    User(String),
    /// The model's own prior text.
    Assistant(String),
    /// The model asked to call a tool.
    ///
    /// `signature` is an opaque provider token that must be handed back verbatim when the call
    /// is replayed in history. Gemini 3.x rejects a `functionCall` whose `thoughtSignature` is
    /// missing -- the model's reasoning is cryptographically bound to the call, and a turn that
    /// drops it fails with INVALID_ARGUMENT rather than degrading quietly. Neutral here on
    /// purpose: the loop carries it without needing to know what it means, and providers that
    /// have no such concept leave it None.
    ToolCall {
        id: String,
        name: String,
        arguments: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// What the tool returned. `is_error` lets a provider mark failure where it supports it.
    ToolResult {
        id: String,
        name: String,
        content: String,
        is_error: bool,
    },
}

/// What a model turn produced. A turn yields text, tool calls, or both.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Response {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    /// The provider's own stop reason, verbatim, for diagnosis.
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    /// See [`Message::ToolCall`]. Opaque; round-tripped verbatim.
    pub signature: Option<String>,
}

/// Token accounting, normalised.
///
/// `cached` is reported separately rather than folded into `input` because the distinction is
/// the entire economics of a long run: EXP-081 recorded a single turn at 1,384,188 input tokens
/// against 15,327,627 cached. A harness that cannot see both numbers cannot tell an expensive
/// turn from a well-cached one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub thinking: u64,
}

/// List prices in USD per million tokens: fresh input, cached input, and output. Thinking is
/// billed as output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pricing {
    pub input: f64,
    pub cached: f64,
    pub output: f64,
}

impl Pricing {
    /// What `u` costs. `Usage.input` is fresh input only; cached input is counted separately.
    pub fn cost(&self, u: &Usage) -> f64 {
        (u.input as f64 * self.input
            + u.cached as f64 * self.cached
            + (u.output + u.thinking) as f64 * self.output)
            / 1e6
    }
}

fn utc_date_from_epoch_secs(secs: u64) -> (u32, u32, u32) {
    let days = secs / 86_400;
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as u32, m as u32, d as u32)
}

fn today_utc() -> (u32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    utc_date_from_epoch_secs(secs)
}

pub fn pricing_on(model_id: &str, date: (u32, u32, u32)) -> Option<Pricing> {
    // Gemini 3.8 Flash, list price through 2026-12-31.
    if model_id.contains("gemini-3.8-flash") {
        return Some(Pricing {
            input: 0.75,
            cached: 0.075,
            output: 3.75,
        });
    }
    // Gemini 4 Argon, introductory price announced 2026-09-30 (cached input 95% off)
    // through 2026-11-30; post-introductory price from 2026-12-01.
    if model_id.contains("gemini-4-argon") {
        if date <= (2026, 11, 30) {
            return Some(Pricing {
                input: 2.00,
                cached: 0.10,
                output: 10.00,
            });
        } else {
            // The cached rate from 2026-12-01 is unannounced; 10% of input ($0.40) is an assumption.
            return Some(Pricing {
                input: 4.00,
                cached: 0.40,
                output: 20.00,
            });
        }
    }
    None
}

pub fn pricing(model_id: &str) -> Option<Pricing> {
    pricing_on(model_id, today_utc())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pricing_gemini_4_argon() {
        let p = pricing("vertex/gemini-4-argon").expect("known price");
        assert_eq!(p.input, 2.00);
        assert_eq!(p.cached, 0.10);
        assert_eq!(p.output, 10.00);

        let p2 = pricing("gemini-4-argon").expect("known price");
        assert_eq!(p2.input, 2.00);
        assert_eq!(p2.cached, 0.10);
        assert_eq!(p2.output, 10.00);
    }

    #[test]
    fn test_pricing_on_gemini_4_argon_cutoff() {
        let intro = Pricing {
            input: 2.00,
            cached: 0.10,
            output: 10.00,
        };
        let post_intro = Pricing {
            input: 4.00,
            cached: 0.40,
            output: 20.00,
        };

        for model in ["gemini-4-argon", "vertex/gemini-4-argon"] {
            assert_eq!(pricing_on(model, (2026, 10, 1)), Some(intro));
            assert_eq!(pricing_on(model, (2026, 11, 30)), Some(intro));
            assert_eq!(pricing_on(model, (2026, 12, 1)), Some(post_intro));
            assert_eq!(pricing_on(model, (2027, 1, 1)), Some(post_intro));
        }
    }

    #[test]
    fn test_pricing_on_other_models_unchanged() {
        let flash = Pricing {
            input: 0.75,
            cached: 0.075,
            output: 3.75,
        };
        assert_eq!(pricing_on("gemini-3.8-flash", (2026, 11, 30)), Some(flash));
        assert_eq!(pricing_on("gemini-3.8-flash", (2026, 12, 1)), Some(flash));
        assert_eq!(pricing_on("unknown-model", (2026, 11, 30)), None);
        assert_eq!(pricing_on("unknown-model", (2026, 12, 1)), None);
    }

    #[test]
    fn test_utc_date_from_epoch_secs() {
        assert_eq!(utc_date_from_epoch_secs(0), (1970, 1, 1));
        assert_eq!(utc_date_from_epoch_secs(1_796_083_199), (2026, 11, 30));
        assert_eq!(utc_date_from_epoch_secs(1_796_083_200), (2026, 12, 1));
    }
}

/// Metadata describing a model supported by a provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelInfo {
    /// Identifier passed to the provider or config (e.g. `gemini-3.8-flash`).
    pub id: String,
    /// Human-friendly display name (e.g. `Gemini 3.8 Flash`).
    pub display_name: String,
    /// Optional description of the model's capabilities or purpose.
    pub description: Option<String>,
    /// Provider identifier (e.g. `gemini`).
    pub provider: String,
}

pub trait Provider {
    /// One blocking model call.
    fn complete(&self, req: &Request) -> Result<Response, ProviderError>;
    /// Identifier recorded in telemetry, e.g. `gemini/gemini-3.8-flash`.
    fn model_id(&self) -> String;
    /// Effort level if configured for this provider, e.g. "high".
    fn effort(&self) -> Option<String> {
        None
    }
    /// Lists available models supported by this provider for the configured credentials.
    fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(Vec::new())
    }
}
