//! The tool surface, and the one rule that shapes it: **everything executes synchronously**.
//!
//! There is no `run_in_background`, no job registry and no `job_output`. That is the single
//! most expensive measured failure mode in this project's history: EXP-081 spent 47.6% of all
//! input tokens on turns whose entire response was a variant of "the task is running in the
//! background, please wait", and the most costly turn ever recorded -- 1,384,188 input tokens
//! against 15,327,627 cached -- said exactly that and nothing else (FINDINGS 13.29).
//!
//! A slow test suite should cost wall-clock, which is free, instead of turns, which are not.
//! Blocking makes the failure unrepresentable rather than merely discouraged.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use blyte_provider::ToolSpec;
use serde_json::{json, Value};

use crate::approval::{matching_rule, Approval, ApprovalConfig};
use crate::fs::Fs;
use crate::shell::{poll_backoff, Shell, POLL_START};

pub trait PeerTools: Send + Sync {
    /// The running surfaces on this machine, as text for the model.
    fn surfaces(&self) -> Result<String, String>;
    /// Sends `message` to the surface named `name`; the text says delivered or held.
    fn tell(&self, name: &str, message: &str) -> Result<String, String>;
}

pub trait PublishTool: Send + Sync {
    fn publish(&self, request: &str, title: &str, body: &str) -> Result<String, String>;
}

pub const PUBLISH_TITLE_MAX_CHARS: usize = 200;
pub const PUBLISH_BODY_MAX_BYTES: usize = 20_000;

static READ_ROOTS: std::sync::RwLock<Vec<PathBuf>> = std::sync::RwLock::new(Vec::new());
static DISPATCH_CMD: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);
static BLYTE_TOOL: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);
static BLYTE_TASKS_DIR: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);
static BLYTE_RUN_STORE: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);
static IN_PLACE_LOCK_PATH: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);
static BLYTE_SURFACE: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);
static APPROVAL: std::sync::RwLock<Option<ApprovalConfig>> = std::sync::RwLock::new(None);
static PEER_TOOLS: std::sync::RwLock<Option<Arc<dyn PeerTools>>> = std::sync::RwLock::new(None);
static PUBLISH_TOOL: std::sync::RwLock<Option<Arc<dyn PublishTool>>> = std::sync::RwLock::new(None);
static WRITE_SCOPE: std::sync::RwLock<Option<WriteScope>> = std::sync::RwLock::new(None);
thread_local! {
    static BLYTE_REQUEST: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
    static BLYTE_STARTED_BY: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Absolute, canonical directories where `write` and `edit` may write without user approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteScope {
    pub allowed: Vec<PathBuf>,
    pub edit_worktrees: Option<PathBuf>,
}

fn refused_blyte_tool_command(args: &[String]) -> Option<&'static str> {
    let first = args.first().map(|s| s.as_str())?;
    if first == "combine" {
        return Some("combine");
    }
    if first == "review" {
        let mut iter = args[1..].iter();
        while let Some(arg) = iter.next() {
            if arg == "--" {
                break;
            }
            if arg == "--model-review" {
                return Some("review --model-review");
            }
            let verdict_val = if arg == "--verdict" {
                iter.next().map(|s| s.as_str())
            } else {
                arg.strip_prefix("--verdict=")
            };
            if let Some(val) = verdict_val {
                if val.eq_ignore_ascii_case("accepted") {
                    return Some("review --verdict accepted");
                }
                if val.eq_ignore_ascii_case("fixed") {
                    return Some("review --verdict fixed");
                }
            }
        }
    }
    None
}

pub const BLYTE_TOOL_COMMANDS: [&str; 9] = [
    "dispatch",
    "wait",
    "status",
    "review",
    "plan",
    "init",
    "clean",
    "edit-start",
    "edit-finish",
];

pub const BLYTE_TOOL_NO_REPO_COMMANDS: [&str; 4] = ["dispatch", "wait", "status", "review"];

const NO_REPO_REFUSED_COMMANDS: [&str; 6] =
    ["plan", "init", "clean", "edit-start", "edit-finish", "undo"];

const DISPATCH_VALUE_FLAGS: [&str; 20] = [
    "-g",
    "--gate",
    "--tag",
    "--plan",
    "--plan-task",
    "--request",
    "--surface",
    "--started-by",
    "--base",
    "--report-to",
    "-m",
    "--model",
    "--max-seconds",
    "--max-turns",
    "--max-cost",
    "--fold-budget",
    "--sandbox",
    "--network",
    "--allow-read",
    "--pass-env",
];

pub const REFUSED_DISPATCH_OPTIONS: [&str; 6] = [
    "--sandbox",
    "--network",
    "--allow-read",
    "--pass-env",
    "--gate",
    "--no-path-check",
];

pub fn refused_dispatch_option_message(option: &str) -> String {
    format!("do not pass {option}; only the user can loosen a run's sandbox or replace its checks")
}

fn refused_dispatch_option(args: &[String]) -> Option<&'static str> {
    for arg in args.iter().skip(1) {
        for &opt in &REFUSED_DISPATCH_OPTIONS {
            if arg == opt
                || arg
                    .strip_prefix(opt)
                    .is_some_and(|rest| rest.starts_with('='))
            {
                return Some(opt);
            }
        }
        if arg.starts_with("-g") {
            return Some("-g");
        }
    }
    None
}

pub fn set_blyte_request(request: Option<String>) {
    BLYTE_REQUEST.with(|slot| {
        *slot.borrow_mut() = request;
    });
}

pub fn blyte_request() -> Option<String> {
    BLYTE_REQUEST.with(|slot| slot.borrow().clone())
}

pub fn set_blyte_started_by(name: Option<String>) {
    BLYTE_STARTED_BY.with(|slot| {
        *slot.borrow_mut() = name;
    });
}

pub fn blyte_started_by() -> Option<String> {
    BLYTE_STARTED_BY.with(|slot| slot.borrow().clone())
}

pub fn set_blyte_tool(exe: Option<std::path::PathBuf>) {
    match BLYTE_TOOL.write() {
        Ok(mut lock) => *lock = exe,
        Err(poisoned) => *poisoned.into_inner() = exe,
    }
}

