//! Claude Code status line showing session Claude cost and blyte dispatch costs.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Resolved auto-compact window configuration for the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactWindow {
    /// An explicit token window (`100_000..=1_000_000`).
    Window(u64),
    /// Use the model's own context window (nothing set, or `"auto"`).
    Tuned,
    /// An invalid setting or unreadable/unparseable settings file, with a description.
    Invalid(String),
}

/// Returns the settings file paths in precedence order (highest first):
/// 1. `/Library/Application Support/ClaudeCode/managed-settings.json`
/// 2. `<project_dir>/.claude/settings.local.json` (when `workspace.project_dir` is present)
/// 3. `<project_dir>/.claude/settings.json` (when `workspace.project_dir` is present)
/// 4. `<home>/.claude/settings.json` (when `home` is `Some`)
pub fn settings_files(d: &serde_json::Value, home: Option<&Path>) -> Vec<PathBuf> {
    let mut files = Vec::with_capacity(4);
    files.push(PathBuf::from(
        "/Library/Application Support/ClaudeCode/managed-settings.json",
    ));
    if let Some(project_dir) = d
        .get("workspace")
        .and_then(|w| w.get("project_dir"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
    {
        let claude_dir = Path::new(project_dir).join(".claude");
        files.push(claude_dir.join("settings.local.json"));
        files.push(claude_dir.join("settings.json"));
    }
    if let Some(h) = home.filter(|p| !p.as_os_str().is_empty()) {
        files.push(h.join(".claude").join("settings.json"));
    }
    files
}

fn normalize_model_key(mut s: &str) -> &str {
    loop {
        let prev = s;
        if let Some(without_bracket) = s.strip_suffix(']') {
            if let Some(idx) = without_bracket.rfind('[') {
                if idx > 0 {
                    s = &without_bracket[..idx];
                }
            }
        }
        if s.len() > 9 {
            let (prefix, suffix) = s.split_at(s.len() - 9);
            if let Some(date) = suffix.strip_prefix('-') {
                if date.bytes().all(|b| b.is_ascii_digit()) {
                    s = prefix;
                }
            }
        }
        if s == prev {
            break;
        }
    }
    s
}

fn parse_env_window(raw: &str) -> CompactWindow {
    let parsed = if let Some(num_str) = raw.strip_suffix('k') {
        num_str
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_mul(1_000))
            .or_else(|| {
                num_str
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite() && *f >= 0.0)
                    .map(|f| (f * 1_000.0).round() as u64)
            })
    } else if let Some(num_str) = raw.strip_suffix('M') {
        num_str
            .parse::<u64>()
            .ok()
            .and_then(|n| n.checked_mul(1_000_000))
            .or_else(|| {
                num_str
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite() && *f >= 0.0)
                    .map(|f| (f * 1_000_000.0).round() as u64)
            })
    } else {
        raw.parse::<u64>().ok()
    };

    match parsed {
        Some(n) if (100_000..=1_000_000).contains(&n) => CompactWindow::Window(n),
        Some(n) => CompactWindow::Invalid(format!(
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW={raw} ({n}) out of range 100000..=1000000"
        )),
        None => CompactWindow::Invalid(format!(
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW={raw} is not a valid window"
        )),
    }
}

fn parse_json_window(path: &Path, key_desc: &str, v: &serde_json::Value) -> CompactWindow {
    if v.as_str() == Some("auto") {
        return CompactWindow::Tuned;
    }
    let num = v.as_u64().or_else(|| {
        v.as_f64()
            .filter(|f| f.is_finite() && f.fract() == 0.0 && *f >= 0.0 && *f <= u64::MAX as f64)
            .map(|f| f as u64)
    });
    match num {
        Some(n) if (100_000..=1_000_000).contains(&n) => CompactWindow::Window(n),
        Some(n) => CompactWindow::Invalid(format!(
            "{}: {key_desc}={n} out of range 100000..=1000000",
            path.display()
        )),
        None => CompactWindow::Invalid(format!("{}: invalid {key_desc} {v}", path.display())),
    }
}

