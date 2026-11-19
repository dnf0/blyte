//! Keeps the machine awake while a dispatch run's child process is alive.

use std::process::{Command, Stdio};

#[cfg(target_os = "macos")]
pub fn command(pid: u32) -> Option<Command> {
    let mut cmd = Command::new("/usr/bin/caffeinate");
    cmd.arg("-i").arg("-w").arg(pid.to_string());
    Some(cmd)
}

#[cfg(not(target_os = "macos"))]
pub fn command(_pid: u32) -> Option<Command> {
    None
}

pub fn hold(pid: u32) -> Result<(), String> {
    if std::env::var("BLYTE_NO_KEEP_AWAKE").as_deref() == Ok("1") {
        return Ok(());
    }
    let Some(mut cmd) = command(pid) else {
        return Ok(());
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("could not start /usr/bin/caffeinate: {e}"))
}
