//! Flags added lines in a unified diff that match common fallback patterns.

use serde::Serialize;
use std::path::{Component, Path};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub pattern: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FallbackRecord {
    pub file: String,
    pub line: usize,
    pub pattern: &'static str,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Added lines of a unified diff (`git diff` output) that match a fallback pattern.
/// A line that was moved or re-indented is a candidate too: the code around it may have
/// changed, and the fresh review judges it with the rest.
pub fn added_fallbacks(diff: &str) -> Vec<Finding> {
    let lines: Vec<&str> = diff.lines().collect();

    let mut findings = Vec::new();

    let mut current_file: Option<(&str, &str)> = None; // (path, ext)
    let mut current_line: usize = 0;
    let mut in_hunk = false;
    let mut ignore_hunk = false;
    let mut after_cfg_test = false;

    for (idx, &raw_line) in lines.iter().enumerate() {
        if let Some(rest) = raw_line.strip_prefix("+++ ") {
            in_hunk = false;
            ignore_hunk = false;
            after_cfg_test = false;
            current_line = 0;
            if rest == "/dev/null" {
                current_file = None;
            } else {
                let path = rest.strip_prefix("b/").unwrap_or(rest);
                if is_test_file(path) {
                    current_file = None;
                } else {
                    let ext = Path::new(path)
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    if is_supported_ext(ext) {
                        current_file = Some((path, ext));
                    } else {
                        current_file = None;
                    }
                }
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
                        let is_rust = current_file.map(|(_, ext)| ext == "rs").unwrap_or(false);
                        ignore_hunk = is_rust && raw_line.contains("mod tests");
                    }
                }
            }
            continue;
        }

        let Some((file_path, ext)) = current_file else {
            continue;
        };
        if !in_hunk {
            continue;
        }

        if let Some(content) = raw_line.strip_prefix(' ') {
            current_line += 1;
            if ext == "rs" && content.contains("#[cfg(test)]") {
                after_cfg_test = true;
            }
            continue;
        }

        if raw_line.starts_with('-') {
            continue;
        }

        if let Some(content) = raw_line.strip_prefix('+') {
            let line_no = current_line;
            current_line += 1;

            if ext == "rs" && content.contains("#[cfg(test)]") {
                after_cfg_test = true;
            }

            if ignore_hunk || after_cfg_test || is_comment(content, ext) {
                continue;
            }

            let patterns = match_line(content, ext, &lines, idx);
            if !patterns.is_empty() {
                let trimmed = content.trim();
                let text: String = trimmed.chars().take(200).collect();
                for pattern in patterns {
                    findings.push(Finding {
                        file: file_path.to_string(),
                        line: line_no,
                        pattern,
                        text: text.clone(),
                    });
                }
            }
        }
    }

    findings
}

/// One `== checks` flag for unjudged findings, or `None` when there are none.
pub fn flag(findings: &[Finding]) -> Option<String> {
    flags_for_review(findings, None).into_iter().next()
}

/// Flags for fallback candidates given an optional fresh model review result.
pub fn flags_for_review(
    candidates: &[Finding],
    model_review: Option<&crate::model_review::ModelReviewResult>,
) -> Vec<String> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let Some(mr) = model_review else {
        return vec![format_not_judged_why(candidates.len(), "no review")];
    };
    if mr.is_failed() {
        let why = mr.error.as_deref().unwrap_or("unknown error");
        return vec![format_not_judged_why(candidates.len(), why)];
    }

    let mut confirmed = Vec::new();
    let mut not_judged = Vec::new();
    for c in candidates {
        match mr
            .fallbacks
            .iter()
            .find(|v| v.file == c.file && v.line == c.line)
        {
            Some(v) if v.verdict == "fallback" => {
                confirmed.push(format!("{}:{}", c.file, c.line));
            }
            Some(v) if v.verdict == "not_fallback" => {}
            _ => {
                not_judged.push(format!("{}:{}", c.file, c.line));
            }
        }
    }

    let mut out = Vec::new();
    if !confirmed.is_empty() {
        out.push(format_fallback_list("fallbacks added", &confirmed));
    }
    if !not_judged.is_empty() {
        out.push(format_fallback_list(
            "fallback candidates not judged",
            &not_judged,
        ));
    }
    out
}

/// Annotate each fallback candidate with its verdict and reason when judged by a succeeded review.
pub fn annotate_fallbacks(
    candidates: &[Finding],
    model_review: Option<&crate::model_review::ModelReviewResult>,
) -> Vec<FallbackRecord> {
    let ok_review = model_review.filter(|mr| !mr.is_failed());
    candidates
        .iter()
        .map(|c| {
            let matched = ok_review.and_then(|mr| {
                mr.fallbacks
                    .iter()
                    .find(|v| v.file == c.file && v.line == c.line)
            });
            FallbackRecord {
                file: c.file.clone(),
                line: c.line,
                pattern: c.pattern,
                text: c.text.clone(),
                verdict: matched.map(|v| v.verdict.clone()),
                reason: matched.map(|v| v.reason.clone()),
            }
        })
        .collect()
}

