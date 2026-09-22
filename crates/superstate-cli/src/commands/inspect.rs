//! Implementation of the `superstate inspect` subcommand.
//!
//! Provides AST symbol extraction, compact code outlines, and bounded line-level
//! source slicing across supported programming languages (Rust, Python, TypeScript, Go)
//! and plain text files.

use clap::Args;
use std::fs;
use std::path::{Path, PathBuf};
use superstate_syntax::inspect::{extract_symbols_from_file, find_symbol, format_outline};
use superstate_syntax::symbols::SyntaxError;

#[derive(Args, Debug, Clone, PartialEq)]
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

/// Parses a 1-indexed line range in the form 'start:end'.
///
/// Returns an error if the range syntax is invalid, either number is 0,
/// or start exceeds end.
fn parse_line_range(s: &str) -> Result<(usize, usize), String> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 2 {
        return Err(format!(
            "invalid line range '{}'. Expected format 'start:end' (e.g. 10:50)",
            s
        ));
    }
    let start: usize = parts[0].parse().map_err(|_| {
        format!(
            "invalid line range '{}'. Expected format 'start:end' (e.g. 10:50)",
            s
        )
    })?;
    let end: usize = parts[1].parse().map_err(|_| {
        format!(
            "invalid line range '{}'. Expected format 'start:end' (e.g. 10:50)",
            s
        )
    })?;
    if start == 0 || end == 0 || start > end {
        return Err(format!(
            "invalid line range '{}'. Expected format 'start:end' (e.g. 10:50)",
            s
        ));
    }
    Ok((start, end))
}

/// Prints a bounded range of lines from an arbitrary text file.
fn inspect_lines(path: &Path, range_str: &str) -> Result<(), String> {
    let (start, end) = parse_line_range(range_str)?;
    let content = fs::read_to_string(path)
        .map_err(|e| format!("failed to read file '{}': {}", path.display(), e))?;
    let lines: Vec<&str> = content.lines().collect();

    println!("=== Lines {}-{} in {} ===", start, end, path.display());
    if start <= lines.len() {
        let max_line = end.min(lines.len());
        for line_no in start..=max_line {
            println!("{}: {}", line_no, lines[line_no - 1]);
        }
    }
    Ok(())
}

/// Extracts and prints a specific symbol from a source file by name.
fn inspect_symbol(path: &Path, symbol_name: &str, json: bool) -> Result<(), String> {
    let (_lang, content, symbols) = match extract_symbols_from_file(path) {
        Ok(res) => res,
        Err(SyntaxError::UnsupportedLanguage(_)) => {
            return Err(format!(
                "unsupported language for AST parsing in '{}'. Use --lines <start:end>",
                path.display()
            ));
        }
        Err(SyntaxError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("file not found at {}", path.display()));
        }
        Err(e) => return Err(e.to_string()),
    };

    if let Some(symbol) = find_symbol(&symbols, symbol_name) {
        if json {
            let json_str = serde_json::to_string_pretty(symbol)
                .map_err(|e| format!("failed to serialize symbol: {}", e))?;
            println!("{}", json_str);
        } else {
            let lines: Vec<&str> = content.lines().collect();
            println!(
                "=== Symbol: {} [lines {}-{}] in {} ===",
                symbol.name,
                symbol.start_line,
                symbol.end_line,
                path.display()
            );
            if symbol.start_line >= 1 && symbol.start_line <= lines.len() {
                let max_line = symbol.end_line.min(lines.len());
                for line_no in symbol.start_line..=max_line {
                    println!("{}: {}", line_no, lines[line_no - 1]);
                }
            }
        }
        Ok(())
    } else {
        let available_names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        Err(format!(
            "symbol '{}' not found in {}. Available symbols: [{}]",
            symbol_name,
            path.display(),
            available_names.join(", ")
        ))
    }
}

/// Extracts and prints a compact symbol outline for a source file.
fn inspect_outline(path: &Path, json: bool) -> Result<(), String> {
    let (lang, _content, symbols) = match extract_symbols_from_file(path) {
        Ok(res) => res,
        Err(SyntaxError::UnsupportedLanguage(_)) => {
            return Err(format!(
                "unsupported language for AST parsing in '{}'. Use --lines <start:end>",
                path.display()
            ));
        }
        Err(SyntaxError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("file not found at {}", path.display()));
        }
        Err(e) => return Err(e.to_string()),
    };

    if json {
        let json_str = serde_json::to_string_pretty(&symbols)
            .map_err(|e| format!("failed to serialize symbols: {}", e))?;
        println!("{}", json_str);
    } else {
        println!("=== Outline: {} ({:?}) ===", path.display(), lang);
        let outline = format_outline(&symbols);
        if !outline.is_empty() {
            println!("{}", outline);
        }
    }
    Ok(())
}

/// Dispatches inspection commands based on the provided arguments.
pub fn cmd_inspect(args: &InspectArgs) -> Result<(), String> {
    if !args.path.exists() {
        return Err(format!("file not found at {}", args.path.display()));
    }

    if let Some(ref lines_arg) = args.lines {
        inspect_lines(&args.path, lines_arg)
    } else if let Some(ref symbol_name) = args.symbol {
        inspect_symbol(&args.path, symbol_name, args.json)
    } else {
        inspect_outline(&args.path, args.json)
    }
}
