# Task 1 Brief: Provider Trait Extension & Gemini Discovery

## Objective
Extend `Provider` trait in `crates/superstate-provider/src/lib.rs` with `ModelInfo` and `list_models(&self)`, implement `Gemini::list_models` with Google GenAI `models.list` parsing and filtering, and update `DEFAULT_MODEL` to `gemini-3.8-flash`.

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-multi-provider-model-discovery`

## Detailed Instructions
1. **Extend `crates/superstate-provider/src/lib.rs`**:
   - Add `ModelInfo`:
     ```rust
     #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
     pub struct ModelInfo {
         pub id: String,
         pub display_name: String,
         pub description: Option<String>,
         pub provider: String,
     }
     ```
   - In `pub trait Provider`:
     ```rust
     /// Lists available models supported by this provider for the configured credentials.
     fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
         Ok(Vec::new())
     }
     ```
2. **Implement in `crates/superstate-provider/src/gemini.rs`**:
   - Change `DEFAULT_MODEL`:
     ```rust
     const DEFAULT_MODEL: &str = "gemini-3.8-flash";
     ```
   - Implement `Gemini::parse_models_list(v: &Value) -> Result<Vec<ModelInfo>, ProviderError>`:
     - Expects array at `v.get("models")`.
     - For each model in the array:
       - Checks if `supportedGenerationMethods` array contains `"generateContent"`. If not, skip.
       - Extracts `name` (e.g. `"models/gemini-3.8-flash"`), strips `"models/"` prefix to yield `id`.
       - Extracts `displayName` as `display_name` (fallback to `id.clone()`).
       - Extracts `description` as `Option<String>`.
       - Sets `provider: "gemini".to_string()`.
   - Implement `Provider::list_models` for `Gemini`:
     ```rust
     let url = format!("{API_ROOT}");
     let agent = ureq::AgentBuilder::new()
         .timeout_connect(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS))
         .timeout_read(std::time::Duration::from_secs(READ_TIMEOUT_SECS))
         .build();
     let resp = agent
         .get(&url)
         .set("x-goog-api-key", &self.api_key)
         .call();
     match resp {
         Ok(r) => {
             let v: Value = r.into_json().map_err(|e| ProviderError::Parse(e.to_string()))?;
             Self::parse_models_list(&v)
         }
         Err(ureq::Error::Status(status, r)) => Err(ProviderError::Api {
             status,
             body: r.into_string().unwrap_or_default(),
         }),
         Err(e) => Err(ProviderError::Transport(e.to_string())),
     }
     ```
     *(Note: `API_ROOT` in `gemini.rs` is `"https://generativelanguage.googleapis.com/v1beta/models"`. Verify the exact GET URL so it queries `https://generativelanguage.googleapis.com/v1beta/models`)*.
3. **Write Unit Tests in `crates/superstate-provider/tests/test_gemini_models.rs`**:
   - Write tests for `Gemini::parse_models_list`:
     - Test that models with `"generateContent"` are kept and `"models/"` prefix is stripped.
     - Test that models with only `["embedContent"]` are filtered out.
     - Test handling of empty or missing `"models"` field.
4. **Verification**:
   - Run `cargo test -p superstate-provider --test test_gemini_models`.
   - Run `cargo check --workspace --all-targets`.
5. **Commit**:
   - `git add crates/superstate-provider/src/lib.rs crates/superstate-provider/src/gemini.rs crates/superstate-provider/tests/test_gemini_models.rs`
   - `git commit --no-gpg-sign -m "feat(provider): add ModelInfo and Gemini list_models discovery with gemini-3.8-flash default"`
6. **Roborev**:
   - Run `roborev status; roborev show HEAD`.
7. **Report**:
   - Write report to `docs/superpowers/plans/task-1-report.md`.
