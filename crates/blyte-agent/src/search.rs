//! `grep` in-process, with ripgrep's own libraries: rg's regex engine and semantics, `.gitignore`
//! respected (in a git repository, as rg does), binary files skipped, and rg's line format
//! (`path:line:text` with the path as searched, or `line:text` for a single file). Results are
//! sorted by path, so the same search always returns the same text. No process is spawned: two
//! spawns per grep cost about 110 ms locally and 345 ms in a container (harness_bench).

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use grep_regex::RegexMatcher;
use grep_searcher::sinks::UTF8;
use grep_searcher::{BinaryDetection, SearcherBuilder};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;

pub(crate) struct SearchOutput {
    pub(crate) hits: Vec<String>,
    pub(crate) skipped: usize,
}

/// Matching lines for `pattern` under `root.join(sub)`, at most `max`, filtered by `glob` (rg
/// `--glob` semantics) when non-empty. `Err` for an invalid pattern or glob.
pub fn grep_lines(
    root: &Path,
    sub: &str,
    pattern: &str,
    glob: &str,
    max: usize,
) -> Result<Vec<String>, String> {
    Ok(grep_lines_checked(root, sub, pattern, glob, max)?.hits)
}

pub(crate) fn grep_lines_checked(
    root: &Path,
    sub: &str,
    pattern: &str,
    glob: &str,
    max: usize,
) -> Result<SearchOutput, String> {
    let matcher =
        RegexMatcher::new_line_matcher(pattern).map_err(|e| format!("invalid pattern: {e}"))?;
    let base = root.join(sub);
    let protected = Arc::new(crate::sandbox::protected_read_paths()?);
    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .build();
    let mut out = Vec::new();

    if base.is_file() {
        if crate::sandbox::is_protected_read_with(&base, &protected)? {
            return Ok(SearchOutput {
                hits: Vec::new(),
                skipped: 1,
            });
        }
        let _ = searcher.search_path(
            &matcher,
            &base,
            UTF8(|n, line| {
                out.push(format!("{n}:{}", line.trim_end_matches(['\n', '\r'])));
                Ok(out.len() < max)
            }),
        );
        return Ok(SearchOutput {
            hits: out,
            skipped: 0,
        });
    }

    let skipped = Arc::new(AtomicUsize::new(0));
    let walk_err: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let protected_filter = Arc::clone(&protected);
    let skipped_filter = Arc::clone(&skipped);
    let err_filter = Arc::clone(&walk_err);

    let mut walk = WalkBuilder::new(&base);
    let overrides = if !glob.is_empty() {
        let mut ob = OverrideBuilder::new(&base);
        ob.add(glob).map_err(|e| format!("invalid glob: {e}"))?;
        let built = ob.build().map_err(|e| format!("invalid glob: {e}"))?;
        walk.overrides(built.clone());
        Some(built)
    } else {
        None
    };
    walk.hidden(false).sort_by_file_path(|a, b| a.cmp(b));
    walk.filter_entry(move |e| {
        if e.depth() == 0 {
            return true;
        }
        if err_filter
            .lock()
            .unwrap_or_else(|er| er.into_inner())
            .is_some()
        {
            return false;
        }
        match crate::sandbox::is_protected_read_with(e.path(), &protected_filter) {
            Ok(true) => {
                skipped_filter.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            Ok(false) => {}
            Err(err) => {
                let mut slot = err_filter.lock().unwrap_or_else(|er| er.into_inner());
                if slot.is_none() {
                    *slot = Some(err);
                }
                return false;
            }
        }
        if e.file_name().as_encoded_bytes().starts_with(b".") {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            let whitelisted = overrides
                .as_ref()
                .is_some_and(|ov| ov.matched(e.path(), is_dir).is_whitelist());
            if !whitelisted {
                return false;
            }
        }
        true
    });
    for entry in walk.build() {
        if let Some(err) = walk_err.lock().unwrap_or_else(|er| er.into_inner()).clone() {
            return Err(err);
        }
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
    if let Some(err) = walk_err.lock().unwrap_or_else(|er| er.into_inner()).take() {
        return Err(err);
    }
    Ok(SearchOutput {
        hits: out,
        skipped: skipped.load(Ordering::Relaxed),
    })
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
    glob_paths_checked(root, pattern, max)
        .ok()
        .flatten()
        .map(|res| res.hits)
}

pub(crate) fn glob_paths_checked(
    root: &Path,
    pattern: &str,
    max: usize,
) -> Result<Option<SearchOutput>, String> {
    let Some(matcher) = glob_matcher(pattern) else {
        return Ok(None);
    };
    let protected = Arc::new(crate::sandbox::protected_read_paths()?);
    if !is_meta(pattern) {
        let candidate = root.join(pattern);
        match std::fs::symlink_metadata(&candidate) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Some(SearchOutput {
                    hits: Vec::new(),
                    skipped: 0,
                }));
            }
            Err(e) => return Err(format!("cannot stat {}: {e}", candidate.display())),
        }
        if crate::sandbox::is_protected_read_with(&candidate, &protected)? {
            return Ok(Some(SearchOutput {
                hits: Vec::new(),
                skipped: 1,
            }));
        }
        return Ok(Some(SearchOutput {
            hits: if candidate.exists() {
                vec![pattern.to_string()]
            } else {
                Vec::new()
            },
            skipped: 0,
        }));
    }
    let comps: Vec<&str> = pattern.split('/').collect();
    let lit = comps[..comps.len() - 1]
        .iter()
        .take_while(|c| !is_meta(c))
        .count();
    let base = comps[..lit]
        .iter()
        .fold(root.to_path_buf(), |p, c| p.join(c));
    match std::fs::symlink_metadata(&base) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some(SearchOutput {
                hits: Vec::new(),
                skipped: 0,
            }));
        }
        Err(e) => return Err(format!("cannot stat {}: {e}", base.display())),
    }
    if lit > 0 && crate::sandbox::is_protected_read_with(&base, &protected)? {
        return Ok(Some(SearchOutput {
            hits: Vec::new(),
            skipped: 1,
        }));
    }
    if !base.is_dir() {
        return Ok(Some(SearchOutput {
            hits: Vec::new(),
            skipped: 0,
        }));
    }
    let skipped = Arc::new(AtomicUsize::new(0));
    let walk_err: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let protected_filter = Arc::clone(&protected);
    let skipped_filter = Arc::clone(&skipped);
    let err_filter = Arc::clone(&walk_err);

    let mut walk = WalkBuilder::new(&base);
    walk.hidden(false)
        .follow_links(false)
        .filter_entry(move |e| {
            if e.depth() == 0 {
                return true;
            }
            if e.file_name() == ".git" {
                return false;
            }
            if err_filter
                .lock()
                .unwrap_or_else(|er| er.into_inner())
                .is_some()
            {
                return false;
            }
            match crate::sandbox::is_protected_read_with(e.path(), &protected_filter) {
                Ok(true) => {
                    skipped_filter.fetch_add(1, Ordering::Relaxed);
                    false
                }
                Ok(false) => true,
                Err(err) => {
                    let mut slot = err_filter.lock().unwrap_or_else(|er| er.into_inner());
                    if slot.is_none() {
                        *slot = Some(err);
                    }
                    false
                }
            }
        });
    if !pattern.contains("**") {
        walk.max_depth(Some(comps.len() - lit));
    }
    let mut hits: Vec<String> = Vec::new();
    for entry in walk.build() {
        if let Some(err) = walk_err.lock().unwrap_or_else(|er| er.into_inner()).clone() {
            return Err(err);
        }
        let Ok(e) = entry else { continue };
        if e.depth() == 0 {
            continue;
        }
        let Ok(rel) = e.path().strip_prefix(root) else {
            continue;
        };
        if matcher.is_match(rel) {
            hits.push(rel.to_string_lossy().into_owned());
        }
    }
    if let Some(err) = walk_err.lock().unwrap_or_else(|er| er.into_inner()).take() {
        return Err(err);
    }
    hits.sort();
    hits.truncate(max);
    Ok(Some(SearchOutput {
        hits,
        skipped: skipped.load(Ordering::Relaxed),
    }))
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
