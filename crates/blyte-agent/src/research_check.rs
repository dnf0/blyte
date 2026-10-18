//! Check that a research report's code quotes and line citations match the files they cite.
//!
//! Research runs have no test suite to answer to, so a report can cite lines that don't exist
//! and quote code nobody wrote: on 2026-10-15 a run passed with 44 of its 442 quotes real. The
//! check is deliberately narrow, so a careful report always passes it:
//! - a citation is `path:N`, `path:N-M` or `path:N-M,K`, where `path` has a file extension or
//!   is a `Makefile`, `Dockerfile` and the like;
//! - a cited path must exist if it contains `/` or has a source-code extension, and every cited
//!   line must be inside the file;
//! - a fenced code block quotes the last citation on the nearest non-blank line before it, or on
//!   a first-line comment in the block. Failing that, a path named on that line (`In
//!   src/x.py:`, `` `x.toml` (lines 3-8) ``) is cited, as the whole file when no lines are given;
//! - a block of mostly numbered lines with no citation there quotes the report's most recent
//!   citation;
//! - blocks tagged `diff`, `patch` or `proposed` are not checked, and neither are commands,
//!   output and untagged plain blocks; any other block tagged with a language, or numbered, must
//!   have a citation;
//! - a quoted line may elide parts with `...`;
//! - a quoted line starting with a line number (as `read` and `nl -ba` print them) must equal
//!   that line of the file; any other quoted line must occur near the cited lines.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

/// Problems listed in full; the rest are counted.
pub const MAX_PROBLEMS: usize = 40;

/// Lines of slack around a cited range within which an unnumbered quoted line must occur.
const SLACK: usize = 5;

/// Bad lines shown per code block.
const EXAMPLES_PER_BLOCK: usize = 3;

/// Files a bare or relative name may match by suffix before it is treated as ambiguous noise.
const MAX_SUFFIX_MATCHES: usize = 20;

/// Maximum number of lines in one [`Evidence`] block.
const MAX_EVIDENCE_LINES: usize = 120;

/// Maximum number of [`Evidence`] blocks attached to one [`Claim`].
const MAX_EVIDENCE_BLOCKS: usize = 4;

/// One prose unit of a report (paragraph or list item) that cites at least one existing file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Claim {
    pub id: usize,
    pub text: String,
    pub quoted: Option<String>,
    pub evidence: Vec<Evidence>,
}

/// Numbered source lines around a citation that resolved to a file.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Evidence {
    pub path: String,
    pub start: usize,
    pub end: usize,
    pub lines: String,
}

/// Where cited files are looked for.
pub struct Sources<'a> {
    /// The run's workspace; relative paths resolve here first.
    pub workspace: &'a Path,
    /// Read roots; relative paths resolve under each in turn.
    pub read_roots: &'a [PathBuf],
    /// Absolute paths of the files in the workspace and read roots (`git ls-files`), for
    /// citations that give only the tail of a path.
    pub listing: &'a [PathBuf],
    /// Reads a file's text.
    pub read: &'a dyn Fn(&Path) -> Result<String, String>,
}

struct Citation {
    path: String,
    /// Empty for a whole file.
    ranges: Vec<(usize, usize)>,
    /// Named in a block's lead-in rather than cited as `path:N`, so not checked as prose.
    named: bool,
}

enum Resolved {
    Files(Vec<(PathBuf, Vec<String>)>),
    /// Not found, and not worth reporting (a bare name may not be a file at all).
    Skip,
    Missing(String),
}

fn citation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"((?:[\w./~-]*\.[A-Za-z][A-Za-z0-9_]*)|(?:[\w./~-]*/)?(?:{BARE_NAMES})):(\d+(?:-\d+)?(?:, ?\d+(?:-\d+)?)*)"
        ))
        .expect("citation regex")
    })
}

/// File names with no extension that are cited as they are.
const BARE_NAMES: &str =
    "Makefile|Dockerfile|Containerfile|Justfile|Gemfile|Rakefile|Procfile|Jenkinsfile|Vagrantfile";

/// Extensions of source files: a citation of one that resolves to nothing is reported even
/// without a `/` in it.
const CODE_EXTENSIONS: &[&str] = &[
    "py", "pyi", "rs", "go", "ts", "tsx", "js", "jsx", "mjs", "java", "kt", "scala", "rb", "php",
    "cs", "c", "h", "cc", "cpp", "hpp", "swift", "lua", "sh", "tf", "hcl", "sql", "yaml", "yml",
    "toml", "json", "ipynb",
];

