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
use std::process::Command;
use std::time::Duration;

use serde_json::{json, Value};
use superstate_provider::ToolSpec;

/// Output over this many lines is distilled to head and tail before entering context.
///
/// Sized from measurement, not intuition: across 64 recorded command outputs the median was
/// 165-307 lines and exactly one exceeded a thousand -- at 25,105 lines, 88% of that run's
/// entire output volume and 26x the next largest (FINDINGS 13.26). The distribution is a heavy
/// tail, so the bound exists for the rare case; it must not clip the common one.
pub const OUTPUT_LINE_BUDGET: usize = 400;

/// Most matches a single search returns. The count of what was cut is always reported, so a
/// truncated search reads as truncated rather than as a complete answer with few hits.
pub const MAX_MATCHES: usize = 200;

/// Hard ceiling on a single command. Long is fine -- unbounded is not.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(900);

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutcome {
    fn ok(s: impl Into<String>) -> Self { Self { content: s.into(), is_error: false } }
    fn err(s: impl Into<String>) -> Self { Self { content: s.into(), is_error: true } }
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
    out.push_str(&format!("\n\n[... {dropped} lines elided by superstate ...]\n\n"));
    out.push_str(&lines[lines.len() - tail..].join("\n"));
    out
}

/// Reject a path that escapes the workspace.
///
/// Checked against the CANONICAL path, so `../` and symlinks cannot walk out. The previous
/// harness relied on a prompt rule -- "confine all inspections to the workspace" -- which is
/// exactly the kind of instruction Invariant IV exists because agents ignore under pressure.
fn resolve_in_workspace(root: &Path, raw: &str) -> Result<PathBuf, String> {
    let joined = if Path::new(raw).is_absolute() { PathBuf::from(raw) } else { root.join(raw) };
    let root_c = root.canonicalize().map_err(|e| format!("workspace root unreadable: {e}"))?;

    // Canonicalise the NEAREST EXISTING ancestor, starting with the target itself. Walking
    // straight to `.parent()` was wrong for a directory target: `Path::new("/a/b/.").parent()`
    // is `/a`, not `/a/b`, so searching the workspace root as `.` resolved to its parent and was
    // refused. Starting at the target and climbing only when it does not exist keeps both cases
    // right -- an existing directory checks itself, a file about to be created checks the
    // directory it will land in -- and neither is waved through unchecked.
    let mut probe = joined.as_path();
    let probe_c = loop {
        if let Ok(c) = probe.canonicalize() {
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

/// Single-quote for `bash -lc`, so a pattern containing spaces, `$` or `"` is passed verbatim.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Whether a binary is on PATH. Used to choose ripgrep over grep, and to tell the model in the
/// system prompt what its environment actually has -- the previous harness left the worker to
/// discover that by trial, and when container guidance was missing it edited the host instead.
pub fn have(bin: &str) -> bool {
    Command::new("bash").arg("-lc").arg(format!("command -v {bin} >/dev/null"))
        .output().map(|o| o.status.success()).unwrap_or(false)
}

/// The CLI tools present in this environment, for the system prompt.
pub fn available_binaries() -> Vec<&'static str> {
    ["rg", "fd", "jq", "git", "sed", "awk", "find", "xargs", "curl", "make",
     "cargo", "go", "npm", "yarn", "pnpm", "python3", "pytest", "node"]
        .into_iter().filter(|b| have(b)).collect()
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read".into(),
            description: "Read a file. Prefer a line range for large files.".into(),
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
            description: format!(
                "Run a shell command in the workspace and wait for it to finish. \
                 Commands run to completion (up to {}s) -- there is no background mode and \
                 nothing to poll. Output over {} lines is elided in the middle.",
                COMMAND_TIMEOUT.as_secs(), OUTPUT_LINE_BUDGET),
            parameters: json!({"type":"object","properties":{
                "command":{"type":"string"}},"required":["command"]}),
        },
        ToolSpec {
            name: "glob".into(),
            description: "Find files by glob pattern, e.g. `src/**/*.rs`. Fast, and the right \
                          first move when you do not know where something lives."
                .into(),
            parameters: json!({"type":"object","properties":{
                "pattern":{"type":"string","description":"glob, relative to the workspace"}},
                "required":["pattern"]}),
        },
        ToolSpec {
            name: "grep".into(),
            description: format!(
                "Search file contents by regular expression, returning `path:line:text`. \
                 Respects .gitignore. Use `glob` to find files by name and `grep` to find them \
                 by content. Capped at {MAX_MATCHES} matches, and the total is always reported."),
            parameters: json!({"type":"object","properties":{
                "pattern":{"type":"string","description":"regular expression"},
                "path":{"type":"string","description":"subtree to search; defaults to the workspace"},
                "glob":{"type":"string","description":"restrict to files matching this glob, e.g. `*.ts`"}},
                "required":["pattern"]}),
        },
        ToolSpec {
            name: "note".into(),
            description: "Record working state: findings, hypotheses, what you ruled out. \
                          Merged into your own region of the state file and carried across \
                          turns. Keys you set persist; setting a key to null removes it."
                .into(),
            parameters: json!({"type":"object","properties":{
                "patch":{"type":"object","description":"JSON object merged into your state"}},
                "required":["patch"]}),
        },
    ]
}

