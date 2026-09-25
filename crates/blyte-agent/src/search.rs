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

/// The glob matcher shared by local and container globs. `None` for patterns not handled here
/// (absolute, `.` or `..` components, a trailing `/`); callers send those to the shell.
fn glob_matcher(pattern: &str) -> Option<globset::GlobMatcher> {
    if pattern.is_empty()
        || pattern.starts_with('/')
        || pattern.ends_with('/')
        || pattern.split('/').any(|c| c == "." || c == "..")
    {
        return None;
    }
    Some(
        globset::GlobBuilder::new(pattern)
            .literal_separator(true)
            .backslash_escape(true)
            .build()
            .ok()?
            .compile_matcher(),
    )
}

fn is_meta(s: &str) -> bool {
    s.contains(['*', '?', '[', '{'])
}

/// Paths under `root` matching a glob, fd/ripgrep style: `.gitignore`d files (in a git
/// repository), `.ignore` rules and `.git` are skipped; other dotfiles are kept; `**` crosses
/// directories; directories match too; symlinks are not followed. Sorted bytewise, at most `max`.
/// `None` for patterns not handled here. A bash `**` walked `target/` and `node_modules`: 273 ms
/// for `**/*.rs` at a Rust repository's root, and results full of build artefacts.
pub fn glob_paths(root: &Path, pattern: &str, max: usize) -> Option<Vec<String>> {
    let matcher = glob_matcher(pattern)?;
    if !is_meta(pattern) {
        return Some(if root.join(pattern).exists() {
            vec![pattern.to_string()]
        } else {
            Vec::new()
        });
    }
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
    walk.hidden(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git");
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

/// Paths from a newline-separated file listing (e.g. `git ls-files`) matching a glob, with the
/// listed files' ancestor directories as candidates too, so `src/*` still shows `src/deep`. The
/// container route: one command lists the repository, matching happens here with the same rules
/// as [`glob_paths`]. Sorted bytewise, at most `max`. `None` for patterns not handled here.
pub fn glob_filter(listing: &str, pattern: &str, max: usize) -> Option<Vec<String>> {
    let matcher = glob_matcher(pattern)?;
    let mut candidates = std::collections::BTreeSet::new();
    for line in listing.lines().map(str::trim_end).filter(|l| !l.is_empty()) {
        let mut p = Path::new(line);
        candidates.insert(line.to_string());
        while let Some(parent) = p.parent() {
            if parent.as_os_str().is_empty() {
                break;
            }
            candidates.insert(parent.to_string_lossy().into_owned());
            p = parent;
        }
    }
    let mut hits: Vec<String> = candidates
        .into_iter()
        .filter(|c| matcher.is_match(Path::new(c)))
        .collect();
    hits.truncate(max);
    Some(hits)
}
