# Task 5: End-to-End Integration Tests & CLI Verification

## Goal
Add comprehensive end-to-end binary integration tests in `crates/superstate-cli/tests/test_e2e_cli.rs` testing the real `superstate` compiled binary (`env!("CARGO_BIN_EXE_superstate")`), and update `README.md` to document the interactive agent harness, slash commands, execution flags, and SKILL.state options.

## Working Directory
`/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness`

## Files
- Create: `crates/superstate-cli/tests/test_e2e_cli.rs`
- Modify: `README.md`

## Detailed Implementation Steps
1. Create `crates/superstate-cli/tests/test_e2e_cli.rs`:
   - `test_cli_binary_help_screen`:
     Runs `env!("CARGO_BIN_EXE_superstate")` with `--help`.
     Asserts exit success and stdout contains:
     `superstate`, `[PROMPT]`, `--gate`, `--context`, `--model`, `--container`, `--max-turns`, `--max-gates`, `--rollback`.
   - `test_cli_empty_prompt_error`:
     Runs binary with `""`.
     Asserts exit code 1 and stderr contains `Error: prompt cannot be empty`.
   - `test_cli_headless_missing_api_key`:
     Runs binary with `"fix math bug"` with `GEMINI_API_KEY` and `GOOGLE_API_KEY` removed from child environment.
     Asserts exit code 1 and stderr contains `GEMINI_API_KEY`.
   - `test_cli_subcommand_dispatch`:
     Runs binary with `doctor`.
     Asserts stdout or stderr contains doctor verification output.
   - `test_cli_interactive_repl_stdin_driving`:
     Runs binary with empty args (interactive mode) and pipes stdin:
     `"/help\n/gate cargo test\n/state\n/exit\n"`
     Asserts exit success, stdout contains banner, `/help` guide, gate update notification, state output, and goodbye message.

2. Update `README.md`:
   - Update the Quickstart and CLI sections to document:
     - `superstate`: starts the interactive agent harness REPL.
     - `superstate "<prompt>" --gate "<gate_cmd>"`: headless single-shot solver with automatic rollback and verification gates.
     - `--context <state-only | conversational>`: SKILL.state $\mathcal{O}(1)$ prompt bounded memory vs conversational history.
     - Table of slash commands: `/help`, `/state`, `/diff`, `/review`, `/doctor`, `/gate`, `/clear`, `/exit`.
     - Table of CLI execution flags: `--gate`, `--context`, `--model`, `--container`, `--max-turns`, `--max-gates`, `--rollback`.

3. Verify:
   `cargo test -p superstate --test test_e2e_cli`
   `cargo check --workspace --all-targets`
   `cargo test --workspace`

4. Commit:
   ```bash
   git add crates/superstate-cli/tests/test_e2e_cli.rs README.md
   git commit --no-gpg-sign -m "feat(cli): add end-to-end integration tests and update README harness documentation"
   ```
5. Run `roborev status; roborev show HEAD`.
6. Write report to `/Users/danielfisher/repos/superstate/.wt/feat-superstate-interactive-harness/docs/superpowers/plans/task-5-report.md`.
