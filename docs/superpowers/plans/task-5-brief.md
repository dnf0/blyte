# Task 5 Brief: REPL /model Selector, Multi-Provider Doctor Audit, & Documentation

## Objective
Implement the interactive `/model` slash command in `crates/superstate-cli/src/repl.rs`, update `ReplSession` default model to `gemini-3.8-flash`, add multi-provider cloud credential inspection to `superstate doctor`, author integration tests in `crates/superstate-cli/tests/test_repl_model_cmd.rs`, and update `README.md`.

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-multi-provider-model-discovery`

## Detailed Instructions
1. **Update `crates/superstate-cli/src/repl.rs`**:
   - In `ReplSession`:
     Add field: `pub cached_models: Vec<superstate_provider::ModelInfo>`.
     In `ReplSession::new`:
     Change fallback model from `"gemini-2.5-flash"` to `"gemini-3.8-flash"`.
     Initialize `cached_models: Vec::new()`.
   - In `handle_repl_command`:
     - Add `/model` command handler:
       - If `args.is_empty()`:
         Run `superstate_provider::discover_all_models()`.
         Clear and populate `session.cached_models`.
         Format output:
         ```text
         Discovering available cloud models...

         [Google Gemini] (via GEMINI_API_KEY)
           [1] gemini-3.8-flash (active) - Next-generation multimodal fast model
           [2] gemini-2.5-flash          - Agentic reasoning model
           [3] gemini-2.5-pro            - Deep reasoning model

         [Anthropic Claude] (via ANTHROPIC_API_KEY)
           [4] claude-3-7-sonnet-latest  - Frontier hybrid reasoning & coding
           ...

         [Ollama Cloud] (via OLLAMA_API_KEY at https://ollama.com/api)
           [5] qwen2.5-coder:32b         - Cloud-hosted coding model
           ...

         Active model: gemini-3.8-flash
         To switch: /model <number> or /model <name> (e.g. /model 1 or /model claude-3-7-sonnet-latest)
         ```
         If a provider returned `Err(ProviderError::MissingKey(key))`:
         Print `[{provider_name}] Not configured (set {key} to discover models)`.
         If other error: print `[{provider_name}] Discovery error: {err}`.
       - If `args` is provided:
         - If `args` parses as a positive integer `idx`:
           - If `session.cached_models.is_empty()`:
             Run `discover_all_models()` and populate `session.cached_models`.
           - If `idx >= 1 && idx <= session.cached_models.len()`:
             let chosen = &session.cached_models[idx - 1];
             session.model = chosen.id.clone();
             writeln!(writer, "Switched active model to: {} ({})", chosen.id, chosen.provider)?;
           - Else:
             writeln!(writer, "Invalid model index: {}. Use /model to view available models.", idx)?;
         - Else (string name, e.g. `/model claude-3-7-sonnet-latest`):
           let kind = superstate_provider::registry::detect_provider_kind(Some(args));
           session.model = args.to_string();
           writeln!(writer, "Switched active model to: {} ({:?})", args, kind)?;
     - Update `/help`:
       Add `  /model [name|idx] - Discover and switch active AI model`.
2. **Update `crates/superstate-cli/src/commands/doctor.rs`**:
   - Add Step 6 to the handshake:
     ```rust
     // Step 6: Multi-Provider Cloud Credential Probe
     println!("[✓] Step 6: Multi-Provider Cloud Models Verified:");
     for (provider, res) in superstate_provider::discover_all_models() {
         match res {
             Ok(models) => println!("      - {}: Verified ({} models discovered)", provider, models.len()),
             Err(superstate_provider::ProviderError::MissingKey(k)) => {
                 println!("      - {}: Not configured (set {})", provider, k);
             }
             Err(e) => println!("      - {}: Probe error ({})", provider, e),
         }
     }
     ```
   - Update handshake header and output messages to reflect 6-Step Verification.
3. **Integration Tests in `crates/superstate-cli/tests/test_repl_model_cmd.rs`**:
   - Test `/model` with empty cache and mock/controlled environment:
     - Verify it lists models and sets `session.cached_models`.
     - Test switching by numeric index `/model 1`: updates `session.model`.
     - Test switching by name `/model claude-3-5-sonnet-latest`: updates `session.model`.
     - Test invalid index reporting: `/model 999`.
     - Test `/help` includes `/model`.
4. **Update `README.md`**:
   - Document cloud provider API keys (`GEMINI_API_KEY` / `GOOGLE_API_KEY`, `ANTHROPIC_API_KEY`, `OLLAMA_API_KEY`).
   - Document `gemini-3.8-flash` as the default model.
   - Document the interactive `/model` command and switching syntax.
5. **Verification**:
   - Run `cargo test -p superstate --test test_repl_model_cmd`.
   - Run `cargo test -p superstate --test test_repl_commands`.
   - Run `cargo check --workspace --all-targets`.
   - Run `cargo clippy --workspace --all-targets -- -D warnings`.
   - Run `cargo test --workspace`.
   - Run `cargo fmt --all -- --check`.
6. **Commit**:
   - `git add crates/superstate-cli/src/repl.rs crates/superstate-cli/src/commands/doctor.rs crates/superstate-cli/tests/test_repl_model_cmd.rs README.md`
   - `git commit --no-gpg-sign -m "feat(cli): add REPL /model discovery selector and multi-provider doctor audit"`
7. **Roborev**:
   - Run `roborev status; roborev show HEAD`.
8. **Report**:
   - Write report to `docs/superpowers/plans/task-5-report.md`.
