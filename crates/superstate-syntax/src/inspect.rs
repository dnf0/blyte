use std::path::Path;
use crate::symbols::{extract_symbols, SupportedLanguage, SymbolInfo, SyntaxError};

/// Detects the programming language of a source file based on its file extension.
pub fn detect_language(path: &Path) -> Option<SupportedLanguage> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(SupportedLanguage::Rust),
        "py" | "pyx" => Some(SupportedLanguage::Python),
        "ts" | "tsx" | "js" | "jsx" => Some(SupportedLanguage::TypeScript),
        "go" => Some(SupportedLanguage::Go),
        _ => None,
    }
}

/// Reads a source file from disk, detects its language, and extracts all AST symbols.
pub fn extract_symbols_from_file(
    path: &Path,
) -> Result<(SupportedLanguage, String, Vec<SymbolInfo>), SyntaxError> {
    if !path.exists() {
        return Err(SyntaxError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("File not found: {}", path.display()),
        )));
    }

    let lang = detect_language(path).ok_or_else(|| {
        SyntaxError::UnsupportedLanguage(format!(
            "Unsupported language for file: {}",
            path.display()
        ))
    })?;

    let content = std::fs::read_to_string(path)?;
    let symbols = extract_symbols(&content, lang)?;
    Ok((lang, content, symbols))
}

/// Slices the exact source code corresponding to `[symbol.start_line..=symbol.end_line]` (1-indexed).
/// Gracefully clamps boundaries if line numbers exceed the file line count.
pub fn extract_symbol_source<'a>(source: &'a str, symbol: &SymbolInfo) -> &'a str {
    let mut line_ranges = Vec::new();
    let mut line_start = 0;
    let bytes = source.as_bytes();

    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            let line_end = if i > line_start && bytes[i - 1] == b'\r' {
                i - 1
            } else {
                i
            };
            line_ranges.push((line_start, line_end));
            line_start = i + 1;
        }
    }

    if line_start < bytes.len() || (!source.is_empty() && !bytes.ends_with(b"\n")) {
        line_ranges.push((line_start, bytes.len()));
    }

    if line_ranges.is_empty() {
        return "";
    }

    let start_idx = if symbol.start_line == 0 {
        1
    } else {
        symbol.start_line
    };

    if start_idx > line_ranges.len() || start_idx > symbol.end_line {
        return "";
    }

    let end_idx = symbol.end_line.min(line_ranges.len());
    let (start_byte, _) = line_ranges[start_idx - 1];
    let (_, end_byte) = line_ranges[end_idx - 1];

    if start_byte > end_byte || end_byte > source.len() {
        return "";
    }

    &source[start_byte..end_byte]
}

/// Formats a human-readable symbol outline suitable for LLM context injection.
pub fn format_outline(symbols: &[SymbolInfo]) -> String {
    symbols
        .iter()
        .map(|s| {
            format!(
                "- [{:?}] {} [lines {}-{}]",
                s.kind, s.name, s.start_line, s.end_line
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Performs a case-insensitive search for a symbol by name, returning the first match.
pub fn find_symbol<'a>(symbols: &'a [SymbolInfo], name: &str) -> Option<&'a SymbolInfo> {
    symbols
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(name))
}
