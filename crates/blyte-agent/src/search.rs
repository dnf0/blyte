//! `grep` in-process, with ripgrep's own libraries: rg's regex engine and semantics, `.gitignore`
//! respected (in a git repository, as rg does), binary files skipped, and rg's line format
//! (`path:line:text` with the path as searched, or `line:text` for a single file). Results are
//! sorted by path, so the same search always returns the same text. No process is spawned: two
//! spawns per grep cost about 110 ms locally and 345 ms in a container (harness_bench).

use std::path::Path;

use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;

/// Matching lines for `pattern` under `root.join(sub)`, at most `max`, filtered by `glob` (rg
/// `--glob` semantics) when non-empty. `Err` for an invalid pattern or glob.
pub fn grep_lines(
    root: &Path,
    sub: &str,
    pattern: &str,
    glob: &str,
    max: usize,
) -> Result<Vec<String>, String> {
    let matcher =
        RegexMatcher::new_line_matcher(pattern).map_err(|e| format!("invalid pattern: {e}"))?;
    let base = root.join(sub);
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .build();
    let mut out = Vec::new();

    if base.is_file() {
        let _ = searcher.search_path(
            &matcher,
            &base,
            UTF8(|n, line| {
                out.push(format!("{n}:{}", line.trim_end_matches(['\n', '\r'])));
                Ok(out.len() < max)
            }),
        );
        return Ok(out);
    }

    let mut walk = WalkBuilder::new(&base);
    if !glob.is_empty() {
        let mut ob = OverrideBuilder::new(&base);
        ob.add(glob).map_err(|e| format!("invalid glob: {e}"))?;
        walk.overrides(ob.build().map_err(|e| format!("invalid glob: {e}"))?);
    }
    walk.sort_by_file_path(|a, b| a.cmp(b));
    for entry in walk.build() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let rel = entry.path().strip_prefix(&base).unwrap_or(entry.path());
        let shown = if sub == "." {
            format!("./{}", rel.display())
        } else {
            format!("{}/{}", sub.trim_end_matches('/'), rel.display())
        };
        let _ = searcher.search_path(
            &matcher,
            entry.path(),
            UTF8(|n, line| {
                out.push(format!(
                    "{shown}:{n}:{}",
                    line.trim_end_matches(['\n', '\r'])
                ));
                Ok(out.len() < max)
            }),
        );
        if out.len() >= max {
            break;
        }
    }
    Ok(out)
}