/// Block tags that are not quotes of a file: commands, their output, and plain text.
const NOT_QUOTE_TAGS: &[&str] = &[
    "proposed",
    "diff",
    "patch",
    "bash",
    "sh",
    "shell",
    "zsh",
    "console",
    "terminal",
    "text",
    "txt",
    "plain",
    "plaintext",
    "output",
    "log",
    "csv",
    "tsv",
    "mermaid",
    "markdown",
    "md",
];

/// A path named without line numbers, for a code block's lead-in.
fn path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!(
            r"(?:^|[\s`(\[])((?:[\w.~-]+/)*(?:[\w.-]*\.[A-Za-z][A-Za-z0-9_]*|{BARE_NAMES}))(?:$|[\s`):,\]])"
        ))
        .expect("path regex")
    })
}

fn lines_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\blines?\s+(\d+)(?:\s*[-–]\s*(\d+))?").expect("lines regex"))
}

fn numbered_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*(\d+)(?:\t|: |:$|\s*\|\s?|\s{2,}|\s*→)(.*)$").expect("numbered regex")
    })
}

fn citations(line: &str) -> Vec<Citation> {
    let mut out = Vec::new();
    for caps in citation_re().captures_iter(line) {
        let m = caps.get(0).expect("whole match");
        let path = caps[1].trim_start_matches("./").to_string();
        let before = &line[..m.start()];
        // `https://host/x.py:12` is a URL, `a.py:3:4` is fine but `x:a.py:3` is not a path start.
        if before.ends_with(':') || before.ends_with(|c: char| c.is_alphanumeric() || c == '_') {
            continue;
        }
        if path.is_empty() || path.starts_with('~') || path.starts_with("..") {
            continue;
        }
        let mut ranges = Vec::new();
        for part in caps[2].split(',') {
            let part = part.trim();
            let (a, b) = match part.split_once('-') {
                Some((a, b)) => (a.parse().unwrap_or(0), b.parse().unwrap_or(0)),
                None => {
                    let n = part.parse().unwrap_or(0);
                    (n, n)
                }
            };
            if a > 0 {
                ranges.push((a, b.max(a)));
            }
        }
        if !ranges.is_empty() {
            out.push(Citation {
                path,
                ranges,
                named: false,
            });
        }
    }
    out
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A line standing for omitted code: `...`, `…`, `# ...`, `// ... rest`, `[...]`.
fn is_elision(line: &str) -> bool {
    let t = line
        .trim()
        .trim_start_matches(['#', '/', '-', '*', ';'])
        .trim_end_matches("*/")
        .trim();
    t.starts_with("...") || t.starts_with('…') || t.starts_with("[...]") || t.starts_with("[…]")
}

struct Resolver<'a> {
    src: &'a Sources<'a>,
    cache: HashMap<String, Resolved>,
}

impl<'a> Resolver<'a> {
    fn lines_of(&self, p: &Path) -> Option<Vec<String>> {
        (self.src.read)(p)
            .ok()
            .map(|t| t.lines().map(str::to_string).collect())
    }

    fn resolve(&mut self, path: &str) -> &Resolved {
        if !self.cache.contains_key(path) {
            let r = self.lookup(path);
            self.cache.insert(path.to_string(), r);
        }
        &self.cache[path]
    }

    fn lookup(&self, path: &str) -> Resolved {
        let p = Path::new(path);
        if p.is_absolute() {
            return match (self.src.read)(p) {
                Ok(t) => Resolved::Files(vec![(
                    p.to_path_buf(),
                    t.lines().map(str::to_string).collect(),
                )]),
                Err(e) => Resolved::Missing(format!("cannot read {path}: {e}")),
            };
        }
        // A read root's parent too: a report on several repositories cites `repo/src/x.py`.
        let bases = std::iter::once(self.src.workspace)
            .chain(self.src.read_roots.iter().map(|r| r.as_path()))
            .chain(self.src.read_roots.iter().filter_map(|r| r.parent()));
        for base in bases {
            let candidate = base.join(p);
            if let Some(lines) = self.lines_of(&candidate) {
                return Resolved::Files(vec![(candidate, lines)]);
            }
        }
        let suffix = format!("/{path}");
        let mut hits: Vec<&PathBuf> = self
            .src
            .listing
            .iter()
            .filter(|f| f.to_string_lossy().ends_with(&suffix))
            .collect();
        hits.dedup();
        if !hits.is_empty() && hits.len() <= MAX_SUFFIX_MATCHES {
            let files: Vec<_> = hits
                .into_iter()
                .filter_map(|f| self.lines_of(f).map(|l| (f.clone(), l)))
                .collect();
            if !files.is_empty() {
                return Resolved::Files(files);
            }
        } else if !hits.is_empty() {
            return Resolved::Skip;
        }
        let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        if path.contains('/') || ext.is_some_and(|e| CODE_EXTENSIONS.contains(&e.as_str())) {
            Resolved::Missing(format!("no such file: {path}"))
        } else {
            Resolved::Skip
        }
    }
}

