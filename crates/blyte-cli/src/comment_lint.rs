//! Finds non-documentation, non-directive comments in source files and unified diffs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Comment {
    pub file: String,
    pub line: usize,
    pub text: String,
}

pub const DIRECTIVES: &[&str] = &[
    "noqa",
    "type:",
    "pragma",
    "pylint:",
    "mypy:",
    "pyright:",
    "ruff:",
    "flake8:",
    "fmt:",
    "isort:",
    "yapf:",
    "nosec",
    "coding:",
    "-*-",
    "eslint",
    "@ts-",
    "prettier-",
    "biome-",
    "istanbul",
    "c8 ",
    "v8 ",
    "webpack",
    "@vite-ignore",
    "@jsx",
    "<reference",
    "jshint",
    "global ",
    "globals ",
    "rustfmt::",
    "clippy::",
    "SAFETY:",
    "SPDX-",
    "Copyright",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    Python,
    JsTs,
}

fn lang_for_path(path: &str) -> Option<Lang> {
    let ext = Path::new(path).extension().and_then(|s| s.to_str())?;
    match ext {
        "rs" => Some(Lang::Rust),
        "py" => Some(Lang::Python),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" => Some(Lang::JsTs),
        _ => None,
    }
}

/// True when `path` has a supported source-file extension.
pub fn is_supported(path: &str) -> bool {
    lang_for_path(path).is_some()
}

/// Lexes `source` and returns every comment that counts, in file order.
pub fn comments_in(path: &str, source: &str) -> Vec<Comment> {
    let Some(lang) = lang_for_path(path) else {
        return Vec::new();
    };
    match lang {
        Lang::Rust => lex_rust(path, source),
        Lang::Python => lex_python(path, source),
        Lang::JsTs => lex_js_ts(path, source),
    }
}

fn next_char(source: &str, i: &mut usize, line: &mut usize) -> char {
    let ch = source[*i..].chars().next().expect("valid byte index");
    *i += ch.len_utf8();
    if ch == '\n' {
        *line += 1;
    }
    ch
}

fn raw_string_hashes(bytes: &[u8], i: usize) -> Option<usize> {
    if bytes.get(i) != Some(&b'r') {
        return None;
    }
    let mut j = i + 1;
    while j < bytes.len() && bytes[j] == b'#' {
        j += 1;
    }
    if j < bytes.len() && bytes[j] == b'"' {
        Some(j - (i + 1))
    } else {
        None
    }
}

fn lex_rust(path: &str, source: &str) -> Vec<Comment> {
    let bytes = source.as_bytes();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut out = Vec::new();

    while i < bytes.len() {
        if let Some(hashes) = raw_string_hashes(bytes, i) {
            i += 2 + hashes;
            while i < bytes.len() {
                if bytes[i] == b'"'
                    && i + 1 + hashes <= bytes.len()
                    && bytes[i + 1..i + 1 + hashes].iter().all(|&b| b == b'#')
                {
                    i += 1 + hashes;
                    break;
                }
                next_char(source, &mut i, &mut line);
            }
            continue;
        }

        if bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() {
                let ch = next_char(source, &mut i, &mut line);
                if ch == '\\' {
                    if i < bytes.len() {
                        next_char(source, &mut i, &mut line);
                    }
                } else if ch == '"' {
                    break;
                }
            }
            continue;
        }

        if bytes[i] == b'\'' {
            let mut look = source[i + 1..].chars();
            match (look.next(), look.next()) {
                (Some('\\'), _) => {
                    i += 1;
                    while i < bytes.len() {
                        let ch = next_char(source, &mut i, &mut line);
                        if ch == '\\' {
                            if i < bytes.len() {
                                next_char(source, &mut i, &mut line);
                            }
                        } else if ch == '\'' {
                            break;
                        }
                    }
                }
                (Some(c1), Some('\'')) => {
                    i += 1 + c1.len_utf8() + 1;
                    if c1 == '\n' {
                        line += 1;
                    }
                }
                _ => {
                    i += 1;
                }
            }
            continue;
        }

        if source[i..].starts_with("//") {
            let start_byte = i;
            let start_line = line;
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                let ch = source[i..].chars().next().expect("valid byte index");
                i += ch.len_utf8();
            }
            let raw = &source[start_byte..i];
            maybe_push(path, start_byte, start_line, raw, Lang::Rust, &mut out);
            continue;
        }

        if source[i..].starts_with("/*") {
            let start_byte = i;
            let start_line = line;
            i += 2;
            let mut depth = 1usize;
            while i < bytes.len() && depth > 0 {
                if source[i..].starts_with("/*") {
                    depth += 1;
                    i += 2;
                } else if source[i..].starts_with("*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    next_char(source, &mut i, &mut line);
                }
            }
            let raw = &source[start_byte..i];
            maybe_push(path, start_byte, start_line, raw, Lang::Rust, &mut out);
            continue;
        }

        next_char(source, &mut i, &mut line);
    }

    out
}

