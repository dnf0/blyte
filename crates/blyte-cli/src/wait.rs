use std::collections::HashSet;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

const PARTIAL_WRITE_WINDOW: Duration = Duration::from_secs(2);

pub fn run(ids: &[String], any: bool, timeout_secs: u64) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let records_dir = match crate::dispatch_in_place::records_dir_for(&cwd) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    wait_in(
        &records_dir,
        ids,
        any,
        Duration::from_secs(timeout_secs),
        Duration::from_secs(2),
        Duration::from_secs(15),
        &mut std::io::stdout(),
        &mut std::io::stderr(),
    )
}

enum SummaryState {
    Absent,
    InProgress,
    Ready(serde_json::Value),
    Corrupt(String),
}

fn is_recently_modified(path: &Path, window: Duration) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    match SystemTime::now().duration_since(mtime) {
        Ok(age) => age < window,
        Err(_) => true,
    }
}

fn read_summary(path: &Path) -> SummaryState {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SummaryState::Absent,
        Err(e) => return SummaryState::Corrupt(format!("cannot read {}: {e}", path.display())),
    };
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(val) => SummaryState::Ready(val),
        Err(e) => {
            if is_recently_modified(path, PARTIAL_WRITE_WINDOW) {
                SummaryState::InProgress
            } else {
                SummaryState::Corrupt(format!("cannot parse {}: {e}", path.display()))
            }
        }
    }
}

fn read_start_pid(path: &Path) -> Result<Option<libc::pid_t>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let val: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("cannot parse {}: {e}", path.display()))?;
    let pid = val
        .get("pid")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| format!("{} is missing integer field \"pid\"", path.display()))?;
    Ok(Some(pid as libc::pid_t))
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

fn format_summary_line(
    path: &Path,
    id: &str,
    summary: &serde_json::Value,
) -> Result<String, String> {
    if !summary.is_object() {
        return Err(format!("{} is not a JSON object", path.display()));
    }
    let outcome = summary
        .get("outcome")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{} is missing string field \"outcome\"", path.display()))?;
    let turns = summary
        .get("turns")
        .and_then(|v| {
            v.as_i64()
                .map(|t| t.to_string())
                .or_else(|| v.as_u64().map(|t| t.to_string()))
        })
        .ok_or_else(|| format!("{} is missing integer field \"turns\"", path.display()))?;
    let cost = match summary.get("cost_usd").and_then(|v| v.as_f64()) {
        Some(c) => format!("${:.2}", c),
        None => "-".to_string(),
    };
    Ok(format!("{id} {outcome} {turns} {cost}"))
}

enum IdStatus {
    Finished(String),
    Running,
    LooksDead,
    Unknown,
    Corrupt(String),
}

fn check_id_status(wt: &Path, id: &str) -> IdStatus {
    let summary_path = wt.join(format!("{id}.json"));
    match read_summary(&summary_path) {
        SummaryState::Ready(summary) => match format_summary_line(&summary_path, id, &summary) {
            Ok(line) => return IdStatus::Finished(line),
            Err(e) => return IdStatus::Corrupt(e),
        },
        SummaryState::Corrupt(e) => return IdStatus::Corrupt(e),
        SummaryState::InProgress => return IdStatus::Running,
        SummaryState::Absent => {}
    }

    let start_path = wt.join(format!("{id}.start.json"));
    match read_start_pid(&start_path) {
        Ok(Some(pid)) => {
            if is_pid_alive(pid) {
                return IdStatus::Running;
            }
            match read_summary(&summary_path) {
                SummaryState::Ready(summary) => {
                    match format_summary_line(&summary_path, id, &summary) {
                        Ok(line) => IdStatus::Finished(line),
                        Err(e) => IdStatus::Corrupt(e),
                    }
                }
                SummaryState::Corrupt(e) => IdStatus::Corrupt(e),
                SummaryState::InProgress => IdStatus::Running,
                SummaryState::Absent => IdStatus::LooksDead,
            }
        }
        Ok(None) => IdStatus::Unknown,
        Err(e) => IdStatus::Corrupt(e),
    }
}

fn is_known(wt: &Path, id: &str) -> bool {
    wt.join(format!("{id}.json")).exists() || wt.join(format!("{id}.start.json")).exists()
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
                IdStatus::Corrupt(msg) => {
                    let _ = writeln!(err, "error: {msg}");
                    let _ = err.flush();
                    return 1;
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

    fn set_older_than_window(path: &Path) {
        let old_time = SystemTime::now() - Duration::from_secs(10);
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(old_time).unwrap();
    }

    #[test]
    fn missing_outcome_in_summary_exits_1_and_names_file_and_field() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let path = wt.join("X.json");
        std::fs::write(&path, r#"{"id":"X"}"#).unwrap();
        set_older_than_window(&path);

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["X".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        let out_str = String::from_utf8(out).unwrap();
        let err_str = String::from_utf8(err).unwrap();
        assert_eq!(code, 1, "stdout={out_str:?} stderr={err_str:?}");
        assert!(!out_str.contains('?'), "stdout={out_str:?}");
        assert!(!err_str.contains('?'), "stderr={err_str:?}");
        assert!(
            err_str.contains(&path.display().to_string()),
            "stderr={err_str:?}"
        );
        assert!(err_str.contains("outcome"), "stderr={err_str:?}");
    }

    #[test]
    fn missing_turns_in_summary_exits_1_and_names_file_and_field() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let path = wt.join("X.json");
        std::fs::write(&path, r#"{"id":"X","outcome":"verified"}"#).unwrap();
        set_older_than_window(&path);

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["X".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        let out_str = String::from_utf8(out).unwrap();
        let err_str = String::from_utf8(err).unwrap();
        assert_eq!(code, 1, "stdout={out_str:?} stderr={err_str:?}");
        assert!(!out_str.contains('?'), "stdout={out_str:?}");
        assert!(!err_str.contains('?'), "stderr={err_str:?}");
        assert!(
            err_str.contains(&path.display().to_string()),
            "stderr={err_str:?}"
        );
        assert!(err_str.contains("turns"), "stderr={err_str:?}");
    }

    #[test]
    fn old_corrupt_summary_json_exits_1_and_names_file() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let path = wt.join("X.json");
        std::fs::write(&path, r#"{"outc"#).unwrap();
        set_older_than_window(&path);

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["X".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        let out_str = String::from_utf8(out).unwrap();
        let err_str = String::from_utf8(err).unwrap();
        assert_eq!(code, 1, "stdout={out_str:?} stderr={err_str:?}");
        assert!(out_str.is_empty(), "stdout={out_str:?}");
        assert!(
            err_str.contains(&path.display().to_string()),
            "stderr={err_str:?}"
        );
    }

    #[test]
    fn corrupt_start_json_exits_1_and_names_file() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path();
        let start_path = wt.join("X.start.json");
        std::fs::write(&start_path, "{broken").unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = wait_in(
            wt,
            &["X".to_string()],
            false,
            Duration::from_secs(5),
            Duration::from_millis(10),
            Duration::ZERO,
            &mut out,
            &mut err,
        );
        let out_str = String::from_utf8(out).unwrap();
        let err_str = String::from_utf8(err).unwrap();
        assert_eq!(code, 1, "stdout={out_str:?} stderr={err_str:?}");
        assert!(out_str.is_empty(), "stdout={out_str:?}");
        assert!(
            err_str.contains(&start_path.display().to_string()),
            "stderr={err_str:?}"
        );
    }
}