fn excerpt(s: &str) -> String {
    let t = s.trim();
    if t.chars().count() > 100 {
        format!("{}…", t.chars().take(100).collect::<String>())
    } else {
        t.to_string()
    }
}

fn cite_text(c: &Citation) -> String {
    let ranges: Vec<String> = c
        .ranges
        .iter()
        .map(|&(a, b)| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}-{b}")
            }
        })
        .collect();
    if ranges.is_empty() {
        c.path.clone()
    } else {
        format!("{}:{}", c.path, ranges.join(","))
    }
}

/// Where `text` occurs in `lines`, 1-based, or None.
fn find_line(lines: &[String], text: &str) -> Option<usize> {
    lines
        .iter()
        .position(|l| normalize(l) == text)
        .map(|i| i + 1)
}

/// `quoted` (normalized) is `file_line`, a long enough part of it, or `file_line` with parts
/// elided as `...`: `run(ti, ...)` matches `run(ti, deps, retries=3)`.
fn matches_line(file_line: &str, quoted: &str) -> bool {
    let f = normalize(file_line);
    if f == quoted || (quoted.chars().count() >= 12 && f.contains(quoted)) {
        return true;
    }
    if !quoted.contains("...") && !quoted.contains('…') {
        return false;
    }
    let mut rest = f.as_str();
    for part in quoted.split('…').flat_map(|p| p.split("...")) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// Why one quoted line does not match any of `files`, or None if it matches one of them.
fn check_quoted_line(
    files: &[(PathBuf, Vec<String>)],
    windows: &[(usize, usize)],
    raw: &str,
) -> Option<String> {
    let whole = normalize(raw);
    let numbered = numbered_re()
        .captures(raw)
        .and_then(|c| Some((c[1].parse::<usize>().ok()?, normalize(&c[2]))));
    let near = |lines: &[String], text: &str| {
        windows.iter().any(|&(lo, hi)| {
            lines
                .iter()
                .enumerate()
                .skip(lo - 1)
                .take(hi + 1 - lo)
                .any(|(_, l)| matches_line(l, text))
        })
    };
    for (_, lines) in files {
        if let Some((n, text)) = &numbered {
            if lines
                .get(n.wrapping_sub(1))
                .is_some_and(|l| matches_line(l, text))
            {
                return None;
            }
        }
        if near(lines, &whole) {
            return None;
        }
    }
    let (_, lines) = &files[0];
    match &numbered {
        Some((n, text)) => {
            let actual = match lines.get(n.wrapping_sub(1)) {
                Some(l) => format!("line {n} is `{}`", excerpt(l)),
                None => format!("the file has {} lines", lines.len()),
            };
            let elsewhere = find_line(lines, text)
                .map(|at| format!("; the quoted text is at line {at}"))
                .unwrap_or_else(|| "; the quoted text is not in the file".to_string());
            Some(format!("quoted `{}`: {actual}{elsewhere}", excerpt(raw)))
        }
        None => {
            let elsewhere = find_line(lines, &whole)
                .map(|at| format!("it is at line {at}, outside the cited lines"))
                .unwrap_or_else(|| "it is not in the file".to_string());
            Some(format!("quoted `{}`: {elsewhere}", excerpt(raw)))
        }
    }
}

/// Check `report`; returns one entry per problem, in report order, prefixed with its line.
pub fn check(report: &str, src: &Sources<'_>) -> Vec<String> {
    let mut resolver = Resolver {
        src,
        cache: HashMap::new(),
    };
    let mut problems = Vec::new();
    let mut last_text: Option<(usize, String)> = None;
    let mut last_cite: Option<Citation> = None;
    let lines: Vec<&str> = report.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        let fence = if trimmed.starts_with("```") {
            "```"
        } else if trimmed.starts_with("~~~") {
            "~~~"
        } else {
            ""
        };
        if fence.is_empty() {
            if !line.trim().is_empty() {
                check_prose(&mut resolver, i + 1, line, &mut problems);
                if let Some(c) = citations(line).pop() {
                    last_cite = Some(c);
                }
                last_text = Some((i + 1, line.to_string()));
            }
            i += 1;
            continue;
        }
        let marker_len = trimmed
            .chars()
            .take_while(|&c| c == fence.chars().next().unwrap())
            .count();
        let info = trimmed[marker_len..].trim().to_lowercase();
        let close = fence.chars().next().unwrap().to_string().repeat(marker_len);
        let start = i + 1;
        let mut end = start;
        while end < lines.len() && !lines[end].trim_start().starts_with(&close) {
            end += 1;
        }
        let mut body = &lines[start..end.min(lines.len())];
        let lang = info.split_whitespace().next().unwrap_or("");
        let lead_in = last_text.take().map(|(_, t)| t);
        let mut anchor = lead_in.as_deref().and_then(|t| citations(t).pop());
        // A block may name its source in a first-line comment: `# path/x.py:34-39`.
        let first = body.iter().position(|l| !l.trim().is_empty());
        if let Some(f) = first {
            let head = body[f].trim_start();
            if ["#", "//", "--", "/*"].iter().any(|c| head.starts_with(c)) {
                if let Some(c) = citations(head).pop() {
                    anchor = Some(c);
                    body = &body[f + 1..];
                }
            }
        }
        if anchor.is_none() {
            anchor = lead_in.as_deref().and_then(named_path);
        }
        // `In line 415:` names lines of the file cited last.
        if anchor.is_none() {
            if let (Some(t), Some(prev)) = (lead_in.as_deref(), &last_cite) {
                if let Some(ranges) = lead_in_lines(t) {
                    anchor = Some(Citation {
                        path: prev.path.clone(),
                        ranges,
                        named: true,
                    });
                }
            }
        }
        let numbered = mostly_numbered(body);
        // Numbered lines are a quote whatever precedes them: they come from the most recent
        // citation when the lead-in has none.
        let anchor = match anchor {
            Some(c) => Some(c),
            None if numbered => last_cite.take(),
            None => None,
        };
        let is_quote = !NOT_QUOTE_TAGS.contains(&lang) && (!lang.is_empty() || numbered);
        match &anchor {
            Some(cite) if !matches!(lang, "diff" | "patch" | "proposed") => {
                check_block(&mut resolver, start, cite, body, &mut problems)
            }
            None if is_quote && body.iter().any(|l| !l.trim().is_empty()) => {
                let tag = if lang.is_empty() { "numbered" } else { lang };
                problems.push(format!(
                    "report.md:{start}: {tag} code block with no citation: put `path:N-M` on the \
                     line before it if it is quoted from a file, or tag it ```proposed if it is \
                     not in the repository"
                ));
            }
            _ => {}
        }
        if anchor.is_some() {
            last_cite = anchor;
        }
        i = end + 1;
    }
    problems
}