fn format_not_judged_why(n: usize, why: &str) -> String {
    let why_one_line: String = why
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let prefix = format!("possible fallbacks added: {n} (not judged: ");
    let full = format!("{prefix}{why_one_line})");
    if full.chars().count() <= 200 {
        return full;
    }
    let avail = 200usize.saturating_sub(prefix.chars().count() + 2);
    let truncated: String = why_one_line.chars().take(avail).collect();
    format!("{prefix}{truncated}…)")
}

fn format_fallback_list(prefix: &str, items: &[String]) -> String {
    let clean: Vec<String> = items.iter().map(|s| s.replace(['\r', '\n'], " ")).collect();
    let n = clean.len();
    let head = format!("{prefix}: {n} (");
    let full = format!("{head}{})", clean.join(", "));
    if full.chars().count() <= 200 {
        return full;
    }
    for m in (1..n).rev() {
        let k = n - m;
        let cand = format!("{head}{}, … and {k} more)", clean[..m].join(", "));
        if cand.chars().count() <= 200 {
            return cand;
        }
    }
    if n > 1 {
        let suffix = format!(", … and {} more)", n - 1);
        let avail = 200usize.saturating_sub(head.chars().count() + suffix.chars().count());
        if avail > 0 {
            let first: String = clean[0].chars().take(avail).collect();
            return format!("{head}{first}{suffix}");
        }
        return sanitize_flag(format!("{head}… and {n} more)"));
    }
    let avail = 200usize.saturating_sub(head.chars().count() + 1);
    let first: String = clean[0].chars().take(avail).collect();
    format!("{head}{first})")
}

fn sanitize_flag(flag: String) -> String {
    let single_line = flag.lines().next().unwrap_or("").to_string();
    if single_line.chars().count() > 200 {
        single_line.chars().take(200).collect()
    } else {
        single_line
    }
}

fn is_supported_ext(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "py" | "js" | "jsx" | "mjs" | "ts" | "tsx" | "sh" | "bash" | "yml" | "yaml"
    )
}

fn is_test_file(path: &str) -> bool {
    let p = Path::new(path);
    for comp in p.components() {
        if let Component::Normal(os) = comp {
            if let Some(s) = os.to_str() {
                if matches!(
                    s,
                    "tests"
                        | "test"
                        | "__tests__"
                        | "benches"
                        | "examples"
                        | "fixtures"
                        | "testdata"
                ) {
                    return true;
                }
            }
        }
    }
    if let Some(file_name) = p.file_name().and_then(|s| s.to_str()) {
        if file_name.starts_with("test_") || file_name == "conftest.py" {
            return true;
        }
        if let (Some(stem), Some(ext)) = (
            p.file_stem().and_then(|s| s.to_str()),
            p.extension().and_then(|s| s.to_str()),
        ) {
            if !ext.is_empty()
                && (stem.ends_with("_test") || stem.ends_with(".test") || stem.ends_with(".spec"))
            {
                return true;
            }
        }
    }
    false
}

fn is_comment(content: &str, ext: &str) -> bool {
    let trimmed = content.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("--") {
        return true;
    }
    if trimmed.starts_with('#') {
        if ext == "rs" && (trimmed.starts_with("#[") || trimmed.starts_with("#!")) {
            return false;
        }
        return true;
    }
    false
}

fn match_line(content: &str, ext: &str, diff_lines: &[&str], idx: usize) -> Vec<&'static str> {
    let mut out = Vec::new();
    match ext {
        "rs" => {
            if content.contains(".unwrap_or(") {
                out.push("unwrap_or");
            }
            if content.contains(".unwrap_or_default(") {
                out.push("unwrap_or_default");
            }
            if content.contains(".unwrap_or_else(") {
                out.push("unwrap_or_else");
            }
        }
        "py" => {
            if matches_py_except_pass(content, diff_lines, idx) {
                out.push("except-pass");
            }
            if matches_py_get_default(content) {
                out.push("get-default");
            }
            if matches_py_or_literal(content) {
                out.push("or-literal");
            }
        }
        "js" | "jsx" | "mjs" | "ts" | "tsx" => {
            if content.contains("??") {
                out.push("nullish");
            }
            if matches_js_or_literal(content) {
                out.push("or-literal");
            }
            if matches_js_empty_catch(content) {
                out.push("empty-catch");
            }
        }
        "sh" | "bash" | "yml" | "yaml" => {
            if content.contains("|| true") || content.contains("|| :") {
                out.push("or-true");
            }
            if matches_shell_param_default(content) {
                out.push("param-default");
            }
        }
        _ => {}
    }
    out
}