fn lex_js_ts(path: &str, source: &str) -> Vec<Comment> {
    let bytes = source.as_bytes();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut out = Vec::new();

    while i < bytes.len() {
        if matches!(bytes[i], b'"' | b'\'' | b'`') {
            let quote = next_char(source, &mut i, &mut line);
            while i < bytes.len() {
                let ch = next_char(source, &mut i, &mut line);
                if ch == '\\' {
                    if i < bytes.len() {
                        next_char(source, &mut i, &mut line);
                    }
                } else if ch == quote {
                    break;
                }
            }
            continue;
        }

        if source[i..].starts_with("//") && !(i > 0 && bytes[i - 1] == b'\\') {
            let start_byte = i;
            let start_line = line;
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                let ch = source[i..].chars().next().expect("valid byte index");
                i += ch.len_utf8();
            }
            let raw = &source[start_byte..i];
            maybe_push(path, start_byte, start_line, raw, Lang::JsTs, &mut out);
            continue;
        }

        if source[i..].starts_with("/*") {
            let start_byte = i;
            let start_line = line;
            i += 2;
            while i < bytes.len() {
                if source[i..].starts_with("*/") {
                    i += 2;
                    break;
                }
                next_char(source, &mut i, &mut line);
            }
            let raw = &source[start_byte..i];
            maybe_push(path, start_byte, start_line, raw, Lang::JsTs, &mut out);
            continue;
        }

        next_char(source, &mut i, &mut line);
    }

    out
}

fn lex_python(path: &str, source: &str) -> Vec<Comment> {
    let bytes = source.as_bytes();
    let mut i = 0usize;
    let mut line = 1usize;
    let mut out = Vec::new();

    while i < bytes.len() {
        if source[i..].starts_with("'''") || source[i..].starts_with("\"\"\"") {
            let delim = &source[i..i + 3];
            i += 3;
            while i < bytes.len() {
                if source[i..].starts_with(delim) {
                    i += 3;
                    break;
                }
                let ch = next_char(source, &mut i, &mut line);
                if ch == '\\' && i < bytes.len() {
                    next_char(source, &mut i, &mut line);
                }
            }
            continue;
        }

        if matches!(bytes[i], b'"' | b'\'') {
            let quote = next_char(source, &mut i, &mut line);
            while i < bytes.len() {
                let ch = next_char(source, &mut i, &mut line);
                if ch == '\\' {
                    if i < bytes.len() {
                        next_char(source, &mut i, &mut line);
                    }
                } else if ch == quote || ch == '\n' {
                    break;
                }
            }
            continue;
        }

        if bytes[i] == b'#' {
            let start_byte = i;
            let start_line = line;
            i += 1;
            while i < bytes.len() && bytes[i] != b'\n' {
                let ch = source[i..].chars().next().expect("valid byte index");
                i += ch.len_utf8();
            }
            let raw = &source[start_byte..i];
            maybe_push(path, start_byte, start_line, raw, Lang::Python, &mut out);
            continue;
        }

        next_char(source, &mut i, &mut line);
    }

    out
}