/// Resolves the auto-compact window from `CLAUDE_CODE_AUTO_COMPACT_WINDOW` or settings files
/// in scope order (highest precedence first).
pub fn resolve_compact_window<P: AsRef<Path>>(
    model_id: Option<&str>,
    env_val: Option<&str>,
    settings_files: &[P],
) -> CompactWindow {
    if let Some(raw) = env_val.map(str::trim).filter(|s| !s.is_empty()) {
        return parse_env_window(raw);
    }

    let norm_model = model_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(normalize_model_key);

    for path_ref in settings_files {
        let path = path_ref.as_ref();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return CompactWindow::Invalid(format!("{}: {e}", path.display())),
        };
        let val: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => return CompactWindow::Invalid(format!("{}: {e}", path.display())),
        };
        let Some(root) = val.as_object() else {
            return CompactWindow::Invalid(format!("{}: expected JSON object", path.display()));
        };

        if let (Some(target_key), Some(ms_obj)) = (
            norm_model,
            root.get("modelSettings").and_then(|v| v.as_object()),
        ) {
            let matched = model_id
                .map(str::trim)
                .and_then(|raw_id| {
                    ms_obj
                        .get(raw_id)
                        .and_then(|v| v.get("autoCompactWindow"))
                        .map(|acw| (raw_id, acw))
                })
                .or_else(|| {
                    ms_obj.iter().find_map(|(k, v)| {
                        (normalize_model_key(k) == target_key)
                            .then(|| v.get("autoCompactWindow").map(|acw| (k.as_str(), acw)))
                            .flatten()
                    })
                });
            if let Some((k, acw)) = matched {
                return parse_json_window(
                    path,
                    &format!("modelSettings.{k}.autoCompactWindow"),
                    acw,
                );
            }
        }

        if let Some(acw) = root.get("autoCompactWindow") {
            return parse_json_window(path, "autoCompactWindow", acw);
        }
    }

    CompactWindow::Tuned
}

