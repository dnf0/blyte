//! PreToolUse file read guard CLI command handler.
//!
//! Evaluates incoming tool call payloads on `stdin` conforming to agent
//! PreToolUse protocols (e.g. Antigravity, Claude Code) against deterministic
//! context boundaries and line budgets.

use clap::{Args, Subcommand};
use std::io::Read;
use std::path::PathBuf;
use superstate_core::command_guard::{
    evaluate_command_guard, CommandGuardConfig, CommandGuardDecision,
};
use superstate_core::guard::{
    bulk_read_target, evaluate_file_read_guard, FileReadGuardConfig, GuardDecision,
};
use superstate_core::read_cache::{self, CacheVerdict, ReadRecord};

/// Subcommands for the `superstate guard` namespace.
#[derive(Subcommand, Debug, Clone)]
pub enum GuardCommands {
    /// Evaluate file read operations against deterministic line boundaries
    PreToolUse(PreToolUseArgs),
    /// Evaluate command and task tool invocations against cooldown debounce
    PreCommand(PreCommandArgs),
}

/// Command-line arguments for `superstate guard pre-tool-use`.
#[derive(Args, Debug, Clone)]
pub struct PreToolUseArgs {
    /// Maximum lines permitted before shunting to symbol outline
    #[arg(long, default_value_t = superstate_core::guard::DEFAULT_READ_THRESHOLD_LINES)]
    pub threshold_lines: usize,

    /// Refuse a read whose lines are already in context, answering with a pointer.
    ///
    /// DEFAULT OFF: measured at +37% and +55% input tokens in EXP-081 (FINDINGS 13.25).
    ///
    /// It was built expecting to save 45-68% of read cost, and the premise was wrong. A
    /// PreToolUse hook cannot prevent a tool call -- by the time it runs, the model has emitted
    /// the request and the conversation has already been re-sent, so the per-call preamble is
    /// spent before the guard sees anything. The guard can only change what the RESPONSE costs.
    ///
    /// Worse, 89-91% of suppressions were immediately insisted on: a worker that has decided it
    /// needs lines 120-180 needs them, and being told it already has them does not change its
    /// plan. So a suppression adds a round trip rather than saving one, about -1,900 tokens
    /// each before compounding.
    ///
    /// Kept switchable because the insist rate is the thing to attack if anyone returns to
    /// this. Nothing measured so far suggests a refusal a worker accepts is achievable here.
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    pub read_cache: bool,
}

/// Command-line arguments for `superstate guard pre-command`.
#[derive(Args, Debug, Clone)]
pub struct PreCommandArgs {
    /// Cooldown window in seconds before allowing repeated polling queries
    #[arg(long, default_value_t = 30)]
    pub debounce_sec: u64,

    /// Optional path to state file tracking execution history
    #[arg(long)]
    pub state_path: Option<PathBuf>,

    /// Line budget for a whole-file dump performed through the shell (`cat`, `less`, unbounded
    /// `head`/`tail`). Matches the file viewing tool's budget so the channel cannot be used to
    /// route around it.
    #[arg(long, default_value_t = superstate_core::guard::DEFAULT_READ_THRESHOLD_LINES)]
    pub bulk_read_threshold_lines: usize,
}

/// Serializes and prints the guard decision JSON to standard output.
///
/// # Parameters
/// - `decision`: The outcome to emit.
/// Upper bound on how long a throttled poll may be delayed inside the hook.
const MAX_THROTTLE_SEC: u64 = 30;

