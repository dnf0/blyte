//! Universal model routing, provider resolution, and multi-provider credential discovery.
//!
//! Provides a single entrypoint for mapping user-provided model identifiers or ambient
//! environment credentials to concrete [`Provider`] implementations, insulating callers
//! from provider-specific wire schemas and credential conventions.

use crate::{ModelInfo, Provider, ProviderError};

/// Identifies the underlying AI backend provider family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// Google Gemini models (e.g., `gemini-3.8-flash`, `gemini-2.5-pro`).
    Gemini,
    /// Anthropic Claude models (e.g., `claude-3-7-sonnet`, `claude-3-5-haiku`).
    Anthropic,
    /// Ollama local or Cloud hosted models (e.g., `qwen2.5-coder:32b`, `llama3.3:70b`).
    Ollama,
    /// OpenAI models (e.g., `gpt-5.5`).
    OpenAi,
    /// OpenRouter routed models (e.g., `meta/muse-spark-1.3`).
    OpenRouter,
    /// ChatGPT subscription models via Codex CLI login (e.g., `chatgpt/gpt-5.5`).
    ChatGpt,
}

/// Detects the target [`ProviderKind`] based on model naming conventions or environment credentials.
///
/// Discovers provider kind through explicit namespace prefixes (`anthropic/`, `gemini/`, `ollama/`),
/// well-known model prefix tags (`claude`, `gemini`), or tag syntax (`:`). If no model is supplied
/// or the identifier is unrecognized, falls back to inspecting ambient environment variables.
///
/// # Parameters
/// * `model` - Optional model identifier string or prefixed specification.
///
/// # Returns
/// The resolved [`ProviderKind`].
pub fn detect_provider_kind(model: Option<&str>) -> ProviderKind {
    // 1. Inspect explicit prefix or well-known model family patterns
    if let Some(m) = model {
        if m.starts_with("vertex/") || m == "vertex" {
            return ProviderKind::Gemini;
        }
        if m.starts_with("chatgpt/") || m == "chatgpt" {
            return ProviderKind::ChatGpt;
        }
        if m.starts_with("openai/") || m == "openai" {
            return ProviderKind::OpenAi;
        }
        if m.starts_with("openrouter/") || m == "openrouter" {
            return ProviderKind::OpenRouter;
        }
        if m.starts_with("claude") || m.starts_with("anthropic/") || m == "anthropic" {
            return ProviderKind::Anthropic;
        }
        if m.starts_with("ollama/") || m == "ollama" || m.contains(':') {
            return ProviderKind::Ollama;
        }
        if m.starts_with("gemini") || m.starts_with("gemini/") {
            return ProviderKind::Gemini;
        }
    }

    // 2. Fall back to ambient environment credentials when model is unspecified or ambiguous
    if std::env::var("GEMINI_API_KEY").is_ok() || std::env::var("GOOGLE_API_KEY").is_ok() {
        ProviderKind::Gemini
    } else if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        ProviderKind::Anthropic
    } else if std::env::var("OLLAMA_API_KEY").is_ok() || std::env::var("OLLAMA_HOST").is_ok() {
        ProviderKind::Ollama
    } else if std::env::var("OPENAI_API_KEY").is_ok() || std::env::var("OPENAI_BASE_URL").is_ok() {
        ProviderKind::OpenAi
    } else if std::env::var("OPENROUTER_API_KEY").is_ok() {
        ProviderKind::OpenRouter
    } else {
        // Default baseline fallback is Gemini
        ProviderKind::Gemini
    }
}

