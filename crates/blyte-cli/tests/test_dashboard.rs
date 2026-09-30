//! `blyte dashboard` collects running and recent jobs from blyte's own files and serves them
//! locally (2026-09-30).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use blyte_cli::dashboard::{collect, route, Sources};

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn job<'a>(v: &'a serde_json::Value, key: &str) -> &'a serde_json::Value {
    v["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["key"] == key)
        .unwrap_or_else(|| panic!("no job {key} in {v}"))
}

/// A root with dispatch-layout jobs (repo-a) and a runs dir (repo-b).
fn fixture(dir: &Path) -> Sources {
    let me = std::process::id();
    let t = now();
    let wt = dir.join("repos/repo-a/.wt");
    write(&dir.join("tasks/t1.md"), "# Add slugify\n\nMore detail.\n");
    write(
        &wt.join("T1.start.json"),
        &format!(
            r#"{{"id":"T1","pid":{me},"started_at":{},"task_file":"{}","gate":"true","tags":["planner: claude"]}}"#,
            t - 120,
            dir.join("tasks/t1.md").display()
        ),
    );
    write(
        &wt.join("T1.log"),
        "[Turn 3] \n● bash cargo test -> exit 0\n⏳ Gate running (attempt 1)\n",
    );

    write(
        &wt.join("T2.start.json"),
        &format!(
            r#"{{"id":"T2","pid":999999,"started_at":{},"task_file":"/nonexistent","gate":"true"}}"#,
            t - 900
        ),
    );
    write(
        &wt.join("T2.log"),
        "[Turn 10] \n✓ Gate passed (attempt 1)\n",
    );
    write(
        &wt.join("T2.report.json"),
        r#"{"outcome":"verified","cost_usd":0.5,"turns":10,"gate_attempts":1,"model_id":"gemini/gemini-3.8-flash","gate_failures":[],"duration_seconds":301.4,"final_message":"Done: added the flag.\nMore."}"#,
    );
    write(
        &wt.join("T2.json"),
        r#"{"commits":[{"sha":"abc12345","subject":"feat: add the flag"}],"changed_files":["a.rs","b.rs"],"worktree":"/w/T2"}"#,
    );

    write(
        &wt.join("T3.start.json"),
        &format!(
            r#"{{"id":"T3","pid":999999,"started_at":{},"task_file":"/nonexistent","gate":"true"}}"#,
            t - 600
        ),
    );
    write(
        &wt.join("T3.log"),
        "[Turn 2] \n● read src/lib.rs -> 40 lines\n",
    );

    let runs = dir.join("runs/repo-b");
    write(
        &runs.join("R1.log"),
        "[Turn 7] \n● edit src/x.py -> edited\n",
    );
    write(&runs.join("R1.pid"), &me.to_string());
    write(&runs.join("R1.prompt"), "You are implementing one task.\n\nPlan: /p/docs/plans/2026-09-30-thing.md\nTask: \"Task 1: Foo the bar\" ONLY.\n");
    write(&runs.join("R1.tags.json"), r#"["A/B: R1"]"#);
    write(
        &runs.join("R1.verdict.json"),
        r#"{"verdict":"accepted","notes":"fine"}"#,
    );

    write(&runs.join("OLD.log"), "[Turn 1] \n");
    write(
        &runs.join("OLD.report.json"),
        r#"{"outcome":"verified","cost_usd":9.0}"#,
    );
    let old = SystemTime::now() - Duration::from_secs(3 * 86400);
    for f in ["OLD.log", "OLD.report.json"] {
        std::fs::File::options()
            .write(true)
            .open(runs.join(f))
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    Sources {
        roots: vec![dir.join("repos")],
        runs_dirs: vec![dir.join("runs")],
        hours: 24,
    }
}

#[test]
fn running_jobs_show_their_progress_and_tags() {
    let d = tempfile::tempdir().unwrap();
    let v = collect(&fixture(d.path()), now());
    let t1 = job(&v, "repo-a/T1");
    assert_eq!(t1["status"], "running");
    assert_eq!(t1["gate_running"], true);
    assert_eq!(t1["turns"], 3);
    assert_eq!(t1["last_tool"], "bash cargo test -> exit 0");
    assert_eq!(t1["title"], "Add slugify");
    assert!(t1["elapsed_s"].as_u64().unwrap() >= 120);
    let tags: Vec<&str> = t1["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    assert!(
        tags.contains(&"repo: repo-a") && tags.contains(&"planner: claude"),
        "{tags:?}"
    );

    let r1 = job(&v, "repo-b/R1");
    assert_eq!(r1["status"], "running");
    assert_eq!(r1["gate_running"], false);
    assert_eq!(r1["title"], "Task 1: Foo the bar");
    assert_eq!(r1["plan"], "/p/docs/plans/2026-09-30-thing.md");
    assert_eq!(r1["verdict"], "accepted");
    let tags: Vec<&str> = r1["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_str().unwrap())
        .collect();
    assert!(
        tags.contains(&"A/B: R1") && tags.contains(&"plan: 2026-09-30-thing"),
        "{tags:?}"
    );
}

#[test]
fn finished_and_stopped_jobs_are_reported_and_old_ones_dropped() {
    let d = tempfile::tempdir().unwrap();
    let v = collect(&fixture(d.path()), now());
    let t2 = job(&v, "repo-a/T2");
    assert_eq!(t2["status"], "verified");
    assert_eq!(t2["cost_usd"], 0.5);
    assert_eq!(t2["model"], "gemini/gemini-3.8-flash");
    assert_eq!(t2["elapsed_s"], 301);
    assert_eq!(t2["now"], "Done: added the flag.");
    assert_eq!(t2["commits"].as_array().unwrap().len(), 1);
    assert_eq!(t2["changed_files"].as_array().unwrap().len(), 2);
    assert_eq!(job(&v, "repo-a/T3")["status"], "stopped");
    assert!(v["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|j| j["key"] != "repo-b/OLD"));

    let order: Vec<&str> = v["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| j["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        &order[..2]
            .iter()
            .filter(|k| ["repo-a/T1", "repo-b/R1"].contains(k))
            .count(),
        &2,
        "running first: {order:?}"
    );

    let s = &v["summary"];
    assert_eq!(s["running"], 2);
    assert_eq!(s["repos"], 2);
    assert_eq!(s["passed_24h"], 1);
    assert_eq!(s["cost_24h_usd"], 0.5);
}

#[test]
fn routes_serve_the_page_the_jobs_and_nothing_else() {
    let d = tempfile::tempdir().unwrap();
    let src = fixture(d.path());
    let (code, ctype, body) = route("/", &src);
    assert_eq!(code, 200);
    assert!(ctype.starts_with("text/html"));
    assert!(String::from_utf8_lossy(&body).contains("<html"));
    let (code, ctype, body) = route("/api/jobs", &src);
    assert_eq!((code, ctype), (200, "application/json"));
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(v["jobs"].is_array() && v["summary"].is_object());
    assert_eq!(route("/etc/passwd", &src).0, 404);
}

#[test]
fn the_server_listens_on_localhost_and_answers() {
    let d = tempfile::tempdir().unwrap();
    fixture(d.path());
    let mut child = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["dashboard", "--port", "0", "--root"])
        .arg(d.path().join("repos"))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let url = line
        .trim()
        .strip_prefix("blyte dashboard: http://")
        .unwrap_or_else(|| panic!("{line}"));
    assert!(url.starts_with("127.0.0.1:"), "{url}");
    let mut s = std::net::TcpStream::connect(url).unwrap();
    s.write_all(b"GET /api/jobs HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    child.kill().unwrap();
    let _ = child.wait();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.contains("\"jobs\""), "{resp}");
}

#[test]
fn dispatch_tags_are_accepted() {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["dispatch", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("--tag"));
}
