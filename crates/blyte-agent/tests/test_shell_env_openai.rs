//! The new provider credentials never reach the model's shell.

use blyte_agent::shell::HARNESS_ENV;

#[test]
fn openai_and_openrouter_credentials_are_scrubbed() {
    for name in ["OPENAI_API_KEY", "OPENAI_BASE_URL", "OPENROUTER_API_KEY"] {
        assert!(HARNESS_ENV.contains(&name), "{name} must be in HARNESS_ENV");
    }
}
