# Task 2 Brief: `superstate guard pre-tool-use` CLI Subcommand

## Context & Base Commit
- **Working Directory:** `/Users/danielfisher/repos/superstate/.worktrees/feat-pre-tool-use-file-guard`
- **Base Commit:** `5726328`
- **Spec:** `docs/superpowers/specs/2026-09-20-file-read-guard-design.md`
- **Plan:** `docs/superpowers/plans/2026-09-20-file-read-guard.md`

## What Needs to be Built
Implement the CLI entrypoint for `superstate guard pre-tool-use [--threshold-lines <N>]` in `crates/superstate-cli`.

### Clap Definition
Add `Guard` command to `superstate-cli`:
```rust
#[derive(clap::Subcommand, Debug)]
pub enum GuardCommands {
    /// Evaluate tool invocation against deterministic context boundaries
    PreToolUse(PreToolUseArgs),
}

#[derive(clap::Args, Debug)]
pub struct PreToolUseArgs {
    /// Maximum lines permitted before shunting to symbol outline
    #[arg(long, default_value_t = 200)]
    pub threshold_lines: usize,
}
```

### Stdin/Stdout JSON Protocol
1. Read `stdin` to string (`std::io::read_to_string`).
2. If `stdin` is empty or malformed JSON, emit `{"decision":"allow"}` and exit 0 (fail-open to protect runtime).
3. Parse payload into a flexible JSON value:
   ```json
   {
     "toolCall": {
       "name": "view_file",
       "args": {
         "AbsolutePath": "...",
         "StartLine": 1,
         "EndLine": 800
       }
     }
   }
   ```
   (Also handle case where `tool_call` is snake_case or top-level `toolName` / `args`).
4. Tool Name Evaluation:
   - Normalize tool name to lowercase.
   - If tool name is NOT one of `view_file`, `view`, `read_file`, `readfile`:
     Emit `{"decision":"allow"}` and exit 0.
5. Arguments Extraction & Normalization:
   - Extract path from `AbsolutePath`, `path`, `file_path`, `filePath`, `TargetFile`, `target_file`, or `file`.
   - If path is missing or empty, emit `{"decision":"allow"}` and exit 0.
   - Extract start line from `StartLine`, `start_line`, `startLine`, `offset`.
   - Extract end line from `EndLine`, `end_line`, `endLine`, `limit`.
   - **Crucial Normalization:** If `end_line` is present but `start_line` is `None`, default `start_line` to `Some(1)`.
6. Evaluation:
   - Call `superstate_core::guard::evaluate_file_read_guard(Path::new(&path), start_line, end_line, &config)`.
   - Print serialized JSON (`serde_json::to_string(&decision)`) to `stdout` followed by newline.
   - Exit with code 0.

## Steps (Follow TDD Strictly)
1. Write failing integration tests in `crates/superstate-cli/tests/test_cli_guard.rs`.
   Test scenarios:
   - Allowed tool call (`view_file` on small file).
   - Denied tool call (`view_file` unbounded on large file > 200 lines) emitting `decision: "deny"` with AST outline.
   - Bounded slice tool call (`view_file` with `StartLine: 1, EndLine: 50`) emitting `decision: "allow"`.
   - Head slice tool call (`view_file` with only `EndLine: 50`) emitting `decision: "allow"`.
   - Non-file tool call (`run_command`) emitting `decision: "allow"`.
   - Invalid or empty JSON on stdin emitting `decision: "allow"`.
   - Custom `--threshold-lines 50` parameter.
2. Run `cargo test -p superstate --test test_cli_guard` to verify failure.
3. Implement `crates/superstate-cli/src/commands/guard.rs`, register in `commands/mod.rs` and `main.rs`.
4. Run `cargo test -p superstate --test test_cli_guard` to verify passing.
5. Run `cargo test --all` and `cargo clippy --all-targets -- -D warnings`.
6. Commit using `git commit --no-gpg-sign -m "feat(cli): add superstate guard pre-tool-use CLI subcommand"`.
7. Write your implementer report to `docs/superpowers/plans/task-2-report.md`.
