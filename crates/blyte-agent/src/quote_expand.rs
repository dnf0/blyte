//! Expand quote directives in a research report.
//!
//! A line that reads `<<quote /absolute/path:N-M>>` (or `<<quote /absolute/path:N>>`) on its own
//! is replaced by a citation and a fenced block holding those lines of the file, so the quote is
//! exact and the model never retypes it. A directive inside a fenced block is not a directive.
//! Lines that hold a code fence cannot be quoted: the fence would end the block.
//! A directive that cannot be expanded stays in the text and is reported, never partly expanded.

use std::path::Path;

/// Most lines one directive may quote.
pub const MAX_QUOTE_LINES: usize = 200;

const OPEN: &str = "<<quote ";
const CLOSE: &str = ">>";
const FORM: &str = "not a quote directive; write <<quote /absolute/path:N-M>> alone on a line";

/// Replace every quote directive in `report` with the lines it names.
/// Returns the new text and one problem per directive that could not be expanded.
pub fn expand(
    report: &str,
    read: &dyn Fn(&Path) -> Result<String, String>,
) -> (String, Vec<String>) {
    let mut out = String::with_capacity(report.len());
    let mut problems = Vec::new();
    let mut in_fence = false;
    for (index, piece) in report.split_inclusive('\n').enumerate() {
        let body = piece.strip_suffix('\n').unwrap_or(piece);
        let body = body.strip_suffix('\r').unwrap_or(body);
        let ending = &piece[body.len()..];
        if body.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push_str(piece);
            continue;
        }
        let trimmed = body.trim();
        if in_fence || !trimmed.starts_with("<<quote") {
            out.push_str(piece);
            continue;
        }
        match expand_one(trimmed, read) {
            Ok(lines) => {
                let eol = if ending.is_empty() { "\n" } else { ending };
                let last = lines.len() - 1;
                for (i, line) in lines.iter().enumerate() {
                    out.push_str(line);
                    if i < last || !ending.is_empty() {
                        out.push_str(eol);
                    }
                }
            }
            Err(reason) => {
                out.push_str(piece);
                problems.push(format!("report.md:{}: {trimmed}: {reason}", index + 1));
            }
        }
    }
    (out, problems)
}

/// The output lines for one directive (without line endings), or the reason it cannot expand.
fn expand_one(
    directive: &str,
    read: &dyn Fn(&Path) -> Result<String, String>,
) -> Result<Vec<String>, String> {
    let target = directive
        .strip_prefix(OPEN)
        .and_then(|s| s.strip_suffix(CLOSE))
        .filter(|s| !s.is_empty() && !s.chars().any(char::is_whitespace))
        .ok_or_else(|| FORM.to_string())?;
    let (path, range) = target.rsplit_once(':').ok_or_else(|| FORM.to_string())?;
    let (first, last) = match range.split_once('-') {
        Some((a, b)) => (a, b),
        None => (range, range),
    };
    let (Ok(first), Ok(last)) = (first.parse::<usize>(), last.parse::<usize>()) else {
        return Err(FORM.to_string());
    };
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err("the path must be absolute".to_string());
    }
    let text = read(path).map_err(|e| format!("cannot read the file: {e}"))?;
    if first == 0 || last < first {
        return Err("bad line range".to_string());
    }
    let lines: Vec<&str> = text.lines().collect();
    if last > lines.len() {
        return Err(format!("the file has {} lines", lines.len()));
    }
    let count = last - first + 1;
    if count > MAX_QUOTE_LINES {
        return Err(format!(
            "quotes {count} lines; the most is {MAX_QUOTE_LINES}, quote the part that matters"
        ));
    }
    let quoted = &lines[first - 1..last];
    if let Some(i) = quoted
        .iter()
        .position(|l| l.trim_start().starts_with("```"))
    {
        return Err(format!(
            "line {} of the file is a code fence, which would end the quote; quote the lines around it",
            first + i
        ));
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let cite = if first == last {
        format!("{}:{first}", path.display())
    } else {
        format!("{}:{first}-{last}", path.display())
    };
    let mut out = vec![format!("`{cite}`"), format!("```{ext}")];
    out.extend(quoted.iter().map(|l| (*l).to_string()));
    out.push("```".to_string());
    Ok(out)
}
