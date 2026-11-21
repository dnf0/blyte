use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};

use blyte_agent::approval::{Approval, Approver};

pub struct TtyApprover;

pub fn decide(answer: &str) -> Approval {
    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Approval::Approved,
        _ => Approval::Denied,
    }
}

impl Approver for TtyApprover {
    fn ask(&self, command: &str, reason: &str) -> Approval {
        let mut tty = match OpenOptions::new().read(true).write(true).open("/dev/tty") {
            Ok(f) => f,
            Err(_) => return Approval::Unavailable,
        };
        if write!(
            tty,
            "Run outside the sandbox? {reason}\n  {command}\n[y/N] "
        )
        .is_err()
            || tty.flush().is_err()
        {
            return Approval::Unavailable;
        }
        let mut reader = BufReader::new(tty);
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return Approval::Denied;
        }
        decide(&line)
    }
}