/// The last path named on a lead-in line, with the lines given as `line N` or `lines N-M`, or
/// the whole file when none are given.
fn named_path(line: &str) -> Option<Citation> {
    let path = path_re()
        .captures_iter(line)
        .map(|c| c[1].trim_start_matches("./").to_string())
        .filter(|p| !p.starts_with('~') && !p.starts_with(".."))
        .filter(|p| p.contains('/') || line.contains(&format!("`{p}`")) || p.ends_with("file"))
        .last()?;
    let ranges = lead_in_lines(line).unwrap_or_default();
    Some(Citation {
        path,
        ranges,
        named: true,
    })
}

/// Lines given as `line N` or `lines N-M` on a lead-in line.
fn lead_in_lines(line: &str) -> Option<Vec<(usize, usize)>> {
    let c = lines_re().captures(line)?;
    let a: usize = c[1].parse().ok()?;
    let b = c.get(2).and_then(|m| m.as_str().parse().ok()).unwrap_or(a);
    (a > 0).then_some(vec![(a, b.max(a))])
}

/// At least half the block's quoted lines start with a line number.
fn mostly_numbered(body: &[&str]) -> bool {
    let lines: Vec<&&str> = body
        .iter()
        .filter(|l| !l.trim().is_empty() && !is_elision(l))
        .collect();
    let numbered = lines.iter().filter(|l| numbered_re().is_match(l)).count();
    !lines.is_empty() && numbered * 2 >= lines.len()
}