fn matches_py_except_pass(content: &str, diff_lines: &[&str], idx: usize) -> bool {
    let trimmed = content.trim_start();
    let after_except = if let Some(rest) = trimmed.strip_prefix("except:") {
        Some(rest)
    } else if let Some(rest) = trimmed.strip_prefix("except Exception") {
        if rest.starts_with(':') || rest.starts_with(char::is_whitespace) {
            rest.split_once(':').map(|(_, r)| r)
        } else {
            None
        }
    } else if let Some(rest) = trimmed.strip_prefix("except BaseException") {
        if rest.starts_with(':') || rest.starts_with(char::is_whitespace) {
            rest.split_once(':').map(|(_, r)| r)
        } else {
            None
        }
    } else {
        None
    };

    let Some(after_colon) = after_except else {
        return false;
    };

    let inline = after_colon.split('#').next().unwrap_or(after_colon).trim();
    if !inline.is_empty() {
        return is_pass_continue_or_return(inline);
    }

    for &next_raw in &diff_lines[idx + 1..] {
        if next_raw.starts_with("+++ ")
            || next_raw.starts_with("@@ ")
            || next_raw.starts_with("diff --git ")
            || next_raw.starts_with(' ')
        {
            break;
        }
        if let Some(added) = next_raw.strip_prefix('+') {
            let added_trim = added.trim();
            if added_trim.is_empty() {
                continue;
            }
            let stmt = added_trim.split('#').next().unwrap_or(added_trim).trim();
            return is_pass_continue_or_return(stmt);
        }
    }
    false
}

fn is_pass_continue_or_return(s: &str) -> bool {
    s == "pass" || s == "continue" || s.starts_with("return")
}

fn matches_py_get_default(content: &str) -> bool {
    let bytes = content.as_bytes();
    for i in 0..bytes.len() {
        let rest = &content[i..];
        let args_start = if let Some(after) = rest.strip_prefix(".get(") {
            Some(after)
        } else if let Some(after) = rest.strip_prefix("getenv(") {
            let prev_ok = i == 0 || {
                let b = bytes[i - 1];
                !b.is_ascii_alphanumeric() && b != b'_'
            };
            if prev_ok {
                Some(after)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(after_open) = args_start {
            if has_top_level_comma(after_open) {
                return true;
            }
        }
    }
    false
}

fn has_top_level_comma(s: &str) -> bool {
    let mut depth = 0usize;
    let mut in_str: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if let Some(q) = in_str {
            if c == '\\' {
                let _ = chars.next();
            } else if c == q {
                in_str = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => in_str = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            ']' | '}' => {
                depth = depth.saturating_sub(1);
            }
            ',' if depth == 0 => return true,
            _ => {}
        }
    }
    false
}

fn matches_py_or_literal(content: &str) -> bool {
    let mut search = content;
    while let Some(pos) = search.find(" or ") {
        let after = &search[pos + 4..];
        let after_trimmed = after.trim_start_matches([' ', '\t']);
        if let Some(c) = after_trimmed.chars().next() {
            if matches!(c, '"' | '\'' | '[' | '{' | '(' | '0'..='9') {
                return true;
            }
        }
        search = after;
    }
    false
}

fn matches_js_or_literal(content: &str) -> bool {
    let mut search = content;
    while let Some(pos) = search.find("||") {
        let after = &search[pos + 2..];
        let after_trimmed = after.trim_start_matches([' ', '\t']);
        if let Some(c) = after_trimmed.chars().next() {
            if matches!(c, '"' | '\'' | '`' | '[' | '{' | '0'..='9') {
                return true;
            }
        }
        search = after;
    }
    false
}

fn matches_js_empty_catch(content: &str) -> bool {
    let bytes = content.as_bytes();
    for i in 0..bytes.len() {
        let Some(after_catch) = content[i..].strip_prefix("catch") else {
            continue;
        };
        let prev_ok = i == 0 || {
            let b = bytes[i - 1];
            !b.is_ascii_alphanumeric() && b != b'_' && b != b'$'
        };
        let next_ok = after_catch
            .chars()
            .next()
            .map(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '$')
            .unwrap_or(false);
        if !prev_ok || !next_ok {
            continue;
        }
        let mut rest = after_catch.trim_start();
        if let Some(after_open_paren) = rest.strip_prefix('(') {
            let mut depth = 1usize;
            let mut end_idx = None;
            for (idx, ch) in after_open_paren.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end_idx = Some(idx + 1);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let Some(end) = end_idx else {
                continue;
            };
            rest = after_open_paren[end..].trim_start();
        }
        if let Some(after_brace) = rest.strip_prefix('{') {
            if after_brace == "}"
                || after_brace == " }"
                || after_brace.trim_start().starts_with('}')
            {
                return true;
            }
        }
    }
    false
}

fn matches_shell_param_default(content: &str) -> bool {
    let mut search = content;
    while let Some(pos) = search.find("${") {
        let after = &search[pos + 2..];
        let inside = after.split('}').next().unwrap_or(after);
        if inside.contains(":-") {
            return true;
        }
        search = after;
    }
    false
}
