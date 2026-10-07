//! Claude Code status line showing session Claude cost and blyte dispatch costs.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Generates the base status line from Claude Code's JSON report.
///
/// Parts joined with ` · `:
/// - `d.model.display_name`, or `"claude"` when missing.
/// - context:
///   - with `p = d.context_window.used_percentage` and `size = d.context_window.context_window_size` (or `.size`):
///     `used = p / 100 * size`, `limit = min(size, compact_window.unwrap_or(200000))`,
///     text `context {used/1000:.0}k of {limit/1000}k`, plus ` — consider /compact or a new session` when
///     `100 * used / limit >= 60`.
///   - with `p` but no size: `context {p:.0}%`.
///   - without `p`: no part.
/// - `${d.cost.total_cost_usd:.2} this session` when present.
pub fn base_line(d: &serde_json::Value, compact_window: Option<u64>) -> String {
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
            let limit = size.min(compact_window.filter(|&cw| cw > 0).unwrap_or(200_000));
            let used = (p / 100.0) * (size as f64);
            let mut text = format!("context {:.0}k of {}k", used / 1000.0, limit / 1000);
            if limit > 0 && (100.0 * used / (limit as f64)) >= 60.0 {
                text.push_str(" — consider /compact or a new session");
            }
            Some(text)
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

/// Retrieves `autoCompactWindow` from `~/.claude/settings.json`, if present.
pub fn compact_window_from_settings() -> Option<u64> {
    let home = std::env::var_os("HOME")?;
    let path = std::path::Path::new(&home)
        .join(".claude")
        .join("settings.json");
    let content = std::fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get("autoCompactWindow")
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
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

    let compact_window = compact_window_from_settings();

    let base = match args.after {
        Some(ref cmd) => run_after(cmd, &stdin_bytes, Duration::from_secs(2))
            .unwrap_or_else(|| base_line(&d, compact_window)),
        None => base_line(&d, compact_window),
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