fn check_prose(resolver: &mut Resolver<'_>, at: usize, line: &str, problems: &mut Vec<String>) {
    for cite in citations(line) {
        match resolver.resolve(&cite.path) {
            Resolved::Skip => {}
            Resolved::Missing(why) => {
                problems.push(format!("report.md:{at}: {}: {why}", cite_text(&cite)))
            }
            Resolved::Files(files) => {
                let last = cite.ranges.iter().map(|&(_, b)| b).max().unwrap_or(0);
                if files.iter().all(|(_, l)| last > l.len()) {
                    let (p, l) = &files[0];
                    problems.push(format!(
                        "report.md:{at}: {}: line {last} is past the end of {} ({} lines)",
                        cite_text(&cite),
                        p.display(),
                        l.len()
                    ));
                }
            }
        }
    }
}

fn check_block(
    resolver: &mut Resolver<'_>,
    start: usize,
    cite: &Citation,
    body: &[&str],
    problems: &mut Vec<String>,
) {
    let files = match resolver.resolve(&cite.path) {
        Resolved::Files(f) => f,
        // A missing `path:N` is already reported for the line that cites it.
        Resolved::Missing(why) if cite.named => {
            let why = why.clone();
            problems.push(format!(
                "report.md:{start}: code block quoting {}: {why}; tag the block ```proposed if \
                 it is a file you propose",
                cite.path
            ));
            return;
        }
        _ => return,
    };
    let quoted: Vec<&str> = body
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty() && !is_elision(l))
        .collect();
    if quoted.is_empty() {
        return;
    }
    // A citation without lines quotes anywhere in the file.
    let windows: Vec<(usize, usize)> = if cite.ranges.is_empty() {
        vec![(1, usize::MAX / 2)]
    } else {
        cite.ranges
            .iter()
            .map(|&(a, b)| {
                (
                    a.saturating_sub(SLACK).max(1),
                    b.max(a + body.len()) + SLACK,
                )
            })
            .collect()
    };
    let bad: Vec<String> = quoted
        .iter()
        .filter_map(|l| check_quoted_line(files, &windows, l))
        .collect();
    if bad.is_empty() {
        return;
    }
    let mut msg = format!(
        "report.md:{start}: code block quoting {}: {} of {} lines do not match the file",
        cite_text(cite),
        bad.len(),
        quoted.len()
    );
    for b in bad.iter().take(EXAMPLES_PER_BLOCK) {
        msg.push_str("\n    ");
        msg.push_str(b);
    }
    problems.push(msg);
}

/// The gate output for a list of problems.
pub fn render(problems: &[String]) -> String {
    let mut out = format!(
        "report.md quote check: {} problem(s). A code block right after a `path:line` citation \
         must be copied from that file, and cited lines must exist.\n",
        problems.len()
    );
    for p in problems.iter().take(MAX_PROBLEMS) {
        out.push_str("- ");
        out.push_str(p);
        out.push('\n');
    }
    if problems.len() > MAX_PROBLEMS {
        out.push_str(&format!(
            "- ... and {} more\n",
            problems.len() - MAX_PROBLEMS
        ));
    }
    out.push_str(
        "Re-read each cited file with the read tool and quote it exactly, correct the line \
         numbers, or remove the claim. Mark code you propose rather than quote as ```proposed.",
    );
    out
}

fn is_list_item(trimmed: &str) -> bool {
    if trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("+ ")
        || trimmed.starts_with("-\t")
        || trimmed.starts_with("*\t")
        || trimmed.starts_with("+\t")
    {
        return true;
    }
    let digits = trimmed.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &trimmed[digits..];
        if rest.starts_with(". ")
            || rest.starts_with(") ")
            || rest.starts_with(".\t")
            || rest.starts_with(")\t")
        {
            return true;
        }
    }
    false
}