fn emit_decision<T: serde::Serialize>(decision: &T) {
    // Decision is serialized to JSON string conforming to hook stdout protocol.
    if let Ok(json) = serde_json::to_string(decision) {
        println!("{}", json);
    } else {
        println!(r#"{{"decision":"allow"}}"#);
    }
}

/// Helper to emit an `allow` decision when fail-open behavior is triggered.
fn emit_allow() {
    emit_decision(&GuardDecision::Allow);
}

/// Extracts tool name and arguments from a JSON container.
///
/// Supports various agent hook payload conventions including `toolName`, `tool_name`,
/// `name`, `args`, `arguments`, `parameters`, and `tool_input`.
fn extract_from_container(
    container: &serde_json::Value,
) -> Option<(String, serde_json::Map<String, serde_json::Value>)> {
    // Extract tool name string from common key variants.
    let raw_name = container
        .get("name")
        .or_else(|| container.get("toolName"))
        .or_else(|| container.get("tool_name"))
        .or_else(|| container.get("tool"))
        .and_then(|v| v.as_str())?;

    // Extract arguments map or parse if arguments was encoded as a JSON string.
    let raw_args = container
        .get("args")
        .or_else(|| container.get("arguments"))
        .or_else(|| container.get("parameters"))
        .or_else(|| container.get("tool_input"));

    let args_map = match raw_args {
        Some(serde_json::Value::Object(map)) => map.clone(),
        Some(serde_json::Value::String(s)) => {
            serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(s)
                .unwrap_or_default()
        }
        _ => serde_json::Map::new(),
    };

    Some((raw_name.to_string(), args_map))
}

/// Finds the tool name and arguments map from standard and alternate hook payload formats.
///
/// Checks both nested objects (`toolCall` or `tool_call`) and top-level fields.
fn extract_tool_invocation(
    root: &serde_json::Value,
) -> Option<(String, serde_json::Map<String, serde_json::Value>)> {
    // First inspect nested tool call wrapper if present.
    if let Some(nested) = root.get("toolCall").or_else(|| root.get("tool_call")) {
        if let Some(res) = extract_from_container(nested) {
            return Some(res);
        }
    }
    // Fall back to inspecting top-level payload attributes.
    extract_from_container(root)
}

/// Extracts the target file path from common argument key variants.
///
/// Returns `None` if no path key is present or the string is empty/whitespace.
fn extract_path(args: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    const PATH_KEYS: &[&str] = &[
        "AbsolutePath",
        "path",
        "file_path",
        "filePath",
        "TargetFile",
        "target_file",
        "file",
    ];

    for &key in PATH_KEYS {
        if let Some(val) = args.get(key) {
            if let Some(s) = val.as_str() {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// Extracts a 1-indexed line number from the arguments map.
///
/// Handles numeric integers as well as string-encoded numbers.
fn extract_line(args: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<usize> {
    for &key in keys {
        if let Some(val) = args.get(key) {
            if let Some(n) = val.as_u64() {
                return Some(n as usize);
            }
            if let Some(n) = val.as_i64() {
                if n >= 0 {
                    return Some(n as usize);
                }
            }
            if let Some(s) = val.as_str() {
                if let Ok(n) = s.parse::<usize>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Pulls the shell command string out of a tool payload, across the key spellings agents use.
fn extract_command_line(args: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    for key in [
        "CommandLine",
        "command_line",
        "commandLine",
        "command",
        "cmd",
        "script",
    ] {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Resolves the workspace root the guards write their telemetry under.
fn telemetry_root() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("SUPERSTATE_WORKTREE_PATH") {
        if !v.trim().is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    let cwd = std::env::current_dir().ok()?;
    if cwd.file_name().and_then(|n| n.to_str()) == Some(".agents") {
        return cwd.parent().map(|p| p.to_path_buf());
    }
    Some(cwd)
}

/// Appends one line to `<root>/.agent-state/guard-activity.jsonl` for EVERY evaluated tool call.
///
/// Denials alone cannot answer the question that matters. EXP-075 had a turn admit 1,917,135
/// tokens against a 133,550-token codebase -- fourteen times the source -- and nothing recorded
/// says whether that was the same files read repeatedly or volume from another channel. A guard
/// sees every read and every command, so it is the cheapest place to record what was touched,
/// how large it was, and how often. This is the trace the daemon arm otherwise lacks (13.7).
///
/// Best effort: a guard that cannot write telemetry must still guard.
fn record_activity(
    kind: &str,
    target: &str,
    lines: usize,
    decision: &str,
    range: Option<(usize, usize)>,
) {
    let Some(root) = telemetry_root() else { return };
    let dir = root.join(".agent-state");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // The requested window, not just the file. Without it "24 reads of formatters.go" cannot be
    // separated into 24 different windows of a 1,559-line file -- correct behaviour under a
    // 200-line budget -- and 24 fetches of the same lines, which a read cache would eliminate.
    // EXP-079 recorded the former shape and could not answer which it was.
    let window = match range {
        Some((start, end)) => format!(",\"start\":{},\"end\":{}", start, end),
        None => String::new(),
    };
    let line = format!(
        "{{\"ts\":{},\"kind\":{},\"target\":{},\"lines\":{},\"decision\":{}{}}}\n",
        stamp,
        serde_json::Value::String(kind.to_string()),
        serde_json::Value::String(target.to_string()),
        lines,
        serde_json::Value::String(decision.to_string()),
        window,
    );
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("guard-activity.jsonl"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Appends one line to `<root>/.agent-state/guard-denials.jsonl` recording a blocked read.
///
/// The guard previously left no trace at all when it denied, which is why it took until
/// EXP-075 to establish that it had never fired: absence of denials in the artefacts was
/// indistinguishable from absence of the guard. A denial is the mechanism working, so it
/// should be countable -- `lines` is what was withheld from the prompt.
///
/// Best effort: a guard that cannot write its own telemetry must still guard.
/// Path of the append-only read-cache log, alongside the activity trace.
fn read_cache_path() -> Option<PathBuf> {
    telemetry_root().map(|r| r.join(".agent-state").join("read-cache.jsonl"))
}

/// Every record written so far. A malformed line is skipped rather than fatal: a corrupt
/// cache must degrade to "serve the read", never to "block the worker".
fn load_read_cache() -> Vec<ReadRecord> {
    let Some(path) = read_cache_path() else {
        return Vec::new();
    };
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(|l| serde_json::from_str::<ReadRecord>(l).ok())
        .collect()
}

fn append_read_cache(record: &ReadRecord) {
    let Some(path) = read_cache_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
    }
    if let Ok(line) = serde_json::to_string(record) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "{line}");
        }
    }
}

fn record_denial(resolved: &std::path::Path, lines: usize) {
    let mut root = std::env::var("SUPERSTATE_WORKTREE_PATH")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from);
    if root.is_none() {
        if let Ok(cwd) = std::env::current_dir() {
            root = if cwd.file_name().and_then(|n| n.to_str()) == Some(".agents") {
                cwd.parent().map(|p| p.to_path_buf())
            } else {
                Some(cwd)
            };
        }
    }
    let Some(root) = root else { return };
    let dir = root.join(".agent-state");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!(
        "{{\"ts\":{},\"path\":{},\"lines\":{}}}\n",
        stamp,
        serde_json::Value::String(resolved.display().to_string()),
        lines
    );
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("guard-denials.jsonl"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Resolves a payload path to something that exists on disk.
///
/// The hook process does not run from the repository root: hooks are registered in
/// `<workspace>/.agents/hooks.json` and the agent invokes them from there, so a relative path
/// like `ark/schema/scope.ts` resolves to `<workspace>/.agents/ark/schema/scope.ts`, which does
/// not exist. `evaluate_file_read_guard` then fails open, and the guard has never once been
/// observed denying a read in any recorded run -- while EXP-075 measured a single turn
/// admitting 1.92M tokens of new context, of which shunted command output accounted for under
/// 2%. Unbounded file reads are the only remaining channel that large.
///
/// Candidates are tried in order and the first that exists wins. Everything still fails open:
/// a path we cannot locate is allowed, exactly as before.
fn resolve_payload_path(raw: &str) -> PathBuf {
    let direct = PathBuf::from(raw);
    if direct.is_absolute() || direct.exists() {
        return direct;
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    // The daemon exports this for the worker it spawns; it is the authoritative root.
    if let Ok(worktree) = std::env::var("SUPERSTATE_WORKTREE_PATH") {
        if !worktree.trim().is_empty() {
            roots.push(PathBuf::from(worktree));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        // Registered from `<workspace>/.agents`, so the workspace is the parent.
        if cwd.file_name().and_then(|n| n.to_str()) == Some(".agents") {
            if let Some(parent) = cwd.parent() {
                roots.push(parent.to_path_buf());
            }
        }
        roots.push(cwd);
    }

    for root in roots {
        let candidate = root.join(raw);
        if candidate.exists() {
            return candidate;
        }
    }
    direct
}

/// Executes the `pre-tool-use` hook logic.
///
/// # Parameters
/// - `args`: CLI arguments specifying threshold limits.
///
/// # Returns
/// - `Ok(())` on completion; exits 0 in all cases (fail-open on parse error).
pub fn cmd_pre_tool_use(args: PreToolUseArgs) -> Result<(), String> {
    // Read complete payload from stdin.
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        // Fail-open: if stdin read encounters an I/O error, allow tool invocation.
        emit_allow();
        return Ok(());
    }

    // Fail-open if stdin is empty.
    if input.trim().is_empty() {
        emit_allow();
        return Ok(());
    }

    // Parse JSON root payload; fail-open on malformed JSON to prevent agent aborts.
    let root: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => {
            emit_allow();
            return Ok(());
        }
    };

    // Extract tool invocation; fail-open if no recognisable tool invocation is found.
    let (tool_name, tool_args) = match extract_tool_invocation(&root) {
        Some(inv) => inv,
        None => {
            emit_allow();
            return Ok(());
        }
    };

    // Normalize tool name to lowercase and verify it is a file-reading tool.
    let norm_name = tool_name.to_lowercase();
    if !matches!(
        norm_name.as_str(),
        "view_file" | "view" | "read_file" | "readfile" | "viewfile"
    ) {
        // Non-file-reading tools are permitted unconditionally.
        emit_allow();
        return Ok(());
    }

    // Extract target file path; fail-open if path is absent or blank.
    let path_str = match extract_path(&tool_args) {
        Some(p) => p,
        None => {
            emit_allow();
            return Ok(());
        }
    };

    // Extract optional line range boundaries.
    let mut start_line = extract_line(
        &tool_args,
        &["StartLine", "start_line", "startLine", "offset"],
    );
    let end_line = extract_line(&tool_args, &["EndLine", "end_line", "endLine", "limit"]);

    // Crucial Normalization: If end_line is specified without start_line (e.g. head slice),
    // default start_line to 1 so the bounded slice calculation succeeds.
    if end_line.is_some() && start_line.is_none() {
        start_line = Some(1);
    }

    // Configure guard with CLI threshold settings.
    let config = FileReadGuardConfig {
        threshold_lines: args.threshold_lines,
    };

    // Evaluate proposed file read via superstate-core engine.
    let resolved = resolve_payload_path(&path_str);
    let body = std::fs::read_to_string(&resolved).ok();
    let lines = body.as_ref().map(|c| c.lines().count()).unwrap_or(0);

    // The read cache runs BEFORE the line budget. The budget decides how much of a file to
    // serve; the cache decides whether to make the call at all, and a call the worker does
    // not make costs nothing to bound. Checking it second would pay the preamble anyway.
    //
    // Only files we could actually read are eligible: an unreadable path must fall through to
    // the ordinary fail-open path so the runtime error reaches the worker.
    let mut cached_decision: Option<GuardDecision> = None;
    let mut cache_hit = false;
    let mut pending: Option<(usize, usize, u64)> = None;
    if args.read_cache {
        if let Some(content) = body.as_ref() {
            let hash = read_cache::content_hash(content);
            // A whole-file read is the window 1..=len, so it participates on the same terms.
            let (req_start, req_end) = match (start_line, end_line) {
                (Some(a), Some(b)) => (a, b),
                (Some(a), None) => (a, lines),
                (None, Some(b)) => (1, b),
                (None, None) => (1, lines),
            };
            let records = load_read_cache();
            match read_cache::evaluate(&records, &resolved, req_start, req_end, hash) {
                CacheVerdict::AlreadyHeld { .. } => {
                    cache_hit = true;
                    append_read_cache(&ReadRecord {
                        path: resolved.to_string_lossy().to_string(),
                        start: req_start,
                        end: req_end,
                        hash,
                        suppressed: true,
                    });
                    cached_decision = Some(GuardDecision::Deny {
                        reason: read_cache::suppression_reason(&resolved, req_start, req_end),
                    });
                }
                // Insisted or Miss: fall through to the budget. Whether the worker actually
                // RECEIVES these lines is not known yet, so nothing is recorded here.
                CacheVerdict::Insisted | CacheVerdict::Miss => {
                    pending = Some((req_start, req_end, hash));
                }
            }
        }
    }

    let decision = cached_decision
        .unwrap_or_else(|| evaluate_file_read_guard(&resolved, start_line, end_line, &config));
    let denied = matches!(decision, GuardDecision::Deny { .. });

    // Record a window as held ONLY when the worker is actually given it.
    //
    // Recording at request time instead was wrong in a way that would have starved the
    // worker: a whole-file read over the line budget is denied and served as an AST outline,
    // so the worker never receives those lines -- but the cache would have believed it had
    // them and suppressed every later read of any window inside the file. Caught by
    // test_flexible_payload_formats_and_tool_names, whose second read went from allow to deny.
    if let Some((req_start, req_end, hash)) = pending {
        if !denied {
            append_read_cache(&ReadRecord {
                path: resolved.to_string_lossy().to_string(),
                start: req_start,
                end: req_end,
                hash,
                suppressed: false,
            });
        }
    }
    // Every read is recorded, not just the blocked ones: the question "was the same file read
    // repeatedly?" cannot be answered from denials alone.
    record_activity(
        if start_line.is_some() || end_line.is_some() {
            "read_bounded"
        } else {
            "read_whole"
        },
        &resolved.display().to_string(),
        lines,
        if cache_hit {
            "cached"
        } else if denied {
            "deny"
        } else {
            "allow"
        },
        match (start_line, end_line) {
            (Some(a), Some(b)) => Some((a, b)),
            (Some(a), None) => Some((a, lines)),
            (None, Some(b)) => Some((1, b)),
            (None, None) => None,
        },
    );
    // A cache hit is not a budget denial: record_denial tracks lines the BUDGET withheld,
    // and conflating the two would make the cache look like a shunt in every report.
    if denied && !cache_hit {
        record_denial(&resolved, lines);
    }
    emit_decision(&decision);

    Ok(())
}

/// Executes the `pre-command` hook logic.
///
/// Evaluates incoming tool execution payloads against debounce and cooldown rules.
///
/// # Parameters
/// - `args`: CLI arguments specifying debounce window and optional state file path.
///
/// # Returns
/// - `Ok(())` on completion; exits 0 in all cases (fail-open on parse error).
pub fn cmd_pre_command(args: PreCommandArgs) -> Result<(), String> {
    // Read complete payload from stdin.
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        // Fail-open: if stdin read encounters an I/O error, allow tool invocation.
        emit_allow();
        return Ok(());
    }

    // Fail-open if stdin is empty.
    if input.trim().is_empty() {
        emit_allow();
        return Ok(());
    }

    // Parse JSON root payload; fail-open on malformed JSON to prevent agent aborts.
    let root: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => {
            emit_allow();
            return Ok(());
        }
    };

    // Extract tool invocation; fail-open if no recognisable tool invocation is found.
    let (tool_name, tool_args) = match extract_tool_invocation(&root) {
        Some(inv) => inv,
        None => {
            emit_allow();
            return Ok(());
        }
    };

    // Configure command guard with CLI parameters.
    let config = CommandGuardConfig {
        debounce_sec: args.debounce_sec,
        state_path: args.state_path,
    };

    // Evaluate proposed command execution via superstate-core engine.
    // Portal's `check-bash-read` equivalent: a shell command that dumps a whole file into the
    // conversation is a file read, and must meet the same line budget as the viewing tool. The
    // agent gets the AST outline back, so it still has a next action -- a guard that only
    // refuses is the mistake that ended turns.
    if let Some(cmd) = extract_command_line(&tool_args) {
        if let Some(target) = bulk_read_target(&cmd) {
            let resolved = resolve_payload_path(&target.to_string_lossy());
            let file_config = FileReadGuardConfig {
                threshold_lines: args.bulk_read_threshold_lines,
            };
            let decision = evaluate_file_read_guard(&resolved, None, None, &file_config);
            let lines = std::fs::read_to_string(&resolved)
                .map(|c| c.lines().count())
                .unwrap_or(0);
            let denied = matches!(decision, GuardDecision::Deny { .. });
            record_activity(
                "shell_read",
                &resolved.display().to_string(),
                lines,
                if denied { "deny" } else { "allow" },
                None,
            );
            if denied {
                record_denial(&resolved, lines);
                emit_decision(&decision);
                return Ok(());
            }
        }
    }

    let decision = evaluate_command_guard(&tool_name, &tool_args, &config);
    if let Some(cmd) = extract_command_line(&tool_args) {
        let label: String = cmd.chars().take(160).collect();
        record_activity(
            "command",
            &label,
            0,
            match decision {
                CommandGuardDecision::Allow => "allow",
                CommandGuardDecision::Throttle { .. } => "throttle",
                CommandGuardDecision::Deny { .. } => "deny",
            },
            None,
        );
    }

    // A throttled poll is delayed here, in the hook, and then allowed. Sleeping is the whole
    // mechanism: the agent's poll rate is capped at one per window, which is the token saving
    // Invariant IV asks for, while the agent still gets its answer and keeps working. Denying
    // instead removed its only means of awaiting a background task and ended the run.
    //
    // The wait is bounded so a corrupt or stale history entry cannot stall a turn indefinitely.
    if let CommandGuardDecision::Throttle { wait_sec, .. } = &decision {
        let capped = (*wait_sec).min(MAX_THROTTLE_SEC);
        if capped > 0 {
            std::thread::sleep(std::time::Duration::from_secs(capped));
        }
        emit_allow();
        return Ok(());
    }

    emit_decision(&decision);

    Ok(())
}

/// Dispatches guard subcommands to their respective handlers.
pub fn dispatch_guard(cmd: GuardCommands) -> Result<(), String> {
    match cmd {
        GuardCommands::PreToolUse(args) => cmd_pre_tool_use(args),
        GuardCommands::PreCommand(args) => cmd_pre_command(args),
    }
}