/// Execute one tool call. `note` is handled by the loop, which owns the state.
pub fn execute(root: &Path, name: &str, args: &Value) -> ToolOutcome {
    let s = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    match name {
        "read" => {
            let path = match resolve_in_workspace(root, &s("path")) {
                Ok(p) => p, Err(e) => return ToolOutcome::err(e) };
            let body = match std::fs::read_to_string(&path) {
                Ok(b) => b, Err(e) => return ToolOutcome::err(format!("cannot read {}: {e}", s("path"))) };
            let lines: Vec<&str> = body.lines().collect();
            let start = args.get("start_line").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
            let end = args.get("end_line").and_then(|v| v.as_u64())
                .map(|e| e as usize).unwrap_or(lines.len()).min(lines.len());
            if start > lines.len() {
                return ToolOutcome::err(format!("{} has {} lines; start_line {start} is past the end",
                                                s("path"), lines.len()));
            }
            // Numbered, because an agent that cannot cite a line cannot edit precisely.
            let body = lines[start - 1..end].iter().enumerate()
                .map(|(i, l)| format!("{:>6}\t{l}", start + i))
                .collect::<Vec<_>>().join("\n");
            ToolOutcome::ok(distil(&body, OUTPUT_LINE_BUDGET))
        }
        "write" => {
            let path = match resolve_in_workspace(root, &s("path")) {
                Ok(p) => p, Err(e) => return ToolOutcome::err(e) };
            if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
            match std::fs::write(&path, s("content")) {
                Ok(_) => ToolOutcome::ok(format!("wrote {}", s("path"))),
                Err(e) => ToolOutcome::err(format!("cannot write {}: {e}", s("path"))),
            }
        }
        "edit" => {
            let path = match resolve_in_workspace(root, &s("path")) {
                Ok(p) => p, Err(e) => return ToolOutcome::err(e) };
            let body = match std::fs::read_to_string(&path) {
                Ok(b) => b, Err(e) => return ToolOutcome::err(format!("cannot read {}: {e}", s("path"))) };
            let (old, new) = (s("old"), s("new"));
            match body.matches(&old).count() {
                0 => ToolOutcome::err(format!("`old` not found in {}. Read it first.", s("path"))),
                // Ambiguity is an error, never a guess: replacing the first of several matches
                // silently edits the wrong place and the agent cannot tell.
                n if n > 1 => ToolOutcome::err(format!(
                    "`old` appears {n} times in {}; include more context to make it unique", s("path"))),
                _ => match std::fs::write(&path, body.replacen(&old, &new, 1)) {
                    Ok(_) => ToolOutcome::ok(format!("edited {}", s("path"))),
                    Err(e) => ToolOutcome::err(format!("cannot write {}: {e}", s("path"))),
                },
            }
        }
        "bash" => {
            let out = Command::new("bash").arg("-lc").arg(s("command"))
                .current_dir(root).output();
            match out {
                Ok(o) => {
                    let merged = format!("{}{}", String::from_utf8_lossy(&o.stdout),
                                         String::from_utf8_lossy(&o.stderr));
                    let code = o.status.code().unwrap_or(-1);
                    let body = distil(merged.trim_end(), OUTPUT_LINE_BUDGET);
                    // Exit code always stated: a non-zero exit with quiet output otherwise
                    // reads as success.
                    ToolOutcome { content: format!("exit {code}\n{body}"), is_error: code != 0 }
                }
                Err(e) => ToolOutcome::err(format!("cannot run command: {e}")),
            }
        }
        "glob" => {
            let pattern = s("pattern");
            // Delegated to the shell rather than a glob crate so that `**`, brace expansion and
            // character classes behave exactly as the model expects from a terminal.
            let cmd = format!(
                "shopt -s globstar nullglob dotglob 2>/dev/null; printf '%s\\n' {pattern} | head -n {MAX_MATCHES}"
            );
            match Command::new("bash").arg("-lc").arg(&cmd).current_dir(root).output() {
                Ok(o) => {
                    let body = String::from_utf8_lossy(&o.stdout);
                    let hits: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
                    if hits.is_empty() {
                        ToolOutcome::ok(format!("no files match {pattern}"))
                    } else {
                        ToolOutcome::ok(format!("{} file(s):\n{}", hits.len(), hits.join("\n")))
                    }
                }
                Err(e) => ToolOutcome::err(format!("glob failed: {e}")),
            }
        }
        "grep" => {
            let pattern = s("pattern");
            let sub = { let p = s("path"); if p.is_empty() { ".".to_string() } else { p } };
            if let Err(e) = resolve_in_workspace(root, &sub) { return ToolOutcome::err(e); }
            let globarg = { let g = s("glob"); if g.is_empty() { String::new() }
                            else { format!("--glob {}", shell_quote(&g)) } };
            // ripgrep when present -- it honours .gitignore, which keeps node_modules and
            // target/ out of the results and is most of why a repo search is usable at all.
            // grep -rn is the fallback so the tool never simply vanishes on a bare machine.
            let cmd = if have("rg") {
                format!("rg --line-number --no-heading --color never {globarg} -e {} {} | head -n {}",
                        shell_quote(&pattern), shell_quote(&sub), MAX_MATCHES + 1)
            } else {
                format!("grep -rnI -e {} {} | head -n {}",
                        shell_quote(&pattern), shell_quote(&sub), MAX_MATCHES + 1)
            };
            match Command::new("bash").arg("-lc").arg(&cmd).current_dir(root).output() {
                Ok(o) => {
                    let body = String::from_utf8_lossy(&o.stdout);
                    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
                    if lines.is_empty() {
                        return ToolOutcome::ok(format!("no matches for {pattern}"));
                    }
                    if lines.len() > MAX_MATCHES {
                        ToolOutcome::ok(format!(
                            "{}\n\n[capped at {MAX_MATCHES} matches; narrow the pattern or pass `glob` \
                             to see the rest]", lines[..MAX_MATCHES].join("\n")))
                    } else {
                        ToolOutcome::ok(format!("{} match(es):\n{}", lines.len(), lines.join("\n")))
                    }
                }
                Err(e) => ToolOutcome::err(format!("grep failed: {e}")),
            }
        }
        other => ToolOutcome::err(format!("unknown tool: {other}")),
    }
}