pub fn blyte_tool() -> Option<PathBuf> {
    BLYTE_TOOL
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

pub fn set_blyte_tasks_dir(dir: Option<PathBuf>) {
    match BLYTE_TASKS_DIR.write() {
        Ok(mut lock) => *lock = dir,
        Err(poisoned) => *poisoned.into_inner() = dir,
    }
}

pub fn blyte_tasks_dir() -> Option<PathBuf> {
    BLYTE_TASKS_DIR
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

pub fn set_blyte_run_store(dir: Option<PathBuf>) {
    match BLYTE_RUN_STORE.write() {
        Ok(mut lock) => *lock = dir,
        Err(poisoned) => *poisoned.into_inner() = dir,
    }
}

pub fn blyte_run_store() -> Option<PathBuf> {
    BLYTE_RUN_STORE
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

pub fn set_in_place_lock_path(path: Option<PathBuf>) {
    match IN_PLACE_LOCK_PATH.write() {
        Ok(mut lock) => *lock = path,
        Err(poisoned) => *poisoned.into_inner() = path,
    }
}

pub fn in_place_lock_path() -> Option<PathBuf> {
    IN_PLACE_LOCK_PATH
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

pub fn set_blyte_surface(surface: Option<String>) {
    match BLYTE_SURFACE.write() {
        Ok(mut lock) => *lock = surface,
        Err(poisoned) => *poisoned.into_inner() = surface,
    }
}

pub fn blyte_surface() -> Option<String> {
    BLYTE_SURFACE
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

pub fn set_approval(config: Option<ApprovalConfig>) {
    match APPROVAL.write() {
        Ok(mut lock) => *lock = config,
        Err(poisoned) => *poisoned.into_inner() = config,
    }
}

pub fn approval() -> Option<ApprovalConfig> {
    APPROVAL
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

pub fn set_peer_tools(tools: Option<Arc<dyn PeerTools>>) {
    match PEER_TOOLS.write() {
        Ok(mut lock) => *lock = tools,
        Err(poisoned) => *poisoned.into_inner() = tools,
    }
}

pub fn peer_tools() -> Option<Arc<dyn PeerTools>> {
    PEER_TOOLS
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

pub fn set_publish_tool(tool: Option<Arc<dyn PublishTool>>) {
    match PUBLISH_TOOL.write() {
        Ok(mut lock) => *lock = tool,
        Err(poisoned) => *poisoned.into_inner() = tool,
    }
}

pub fn publish_tool() -> Option<Arc<dyn PublishTool>> {
    PUBLISH_TOOL
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

pub fn set_write_scope(scope: Option<WriteScope>) {
    match WRITE_SCOPE.write() {
        Ok(mut lock) => *lock = scope,
        Err(poisoned) => *poisoned.into_inner() = scope,
    }
}

pub fn write_scope() -> Option<WriteScope> {
    WRITE_SCOPE
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

/// Set the host dispatch command for the conversation model's `dispatch` tool.
pub fn set_dispatch_cmd(cmd: Option<String>) {
    match DISPATCH_CMD.write() {
        Ok(mut lock) => *lock = cmd,
        Err(poisoned) => *poisoned.into_inner() = cmd,
    }
}

/// The currently configured dispatch command, if any.
pub fn dispatch_cmd() -> Option<String> {
    DISPATCH_CMD
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

/// Extra directories allowed for reading with `read` and `grep`.
///
/// Later calls replace earlier ones.
pub fn set_read_roots(roots: Vec<PathBuf>) {
    match READ_ROOTS.write() {
        Ok(mut lock) => *lock = roots,
        Err(poisoned) => *poisoned.into_inner() = roots,
    }
}

/// The directories set with `set_read_roots`.
pub fn read_roots() -> Vec<PathBuf> {
    READ_ROOTS
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone())
}

/// Output over this many lines is distilled to head and tail before entering context.
///
/// Sized from measurement, not intuition: across 64 recorded command outputs the median was
/// 165-307 lines and exactly one exceeded a thousand -- at 25,105 lines, 88% of that run's
/// entire output volume and 26x the next largest (FINDINGS 13.26). The distribution is a heavy
/// tail, so the bound exists for the rare case; it must not clip the common one.
pub const OUTPUT_LINE_BUDGET: usize = 400;

/// Lines a `read` returns at once, within `MAX_OUTPUT_CHARS`. Larger than `OUTPUT_LINE_BUDGET`
/// because a file the model asked for is the point, not noise: at 400 lines a dogfood run spent
/// 15 turns paging through one 650-line file, each re-sending the whole prompt.
pub const READ_LINE_BUDGET: usize = 2_000;

/// Most matches a single search returns. The count of what was cut is always reported, so a
/// truncated search reads as truncated rather than as a complete answer with few hits.
pub const MAX_MATCHES: usize = 200;

/// Longest line kept whole. Minified code puts megabytes on one line, which a line budget alone
/// never catches: a single grep over node_modules overflowed a 1,048,576-token request.
pub const MAX_LINE_CHARS: usize = 500;

/// Longest line `read` keeps whole. Larger than `MAX_LINE_CHARS` because a file the model asked
/// for is the point, not noise: a plan's 640-character contract line was clipped at 500 and the
/// model spent three turns failing to see it. Minified files are still clipped, here and by
/// `MAX_OUTPUT_CHARS`.
pub const READ_LINE_CHARS: usize = 2_000;

/// Most characters a single tool result may carry (about 15K tokens), after line clipping and
/// line distillation. The final guarantee that one result cannot swamp the context.
pub const MAX_OUTPUT_CHARS: usize = 60_000;

/// `bound_output_lines` with a caller-chosen line length (`read` uses `READ_LINE_CHARS`).
pub fn bound_output_with(text: &str, line_budget: usize, line_chars: usize) -> String {
    let clipped: Vec<String> = text
        .lines()
        .map(|l| {
            let n = l.chars().count();
            if n > line_chars {
                let head: String = l.chars().take(line_chars).collect();
                format!(
                    "{head} …[{} more chars clipped by blyte; the grep tool shows the text around a match]",
                    n - line_chars
                )
            } else {
                l.to_string()
            }
        })
        .collect();
    let mut joined = clipped.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    let distilled = distil(&joined, line_budget);
    let total = distilled.chars().count();
    if total <= MAX_OUTPUT_CHARS {
        return distilled;
    }
    let head_n = MAX_OUTPUT_CHARS * 2 / 3;
    let tail_n = MAX_OUTPUT_CHARS - head_n;
    let head: String = distilled.chars().take(head_n).collect();
    let tail: String = distilled.chars().skip(total - tail_n).collect();
    format!(
        "{head}\n\n[... {} chars elided by blyte ...]\n\n{tail}",
        total - head_n - tail_n
    )
}

/// `bound_output` with a caller-chosen line budget (`read` uses `READ_LINE_BUDGET`).
pub fn bound_output_lines(text: &str, line_budget: usize) -> String {
    bound_output_with(text, line_budget, MAX_LINE_CHARS)
}

/// Bound a tool's output by size: clip over-long lines, distil to `OUTPUT_LINE_BUDGET` lines,
/// then cap the total, keeping head and tail. Ordinary output passes through unchanged.
pub fn bound_output(text: &str) -> String {
    bound_output_lines(text, OUTPUT_LINE_BUDGET)
}

/// Match-centred excerpt for long lines.
///
/// When `text` has at most `max` characters, return it unchanged. Otherwise keep the whole match
/// (cut the match itself to `max` characters if it alone is longer) and split the rest of the
/// `max` budget evenly before and after it, shifting any unused side to the other; prefix
/// `…[<n> chars] ` when characters were dropped before, and suffix ` …[<n> chars]` when dropped
/// after, `<n>` being the count dropped. Never split a UTF-8 character.
pub fn excerpt_around(text: &str, start: usize, end: usize, max: usize) -> String {
    let total_chars = text.chars().count();
    if total_chars <= max {
        return text.to_string();
    }
    if start > text.len()
        || end > text.len()
        || start > end
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
    {
        return text.to_string();
    }

    let before = &text[..start];
    let matched = &text[start..end];
    let after = &text[end..];

    let before_chars = before.chars().count();
    let match_chars = matched.chars().count();
    let after_chars = after.chars().count();

    if match_chars >= max {
        let kept_match: String = matched.chars().take(max).collect();
        let dropped_before = before_chars;
        let dropped_after = after_chars + (match_chars - max);
        let mut out = String::new();
        if dropped_before > 0 {
            out.push_str(&format!("…[{dropped_before} chars] "));
        }
        out.push_str(&kept_match);
        if dropped_after > 0 {
            out.push_str(&format!(" …[{dropped_after} chars]"));
        }
        return out;
    }

    let rem = max - match_chars;
    let half_before = rem / 2;
    let half_after = rem - half_before;

    let (take_before, take_after) = if before_chars < half_before {
        let tb = before_chars;
        let ta = (rem - tb).min(after_chars);
        (tb, ta)
    } else if after_chars < half_after {
        let ta = after_chars;
        let tb = (rem - ta).min(before_chars);
        (tb, ta)
    } else {
        (half_before, half_after)
    };

    let dropped_before = before_chars - take_before;
    let dropped_after = after_chars - take_after;

    let mut out = String::new();
    if dropped_before > 0 {
        out.push_str(&format!("…[{dropped_before} chars] "));
    }
    let kept_before: String = before
        .chars()
        .skip(dropped_before)
        .take(take_before)
        .collect();
    out.push_str(&kept_before);
    out.push_str(matched);
    let kept_after: String = after.chars().take(take_after).collect();
    out.push_str(&kept_after);
    if dropped_after > 0 {
        out.push_str(&format!(" …[{dropped_after} chars]"));
    }
    out
}

/// Most matches `excerpt_matches` shows from one line.
const MAX_EXCERPTS_PER_LINE: usize = 5;

/// An excerpt of a long `text` showing every match in `found` (byte ranges, in order; at most
/// `MAX_EXCERPTS_PER_LINE`) with some context each, within about `max` characters. One match is
/// `excerpt_around`. On sanitize-git-repo the first match in a 21,367-character line was an
/// innocent `hf_cache_dir`; the secret was 15,000 characters later.
pub fn excerpt_matches(text: &str, found: &[(usize, usize)], max: usize) -> String {
    if found.len() <= 1 || text.chars().count() <= max {
        return match found.first() {
            Some(&(s, e)) => excerpt_around(text, s, e, max),
            None => text.to_string(),
        };
    }
    let shown = &found[..found.len().min(MAX_EXCERPTS_PER_LINE)];
    let match_chars: usize = shown.iter().map(|&(s, e)| text[s..e].chars().count()).sum();
    let context = max.saturating_sub(match_chars) / (2 * shown.len());
    // Char-index windows around each match, merged where they overlap.
    let to_char = |b: usize| text[..b].chars().count();
    let total = text.chars().count();
    let mut windows: Vec<(usize, usize)> = Vec::new();
    for &(s, e) in shown {
        let (cs, ce) = (to_char(s), to_char(e));
        let w = (cs.saturating_sub(context), (ce + context).min(total));
        match windows.last_mut() {
            Some(last) if w.0 <= last.1 => last.1 = last.1.max(w.1),
            _ => windows.push(w),
        }
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut pos = 0;
    for (i, &(a, b)) in windows.iter().enumerate() {
        if a > pos {
            if i == 0 {
                out.push_str(&format!("…[{} chars] ", a - pos));
            } else {
                out.push_str(&format!(" …[{} chars] ", a - pos));
            }
        }
        out.extend(&chars[a..b]);
        pos = b;
    }
    if pos < total {
        out.push_str(&format!(" …[{} chars]", total - pos));
    }
    if found.len() > shown.len() {
        out.push_str(&format!(
            " [{} more matches in this line]",
            found.len() - shown.len()
        ));
    }
    out
}

fn split_grep_prefix(line: &str) -> (&str, &str) {
    static PREFIX_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^(\d+:\d+:|\d+:|.*?:\d+:)").unwrap());
    if let Some(m) = PREFIX_RE.find(line) {
        (&line[..m.end()], &line[m.end()..])
    } else {
        ("", line)
    }
}

/// Ceiling on a single MODEL-ISSUED command.
///
/// Deliberately much shorter than the gate's budget, because the two have different shapes. A
/// verification suite legitimately runs for many minutes and gets its own longer deadline in
/// `run::Runner`. A command the model types while exploring is almost always seconds, and when
/// it is not it is usually wrong: the first live run opened with `find / -name pytest`, which
/// at the previous 900s ceiling was entitled to fifteen minutes of walking the filesystem
/// before anything intervened. Enforcing a bound is not enough on its own -- the bound has to
/// be near the size of the work.
///
/// 300s still accommodates a genuine `npm install` or `cargo build`. Past that the command is
/// killed, the model is told so, and it can re-issue something narrower.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

/// Ceiling on `blyte edit-finish`, which runs the repository's verification gate in the foreground
/// and therefore needs a gate-sized budget rather than the 300s exploration command limit.
pub const EDIT_FINISH_TIMEOUT: Duration = Duration::from_secs(3600);

pub fn blyte_command_timeout(subcommand: &str) -> Duration {
    if subcommand == "edit-finish" {
        EDIT_FINISH_TIMEOUT
    } else {
        COMMAND_TIMEOUT
    }
}

fn command_ceiling(name: &str, args: &Value) -> Duration {
    if name == "blyte" {
        if let Some(first) = args
            .get("args")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
        {
            return blyte_command_timeout(first);
        }
    }
    COMMAND_TIMEOUT
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutcome {
    fn ok(s: impl Into<String>) -> Self {
        Self {
            content: s.into(),
            is_error: false,
        }
    }
    fn err(s: impl Into<String>) -> Self {
        Self {
            content: s.into(),
            is_error: true,
        }
    }
}

/// Keep head and tail, drop the middle, and say how much was dropped.
///
/// Truncating the tail alone would be worse than useless for the case this exists for: a test
/// runner puts its summary at the end, so tail-clipping hides the answer and keeps the noise.
pub fn distil(text: &str, budget: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= budget {
        return text.to_string();
    }
    let head = budget * 2 / 3;
    let tail = budget - head;
    let dropped = lines.len() - budget;
    let mut out = lines[..head].join("\n");
    out.push_str(&format!(
        "\n\n[... {dropped} lines elided by blyte ...]\n\n"
    ));
    out.push_str(&lines[lines.len() - tail..].join("\n"));
    out
}

/// Resolve `.` and `..` textually, without touching the filesystem.
///
/// Belt to `canonicalize`'s braces, and not redundant. `Path::starts_with` compares COMPONENTS,
/// so `/ws/../../escaped.rs` lexically starts with `/ws` and passes a naive containment check.
/// That makes the whole escape guarantee rest on canonicalize normalising `..` -- true for
/// `std::fs` and for `readlink -f`, but a property of the implementation rather than of the
/// check. Normalising first means a weak or failing canonicalize cannot be walked past.
///
/// Leading `..` that would climb above the root are dropped rather than preserved: there is no
/// legitimate path above the workspace, so collapsing them keeps the result inside where the
/// caller can still reject it on its merits.
fn lexically_normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    // Nothing to pop: keep it so an escape remains visible to the check
                    // rather than silently becoming the root itself.
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Reject a path that escapes the workspace.
///
/// Checked against the CANONICAL path, so `../` and symlinks cannot walk out. The previous
/// harness relied on a prompt rule -- "confine all inspections to the workspace" -- which is
/// exactly the kind of instruction Invariant IV exists because agents ignore under pressure.
fn resolve_in_workspace(fs: &dyn Fs, root: &Path, raw: &str) -> Result<PathBuf, String> {
    let joined = lexically_normalize(&if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        root.join(raw)
    });
    let root_c = fs
        .canonicalize(root)
        .map_err(|e| format!("workspace root unreadable: {e}"))?;

    // Canonicalise the NEAREST EXISTING ancestor, starting with the target itself. Walking
    // straight to `.parent()` was wrong for a directory target: `Path::new("/a/b/.").parent()`
    // is `/a`, not `/a/b`, so searching the workspace root as `.` resolved to its parent and was
    // refused. Starting at the target and climbing only when it does not exist keeps both cases
    // right -- an existing directory checks itself, a file about to be created checks the
    // directory it will land in -- and neither is waved through unchecked.
    let mut probe = joined.as_path();
    let probe_c = loop {
        if let Ok(c) = fs.canonicalize(probe) {
            break c;
        }
        match probe.parent() {
            Some(p) => probe = p,
            // Nothing in the chain exists: fall back to a lexical check rather than allowing it.
            None => break joined.clone(),
        }
    };
    if !probe_c.starts_with(&root_c) {
        return Err(format!("path escapes the workspace: {raw}"));
    }
    Ok(joined)
}

fn resolve_real_target(fs: &dyn Fs, path: &Path) -> Result<PathBuf, String> {
    let mut probe = path;
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match fs.canonicalize(probe) {
            Ok(mut resolved) => {
                for part in tail.into_iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(e) => {
                let exists_or_unreadable = if fs.is_local() {
                    match std::fs::symlink_metadata(probe) {
                        Ok(_) => true,
                        Err(meta_err) => meta_err.kind() != std::io::ErrorKind::NotFound,
                    }
                } else {
                    false
                };
                if exists_or_unreadable {
                    return Err(format!("cannot canonicalize {}: {e}", probe.display()));
                }
                let Some(name) = probe.file_name() else {
                    return Err(format!("cannot canonicalize {}: {e}", probe.display()));
                };
                tail.push(name.to_os_string());
                let Some(parent) = probe.parent() else {
                    return Err(format!("cannot canonicalize {}: {e}", probe.display()));
                };
                probe = parent;
            }
        }
    }
}

/// Reports whether `real_target` lies inside an open direct edit's worktree under
/// `edit_worktrees`: `<edit_worktrees>/<id>/<rest>` where `<edit_worktrees>/<id>.edit.json`
/// exists, `<edit_worktrees>/<id>.json` does not, `<edit_worktrees>/<id>` is a directory, and no
/// path component below `<id>/` is `.git`.
fn is_in_open_edit_worktree(edit_worktrees: &Path, real_target: &Path) -> Result<bool, String> {
    use std::path::Component;
    let Ok(rel) = real_target.strip_prefix(edit_worktrees) else {
        return Ok(false);
    };
    let mut comps = rel.components();
    let Some(Component::Normal(id)) = comps.next() else {
        return Ok(false);
    };
    let mut has_child = false;
    for comp in comps {
        has_child = true;
        match comp {
            Component::Normal(name) if name == ".git" => return Ok(false),
            Component::Normal(_) => {}
            _ => return Ok(false),
        }
    }
    if !has_child {
        return Ok(false);
    }
    crate::sandbox::is_open_edit_worktree_id(edit_worktrees, id)
}

fn protected_read_refusal(verb: &str, path: &Path) -> String {
    format!(
        "{verb} was not made: {} is a credential path that this conversation cannot read",
        path.display()
    )
}

fn append_skipped_line(mut text: String, skipped: usize) -> String {
    if skipped > 0 {
        text.push_str(&format!("\n(credential paths skipped: {skipped})"));
    }
    text
}

fn resolve_host_prefix(fs: &dyn Fs, path: &Path) -> Result<PathBuf, String> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("cannot determine current directory: {e}"))?
            .join(path)
    };
    let mut probe = abs.as_path();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match fs.canonicalize(probe) {
            Ok(mut resolved) => {
                for part in tail.into_iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(e) => {
                let not_present = match std::fs::symlink_metadata(probe) {
                    Ok(_) => false,
                    Err(meta_err) => matches!(
                        meta_err.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                    ),
                };
                if !not_present {
                    return Err(format!("cannot canonicalize {}: {e}", probe.display()));
                }
                let Some(name) = probe.file_name() else {
                    return Err(format!("cannot canonicalize {}: {e}", probe.display()));
                };
                tail.push(name.to_os_string());
                let Some(parent) = probe.parent() else {
                    return Err(format!("cannot canonicalize {}: {e}", probe.display()));
                };
                probe = parent;
            }
        }
    }
}

fn check_protected_write(
    fs: &dyn Fs,
    root: &Path,
    tool: &str,
    raw_path: &str,
    path: &Path,
) -> Result<(), String> {
    if !fs.is_local() {
        return Ok(());
    }
    let home_raw = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .ok_or_else(|| format!("{tool}: cannot check protected paths without HOME"))?;
    let real_target = resolve_real_target(fs, path)?;
    let real_root = resolve_host_prefix(fs, root)?;
    let home = resolve_host_prefix(fs, Path::new(&home_raw))?;
    let mut state_dirs: Vec<PathBuf> = Vec::new();
    let mut push_state_dir = |raw: PathBuf| -> Result<(), String> {
        let resolved = resolve_host_prefix(fs, &raw)?;
        if !state_dirs.contains(&resolved) {
            state_dirs.push(resolved);
        }
        Ok(())
    };
    if let Some(dir) = crate::sandbox::state_blyte_dir() {
        push_state_dir(dir)?;
    }
    if let Some(dir) = std::env::var_os("BLYTE_STATE_DIR").filter(|d| !d.is_empty()) {
        push_state_dir(PathBuf::from(dir))?;
    }
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME").filter(|d| !d.is_empty()) {
        push_state_dir(PathBuf::from(xdg).join("blyte"))?;
    }
    if state_dirs.is_empty() {
        if let Some(reason) = crate::write_guard::protected_write(&real_target, &home, None) {
            return Err(format!(
                "{tool}: {raw_path} is protected ({reason}); blyte does not write there"
            ));
        }
    } else {
        for dir in &state_dirs {
            let check_dir = if crate::sandbox::is_run_scratch_workspace(&real_root, dir)
                && real_target.starts_with(&real_root)
            {
                None
            } else {
                Some(dir.as_path())
            };
            if let Some(reason) =
                crate::write_guard::protected_write(&real_target, &home, check_dir)
            {
                return Err(format!(
                    "{tool}: {raw_path} is protected ({reason}); blyte does not write there"
                ));
            }
        }
    }
    Ok(())
}

pub fn check_write_scope(fs: &dyn Fs, tool: &str, path: &Path) -> Result<(), String> {
    let Some(scope) = write_scope() else {
        return Ok(());
    };
    let real_target = resolve_real_target(fs, path)?;
    let in_blyte_own = if fs.is_local() {
        crate::sandbox::is_in_blyte_own_dir(&real_target)?
    } else {
        false
    };
    if !in_blyte_own && scope.allowed.iter().any(|dir| real_target.starts_with(dir)) {
        return Ok(());
    }
    if let Some(edit_worktrees) = &scope.edit_worktrees {
        if is_in_open_edit_worktree(edit_worktrees, &real_target)? {
            return Ok(());
        }
    }
    let (reason, hint) = if in_blyte_own {
        (
            "inside blyte's own state or configuration".to_string(),
            "the user can approve writes to blyte's own state or configuration",
        )
    } else {
        let allowed_list = scope
            .allowed
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        (
            format!("outside the directories this conversation may write ({allowed_list})"),
            "the user can approve writes outside these directories",
        )
    };
    let cmd = format!("{tool} {}", real_target.display());
    let Some(cfg) = approval() else {
        return Err(format!(
            "write was not made: {} is {reason}; no approver is configured ({hint}).",
            real_target.display()
        ));
    };
    match cfg.approver.ask(&cmd, &reason) {
        Approval::Approved => Ok(()),
        Approval::Denied => Err(format!(
            "write was not made: {} is {reason}; the user refused ({hint}).",
            real_target.display()
        )),
        Approval::Unavailable => Err(format!(
            "write was not made: {} is {reason}; nobody can approve it in this run ({hint}).",
            real_target.display()
        )),
    }
}

/// Reject a path that escapes the workspace, unless an absolute path is inside a read root.
pub(crate) fn resolve_for_read(fs: &dyn Fs, root: &Path, raw: &str) -> Result<PathBuf, String> {
    if let Ok(p) = resolve_in_workspace(fs, root, raw) {
        return Ok(p);
    }
    let roots = READ_ROOTS
        .read()
        .map(|r| r.clone())
        .unwrap_or_else(|e| e.into_inner().clone());
    let p = Path::new(raw);
    if p.is_absolute() {
        let joined = lexically_normalize(p);
        let mut probe = joined.as_path();
        let probe_c = loop {
            if let Ok(c) = fs.canonicalize(probe) {
                break Some(c);
            }
            match probe.parent() {
                Some(p) => probe = p,
                None => break None,
            }
        };
        if let Some(probe_c) = probe_c {
            for r in &roots {
                if let Ok(rc) = fs.canonicalize(r) {
                    if probe_c.starts_with(&rc) {
                        return Ok(joined);
                    }
                }
            }
        }
    }
    let extra = if roots.is_empty() {
        String::new()
    } else {
        let list: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
        format!(" and {}", list.join(", "))
    };
    Err(format!(
        "path escapes the workspace: {raw}. This run can read the workspace{extra}. Do not look for another copy: if the task needs this path, say in your answer that it was not readable."
    ))
}

/// Single-quote for `bash -lc`, so a pattern containing spaces, `$` or `"` is passed verbatim.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Optional accelerators worth telling the model about, because their presence changes strategy.
///
/// POSIX-guaranteed utilities (`sed`, `awk`, `find`, `xargs`, `grep`) are deliberately absent:
/// they are present everywhere, so listing them is noise in every prompt for no decision.
/// Pretty printers (bat, eza, dust, delta) and interactive tools are deliberately absent: their
/// colour, icons and pagers cost a model tokens it cannot use.
pub const ACCELERATORS: &[&str] = &["rg", "fd", "sd", "tokei", "jq", "ast-grep"];

/// Language toolchains. Reported against what the REPOSITORY declares, not as a bare list --
/// "this is a Node project and `npm` is not reachable" is actionable; "npm: absent" is not.
pub const TOOLCHAINS: &[&str] = &[
    "git", "cargo", "go", "npm", "yarn", "pnpm", "python3", "pytest", "node", "make",
];

/// What is reachable in the execution environment, probed through the seam.
pub fn available_binaries(shell: &dyn Shell, root: &Path) -> Result<Vec<String>, String> {
    let mut all: Vec<&'static str> = ACCELERATORS.to_vec();
    all.extend_from_slice(TOOLCHAINS);
    shell.available_binaries(root, &all)
}

fn bash_description() -> String {
    format!(
        "Run a shell command in the workspace and wait for it to finish. \
         Commands run to completion (up to {}s). \
         A background job started with its output redirected keeps running after the command \
         returns, and nothing reports when it exits. \
         Output over {} lines is elided in the middle. \
         To find files or search file contents use the `glob` and `grep` tools, not \
         `find`, `grep -r` or `rg` in the shell; they respect .gitignore, are capped \
         and cannot hang.",
        COMMAND_TIMEOUT.as_secs(),
        OUTPUT_LINE_BUDGET
    )
}

pub fn specs() -> Vec<ToolSpec> {
    let mut tools = vec![
        ToolSpec {
            name: "read".into(),
            description: "Read a file. Up to 2,000 lines (60,000 characters) come back in one call, so read whole files; pass start_line/end_line only for larger ones.".into(),
            parameters: json!({"type":"object","properties":{
                "path":{"type":"string"},
                "start_line":{"type":"integer","description":"1-indexed, inclusive"},
                "end_line":{"type":"integer","description":"1-indexed, inclusive"}},
                "required":["path"]}),
        },
        ToolSpec {
            name: "write".into(),
            description: "Create or overwrite a file with exact content.".into(),
            parameters: json!({"type":"object","properties":{
                "path":{"type":"string"},"content":{"type":"string"}},
                "required":["path","content"]}),
        },
        ToolSpec {
            name: "edit".into(),
            description: "Replace an exact, unique string in a file. Fails if absent or ambiguous."
                .into(),
            parameters: json!({"type":"object","properties":{
                "path":{"type":"string"},"old":{"type":"string"},"new":{"type":"string"}},
                "required":["path","old","new"]}),
        },
        ToolSpec {
            name: "bash".into(),
            description: bash_description(),
            parameters: if approval().is_some() {
                json!({
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"},
                        "unsandboxed": {
                            "type": "boolean",
                            "description": "Run this one command outside the sandbox, with the user's network and credentials. It runs only if the user approves it."
                        },
                        "reason": {
                            "type": "string",
                            "description": "One line telling the user why the command must leave the sandbox."
                        }
                    },
                    "required": ["command"]
                })
            } else {
                json!({"type":"object","properties":{
                    "command":{"type":"string"}},"required":["command"]})
            },
        },
        ToolSpec {
            name: "glob".into(),
            description: "Find files by glob pattern, e.g. `src/**/*.rs`. Respects .gitignore; \
                          `.git` is skipped. Fast, and the right \
                          first move when you do not know where something lives."
                .into(),
            parameters: json!({"type":"object","properties":{
                "pattern":{"type":"string","description":"glob, relative to path (or the workspace)"},
                "path":{"type":"string","description":"directory to search under; defaults to the workspace; may be an absolute path under a read root"}},
                "required":["pattern"]}),
        },
        ToolSpec {
            name: "grep".into(),
            description: format!(
                "Search file contents by regular expression, returning `path:line:text`. \
                 Respects .gitignore. Use `glob` to find files by name and `grep` to find them \
                 by content. Capped at {MAX_MATCHES} matches, and the total is always reported."
            ),
            parameters: json!({"type":"object","properties":{
                "pattern":{"type":"string","description":"regular expression"},
                "path":{"type":"string","description":"subtree to search; defaults to the workspace"},
                "glob":{"type":"string","description":"restrict to files matching this glob, e.g. `*.ts`"}},
                "required":["pattern"]}),
        },
    ];
    if dispatch_cmd().is_some() {
        tools.push(ToolSpec {
            name: "dispatch".into(),
            description: "Delegate work to an implementer task on the host.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "Task instructions for the implementer"
                    },
                    "gate": {
                        "type": "string",
                        "description": "Verification command that decides completion"
                    },
                    "max_seconds": {
                        "type": "integer",
                        "description": "Optional wall-clock limit in seconds"
                    }
                },
                "required": ["task", "gate"]
            }),
        });
    }
    if blyte_tool().is_some() {
        tools.push(ToolSpec {
            name: "blyte".into(),
            description: "Run one of blyte's own commands on the host: dispatch, wait, status, review, plan, init, clean, edit-start, edit-finish.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "args": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "The words after `blyte`"
                    },
                    "background": {
                        "type": "boolean",
                        "description": "Start a `dispatch` in the background and return at once"
                    },
                    "task": {
                        "type": "string",
                        "description": "Task text for `dispatch`; blyte writes it to the tasks directory and passes that path"
                    }
                },
                "required": ["args"]
            }),
        });
    }
    if peer_tools().is_some() {
        tools.push(ToolSpec {
            name: "surfaces".into(),
            description: "List the surfaces running on this machine: name, repository, branch and what each is doing.".into(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        });
        tools.push(ToolSpec {
            name: "tell".into(),
            description: "Send a message to another running surface by name. Its model reads it between turns as a message from this surface, not from its user. Returns delivered or held.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the target surface"
                    },
                    "message": {
                        "type": "string",
                        "description": "Message to send"
                    }
                },
                "required": ["name", "message"]
            }),
        });
    }
    if publish_tool().is_some() {
        tools.push(ToolSpec {
            name: "publish".into(),
            description: "Ask the user to approve pushing a combined request's branch to origin and opening a pull request against the default branch. Runs only when the user approves it on the page.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "request": {
                        "type": "string",
                        "description": "Request id of this surface to publish"
                    },
                    "title": {
                        "type": "string",
                        "description": "Pull request title"
                    },
                    "body": {
                        "type": "string",
                        "description": "Pull request body"
                    }
                },
                "required": ["request", "title", "body"]
            }),
        });
    }
    tools
}

