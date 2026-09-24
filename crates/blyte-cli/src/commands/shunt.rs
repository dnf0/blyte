//! CLI subcommand implementation for tool output shunting ("Spotify Portal" pattern).
//!
//! Executes arbitrary shell commands through the output shunting distillation layer,
//! streaming large outputs to raw logs on disk and returning compact summaries to keep
//! LLM context bounded and prevent context exhaustion.

use blyte_core::shunt::{evaluate_and_distill, DistilledOutput, ShuntConfig};
use std::env;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;

/// Executes a child command with concurrent stdout/stderr stream draining and output distillation.
///
/// Spawns the specified command with piped stdout/stderr while inheriting stdin. Drains both
/// output streams concurrently on background threads to prevent OS pipe buffer deadlocks on
/// outputs exceeding 64 KB. Passes the captured streams to [`evaluate_and_distill`] for threshold
/// evaluation, emitting raw streams or the distilled summary accordingly.
///
/// # Parameters
/// - `cmd_args`: Command and argument vector to execute (first element is the program).
/// - `threshold_lines`: Maximum combined line count before shunting distillation triggers.
/// - `logs_dir`: Optional directory where raw unstripped logs are stored (defaults to `.agent-state/logs`).
///
/// # Returns
/// - `Ok(i32)` representing the child's exact integer exit status code.
/// - `Err(String)` if argument list is empty, process spawning fails, or waiting fails.
pub fn execute_shunt(
    cmd_args: &[String],
    threshold_lines: usize,
    logs_dir: Option<PathBuf>,
) -> Result<i32, String> {
    // Validate command arguments vector is non-empty before attempting invocation
    if cmd_args.is_empty() {
        return Err("No command specified to shunt".to_string());
    }

    let program = &cmd_args[0];
    let args = &cmd_args[1..];
    let cmd_str = cmd_args.join(" ");

    // Determine current working directory to anchor log file relative resolution
    let current_dir = env::current_dir().map_err(|e| e.to_string())?;

    // Record start time to track total execution duration
    let start_time = std::time::Instant::now();

    // Spawn child process with inherited standard input and piped output streams
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn command '{}': {}", program, e))?;

    // Take ownership of stream pipes for concurrent draining across dedicated threads
    let mut stdout_pipe = child.stdout.take().ok_or("Failed to capture stdout")?;
    let mut stderr_pipe = child.stderr.take().ok_or("Failed to capture stderr")?;

    // Spawn stdout reader thread to prevent deadlock when buffer exceeds 64 KB
    let stdout_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });

    // Spawn stderr reader thread to prevent deadlock when buffer exceeds 64 KB
    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    // Await child process termination before collecting stream buffers
    let status = child
        .wait()
        .map_err(|e| format!("Failed to wait for child command: {}", e))?;

    // Measure total duration elapsed since child process launch
    let duration = start_time.elapsed();

    // Join reader threads to retrieve captured stdout and stderr byte vectors
    let stdout_bytes = stdout_handle.join().unwrap_or_default();
    let stderr_bytes = stderr_handle.join().unwrap_or_default();

    // Resolve exit code, preserving POSIX 128+SIG termination semantics on Unix
    let exit_code = status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            status.signal().map(|s| 128 + s).unwrap_or(1)
        }
        #[cfg(not(unix))]
        1
    });

    // Build shunting configuration using configured threshold lines and 4 KB byte ceiling
    let config = ShuntConfig {
        threshold_lines,
        threshold_bytes: 4096,
        logs_dir: logs_dir.unwrap_or_else(|| PathBuf::from(".agent-state/logs")),
    };

    // Apply distillation engine evaluation over captured outputs
    let result = evaluate_and_distill(
        &cmd_str,
        &current_dir,
        &stdout_bytes,
        &stderr_bytes,
        exit_code,
        duration,
        &config,
    )
    .map_err(|e| format!("Shunt distillation error: {}", e))?;

    // Emit either verbatim uncorrupted output or compact distilled summary
    match result {
        DistilledOutput::Raw { stdout, stderr } => {
            let _ = io::stdout().write_all(&stdout);
            let _ = io::stderr().write_all(&stderr);
        }
        DistilledOutput::Distilled { summary, .. } => {
            println!("{}", summary);
        }
    }

    let _ = io::stdout().flush();
    let _ = io::stderr().flush();

    Ok(exit_code)
}
