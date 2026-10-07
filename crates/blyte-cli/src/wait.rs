use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

pub fn run(ids: &[String], any: bool, timeout_secs: u64) -> i32 {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();
    let Ok(out) = out else {
        eprintln!("error: git not found");
        return 2;
    };
    if !out.status.success() {
        eprintln!("error: not a git repository");
        return 2;
    }
    let root = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    wait_in(
        &root.join(".wt"),
        ids,
        any,
        Duration::from_secs(timeout_secs),
        Duration::from_secs(2),
        Duration::from_secs(15),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

fn try_read_summary(wt: &Path, id: &str) -> Option<serde_json::Value> {
    let path = wt.join(format!("{id}.json"));
    let s = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&s).ok()
}

fn read_start_pid(path: &Path) -> Option<libc::pid_t> {
    let s = std::fs::read_to_string(path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&s).ok()?;
    val.get("pid")
        .and_then(|v| v.as_i64())
        .map(|v| v as libc::pid_t)
}

fn is_pid_alive(pid: libc::pid_t) -> bool {
    if pid <= 0 {
        return false;
    }
    let ret = unsafe { libc::kill(pid, 0) };
    if ret == 0 {
        true
    } else {
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

fn format_summary_line(id: &str, summary: &serde_json::Value) -> String {
    let outcome = summary
        .get("outcome")
        .and_then(|v| v.as_str())
        .unwrap_or("?");
    let turns = summary
        .get("turns")
        .and_then(|v| {
            v.as_i64()
                .map(|t| t.to_string())
                .or_else(|| v.as_u64().map(|t| t.to_string()))
        })
        .unwrap_or_else(|| "?".to_string());
    let cost = match summary.get("cost_usd").and_then(|v| v.as_f64()) {
        Some(c) => format!("${:.2}", c),
        None => "-".to_string(),
    };
    format!("{id} {outcome} {turns} {cost}")
}

enum IdStatus {
    Finished(String),
    Running,
    LooksDead,
    Unknown,
}

fn check_id_status(wt: &Path, id: &str) -> IdStatus {
    // 1. finished: wt/<id>.json exists and parses as JSON
    if let Some(summary) = try_read_summary(wt, id) {
        return IdStatus::Finished(format_summary_line(id, &summary));
    }

    // 2. running: wt/<id>.start.json exists, its pid is > 0 and alive
    let start_path = wt.join(format!("{id}.start.json"));
    if start_path.exists() {
        if let Some(pid) = read_start_pid(&start_path) {
            if is_pid_alive(pid) {
                return IdStatus::Running;
            }
        }

        // 3. dead: otherwise, if wt/<id>.start.json exists. Before declaring a run dead, check
        // wt/<id>.json once more (the run may have written its summary and exited between the two checks);
        // if it now exists the run is finished.
        if let Some(summary) = try_read_summary(wt, id) {
            return IdStatus::Finished(format_summary_line(id, &summary));
        }
        return IdStatus::LooksDead;
    }

    IdStatus::Unknown
}

fn is_known(wt: &Path, id: &str) -> bool {
    let has_summary = try_read_summary(wt, id).is_some();
    let has_start = wt.join(format!("{id}.start.json")).exists();
    has_summary || has_start
}

#[allow(clippy::too_many_arguments)]
pub fn wait_in(
    wt: &Path,
    ids: &[String],
    any: bool,
    timeout: Duration,
    poll: Duration,
    grace: Duration,
    out: &mut impl Write,
    err: &mut impl Write,
) -> i32 {
    let mut unique_ids: Vec<&str> = Vec::new();
    let mut seen = HashSet::new();
    for id in ids {
        if seen.insert(id.as_str()) {
            unique_ids.push(id.as_str());
        }
    }

    if unique_ids.is_empty() {
        return 0;
    }

    let start_time = Instant::now();
    let mut printed = vec![false; unique_ids.len()];
    let mut first_dead = vec![None; unique_ids.len()];
    let mut printed_count = 0;

    loop {
        let mut printed_this_poll = false;
        for (i, id) in unique_ids.iter().enumerate() {
            if printed[i] {
                continue;
            }
            match check_id_status(wt, id) {
                IdStatus::Finished(line) => {
                    let _ = writeln!(out, "{line}");
                    let _ = out.flush();
                    printed[i] = true;
                    first_dead[i] = None;
                    printed_count += 1;
                    printed_this_poll = true;
                }
                IdStatus::Running => {
                    first_dead[i] = None;
                }
                IdStatus::LooksDead => {
                    let first_seen = *first_dead[i].get_or_insert_with(Instant::now);
                    if grace.is_zero() || first_seen.elapsed() >= grace {
                        let _ = writeln!(out, "{id} dead - -");
                        let _ = out.flush();
                        printed[i] = true;
                        first_dead[i] = None;
                        printed_count += 1;
                        printed_this_poll = true;
                    }
                }
                IdStatus::Unknown => {
                    first_dead[i] = None;
                }
            }
        }

        if printed_count == unique_ids.len() {
            return 0;
        }

        if any && printed_this_poll {
            return 0;
        }

        let elapsed = start_time.elapsed();

        if elapsed >= timeout && timeout < grace {
            let unprinted: Vec<&str> = unique_ids
                .iter()
                .enumerate()
                .filter(|(i, _)| !printed[*i])
                .map(|(_, id)| *id)
                .collect();
            let _ = writeln!(err, "still running: {}", unprinted.join(" "));
            let _ = err.flush();
            return 2;
        }

        if elapsed >= grace {
            let unknown_ids: Vec<&str> = unique_ids
                .iter()
                .enumerate()
                .filter(|(i, id)| !printed[*i] && !is_known(wt, id))
                .map(|(_, id)| *id)
                .collect();
            if !unknown_ids.is_empty() {
                for id in unknown_ids {
                    let _ = writeln!(err, "no dispatch {id} here");
                }
                let _ = err.flush();
                return 1;
            }
        }

        if elapsed >= timeout {
            let unprinted: Vec<&str> = unique_ids
                .iter()
                .enumerate()
                .filter(|(i, _)| !printed[*i])
                .map(|(_, id)| *id)
                .collect();
            let _ = writeln!(err, "still running: {}", unprinted.join(" "));
            let _ = err.flush();
            return 2;
        }

        let remaining = timeout.saturating_sub(start_time.elapsed());
        let sleep_dur = poll.min(remaining);
        std::thread::sleep(sleep_dur);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finished_run_prints_summary_line() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::write(
            wt.join("T1.json"),
            r#"{"outcome":"verified","turns":41,"cost_usd":0.374}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "T1 verified 41 $0.37\n");
        assert!(err.is_empty());
    }

    #[test]
    fn missing_cost_prints_dash() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::write(
            wt.join("T1.json"),
            r#"{"outcome":"gate_failed","turns":7,"cost_usd":null}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "T1 gate_failed 7 -\n");
        assert!(err.is_empty());
    }

    #[test]
    fn dead_run() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        std::fs::write(wt.join("T1.start.json"), format!(r#"{{"pid": {pid}}}"#)).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "T1 dead - -\n");
        assert!(err.is_empty());
    }

    #[test]
    fn zero_pid_is_dead() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::write(wt.join("T1.start.json"), r#"{"pid": 0}"#).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "T1 dead - -\n");
        assert!(err.is_empty());
    }

    #[test]
    fn unknown_id_errors_without_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let start = Instant::now();
        let code = wait_in(
            wt,
            &["T9".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::from_millis(50),
            &mut out,
            &mut err,
        );
        let elapsed = start.elapsed();
        assert_eq!(code, 1);
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("no dispatch T9 here"));
        assert!(out.is_empty());
        assert!(elapsed < Duration::from_secs(1));
    }

    #[test]
    fn timeout_with_running_run() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let pid = std::process::id();
        std::fs::write(wt.join("T1.start.json"), format!(r#"{{"pid": {pid}}}"#)).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            false,
            Duration::from_millis(50),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 2);
        assert!(out.is_empty());
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("still running: T1"));
    }

    #[test]
    fn all_mode_waits_for_every_run() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::write(
            wt.join("T1.json"),
            r#"{"outcome":"verified","turns":41,"cost_usd":0.374}"#,
        )
        .unwrap();
        let pid = std::process::id();
        std::fs::write(wt.join("T2.start.json"), format!(r#"{{"pid": {pid}}}"#)).unwrap();
        let wt_clone = wt.to_path_buf();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::fs::write(
                wt_clone.join("T2.json"),
                r#"{"outcome":"verified","turns":10,"cost_usd":0.10}"#,
            )
            .unwrap();
        });
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string(), "T2".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        let out_str = String::from_utf8(out).unwrap();
        assert_eq!(out_str, "T1 verified 41 $0.37\nT2 verified 10 $0.10\n");
        assert!(err.is_empty());
    }

    #[test]
    fn any_mode_returns_after_first() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let pid = std::process::id();
        std::fs::write(wt.join("T1.start.json"), format!(r#"{{"pid": {pid}}}"#)).unwrap();
        std::fs::write(
            wt.join("T2.json"),
            r#"{"outcome":"verified","turns":12,"cost_usd":0.20}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string(), "T2".to_string()],
            true,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "T2 verified 12 $0.20\n");
        assert!(err.is_empty());
    }

    #[test]
    fn any_mode_prints_all_already_finished() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::write(
            wt.join("T1.json"),
            r#"{"outcome":"verified","turns":1,"cost_usd":0.01}"#,
        )
        .unwrap();
        std::fs::write(
            wt.join("T2.json"),
            r#"{"outcome":"gate_failed","turns":2,"cost_usd":0.02}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string(), "T2".to_string()],
            true,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        let out_str = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = out_str.lines().collect();
        assert_eq!(lines, vec!["T1 verified 1 $0.01", "T2 gate_failed 2 $0.02"]);
        assert!(err.is_empty());
    }

    #[test]
    fn any_mode_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let pid = std::process::id();
        std::fs::write(wt.join("T1.start.json"), format!(r#"{{"pid": {pid}}}"#)).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            true,
            Duration::from_millis(50),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 2);
        assert!(out.is_empty());
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("still running: T1"));
    }

    #[test]
    fn summary_written_after_start_counts_as_finished() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        std::fs::write(wt.join("T1.start.json"), format!(r#"{{"pid": {pid}}}"#)).unwrap();
        std::fs::write(
            wt.join("T1.json"),
            r#"{"outcome":"verified","turns":5,"cost_usd":0.05}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "T1 verified 5 $0.05\n");
        assert!(err.is_empty());
    }

    #[test]
    fn unparseable_summary_is_not_finished() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::write(wt.join("T1.json"), r#"{"outc"#).unwrap();
        let pid = std::process::id();
        std::fs::write(wt.join("T1.start.json"), format!(r#"{{"pid": {pid}}}"#)).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            false,
            Duration::from_millis(50),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 2);
        assert!(out.is_empty());
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("still running: T1"));
    }

    #[test]
    fn duplicate_ids_reported_once() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::write(
            wt.join("T1.json"),
            r#"{"outcome":"verified","turns":41,"cost_usd":0.374}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string(), "T1".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "T1 verified 41 $0.37\n");
        assert!(err.is_empty());
    }

    #[test]
    fn finishing_run_within_grace_reports_summary() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        std::fs::write(wt.join("T1.start.json"), format!(r#"{{"pid": {pid}}}"#)).unwrap();
        let wt_clone = wt.to_path_buf();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::fs::write(
                wt_clone.join("T1.json"),
                r#"{"outcome":"verified","turns":3,"cost_usd":0.1}"#,
            )
            .unwrap();
        });
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::from_secs(2),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "T1 verified 3 $0.10\n");
        assert!(err.is_empty());
    }

    #[test]
    fn dead_run_reported_after_grace() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        std::fs::write(wt.join("T1.start.json"), format!(r#"{{"pid": {pid}}}"#)).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let start = Instant::now();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::from_millis(150),
            &mut out,
            &mut err,
        );
        let elapsed = start.elapsed();
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "T1 dead - -\n");
        assert!(elapsed >= Duration::from_millis(150));
        assert!(err.is_empty());
    }

    #[test]
    fn timeout_inside_grace_is_still_running() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        std::fs::write(wt.join("T1.start.json"), format!(r#"{{"pid": {pid}}}"#)).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            false,
            Duration::from_millis(100),
            Duration::from_millis(10),
            Duration::from_secs(5),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 2);
        assert!(out.is_empty());
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("still running: T1"));
    }

    #[test]
    fn summary_without_start_file_written_after_start() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let wt_clone = wt.to_path_buf();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            std::fs::write(
                wt_clone.join("T1.json"),
                r#"{"outcome":"verified","turns":15,"cost_usd":0.25}"#,
            )
            .unwrap();
        });
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::from_secs(2),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).unwrap(), "T1 verified 15 $0.25\n");
        assert!(err.is_empty());
    }

    #[test]
    fn one_summary_one_never_appearing() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        std::fs::write(
            wt.join("T1.json"),
            r#"{"outcome":"verified","turns":10,"cost_usd":0.10}"#,
        )
        .unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T1".to_string(), "T2".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::from_millis(50),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 1);
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("no dispatch T2 here"));
        assert!(!err_str.contains("no dispatch T1 here"));
        assert_eq!(String::from_utf8(out).unwrap(), "T1 verified 10 $0.10\n");
    }

    #[test]
    fn timeout_before_grace_with_unknown_id() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["T9".to_string()],
            false,
            Duration::from_millis(50),
            Duration::from_millis(10),
            Duration::from_secs(5),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 2);
        assert!(out.is_empty());
        let err_str = String::from_utf8(err).unwrap();
        assert!(err_str.contains("still running: T9"));
    }

    #[test]
    fn multiple_unknown_ids_reported_in_argument_order() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["B".to_string(), "A".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::from_millis(50),
            &mut out,
            &mut err,
        );
        assert_eq!(code, 1);
        assert!(out.is_empty());
        let err_str = String::from_utf8(err).unwrap();
        assert_eq!(err_str, "no dispatch B here\nno dispatch A here\n");
    }

    #[test]
    fn cli_parses_wait() {
        use crate::cli::{Cli, Command};
        use clap::Parser;

        let cli =
            Cli::try_parse_from(["blyte", "wait", "A", "B", "--any", "--timeout", "10"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Command::Wait {
                ids: vec!["A".to_string(), "B".to_string()],
                any: true,
                timeout: 10,
            })
        );

        let cli = Cli::try_parse_from(["blyte", "wait", "A"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Command::Wait {
                ids: vec!["A".to_string()],
                any: false,
                timeout: 280,
            })
        );

        assert!(Cli::try_parse_from(["blyte", "wait"]).is_err());
    }
}
