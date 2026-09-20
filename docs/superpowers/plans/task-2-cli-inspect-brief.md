# Task 2 Brief: `superstate-cli` `inspect` Command Implementation & Installation

## Context & Objective
In `repos/superstate`, expose `superstate inspect` in `crates/superstate-cli` by wiring `superstate-syntax::inspect`. Reinstall release binary to system (`cargo install --path crates/superstate-cli --locked --force`) and verify `--help` and execution.

## Working Directory & Branch
- Repo: `/Users/danielfisher/repos/superstate`
- Branch: `feat/ast-symbol-inspection`

## Files to Touch
- Create: `crates/superstate-cli/src/commands/inspect.rs`
- Modify: `crates/superstate-cli/src/commands/mod.rs` (register `inspect`)
- Modify: `crates/superstate-cli/src/main.rs` (add `Inspect(InspectArgs)` to `CliCommand` enum and dispatch)
- Create Test: `crates/superstate-cli/tests/test_cli_inspect.rs`

## Exact CLI Arguments & Specification
```rust
#[derive(clap::Args, Debug, Clone)]
pub struct InspectArgs {
    /// Path to target source file to inspect
    pub path: PathBuf,

    /// Extract and print only the body of a specific function, class, or struct
    #[arg(long)]
    pub symbol: Option<String>,

    /// Print compact symbol outline (signatures and line numbers)
    #[arg(long)]
    pub outline: bool,

    /// Print bounded line range in format 'start:end' (1-indexed)
    #[arg(long)]
    pub lines: Option<String>,

    /// Emit output in structured JSON format
    #[arg(long)]
    pub json: bool,
}
```

## Behavior & Formats
1. **`--lines <start:end>`**:
   - Works on ANY readable text file (even unsupported AST languages like `.c`, `.json`, etc.).
   - Parse `start` and `end` (usize, 1-indexed). If invalid syntax, exit code 1 with:
     `Error: invalid line range '<arg>'. Expected format 'start:end' (e.g. 10:50)`
   - Print:
     ```text
     === Lines {start}-{end} in {path} ===
     {line_no}: {line_content}
     ```
2. **`--symbol <name>`**:
   - Extract symbols via `extract_symbols_from_file(&args.path)`.
   - If symbol found via `find_symbol`:
     - If `--json`: serialize `SymbolInfo` to JSON string.
     - Else: print header with line range and source body with line numbers:
       ```text
       === Symbol: {symbol.name} [lines {symbol.start_line}-{symbol.end_line}] in {path} ===
       {line_no}: {line_content}
       ```
   - If symbol NOT found:
     - Print to stderr: `Error: symbol '{name}' not found in {path}. Available symbols: [name1, name2, ...]`
     - Exit with code 1.
3. **Default / `--outline` Mode**:
   - If `--json`: serialize `Vec<SymbolInfo>` as JSON array.
   - Else: print outline header and formatted symbols:
     ```text
     === Outline: {path} ({lang:?}) ===
     - [{kind:?}] {name} [lines {start_line}-{end_line}]
     ```
4. **File & Language Errors**:
   - If file does not exist: stderr `Error: file not found at {path}`, exit code 1.
   - If unsupported language and AST requested: stderr `Error: unsupported language for AST parsing in '{path}'. Use --lines <start:end>`, exit code 1.

## Installation & Verification
- Run: `cargo install --path crates/superstate-cli --locked --force`
- Verify: `~/.cargo/bin/superstate inspect --help` shows flags.

## TDD Instructions
1. Write failing integration test in `crates/superstate-cli/tests/test_cli_inspect.rs`.
2. Run `cargo test -p superstate --test test_cli_inspect` to verify failure.
3. Implement `inspect.rs`, update `mod.rs` and `main.rs`.
4. Run `cargo test -p superstate --test test_cli_inspect` to verify passing.
5. Run `cargo clippy --all-targets -- -D warnings` and `cargo test --all` (must be 100% clean).
6. Reinstall binary: `cargo install --path crates/superstate-cli --locked --force`.
7. Verify binary on system.
8. Commit with `feat(cli): add superstate inspect command for AST symbol extraction and slicing` (`--no-gpg-sign`).
9. Write report to `/Users/danielfisher/repos/superstate/docs/superpowers/plans/task-2-cli-inspect-report.md`.