/// Resolves a model identifier into a heap-allocated [`Provider`] instance configured with credentials.
///
/// Strips provider namespace prefixes (`anthropic/`, `gemini/`, `ollama/`) from the model identifier
/// before configuring the underlying adapter. If credentials for the detected provider are missing
/// in the environment, returns a descriptive [`ProviderError::MissingKey`].
///
/// # Parameters
/// * `model` - Optional model name or namespaced identifier.
///
/// # Returns
/// Returns a boxed dynamic [`Provider`] ready for completion requests, or a [`ProviderError`].
pub fn resolve_provider(model: Option<&str>) -> Result<Box<dyn Provider>, ProviderError> {
    // Determine provider kind first so routing logic is unified
    let kind = detect_provider_kind(model);

    let is_vertex = match model {
        Some(m) => m.starts_with("vertex/") || m == "vertex",
        None => std::env::var("BLYTE_MODEL")
            .map(|m| m.starts_with("vertex/") || m == "vertex")
            .unwrap_or(false),
    };

    // Strip provider prefix from model string if present, treating bare provider names as default
    let clean_model = match model {
        Some("anthropic") | Some("ollama") | Some("gemini") | Some("openai")
        | Some("openrouter") | Some("chatgpt") | Some("vertex") => None,
        Some(m) => {
            let stripped = if let Some(s) = m.strip_prefix("vertex/") {
                s
            } else if let Some(s) = m.strip_prefix("chatgpt/") {
                s
            } else if let Some(s) = m.strip_prefix("openai/") {
                s
            } else if let Some(s) = m.strip_prefix("openrouter/") {
                s
            } else if let Some(s) = m.strip_prefix("anthropic/") {
                s
            } else if let Some(s) = m.strip_prefix("gemini/") {
                s
            } else if let Some(s) = m.strip_prefix("ollama/") {
                s
            } else {
                m
            };
            if stripped.is_empty() {
                None
            } else {
                Some(stripped)
            }
        }
        None => None,
    };

    // Instantiate provider adapter and apply stripped model identifier override if provided
    match kind {
        ProviderKind::Gemini => {
            let mut p = if is_vertex {
                crate::gemini::Gemini::vertex_from_env()?
            } else {
                crate::gemini::Gemini::from_env()?
            };
            if let Some(m) = clean_model {
                p = p.with_model(m);
            }
            Ok(Box::new(p))
        }
        ProviderKind::Anthropic => {
            let mut p = crate::anthropic::Anthropic::from_env()?;
            if let Some(m) = clean_model {
                p = p.with_model(m);
            }
            Ok(Box::new(p))
        }
        ProviderKind::Ollama => {
            let mut p = crate::ollama::Ollama::from_env()?;
            if let Some(m) = clean_model {
                p = p.with_model(m);
            }
            Ok(Box::new(p))
        }
        ProviderKind::OpenAi => {
            let mut p = crate::openai::OpenAi::openai_from_env()?;
            if let Some(m) = clean_model {
                p = p.with_model(m);
            }
            Ok(Box::new(p))
        }
        ProviderKind::OpenRouter => {
            let mut p = crate::openai::OpenAi::openrouter_from_env()?;
            if let Some(m) = clean_model {
                p = p.with_model(m);
            }
            Ok(Box::new(p))
        }
        ProviderKind::ChatGpt => {
            let mut p = crate::chatgpt::ChatGpt::from_env()?;
            if let Some(m) = clean_model {
                p = p.with_model(m);
            }
            Ok(Box::new(p))
        }
    }
}

/// Probes all supported providers in the current environment and lists available models.
///
/// Each provider is queried using its environment credentials. Providers with missing keys
/// or network errors report their individual [`ProviderError`] without halting discovery
/// of remaining providers.
///
/// # Returns
/// A vector of tuples containing human-friendly provider labels and either the list of
/// discovered [`ModelInfo`] records or the error encountered while querying.
pub fn discover_all_models() -> Vec<(String, Result<Vec<ModelInfo>, ProviderError>)> {
    // Query each provider adapter sequentially, collecting successful models or failure errors
    vec![
        match crate::gemini::Gemini::from_env() {
            Ok(p) => ("Google Gemini".to_string(), p.list_models()),
            Err(e) => ("Google Gemini".to_string(), Err(e)),
        },
        match crate::anthropic::Anthropic::from_env() {
            Ok(p) => ("Anthropic Claude".to_string(), p.list_models()),
            Err(e) => ("Anthropic Claude".to_string(), Err(e)),
        },
        match crate::ollama::Ollama::from_env() {
            Ok(p) => ("Ollama Cloud".to_string(), p.list_models()),
            Err(e) => ("Ollama Cloud".to_string(), Err(e)),
        },
        match crate::openai::OpenAi::openai_from_env() {
            Ok(p) => ("OpenAI".to_string(), p.list_models()),
            Err(e) => ("OpenAI".to_string(), Err(e)),
        },
        match crate::openai::OpenAi::openrouter_from_env() {
            Ok(p) => ("OpenRouter".to_string(), p.list_models()),
            Err(e) => ("OpenRouter".to_string(), Err(e)),
        },
        match crate::chatgpt::ChatGpt::from_env() {
            Ok(p) => ("ChatGPT".to_string(), p.list_models()),
            Err(e) => ("ChatGPT".to_string(), Err(e)),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_provider_kind_vertex() {
        assert_eq!(
            detect_provider_kind(Some("vertex/gemini-4-argon")),
            ProviderKind::Gemini
        );
        assert_eq!(detect_provider_kind(Some("vertex")), ProviderKind::Gemini);
    }
}