/// Generates the base status line from Claude Code's JSON report.
///
/// Parts joined with ` · `:
/// - `d.model.display_name`, or `"claude"` when missing.
/// - context:
///   - with `p = d.context_window.used_percentage` and `size = d.context_window.context_window_size` (or `.size`):
///     `used = p / 100 * size`:
///     - `CompactWindow::Window(w)`: `limit = min(size, w)`, text `context {used/1000:.0}k of {limit/1000}k`,
///       plus ` — consider /compact or a new session` when `100 * used / limit >= 60`.
///     - `CompactWindow::Tuned`: `limit = size`, text `context {used/1000:.0}k of {limit/1000}k`,
///       plus ` — consider /compact or a new session` when `100 * used / limit >= 60`.
///     - `CompactWindow::Invalid(msg)`: `context {used/1000:.0}k of {size/1000}k (auto-compact window unknown: {msg})`,
///       with no suggestion.
///   - with `p` but no size: `context {p:.0}%`.
///   - without `p`: no part.
/// - `${d.cost.total_cost_usd:.2} this session` when present.
pub fn base_line(d: &serde_json::Value, compact_window: CompactWindow) -> String {
    let model_name = d
        .get("model")
        .and_then(|m| {
            m.get("display_name")
                .and_then(|v| v.as_str())
                .or_else(|| m.as_str())
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("claude");

    let cw = d.get("context_window");
    let context_part = if let Some(p) = cw
        .and_then(|c| c.get("used_percentage"))
        .and_then(|v| v.as_f64())
        .filter(|&p| !p.is_nan())
    {
        let size = cw
            .and_then(|c| c.get("context_window_size").or_else(|| c.get("size")))
            .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
            .filter(|&s| s > 0);

        if let Some(size) = size {
            let used = (p / 100.0) * (size as f64);
            match compact_window {
                CompactWindow::Window(w) => {
                    let limit = size.min(w);
                    let mut text = format!("context {:.0}k of {}k", used / 1000.0, limit / 1000);
                    if limit > 0 && (100.0 * used / (limit as f64)) >= 60.0 {
                        text.push_str(" — consider /compact or a new session");
                    }
                    Some(text)
                }
                CompactWindow::Tuned => {
                    let limit = size;
                    let mut text = format!("context {:.0}k of {}k", used / 1000.0, limit / 1000);
                    if (100.0 * used / (limit as f64)) >= 60.0 {
                        text.push_str(" — consider /compact or a new session");
                    }
                    Some(text)
                }
                CompactWindow::Invalid(msg) => Some(format!(
                    "context {:.0}k of {}k (auto-compact window unknown: {msg})",
                    used / 1000.0,
                    size / 1000
                )),
            }
        } else {
            Some(format!("context {:.0}%", p))
        }
    } else {
        None
    };

    let cost_part = d
        .get("cost")
        .and_then(|c| c.get("total_cost_usd"))
        .and_then(|v| v.as_f64())
        .filter(|&c| !c.is_nan())
        .map(|cost| format!("${:.2} this session", cost));

    let mut parts = Vec::new();
    parts.push(model_name.to_string());
    if let Some(cp) = context_part {
        parts.push(cp);
    }
    if let Some(cp) = cost_part {
        parts.push(cp);
    }
    parts.join(" · ")
}

/// Formats the blyte dispatch cost segment.
///
/// Returns `None` when `totals.runs == 0`; otherwise `blyte ${implementers_usd:.2} ({runs} runs)`,
/// followed by ` · total ${claude_cost + implementers_usd:.2}` when `claude_cost` is `Some`.
pub fn blyte_part(totals: &crate::cost::Totals, claude_cost: Option<f64>) -> Option<String> {
    if totals.runs == 0 {
        return None;
    }
    match claude_cost {
        Some(cc) => Some(format!(
            "blyte ${:.2} ({} runs) · total ${:.2}",
            totals.implementers_usd,
            totals.runs,
            cc + totals.implementers_usd
        )),
        None => Some(format!(
            "blyte ${:.2} ({} runs)",
            totals.implementers_usd, totals.runs
        )),
    }
}

/// Runs a command via `sh -c` passing `stdin_bytes`, with a timeout and discarded stderr.
///
/// Returns the first line of trimmed stdout if the command succeeded and output was non-empty.
fn run_after(cmd_str: &str, stdin_bytes: &[u8], timeout: Duration) -> Option<String> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(cmd_str);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().ok()?;

    // The command may exit without reading its input; main restores the default SIGPIPE, which would
    // then kill this process on the write, so ignore it here and take the EPIPE error instead.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_bytes);
        let _ = stdin.flush();
    }

    let stdout_handle = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(mut r) = stdout_handle {
            let _ = std::io::Read::read_to_end(&mut r, &mut out);
        }
        out
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() >= deadline => {
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
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };

    let stdout_bytes = reader.join().unwrap_or_default();
    let status = status?;
    if !status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&stdout_bytes);
    let first_line = s.lines().next()?.trim();
    if first_line.is_empty() {
        return None;
    }
    Some(first_line.to_string())
}

/// Executes the `blyte statusline` subcommand.
pub fn run(args: &crate::cli::StatuslineArgs) -> i32 {
    let mut stdin_bytes = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut stdin_bytes);

    let d: serde_json::Value = serde_json::from_slice(&stdin_bytes)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));

    let resolve = || {
        let model_id = d
            .get("model")
            .and_then(|m| m.get("id"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty());
        let env_val = std::env::var("CLAUDE_CODE_AUTO_COMPACT_WINDOW").ok();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let files = settings_files(&d, home.as_deref());
        resolve_compact_window(model_id, env_val.as_deref(), &files)
    };

    let base = match args.after {
        Some(ref cmd) => run_after(cmd, &stdin_bytes, Duration::from_secs(2))
            .unwrap_or_else(|| base_line(&d, resolve())),
        None => base_line(&d, resolve()),
    };

    let session_id = d
        .get("session_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty());

    let blyte_p = match session_id {
        Some(sid) => {
            let entries = crate::usage_log::default_path()
                .as_deref()
                .map(crate::usage_log::read)
                .unwrap_or_default();
            let totals = crate::cost::totals(&entries, Some(sid), None, None);
            let claude_cost = d
                .get("cost")
                .and_then(|c| c.get("total_cost_usd"))
                .and_then(|v| v.as_f64());
            blyte_part(&totals, claude_cost)
        }
        None => None,
    };

    let line = match blyte_p {
        Some(part) => format!("{base} · {part}"),
        None => base,
    };

    println!("{line}");
    0
}