/// The search command. ripgrep honours .gitignore, which keeps dependency and build trees out of
/// results; the grep fallback (used when a container has no rg) excludes them explicitly, or a
/// search over node_modules returns megabyte-long minified lines.
pub fn grep_command(use_rg: bool, pattern: &str, sub: &str, glob: &str) -> String {
    if use_rg {
        let globarg = if glob.is_empty() {
            String::new()
        } else {
            format!("--glob {}", shell_quote(glob))
        };
        format!(
            "rg --line-number --no-heading --color never {globarg} -e {} {} | head -n {}",
            shell_quote(pattern),
            shell_quote(sub),
            MAX_MATCHES + 1
        )
    } else {
        let globarg = if glob.is_empty() {
            String::new()
        } else {
            format!("--include={}", shell_quote(glob))
        };
        let excludes = ["node_modules", ".git", "target", ".venv", "dist", "build"]
            .iter()
            .map(|d| format!("--exclude-dir={d}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "grep -rnIE {excludes} {globarg} -e {} {} | head -n {}",
            shell_quote(pattern),
            shell_quote(sub),
            MAX_MATCHES + 1
        )
    }
}

/// Execute one tool call.
///
/// `shell` decides WHERE commands land -- host, container, remote. The tools never know, which
/// is the point: the previous harness encoded that in a prompt instruction and the worker
/// edited the host whenever the instruction went missing.
///
/// `fs` decides WHERE files live. Kept separate from `shell` because the two can differ: a
/// container with a bind-mounted workspace shares the conversation model's files while running its
/// own commands, and a remote sandbox shares neither. Pairing the wrong two would have the
/// model editing one filesystem and verifying another.
pub fn execute(
    root: &Path,
    shell: &dyn Shell,
    fs: &dyn Fs,
    name: &str,
    args: &Value,
) -> ToolOutcome {
    execute_with_timeout(root, shell, fs, name, args, command_ceiling(name, args))
}

pub fn execute_with_timeout(
    root: &Path,
    shell: &dyn Shell,
    fs: &dyn Fs,
    name: &str,
    args: &Value,
    timeout: Duration,
) -> ToolOutcome {
    let ceiling = command_ceiling(name, args);
    let mut outcome = execute_unbounded(root, shell, fs, name, args, ceiling.min(timeout));
    outcome.content = if name == "read" {
        bound_output_with(&outcome.content, READ_LINE_BUDGET, READ_LINE_CHARS)
    } else if name == "grep" {
        bound_output_with(&outcome.content, OUTPUT_LINE_BUDGET, READ_LINE_CHARS)
    } else {
        bound_output_lines(&outcome.content, OUTPUT_LINE_BUDGET)
    };
    outcome
}

/// The tools themselves; `execute` bounds whatever they return.
fn execute_unbounded(
    root: &Path,
    shell: &dyn Shell,
    fs: &dyn Fs,
    name: &str,
    args: &Value,
    command_timeout: Duration,
) -> ToolOutcome {
    let s = |k: &str| {
        args.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    match name {
        "read" => {
            let path = match resolve_for_read(fs, root, &s("path")) {
                Ok(p) => p,
                Err(e) => return ToolOutcome::err(e),
            };
            if fs.is_local() {
                match crate::sandbox::is_protected_read(&path) {
                    Ok(true) => return ToolOutcome::err(protected_read_refusal("read", &path)),
                    Ok(false) => {}
                    Err(e) => return ToolOutcome::err(e),
                }
            }
            let body = match fs.read(&path) {
                Ok(b) => b,
                Err(e) => return ToolOutcome::err(e),
            };
            let lines: Vec<&str> = body.lines().collect();
            let start = args
                .get("start_line")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .max(1) as usize;
            // A read from the top of a file that fits comes back whole, whatever end_line says:
            // models page from line 1 out of habit (a shadow run read 700 lines as six 200-line
            // calls). A range starting later is a deliberate re-read and is honoured.
            let fits = lines.len() <= READ_LINE_BUDGET && body.chars().count() <= MAX_OUTPUT_CHARS;
            let end = if start == 1 && fits {
                lines.len()
            } else {
                args.get("end_line")
                    .and_then(|v| v.as_u64())
                    .map(|e| e as usize)
                    .unwrap_or(lines.len())
                    .min(lines.len())
            };
            if start > lines.len() {
                return ToolOutcome::err(format!(
                    "{} has {} lines; start_line {start} is past the end",
                    s("path"),
                    lines.len()
                ));
            }
            if end < start {
                return ToolOutcome::err(format!(
                    "end_line {end} is before start_line {start} in {}",
                    s("path")
                ));
            }
            // Numbered, because an agent that cannot cite a line cannot edit precisely.
            let body = lines[start - 1..end]
                .iter()
                .enumerate()
                .map(|(i, l)| format!("{:>6}\t{l}", start + i))
                .collect::<Vec<_>>()
                .join("\n");
            ToolOutcome::ok(distil(&body, READ_LINE_BUDGET))
        }
        "write" => {
            let raw_path = s("path");
            let path = match resolve_in_workspace(fs, root, &raw_path) {
                Ok(p) => p,
                Err(e) => return ToolOutcome::err(e),
            };
            if let Err(e) = check_protected_write(fs, root, "write", &raw_path, &path) {
                return ToolOutcome::err(e);
            }
            if let Err(e) = check_in_place_lock(fs, root) {
                return ToolOutcome::err(e);
            }
            if let Err(e) = check_write_scope(fs, "write", &path) {
                return ToolOutcome::err(e);
            }
            match fs.write(&path, &s("content")) {
                Ok(_) => ToolOutcome::ok(format!("wrote {raw_path}")),
                Err(e) => ToolOutcome::err(e),
            }
        }
        "edit" => {
            let raw_path = s("path");
            let path = match resolve_in_workspace(fs, root, &raw_path) {
                Ok(p) => p,
                Err(e) => return ToolOutcome::err(e),
            };
            if let Err(e) = check_protected_write(fs, root, "edit", &raw_path, &path) {
                return ToolOutcome::err(e);
            }
            if let Err(e) = check_in_place_lock(fs, root) {
                return ToolOutcome::err(e);
            }
            if let Err(e) = check_write_scope(fs, "edit", &path) {
                return ToolOutcome::err(e);
            }
            let body = match fs.read(&path) {
                Ok(b) => b,
                Err(e) => return ToolOutcome::err(e),
            };
            let (old, new) = (s("old"), s("new"));
            match body.matches(&old).count() {
                0 => ToolOutcome::err(format!("`old` not found in {}. Read it first.", s("path"))),
                // Ambiguity is an error, never a guess: replacing the first of several matches
                // silently edits the wrong place and the agent cannot tell.
                n if n > 1 => ToolOutcome::err(format!(
                    "`old` appears {n} times in {}; include more context to make it unique",
                    s("path")
                )),
                _ => match fs.write(&path, &body.replacen(&old, &new, 1)) {
                    Ok(_) => ToolOutcome::ok(format!("edited {}", s("path"))),
                    Err(e) => ToolOutcome::err(e),
                },
            }
        }
        "bash" => {
            let command = s("command");
            let unsandboxed = args
                .get("unsandboxed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if unsandboxed {
                let Some(cfg) = approval() else {
                    return ToolOutcome::err("this run cannot run commands outside the sandbox");
                };
                let reason_raw = s("reason");
                let reason = reason_raw.trim();
                if reason.is_empty() {
                    return ToolOutcome::err("give a one-line reason with an unsandboxed command");
                }
                let banner = if let Some(rule) = matching_rule(&command, &cfg.rules) {
                    format!("ran outside the sandbox (allowed by rule \"{rule}\")")
                } else {
                    match cfg.approver.ask(&command, reason) {
                        Approval::Approved => {
                            "ran outside the sandbox (approved by the user)".to_string()
                        }
                        Approval::Denied => {
                            return ToolOutcome::err(
                                "the user refused this command; it was not run. Do not try to run it another way.",
                            );
                        }
                        Approval::Unavailable => {
                            return ToolOutcome::err(
                                "nobody can approve a command in this run, so it was not run",
                            );
                        }
                    }
                };
                match shell.exec_host(&command, root, command_timeout) {
                    Ok(o) => {
                        let body = distil(o.merged().trim_end(), OUTPUT_LINE_BUDGET);
                        ToolOutcome {
                            content: format!("{banner}\nexit {}\n{body}", o.exit_code),
                            is_error: o.exit_code != 0,
                        }
                    }
                    Err(e) => ToolOutcome::err(e),
                }
            } else {
                match shell.exec_user(&command, root, command_timeout) {
                    Ok(o) => {
                        let body = distil(o.merged().trim_end(), OUTPUT_LINE_BUDGET);
                        ToolOutcome {
                            content: format!("exit {}\n{body}", o.exit_code),
                            is_error: o.exit_code != 0,
                        }
                    }
                    Err(e) => ToolOutcome::err(e),
                }
            }
        }
        "glob" => {
            let pattern = s("pattern");
            let raw_path = s("path");
            let sub = if raw_path.is_empty() { "." } else { &raw_path };
            let dir = match resolve_for_read(fs, root, sub) {
                Ok(d) => d,
                Err(e) => return ToolOutcome::err(e),
            };
            if fs.is_local() {
                match crate::sandbox::is_protected_read(&dir) {
                    Ok(true) => return ToolOutcome::err(protected_read_refusal("glob", &dir)),
                    Ok(false) => {}
                    Err(e) => return ToolOutcome::err(e),
                }
            }
            let outcome = |hits: Vec<String>, skipped: usize| {
                let text = if hits.is_empty() {
                    format!("no files match {pattern}")
                } else {
                    let shown: Vec<String> = if raw_path.is_empty() {
                        hits
                    } else {
                        let base = Path::new(&raw_path);
                        hits.into_iter()
                            .map(|h| base.join(h).to_string_lossy().into_owned())
                            .collect()
                    };
                    format!("{} file(s):\n{}", shown.len(), shown.join("\n"))
                };
                ToolOutcome::ok(append_skipped_line(text, skipped))
            };
            if fs.is_local() {
                match crate::search::glob_paths_checked(&dir, &pattern, MAX_MATCHES) {
                    Ok(Some(res)) => return outcome(res.hits, res.skipped),
                    Ok(None) => {}
                    Err(e) => return ToolOutcome::err(e),
                }
            } else if let Ok(o) = shell.exec(
                "if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then \
                 git -c core.quotePath=off -c core.fsmonitor=false ls-files -co --exclude-standard; \
                 else echo __BLYTE_NO_GIT__; fi",
                &dir,
                command_timeout,
            ) {
                if o.exit_code == 0 && !o.stdout.starts_with("__BLYTE_NO_GIT__") {
                    if let Some(hits) = crate::search::glob_filter(&o.stdout, &pattern, MAX_MATCHES)
                    {
                        return outcome(hits, 0);
                    }
                }
            }
            let cmd = format!(
                "if shopt -s globstar 2>/dev/null; then \
                     shopt -s nullglob dotglob 2>/dev/null; \
                     printf '%s\\n' {pattern}; \
                 elif command -v zsh >/dev/null 2>&1; then \
                     zsh -c 'setopt null_glob glob_dots 2>/dev/null; printf \"%s\\n\" {pattern}'; \
                 else \
                     shopt -s nullglob dotglob 2>/dev/null; \
                     printf '%s\\n' {pattern}; \
                 fi | head -n {MAX_MATCHES}"
            );
            match shell.exec(&cmd, &dir, command_timeout) {
                Ok(o) => {
                    let raw_hits: Vec<String> = o
                        .stdout
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(String::from)
                        .collect();
                    if fs.is_local() {
                        let protected = match crate::sandbox::protected_read_paths() {
                            Ok(p) => p,
                            Err(e) => return ToolOutcome::err(e),
                        };
                        let mut hits = Vec::with_capacity(raw_hits.len());
                        let mut skipped = 0usize;
                        for hit in raw_hits {
                            let full = dir.join(&hit);
                            match crate::sandbox::is_protected_read_with(&full, &protected) {
                                Ok(true) => skipped += 1,
                                Ok(false) => hits.push(hit),
                                Err(e) => return ToolOutcome::err(e),
                            }
                        }
                        outcome(hits, skipped)
                    } else {
                        outcome(raw_hits, 0)
                    }
                }
                Err(e) => ToolOutcome::err(format!("glob failed: {e}")),
            }
        }
        "grep" => {
            let pattern = s("pattern");
            let sub = {
                let p = s("path");
                if p.is_empty() {
                    ".".to_string()
                } else {
                    p
                }
            };
            let search_root = match resolve_for_read(fs, root, &sub) {
                Ok(p) => p,
                Err(e) => return ToolOutcome::err(e),
            };
            if fs.is_local() {
                match crate::sandbox::is_protected_read(&search_root) {
                    Ok(true) => {
                        return ToolOutcome::err(protected_read_refusal("grep", &search_root))
                    }
                    Ok(false) => {}
                    Err(e) => return ToolOutcome::err(e),
                }
            }
            let glob_str = s("glob");
            let (lines, skipped): (Vec<String>, usize) = if fs.is_local() {
                match crate::search::grep_lines_checked(
                    root,
                    &sub,
                    &pattern,
                    &glob_str,
                    MAX_MATCHES + 1,
                ) {
                    Ok(res) => (res.hits, res.skipped),
                    Err(e) => return ToolOutcome::err(format!("grep failed: {e}")),
                }
            } else {
                let cmd = format!(
                    "if command -v rg >/dev/null 2>&1; then {}; else {}; fi",
                    grep_command(true, &pattern, &sub, &glob_str),
                    grep_command(false, &pattern, &sub, &glob_str)
                );
                match shell.exec(&cmd, root, command_timeout) {
                    Ok(o) => (
                        o.stdout
                            .lines()
                            .filter(|l| !l.trim().is_empty())
                            .map(String::from)
                            .collect(),
                        0,
                    ),
                    Err(e) => return ToolOutcome::err(format!("grep failed: {e}")),
                }
            };
            if lines.is_empty() {
                return ToolOutcome::ok(append_skipped_line(
                    format!("no matches for {pattern}"),
                    skipped,
                ));
            }
            let pattern_re = regex::Regex::new(&pattern).ok();
            let lines: Vec<String> = lines
                .into_iter()
                .map(|line| {
                    if line.chars().count() <= MAX_LINE_CHARS {
                        return line;
                    }
                    let Some(ref re) = pattern_re else {
                        return line;
                    };
                    let (prefix, rest) = split_grep_prefix(&line);
                    let found: Vec<(usize, usize)> =
                        re.find_iter(rest).map(|m| (m.start(), m.end())).collect();
                    if found.is_empty() {
                        line
                    } else {
                        format!("{prefix}{}", excerpt_matches(rest, &found, MAX_LINE_CHARS))
                    }
                })
                .collect();
            if lines.len() > MAX_MATCHES {
                ToolOutcome::ok(append_skipped_line(
                    format!(
                        "{}\n\n[capped at {MAX_MATCHES} matches; narrow the pattern or pass `glob` \
                         to see the rest]",
                        lines[..MAX_MATCHES].join("\n")
                    ),
                    skipped,
                ))
            } else {
                ToolOutcome::ok(append_skipped_line(
                    format!("{} match(es):\n{}", lines.len(), lines.join("\n")),
                    skipped,
                ))
            }
        }
        "dispatch" => {
            let Some(cmd) = dispatch_cmd() else {
                return ToolOutcome::err("unknown tool: dispatch");
            };
            let json_bytes = match serialize_dispatch_args(args) {
                Ok(bytes) => bytes,
                Err(msg) => return ToolOutcome::err(msg),
            };
            let parent_session = dispatch_parent_session();
            let mut cmd_builder = std::process::Command::new("sh");
            cmd_builder
                .arg("-c")
                .arg(&cmd)
                .env("BLYTE_DISPATCH_PARENT", &parent_session)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            if root.is_dir() {
                cmd_builder.current_dir(root);
            }
            let mut child = match cmd_builder.spawn() {
                Ok(c) => c,
                Err(e) => return ToolOutcome::err(format!("dispatch failed to spawn: {e}")),
            };

            let Some(mut stdin) = child.stdin.take() else {
                let _ = child.kill();
                let _ = child.wait();
                return ToolOutcome::err("dispatch failed: stdin pipe unavailable");
            };
            let write_err = std::io::Write::write_all(&mut stdin, &json_bytes).err();
            drop(stdin);

            match (write_err, child.wait_with_output()) {
                (None, Ok(output)) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
                    ToolOutcome::ok(stdout)
                }
                (None, Ok(output)) => ToolOutcome::err(format_dispatch_failure(&output)),
                (None, Err(e)) => ToolOutcome::err(format!("dispatch command failed: {e}")),
                (Some(write_err), Ok(output)) if output.status.success() => ToolOutcome::err(
                    format!("dispatch failed to write arguments to stdin: {write_err}"),
                ),
                (Some(write_err), Ok(output)) => ToolOutcome::err(format!(
                    "{}\ndispatch also failed to write arguments to stdin: {write_err}",
                    format_dispatch_failure(&output)
                )),
                (Some(write_err), Err(wait_err)) => ToolOutcome::err(format!(
                    "dispatch failed to write arguments to stdin: {write_err}; dispatch command also failed: {wait_err}"
                )),
            }
        }
        "blyte" => {
            let Some(exe) = blyte_tool() else {
                return ToolOutcome::err("unknown tool: blyte");
            };
            let cmd_args: Vec<String> = match args.get("args").and_then(|v| v.as_array()) {
                Some(arr) => {
                    let mut out = Vec::with_capacity(arr.len());
                    for item in arr {
                        match item.as_str() {
                            Some(s) => out.push(s.to_string()),
                            None => return ToolOutcome::err("args must be an array of strings"),
                        }
                    }
                    out
                }
                None => Vec::new(),
            };
            let Some(first) = cmd_args.first().map(|s| s.as_str()) else {
                return ToolOutcome::err(format!(
                    "the blyte tool runs only: {}",
                    BLYTE_TOOL_COMMANDS.join(", ")
                ));
            };
            if let Some(refused) = refused_blyte_tool_command(&cmd_args) {
                return ToolOutcome::err(crate::shell::model_shell_refusal_message(refused));
            }
            let is_repo = crate::sandbox::is_git_repository(root);
            if !is_repo {
                if NO_REPO_REFUSED_COMMANDS.contains(&first) {
                    return ToolOutcome::err(NO_REPO_BLYTE_COMMAND_ERROR);
                }
                if !BLYTE_TOOL_NO_REPO_COMMANDS.contains(&first) {
                    return ToolOutcome::err(format!(
                        "the blyte tool runs only: {}",
                        BLYTE_TOOL_NO_REPO_COMMANDS.join(", ")
                    ));
                }
            } else if !BLYTE_TOOL_COMMANDS.contains(&first) {
                return ToolOutcome::err(format!(
                    "the blyte tool runs only: {}",
                    BLYTE_TOOL_COMMANDS.join(", ")
                ));
            }
            let background = args
                .get("background")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if background && first != "dispatch" {
                return ToolOutcome::err("only dispatch can run in the background");
            }
            let task_text: Option<&str> = match args.get("task") {
                None | Some(Value::Null) => None,
                Some(v) => match v.as_str() {
                    Some(s) => Some(s),
                    None => return ToolOutcome::err("task must be a string"),
                },
            };
            if task_text.is_some() && first != "dispatch" {
                return ToolOutcome::err("task can only be passed with dispatch");
            }

            let mut exec_args = cmd_args.clone();
            let mut dispatch_id: Option<String> = None;
            let mut background_out_dir: Option<PathBuf> = None;
            if matches!(first, "dispatch" | "edit-finish") {
                if cmd_args
                    .iter()
                    .skip(1)
                    .any(|a| a == "--request" || a.starts_with("--request="))
                {
                    return ToolOutcome::err("do not pass --request; blyte sets it automatically");
                }
                if cmd_args
                    .iter()
                    .skip(1)
                    .any(|a| a == "--surface" || a.starts_with("--surface="))
                {
                    return ToolOutcome::err("do not pass --surface; blyte sets it automatically");
                }
                if cmd_args
                    .iter()
                    .skip(1)
                    .any(|a| a == "--started-by" || a.starts_with("--started-by="))
                {
                    return ToolOutcome::err(
                        "do not pass --started-by; blyte sets it automatically",
                    );
                }
                if first == "dispatch" {
                    if let Some(opt) = refused_dispatch_option(&cmd_args) {
                        return ToolOutcome::err(refused_dispatch_option_message(opt));
                    }
                    let parsed = parse_dispatch_args(&cmd_args);
                    if background {
                        let out_dir = if is_repo {
                            root.join(".wt")
                        } else {
                            let Some(store) = blyte_run_store() else {
                                return ToolOutcome::err(
                                    "run store is not configured for background dispatch outside a git repository",
                                );
                            };
                            store
                        };
                        background_out_dir = Some(out_dir);
                    }
                    if let Some(text) = task_text {
                        if text.trim().is_empty() {
                            return ToolOutcome::err("task must not be empty");
                        }
                        if parsed.has_plan || parsed.has_plan_task {
                            return ToolOutcome::err("do not pass task with --plan or --plan-task");
                        }
                        if parsed.has_task_file {
                            return ToolOutcome::err("do not pass both task and a task file path");
                        }
                        let Some((id_idx, ref id)) = parsed.id else {
                            return ToolOutcome::err("dispatch needs an id");
                        };
                        if id.is_empty() {
                            return ToolOutcome::err("dispatch needs an id");
                        }
                        let tasks_dir = if is_repo {
                            root.join(".wt").join("tasks")
                        } else {
                            let Some(dir) = blyte_tasks_dir() else {
                                return ToolOutcome::err(
                                    "tasks directory is not configured for dispatch outside a git repository",
                                );
                            };
                            dir
                        };
                        let task_path = match write_inline_task(&tasks_dir, id, text) {
                            Ok(p) => p,
                            Err(e) => return ToolOutcome::err(e),
                        };
                        exec_args.insert(id_idx + 1, task_path.to_string_lossy().into_owned());
                    }
                    dispatch_id = parsed.id.map(|(_, id)| id).filter(|id| !id.is_empty());
                }
                if let Some(req_id) = blyte_request() {
                    if let Some(pos) = exec_args.iter().position(|a| a == "--") {
                        exec_args.insert(pos, req_id);
                        exec_args.insert(pos, "--request".to_string());
                    } else {
                        exec_args.push("--request".to_string());
                        exec_args.push(req_id);
                    }
                }
                if let Some(surface_id) = blyte_surface() {
                    if let Some(pos) = exec_args.iter().position(|a| a == "--") {
                        exec_args.insert(pos, surface_id);
                        exec_args.insert(pos, "--surface".to_string());
                    } else {
                        exec_args.push("--surface".to_string());
                        exec_args.push(surface_id);
                    }
                }
                if let Some(started_by) = blyte_started_by() {
                    if let Some(pos) = exec_args.iter().position(|a| a == "--") {
                        exec_args.insert(pos, started_by);
                        exec_args.insert(pos, "--started-by".to_string());
                    } else {
                        exec_args.push("--started-by".to_string());
                        exec_args.push(started_by);
                    }
                }
            }

            let parent_session = dispatch_parent_session();
            let mut cmd_builder = std::process::Command::new(&exe);
            cmd_builder
                .args(&exec_args)
                .env("BLYTE_DISPATCH_PARENT", &parent_session)
                .env_remove(crate::shell::MODEL_SHELL_ENV)
                .stdin(std::process::Stdio::null());
            if root.is_dir() {
                cmd_builder.current_dir(root);
            }

            if background {
                let Some(id) = dispatch_id else {
                    return ToolOutcome::err("dispatch needs an id");
                };
                if let Err(e) = validate_dispatch_id(&id) {
                    return ToolOutcome::err(e);
                }
                let Some(out_dir) = background_out_dir else {
                    return ToolOutcome::err(
                        "run store is not configured for background dispatch outside a git repository",
                    );
                };
                return run_background_dispatch(cmd_builder, &out_dir, &id);
            }

            match run_command_bounded(cmd_builder, command_timeout) {
                Ok((0, stdout, _stderr)) => ToolOutcome::ok(stdout),
                Ok((2, stdout, stderr)) if first == "wait" => {
                    let lines: Vec<&str> = stderr.lines().collect();
                    let tail = if lines.len() > OUTPUT_LINE_BUDGET {
                        lines[lines.len() - OUTPUT_LINE_BUDGET..].join("\n")
                    } else {
                        stderr.trim_end().to_string()
                    };
                    let mut parts = vec!["still waiting".to_string()];
                    if !stdout.trim().is_empty() {
                        parts.push(stdout.trim_end().to_string());
                    }
                    if !tail.trim().is_empty() {
                        parts.push(tail);
                    }
                    ToolOutcome::ok(parts.join("\n"))
                }
                Ok((code, stdout, stderr)) => {
                    let lines: Vec<&str> = stderr.lines().collect();
                    let tail = if lines.len() > OUTPUT_LINE_BUDGET {
                        lines[lines.len() - OUTPUT_LINE_BUDGET..].join("\n")
                    } else {
                        stderr.trim_end().to_string()
                    };
                    let mut parts = vec![format!("exit {code}")];
                    if !stdout.trim().is_empty() {
                        parts.push(stdout.trim_end().to_string());
                    }
                    if !tail.trim().is_empty() {
                        parts.push(tail);
                    }
                    ToolOutcome::err(parts.join("\n"))
                }
                Err(e) => ToolOutcome::err(e),
            }
        }
        "surfaces" => {
            let Some(peer) = peer_tools() else {
                return ToolOutcome::err("the surfaces tool is not available here");
            };
            match peer.surfaces() {
                Ok(text) => ToolOutcome::ok(text),
                Err(text) => ToolOutcome::err(text),
            }
        }
        "tell" => {
            let Some(peer) = peer_tools() else {
                return ToolOutcome::err("the tell tool is not available here");
            };
            let Some(target) = args
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|v| !v.trim().is_empty())
            else {
                return ToolOutcome::err("tell needs a name and a message");
            };
            let Some(message) = args
                .get("message")
                .and_then(|v| v.as_str())
                .filter(|v| !v.trim().is_empty())
            else {
                return ToolOutcome::err("tell needs a name and a message");
            };
            match peer.tell(target, message) {
                Ok(text) => ToolOutcome::ok(text),
                Err(text) => ToolOutcome::err(text),
            }
        }
        "publish" => {
            let Some(tool) = publish_tool() else {
                return ToolOutcome::err("the publish tool is not available here");
            };
            if !crate::sandbox::is_git_repository(root) {
                return ToolOutcome::err(NOT_A_GIT_REPOSITORY_ERROR);
            }
            if let Some(started_by) = blyte_started_by() {
                return ToolOutcome::err(format!(
                    "cannot publish during a turn started by another surface ({started_by})"
                ));
            }
            let Some(obj) = args.as_object() else {
                return ToolOutcome::err("publish arguments must be an object");
            };
            for key in obj.keys() {
                if !matches!(key.as_str(), "request" | "title" | "body") {
                    return ToolOutcome::err(format!("publish does not accept field {key}"));
                }
            }
            let Some(request) = obj
                .get("request")
                .and_then(|v| v.as_str())
                .filter(|v| !v.trim().is_empty())
            else {
                return ToolOutcome::err("publish needs a non-empty request");
            };
            let Some(title) = obj.get("title").and_then(|v| v.as_str()) else {
                return ToolOutcome::err("publish needs a title");
            };
            let Some(body) = obj.get("body").and_then(|v| v.as_str()) else {
                return ToolOutcome::err("publish needs a body");
            };
            if title.trim().is_empty() {
                return ToolOutcome::err("publish title must not be blank");
            }
            if title.chars().count() > PUBLISH_TITLE_MAX_CHARS {
                return ToolOutcome::err(format!(
                    "publish title must be at most {PUBLISH_TITLE_MAX_CHARS} characters"
                ));
            }
            if body.len() > PUBLISH_BODY_MAX_BYTES {
                return ToolOutcome::err(format!(
                    "publish body must be at most {PUBLISH_BODY_MAX_BYTES} bytes"
                ));
            }
            match tool.publish(request, title.trim(), body) {
                Ok(text) => ToolOutcome::ok(text),
                Err(text) => ToolOutcome::err(text),
            }
        }
        other => ToolOutcome::err(format!("unknown tool: {other}")),
    }
}

