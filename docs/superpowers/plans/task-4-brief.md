# Task 4 Brief: Universal Provider Registry & Discovery Engine

## Objective
Implement universal model routing, provider resolution, and multi-provider credential discovery in `crates/superstate-provider/src/registry.rs`, and wire `crates/superstate-cli/src/task_runner.rs` to use universal provider resolution.

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-multi-provider-model-discovery`

## Detailed Instructions
1. **Implement `crates/superstate-provider/src/registry.rs`**:
   - Expose `pub mod registry;` in `crates/superstate-provider/src/lib.rs`.
   - Re-export `resolve_provider` and `discover_all_models` from `crates/superstate-provider/src/lib.rs` for convenience.
   - Define:
     ```rust
     #[derive(Debug, Clone, Copy, PartialEq, Eq)]
     pub enum ProviderKind {
         Gemini,
         Anthropic,
         Ollama,
     }
     ```
   - Implement `pub fn detect_provider_kind(model: Option<&str>) -> ProviderKind`:
     - If `model` starts with `"claude"` or starts with `"anthropic/"`: `ProviderKind::Anthropic`
     - If `model` starts with `"ollama/"` or `model == Some("ollama")` or contains `":"` (Ollama tag convention like `qwen2.5-coder:32b`, `llama3.3:70b`, `gemma4:31b`): `ProviderKind::Ollama`
     - If `model` starts with `"gemini"` or starts with `"gemini/"`: `ProviderKind::Gemini`
     - If `model` is `None` (or unrecognized name without prefix):
       - If `std::env::var("GEMINI_API_KEY").is_ok() || std::env::var("GOOGLE_API_KEY").is_ok()`: `ProviderKind::Gemini`
       - Else if `std::env::var("ANTHROPIC_API_KEY").is_ok()`: `ProviderKind::Anthropic`
       - Else if `std::env::var("OLLAMA_API_KEY").is_ok() || std::env::var("OLLAMA_HOST").is_ok()`: `ProviderKind::Ollama`
       - Else fallback to `ProviderKind::Gemini`
   - Implement `pub fn resolve_provider(model: Option<&str>) -> Result<Box<dyn Provider>, ProviderError>`:
     - Strips any provider prefix (e.g. `"anthropic/claude-3-7-sonnet"` -> `"claude-3-7-sonnet"`, `"gemini/gemini-3.8-flash"` -> `"gemini-3.8-flash"`, `"ollama/qwen2.5-coder:32b"` -> `"qwen2.5-coder:32b"`).
     - Resolves the provider:
       - `ProviderKind::Gemini`:
         let mut p = crate::gemini::Gemini::from_env()?;
         if let Some(m) = clean_model { p = p.with_model(m); }
         Ok(Box::new(p))
       - `ProviderKind::Anthropic`:
         let mut p = crate::anthropic::Anthropic::from_env()?;
         if let Some(m) = clean_model { p = p.with_model(m); }
         Ok(Box::new(p))
       - `ProviderKind::Ollama`:
         let mut p = crate::ollama::Ollama::from_env()?;
         if let Some(m) = clean_model { p = p.with_model(m); }
         Ok(Box::new(p))
   - Implement `pub fn discover_all_models() -> Vec<(String, Result<Vec<ModelInfo>, ProviderError>)>`:
     - Checks Gemini:
       match crate::gemini::Gemini::from_env() {
           Ok(p) => ("Google Gemini".to_string(), p.list_models()),
           Err(e) => ("Google Gemini".to_string(), Err(e)),
       }
     - Checks Anthropic:
       match crate::anthropic::Anthropic::from_env() {
           Ok(p) => ("Anthropic Claude".to_string(), p.list_models()),
           Err(e) => ("Anthropic Claude".to_string(), Err(e)),
       }
     - Checks Ollama Cloud:
       match crate::ollama::Ollama::from_env() {
           Ok(p) => ("Ollama Cloud".to_string(), p.list_models()),
           Err(e) => ("Ollama Cloud".to_string(), Err(e)),
       }
2. **Update `crates/superstate-cli/src/task_runner.rs`**:
   - In `run_task_with_state_and_writer`:
     Replace hardcoded `Gemini::from_env()` with:
     ```rust
     let provider = match superstate_provider::resolve_provider(config.model.as_deref()) {
         Ok(p) => p,
         Err(superstate_provider::ProviderError::MissingKey(key)) => {
             return Err(format!(
                 "Provider error: no API key: set {key} in environment"
             ));
         }
         Err(e) => return Err(format!("Provider error: {e}")),
     };
     ```
   - Update `Runner` call to pass `provider: provider.as_ref()`.
3. **Update `crates/superstate-cli/tests/test_task_runner.rs`**:
   - Update tests where default model is asserted (e.g. `test_task_config_defaults`): change `"gemini-2.5-flash"` to `"gemini-3.8-flash"`.
4. **Author Unit Tests in `crates/superstate-provider/tests/test_registry.rs`**:
   - Test `detect_provider_kind`:
     - `"claude-3-7-sonnet"`, `"anthropic/claude-3-5-haiku"` -> Anthropic
     - `"gemini-3.8-flash"`, `"gemini/gemini-2.5-pro"` -> Gemini
     - `"qwen2.5-coder:32b"`, `"ollama/llama3.3:70b"`, `"llama3.1:8b"` -> Ollama
   - Test `resolve_provider` error handling when API keys are absent (returns corresponding `MissingKey`).
5. **Verification**:
   - Run `cargo test -p superstate-provider --test test_registry`.
   - Run `cargo test -p superstate --test test_task_runner`.
   - Run `cargo check --workspace --all-targets`.
   - Run `cargo clippy --workspace --all-targets -- -D warnings`.
6. **Commit**:
   - `git add crates/superstate-provider/src/lib.rs crates/superstate-provider/src/registry.rs crates/superstate-provider/tests/test_registry.rs crates/superstate-cli/src/task_runner.rs crates/superstate-cli/tests/test_task_runner.rs`
   - `git commit --no-gpg-sign -m "feat(provider): add universal provider resolver and multi-provider discovery engine"`
7. **Roborev**:
   - Run `roborev status; roborev show HEAD`.
8. **Report**:
   - Write report to `docs/superpowers/plans/task-4-report.md`.
