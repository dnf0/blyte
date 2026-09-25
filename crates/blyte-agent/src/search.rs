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

/// Paths matching a shell glob, as bash expands it with `globstar`, `dotglob` and `nullglob`,
/// relative to `root`: `**` crosses directories, `*` matches dotfiles, directories match too, and
/// symlinked directories are not followed. `None` for patterns this does not emulate exactly
/// (absolute, `.` or `..` components, or a trailing `/`); callers send those to the shell.
/// Sorted bytewise, at most `max`. No process is spawned: a shell glob cost 26 ms locally and
/// 84 ms in a container (harness_bench).
pub fn glob_paths(root: &Path, pattern: &str, max: usize) -> Option<Vec<String>> {
    if pattern.is_empty()
        || pattern.starts_with('/')
        || pattern.ends_with('/')
        || pattern.split('/').any(|c| c == "." || c == "..")
    {
        return None;
    }
    let is_meta = |c: &str| c.contains(['*', '?', '[', '{']);
    if !is_meta(pattern) {
        return Some(if root.join(pattern).exists() {
            vec![pattern.to_string()]
        } else {
            Vec::new()
        });
    }
    let matcher = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
        .ok()?
        .compile_matcher();
    // Walk from the pattern's literal leading directories, only as deep as it can reach.
    let comps: Vec<&str> = pattern.split('/').collect();
    let lit = comps[..comps.len() - 1]
        .iter()
        .take_while(|c| !is_meta(c))
        .count();
    let base = comps[..lit]
        .iter()
        .fold(root.to_path_buf(), |p, c| p.join(c));
    if !base.is_dir() {
        return Some(Vec::new());
    }
    let mut walk = WalkBuilder::new(&base);
    walk.standard_filters(false)
        .hidden(false)
        .follow_links(false);
    if !pattern.contains("**") {
        walk.max_depth(Some(comps.len() - lit));
    }
    let mut hits: Vec<String> = walk
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() > 0)
        .filter_map(|e| {
            let rel = e.path().strip_prefix(root).ok()?.to_path_buf();
            matcher
                .is_match(&rel)
                .then(|| rel.to_string_lossy().into_owned())
        })
        .collect();
    hits.sort();
    hits.truncate(max);
    Some(hits)
}