const NOT_A_GIT_REPOSITORY_ERROR: &str = "this folder is not a git repository, and direct edits, combining and publishing need one. If the user wants a repository here, ask to run \"git init\" and then an empty first commit (git commit --allow-empty) with the bash tool and unsandboxed: true; the user approves each command. Add only the files the user names afterwards, never \"git add -A\": the folder may hold credentials.";
const NO_REPO_BLYTE_COMMAND_ERROR: &str = "this folder is not a git repository; here the blyte tool runs only dispatch, wait, status and review. `blyte undo <id>` is for the user to run. Other commands need a git repository (run `git init` and an empty first commit only when the user wants one).";
const BACKGROUND_STARTUP_WAIT: Duration = Duration::from_secs(2);
const BACKGROUND_STARTUP_POLL: Duration = Duration::from_millis(50);

struct ParsedDispatchArgs {
    id: Option<(usize, String)>,
    has_task_file: bool,
    has_plan: bool,
    has_plan_task: bool,
}

fn parse_dispatch_args(cmd_args: &[String]) -> ParsedDispatchArgs {
    let mut i = 1;
    let mut after_dd = false;
    let mut id: Option<(usize, String)> = None;
    let mut has_task_file = false;
    let mut has_plan = false;
    let mut has_plan_task = false;
    while i < cmd_args.len() {
        let arg = &cmd_args[i];
        if !after_dd {
            if arg == "--" {
                after_dd = true;
                i += 1;
                continue;
            }
            if arg == "--plan" || arg.starts_with("--plan=") {
                has_plan = true;
            }
            if arg == "--plan-task" || arg.starts_with("--plan-task=") {
                has_plan_task = true;
            }
            if DISPATCH_VALUE_FLAGS.contains(&arg.as_str()) {
                i += 2;
                continue;
            }
            if arg.starts_with('-') && arg != "-" {
                i += 1;
                continue;
            }
        }
        if id.is_none() {
            id = Some((i, arg.clone()));
        } else {
            has_task_file = true;
        }
        i += 1;
    }
    ParsedDispatchArgs {
        id,
        has_task_file,
        has_plan,
        has_plan_task,
    }
}

