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
use std::time::Duration;

use blyte_provider::ToolSpec;
use serde_json::{json, Value};

use crate::fs::Fs;
use crate::shell::Shell;

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
                format!("{head} …[{} more chars clipped by blyte]", n - line_chars)
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
pub fn available_binaries(shell: &dyn Shell, root: &Path) -> Vec<String> {
    let mut all: Vec<&'static str> = ACCELERATORS.to_vec();
    all.extend_from_slice(TOOLCHAINS);
    shell.available_binaries(root, &all)
}

pub fn specs() -> Vec<ToolSpec> {
    vec![
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
            description: format!(
                "Run a shell command in the workspace and wait for it to finish. \
                 Commands run to completion (up to {}s) -- there is no background mode and \
                 nothing to poll. Output over {} lines is elided in the middle.",
                COMMAND_TIMEOUT.as_secs(),
                OUTPUT_LINE_BUDGET
            ),
            parameters: json!({"type":"object","properties":{
                "command":{"type":"string"}},"required":["command"]}),
        },
        ToolSpec {
            name: "glob".into(),
            description: "Find files by glob pattern, e.g. `src/**/*.rs`. Respects .gitignore; \
                          `.git` is skipped. Fast, and the right \
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
                 by content. Capped at {MAX_MATCHES} matches, and the total is always reported."
            ),
            parameters: json!({"type":"object","properties":{
                "pattern":{"type":"string","description":"regular expression"},
                "path":{"type":"string","description":"subtree to search; defaults to the workspace"},
                "glob":{"type":"string","description":"restrict to files matching this glob, e.g. `*.ts`"}},
                "required":["pattern"]}),
        },
    ]
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
            "grep -rnI {excludes} {globarg} -e {} {} | head -n {}",
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
/// container with a bind-mounted workspace shares the orchestrator's files while running its
/// own commands, and a remote sandbox shares neither. Pairing the wrong two would have the
/// model editing one filesystem and verifying another.
pub fn execute(
    root: &Path,
    shell: &dyn Shell,
    fs: &dyn Fs,
    name: &str,
    args: &Value,
) -> ToolOutcome {
    let mut outcome = execute_unbounded(root, shell, fs, name, args);
    outcome.content = if name == "read" {
        bound_output_with(&outcome.content, READ_LINE_BUDGET, READ_LINE_CHARS)
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
) -> ToolOutcome {
    let s = |k: &str| {
        args.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    match name {
        "read" => {
            let path = match resolve_in_workspace(fs, root, &s("path")) {
                Ok(p) => p,
                Err(e) => return ToolOutcome::err(e),
            };
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
            let path = match resolve_in_workspace(fs, root, &s("path")) {
                Ok(p) => p,
                Err(e) => return ToolOutcome::err(e),
            };
            match fs.write(&path, &s("content")) {
                Ok(_) => ToolOutcome::ok(format!("wrote {}", s("path"))),
                Err(e) => ToolOutcome::err(e),
            }
        }
        "edit" => {
            let path = match resolve_in_workspace(fs, root, &s("path")) {
                Ok(p) => p,
                Err(e) => return ToolOutcome::err(e),
            };
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
        "bash" => match shell.exec_user(&s("command"), root, COMMAND_TIMEOUT) {
            Ok(o) => {
                let body = distil(o.merged().trim_end(), OUTPUT_LINE_BUDGET);
                // Exit code always stated: a non-zero exit with quiet output reads as success.
                ToolOutcome {
                    content: format!("exit {}\n{body}", o.exit_code),
                    is_error: o.exit_code != 0,
                }
            }
            Err(e) => ToolOutcome::err(e),
        },
        "glob" => {
            let pattern = s("pattern");
            // .gitignore-aware, fd/ripgrep style. A local workspace expands in-process; a
            // container lists its repository with one `git ls-files` and matches here. Outside a
            // git repository, or for patterns not handled here, the shell glob below runs as before.
            let outcome = |hits: Vec<String>| {
                if hits.is_empty() {
                    ToolOutcome::ok(format!("no files match {pattern}"))
                } else {
                    ToolOutcome::ok(format!("{} file(s):\n{}", hits.len(), hits.join("\n")))
                }
            };
            if fs.is_local() {
                if let Some(hits) = crate::search::glob_paths(root, &pattern, MAX_MATCHES) {
                    return outcome(hits);
                }
            } else if let Ok(o) = shell.exec(
                "if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then \
                 git -c core.quotePath=off ls-files -co --exclude-standard; \
                 else echo __BLYTE_NO_GIT__; fi",
                root,
                COMMAND_TIMEOUT,
            ) {
                if o.exit_code == 0 && !o.stdout.starts_with("__BLYTE_NO_GIT__") {
                    if let Some(hits) = crate::search::glob_filter(&o.stdout, &pattern, MAX_MATCHES)
                    {
                        return outcome(hits);
                    }
                }
            }
            // Delegated to the shell rather than a glob crate so that `**`, brace expansion and
            // character classes behave exactly as the model expects from a terminal.
            // When bash has globstar (Linux / bash 4+), use it. On platforms where bash lacks
            // globstar (such as macOS /bin/bash 3.2), fall back to zsh which supports `**` natively.
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
            match shell.exec(&cmd, root, COMMAND_TIMEOUT) {
                Ok(o) => {
                    let body = o.stdout.clone();
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
            let sub = {
                let p = s("path");
                if p.is_empty() {
                    ".".to_string()
                } else {
                    p
                }
            };
            if let Err(e) = resolve_in_workspace(fs, root, &sub) {
                return ToolOutcome::err(e);
            }
            let glob_str = s("glob");
            // A local workspace is searched in-process with ripgrep's libraries; elsewhere one
            // command, using rg when the environment has it (no separate probe).
            let lines: Vec<String> = if fs.is_local() {
                match crate::search::grep_lines(root, &sub, &pattern, &glob_str, MAX_MATCHES + 1) {
                    Ok(l) => l,
                    Err(e) => return ToolOutcome::err(format!("grep failed: {e}")),
                }
            } else {
                let cmd = format!(
                    "if command -v rg >/dev/null 2>&1; then {}; else {}; fi",
                    grep_command(true, &pattern, &sub, &glob_str),
                    grep_command(false, &pattern, &sub, &glob_str)
                );
                match shell.exec(&cmd, root, COMMAND_TIMEOUT) {
                    Ok(o) => o
                        .stdout
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(String::from)
                        .collect(),
                    Err(e) => return ToolOutcome::err(format!("grep failed: {e}")),
                }
            };
            if lines.is_empty() {
                return ToolOutcome::ok(format!("no matches for {pattern}"));
            }
            if lines.len() > MAX_MATCHES {
                ToolOutcome::ok(format!(
                    "{}\n\n[capped at {MAX_MATCHES} matches; narrow the pattern or pass `glob` \
                     to see the rest]",
                    lines[..MAX_MATCHES].join("\n")
                ))
            } else {
                ToolOutcome::ok(format!("{} match(es):\n{}", lines.len(), lines.join("\n")))
            }
        }
        other => ToolOutcome::err(format!("unknown tool: {other}")),
    }
}