fn flush_claim_unit(
    unit_lines: &mut Vec<&str>,
    resolver: &mut Resolver<'_>,
    last_path: &mut Option<String>,
    out: &mut Vec<Claim>,
    attach_quote_to_last: &mut bool,
) {
    if unit_lines.is_empty() {
        return;
    }
    let text = unit_lines.join("\n").trim().to_string();
    unit_lines.clear();
    if text.is_empty() {
        return;
    }

    let mut cites = Vec::new();
    for line in text.lines() {
        let line_cites = citations(line);
        if !line_cites.is_empty() {
            cites.extend(line_cites);
        } else if let Some(c) = named_path(line) {
            if !c.ranges.is_empty() || cites.is_empty() {
                cites.push(c);
            }
        }
    }
    if cites.is_empty() {
        if let Some(c) = named_path(&text) {
            cites.push(c);
        } else if let (Some(ranges), Some(prev)) = (lead_in_lines(&text), last_path.as_deref()) {
            cites.push(Citation {
                path: prev.to_string(),
                ranges,
                named: true,
            });
        }
    }

    let mut evidence = Vec::new();
    for cite in cites {
        let Resolved::Files(files) = resolver.resolve(&cite.path) else {
            continue;
        };
        let Some((_, file_lines)) = files.first() else {
            continue;
        };
        let total = file_lines.len();
        if total == 0 {
            continue;
        }
        *last_path = Some(cite.path.clone());
        let ranges = if cite.ranges.is_empty() {
            vec![(1, total)]
        } else {
            cite.ranges
        };
        for (a, b) in ranges {
            if evidence.len() >= MAX_EVIDENCE_BLOCKS {
                break;
            }
            let cite_start = a.clamp(1, total);
            let cite_end = b.max(a).clamp(1, total);
            let mut start = cite_start.saturating_sub(SLACK).max(1);
            let mut end = (cite_end + SLACK).min(total);
            while end - start + 1 > MAX_EVIDENCE_LINES {
                if end > cite_end {
                    end -= 1;
                } else if start < cite_start {
                    start += 1;
                } else {
                    end = start + MAX_EVIDENCE_LINES - 1;
                    break;
                }
            }
            let lines = (start..=end)
                .map(|n| format!("{n}\t{}", file_lines[n - 1]))
                .collect::<Vec<_>>()
                .join("\n");
            evidence.push(Evidence {
                path: cite.path.clone(),
                start,
                end,
                lines,
            });
        }
        if evidence.len() >= MAX_EVIDENCE_BLOCKS {
            break;
        }
    }

    if !evidence.is_empty() {
        let id = out.len() + 1;
        out.push(Claim {
            id,
            text,
            quoted: None,
            evidence,
        });
        *attach_quote_to_last = true;
    } else {
        *attach_quote_to_last = false;
    }
}

/// Extract prose claims (paragraphs and list items outside fenced code blocks) that cite at least
/// one resolving file, together with any immediately following fenced code block and widened
/// numbered evidence lines.
pub fn claims(report: &str, src: &Sources<'_>) -> Vec<Claim> {
    let mut resolver = Resolver {
        src,
        cache: HashMap::new(),
    };
    let mut out = Vec::new();
    let mut unit_lines: Vec<&str> = Vec::new();
    let mut last_path: Option<String> = None;
    let mut attach_quote_to_last = false;

    let lines: Vec<&str> = report.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        let fence = if trimmed.starts_with("```") {
            "```"
        } else if trimmed.starts_with("~~~") {
            "~~~"
        } else {
            ""
        };
        if !fence.is_empty() {
            flush_claim_unit(
                &mut unit_lines,
                &mut resolver,
                &mut last_path,
                &mut out,
                &mut attach_quote_to_last,
            );
            let marker_len = trimmed
                .chars()
                .take_while(|&c| c == fence.chars().next().unwrap())
                .count();
            let close = fence.chars().next().unwrap().to_string().repeat(marker_len);
            let start = i + 1;
            let mut end = start;
            while end < lines.len() && !lines[end].trim_start().starts_with(&close) {
                end += 1;
            }
            let body = &lines[start..end.min(lines.len())];
            if attach_quote_to_last {
                if let Some(last_claim) = out.last_mut() {
                    last_claim.quoted = Some(body.join("\n"));
                }
                attach_quote_to_last = false;
            }
            i = end + 1;
            continue;
        }

        if trimmed.is_empty() {
            flush_claim_unit(
                &mut unit_lines,
                &mut resolver,
                &mut last_path,
                &mut out,
                &mut attach_quote_to_last,
            );
            i += 1;
            continue;
        }

        if trimmed.starts_with('#') {
            flush_claim_unit(
                &mut unit_lines,
                &mut resolver,
                &mut last_path,
                &mut out,
                &mut attach_quote_to_last,
            );
            attach_quote_to_last = false;
            i += 1;
            continue;
        }

        if is_list_item(trimmed) {
            flush_claim_unit(
                &mut unit_lines,
                &mut resolver,
                &mut last_path,
                &mut out,
                &mut attach_quote_to_last,
            );
            unit_lines.push(line);
            i += 1;
            continue;
        }

        unit_lines.push(line);
        i += 1;
    }

    flush_claim_unit(
        &mut unit_lines,
        &mut resolver,
        &mut last_path,
        &mut out,
        &mut attach_quote_to_last,
    );
    out
}