fn validate_dispatch_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id == "."
        || id == ".."
        || id.contains('/')
        || id.contains('\\')
        || id.contains('\0')
    {
        return Err(format!("invalid dispatch id: {id}"));
    }
    Ok(())
}

fn write_inline_task(tasks_dir: &Path, id: &str, text: &str) -> Result<PathBuf, String> {
    validate_dispatch_id(id)?;
    let task_path = tasks_dir.join(format!("{id}.md"));
    if let Err(e) = std::fs::create_dir_all(tasks_dir) {
        return Err(format!(
            "cannot create tasks directory {} for {}: {e}",
            tasks_dir.display(),
            task_path.display()
        ));
    }
    match std::fs::read_to_string(&task_path) {
        Ok(existing) => {
            if existing != text {
                return Err(format!(
                    "task file {} already exists with different content",
                    task_path.display()
                ));
            }
            Ok(task_path)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::write(&task_path, text)
                .map_err(|we| format!("cannot write task file {}: {we}", task_path.display()))?;
            Ok(task_path)
        }
        Err(e) => Err(format!(
            "cannot read existing task file {}: {e}",
            task_path.display()
        )),
    }
}

fn check_in_place_lock(fs: &dyn Fs, root: &Path) -> Result<(), String> {
    if !fs.is_local() || crate::sandbox::is_git_repository(root) {
        return Ok(());
    }
    let lock_path = match in_place_lock_path() {
        Some(p) => p,
        None if blyte_tool().is_some() => {
            return Err(
                "in-place lock path is not configured for writing outside a git repository"
                    .to_string(),
            );
        }
        None => return Ok(()),
    };
    match crate::folder_lock::holder(&lock_path)? {
        Some(holder) => Err(format!(
            "run {} is editing this folder; wait for it to finish",
            holder.id
        )),
        None => Ok(()),
    }
}

