//! Needs attention, worktree existence check, and plan graph node selection outline (2026-10-02).

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

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

#[test]
fn worktree_exists_reports_true_when_present_and_false_when_removed() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("repos/repo-a");
    let wt = root.join(".wt");
    let t = now();

    let wt_dir = d.path().join("worktrees/T1");
    std::fs::create_dir_all(&wt_dir).unwrap();

    write(
        &wt.join("T1.start.json"),
        &format!(
            r#"{{"id":"T1","pid":999999,"started_at":{},"gate":"true"}}"#,
            t - 100
        ),
    );
    write(&wt.join("T1.log"), "[Turn 1]\n");
    write(
        &wt.join("T1.json"),
        &format!(r#"{{"worktree":"{}"}}"#, wt_dir.display()),
    );

    let src = Sources {
        roots: vec![d.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    // 1. When worktree directory exists, /api/jobs reports worktree_exists: true
    let (code, ctype, body) = route("/api/jobs", &src);
    assert_eq!((code, ctype), (200, "application/json"));
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let t1 = job(&v, "repo-a/T1");
    assert_eq!(t1["worktree_exists"], true);

    let v_direct = collect(&src, t);
    let t1_direct = job(&v_direct, "repo-a/T1");
    assert_eq!(t1_direct["worktree_exists"], true);

    // 2. Remove worktree directory
    std::fs::remove_dir(&wt_dir).unwrap();

    // Now reports worktree_exists: false
    let (code, ctype, body) = route("/api/jobs", &src);
    assert_eq!((code, ctype), (200, "application/json"));
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let t1 = job(&v, "repo-a/T1");
    assert_eq!(t1["worktree_exists"], false);

    let v_direct = collect(&src, t);
    let t1_direct = job(&v_direct, "repo-a/T1");
    assert_eq!(t1_direct["worktree_exists"], false);

    // 3. When worktree path is unset, worktree_exists is null
    write(
        &wt.join("T2.start.json"),
        &format!(
            r#"{{"id":"T2","pid":999999,"started_at":{},"gate":"true"}}"#,
            t - 200
        ),
    );
    write(&wt.join("T2.log"), "[Turn 1]\n");
    let v_direct2 = collect(&src, t);
    let t2 = job(&v_direct2, "repo-a/T2");
    assert!(t2["worktree_exists"].is_null());
}

#[test]
fn served_index_html_has_honest_plans_empty_and_no_rust_selection() {
    let d = tempfile::tempdir().unwrap();
    let src = Sources {
        roots: vec![d.path().join("repos")],
        runs_dirs: vec![],
        hours: 24,
    };

    let (code, ctype, body) = route("/", &src);
    assert_eq!(code, 200);
    assert!(ctype.starts_with("text/html"));
    let html = String::from_utf8(body).unwrap();

    assert!(
        !html.contains("isSel ? \"rust\""),
        "served index.html must not contain `isSel ? \"rust\"`"
    );
    assert!(
        html.contains("Nothing running. A plan shows here while one of its tasks is dispatched."),
        "served index.html must contain `Nothing running. A plan shows here while one of its tasks is dispatched.`"
    );
    assert!(
        html.contains("class=\"node-outline\""),
        "served index.html must contain node-outline element"
    );
    assert!(
        html.contains("stroke=\"var(--ink)\""),
        "served index.html must use var(--ink) for node outline stroke"
    );
}
