//! Tests for Gemini model discovery and Provider trait model listing.

use blyte_provider::gemini::Gemini;
use blyte_provider::{ModelInfo, Provider, ProviderError, Request, Response};
use serde_json::json;

#[test]
fn parse_models_list_filters_generate_content_and_strips_prefix() {
    let v = json!({
        "models": [
            {
                "name": "models/gemini-3.8-flash",
                "displayName": "Gemini 3.8 Flash",
                "description": "Next generation multimodal model",
                "supportedGenerationMethods": ["generateContent", "countTokens"]
            },
            {
                "name": "models/text-embedding-004",
                "displayName": "Text Embedding 004",
                "description": "Embedding model",
                "supportedGenerationMethods": ["embedContent"]
            }
        ]
    });
    let models = Gemini::parse_models_list(&v).expect("parsing should succeed");
    assert_eq!(models.len(), 1);
    assert_eq!(
        models[0],
        ModelInfo {
            id: "gemini-3.8-flash".into(),
            display_name: "Gemini 3.8 Flash".into(),
            description: Some("Next generation multimodal model".into()),
            provider: "gemini".into(),
        }
    );
}

#[test]
fn parse_models_list_handles_missing_display_name_or_description() {
    let v = json!({
        "models": [
            {
                "name": "models/gemini-pro-custom",
                "supportedGenerationMethods": ["generateContent"]
            }
        ]
    });
    let models = Gemini::parse_models_list(&v).expect("parsing should succeed");
    assert_eq!(models.len(), 1);
    assert_eq!(
        models[0],
        ModelInfo {
            id: "gemini-pro-custom".into(),
            display_name: "gemini-pro-custom".into(),
            description: None,
            provider: "gemini".into(),
        }
    );
}

#[test]
fn parse_models_list_handles_empty_or_missing_models() {
    let empty = json!({"models": []});
    assert_eq!(Gemini::parse_models_list(&empty).unwrap(), Vec::new());

    let missing = json!({});
    assert_eq!(Gemini::parse_models_list(&missing).unwrap(), Vec::new());
}

#[test]
fn default_model_is_gemini_3_8_flash() {
    std::env::remove_var("BLYTE_MODEL");
    std::env::set_var("GEMINI_API_KEY", "test-key");
    let g = Gemini::from_env().expect("Gemini::from_env should succeed");
    assert_eq!(g.model_id(), "gemini/gemini-3.8-flash");
}

#[test]
fn default_provider_trait_list_models_returns_empty_vec() {
    struct DummyProvider;
    impl Provider for DummyProvider {
        fn complete(&self, _req: &Request) -> Result<Response, ProviderError> {
            unimplemented!()
        }
        fn model_id(&self) -> String {
            "dummy".into()
        }
    }
    let dummy = DummyProvider;
    assert_eq!(dummy.list_models().unwrap(), Vec::new());
}