fn run_background_dispatch(
    mut cmd_builder: std::process::Command,
    out_dir: &Path,
    id: &str,
) -> ToolOutcome {
    let out_path = out_dir.join(format!("{id}.dispatch.out"));
    if let Err(e) = std::fs::create_dir_all(out_dir) {
        return ToolOutcome::err(format!(
            "cannot create {} for {}: {e}",
            out_dir.display(),
            out_path.display()
        ));
    }
    let out_file = match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&out_path)
    {
        Ok(f) => f,
        Err(e) => {
            return ToolOutcome::err(format!("cannot open {}: {e}", out_path.display()));
        }
    };
    let err_file = match out_file.try_clone() {
        Ok(f) => f,
        Err(e) => {
            return ToolOutcome::err(format!(
                "cannot clone output file {}: {e}",
                out_path.display()
            ));
        }
    };
    cmd_builder
        .stdout(std::process::Stdio::from(out_file))
        .stderr(std::process::Stdio::from(err_file));

    let mut child = match cmd_builder.spawn() {
        Ok(c) => c,
        Err(e) => return ToolOutcome::err(format!("blyte failed to spawn: {e}")),
    };
    drop(cmd_builder);

    let deadline = std::time::Instant::now() + BACKGROUND_STARTUP_WAIT;
    let exited = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break None;
                }
                std::thread::sleep(BACKGROUND_STARTUP_POLL.min(deadline - now));
            }
            Err(e) => {
                return ToolOutcome::err(format!("blyte wait failed for dispatch {id}: {e}"));
            }
        }
    };

    match exited {
        Some(status) => {
            let raw = match std::fs::read(&out_path) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => {
                    return ToolOutcome::err(format!(
                        "dispatch {id} exited ({status}), and reading {} failed: {e}",
                        out_path.display()
                    ));
                }
            };
            let body = raw.trim_end();
            if status.success() {
                if body.is_empty() {
                    ToolOutcome::ok(format!("dispatch {id} finished"))
                } else {
                    ToolOutcome::ok(format!("dispatch {id} finished\n{body}"))
                }
            } else {
                let header = match status.code() {
                    Some(code) => format!("dispatch {id} failed to start (exit {code})"),
                    None => format!("dispatch {id} failed to start ({status})"),
                };
                if body.is_empty() {
                    ToolOutcome::err(header)
                } else {
                    ToolOutcome::err(format!("{header}\n{body}"))
                }
            }
        }
        None => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            ToolOutcome::ok(format!(
                "started {id} in the background; use wait to learn when it ends; output: {}",
                out_path.display()
            ))
        }
    }
}