fn maybe_push(
    path: &str,
    start_byte: usize,
    start_line: usize,
    raw: &str,
    lang: Lang,
    out: &mut Vec<Comment>,
) {
    if start_byte == 0 && raw.starts_with("#!") {
        return;
    }
    if is_doc_comment(raw, lang) || is_directive(raw) {
        return;
    }
    let first_line = raw.lines().next().unwrap_or("").trim();
    let text: String = first_line.chars().take(120).collect();
    out.push(Comment {
        file: path.to_string(),
        line: start_line,
        text,
    });
}

fn is_doc_comment(raw: &str, lang: Lang) -> bool {
    match lang {
        Lang::Rust => {
            (raw.starts_with("///") && !raw.starts_with("////"))
                || raw.starts_with("//!")
                || (raw.starts_with("/**") && !raw.starts_with("/***") && raw != "/**/")
                || raw.starts_with("/*!")
        }
        Lang::JsTs => raw.starts_with("/**") && !raw.starts_with("/***") && raw != "/**/",
        Lang::Python => false,
    }
}

fn is_directive(raw: &str) -> bool {
    let body = raw
        .strip_prefix("//")
        .or_else(|| raw.strip_prefix("/*"))
        .or_else(|| raw.strip_prefix('#'))
        .unwrap_or(raw)
        .trim_start();
    DIRECTIVES.iter().any(|&d| {
        body.starts_with(d)
            || (d == "Copyright" && (body.starts_with("copyright") || body.starts_with("(c)")))
    })
}

/// Maps each new path in a unified diff to the 1-based line numbers of its added lines.
pub fn added_lines(diff: &str) -> BTreeMap<String, BTreeSet<usize>> {
    let mut out: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut current_file: Option<String> = None;
    let mut current_line = 0usize;
    let mut in_hunk = false;

    for raw_line in diff.lines() {
        if let Some(rest) = raw_line.strip_prefix("+++ ") {
            in_hunk = false;
            current_line = 0;
            if rest == "/dev/null" {
                current_file = None;
            } else {
                let path = rest.strip_prefix("b/").unwrap_or(rest).to_string();
                out.entry(path.clone()).or_default();
                current_file = Some(path);
            }
            continue;
        }

        if let Some(after_at) = raw_line.strip_prefix("@@ ") {
            if let Some((coords, _)) = after_at.split_once("@@") {
                if let Some(plus_part) = coords.split_whitespace().find_map(|t| t.strip_prefix('+'))
                {
                    let c_str = plus_part.split(',').next().unwrap_or(plus_part);
                    if let Ok(c) = c_str.parse::<usize>() {
                        current_line = c;
                        in_hunk = true;
                    }
                }
            }
            continue;
        }

        let Some(ref path) = current_file else {
            continue;
        };
        if !in_hunk {
            continue;
        }

        if raw_line.starts_with(' ') {
            current_line += 1;
        } else if raw_line.starts_with('+') {
            if let Some(set) = out.get_mut(path) {
                set.insert(current_line);
            }
            current_line += 1;
        }
    }

    out
}

/// Finds comments on lines added by `diff`, reading each supported file with `read`.
pub fn added_comments(
    diff: &str,
    read: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Vec<Comment>, String> {
    let mut out = Vec::new();
    for (path, lines) in added_lines(diff) {
        if !is_supported(&path) {
            continue;
        }
        let source = read(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
        for comment in comments_in(&path, &source) {
            if lines.contains(&comment.line) {
                out.push(comment);
            }
        }
    }
    Ok(out)
}

/// Formats a review flag for added comments, or `None` when none were found.
pub fn flag(found: &[Comment]) -> Option<String> {
    if found.is_empty() {
        return None;
    }
    let n = found.len();
    let listed: Vec<String> = found
        .iter()
        .take(3)
        .map(|c| format!("{}:{}", c.file, c.line))
        .collect();
    let more = if n > 3 {
        format!(" and {} more", n - 3)
    } else {
        String::new()
    };
    Some(format!(
        "comments added ({n}): {}{more}: remove them, and check what each one was covering for",
        listed.join(", ")
    ))
}