fn format_dispatch_failure(output: &std::process::Output) -> String {
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stderr_str.lines().collect();
    let tail = if lines.len() > OUTPUT_LINE_BUDGET {
        lines[lines.len() - OUTPUT_LINE_BUDGET..].join("\n")
    } else if !stderr_str.trim().is_empty() {
        stderr_str.trim_end().to_string()
    } else {
        match output.status.code() {
            Some(code) => format!("exit {code}"),
            None => format!("terminated without exit code ({})", output.status),
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        tail
    } else {
        format!("{}\n{tail}", stdout.trim_end())
    }
}

fn serialize_dispatch_args<T: serde::Serialize>(args: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(args).map_err(|e| format!("dispatch failed to serialize arguments: {e}"))
}

fn dispatch_parent_session() -> String {
    std::env::var("BLYTE_SESSION")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            format!("{now}-{}", std::process::id())
        })
}

fn run_command_bounded(
    mut cmd: std::process::Command,
    timeout: Duration,
) -> Result<(i32, String, String), String> {
    use std::io::Read;
    use std::process::Stdio;

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("blyte failed to spawn: {e}"))?;

    let mut so = child.stdout.take().ok_or("no stdout")?;
    let mut se = child.stderr.take().ok_or("no stderr")?;
    let t_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        String::from_utf8_lossy(&b).into_owned()
    });
    let t_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        String::from_utf8_lossy(&b).into_owned()
    });

    let deadline = std::time::Instant::now() + timeout;
    let mut poll = POLL_START;
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| format!("blyte wait failed: {e}"))?
        {
            Some(s) => break Some(s),
            None if std::time::Instant::now() >= deadline => {
                #[cfg(unix)]
                {
                    let pgid = child.id() as i32;
                    if pgid > 0 {
                        unsafe {
                            libc::kill(-pgid, libc::SIGKILL);
                        }
                    } else {
                        let _ = child.kill();
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill();
                }
                let _ = child.wait();
                break None;
            }
            None => {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                let (sleep, next) = poll_backoff(poll, remaining);
                std::thread::sleep(sleep);
                poll = next;
            }
        }
    };

    let stdout = t_out
        .join()
        .map_err(|_| "blyte: reading the command's output failed".to_string())?;
    let stderr = t_err
        .join()
        .map_err(|_| "blyte: reading the command's errors failed".to_string())?;

    match status {
        Some(s) => match s.code() {
            Some(code) => Ok((code, stdout, stderr)),
            None => Err(format!(
                "blyte ended without an exit code ({s}); its output so far:\n{stdout}{stderr}"
            )),
        },
        None => Ok((
            124,
            stdout,
            format!("{stderr}\n[killed by blyte after {}s]", timeout.as_secs()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_description_says_background_jobs_keep_running() {
        let all = specs();
        let desc = &all
            .iter()
            .find(|t| t.name == "bash")
            .expect("bash spec missing")
            .description;
        assert!(
            desc.contains(
                "A background job started with its output redirected keeps running after the command returns, and nothing reports when it exits."
            ),
            "got {desc:?}"
        );
        assert!(
            !desc.contains("there is no background mode"),
            "got {desc:?}"
        );
    }

    #[test]
    fn serialize_dispatch_args_surfaces_serialization_error() {
        struct FailingSerialize;

        impl serde::Serialize for FailingSerialize {
            fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("unsupported map key"))
            }
        }

        let err = serialize_dispatch_args(&FailingSerialize).unwrap_err();
        assert!(err.contains("serialize"), "got: {err}");
        assert!(err.contains("unsupported map key"), "got: {err}");
    }
}
