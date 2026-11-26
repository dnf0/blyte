use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::thread;
use std::time::Duration;

use blyte_agent::sandbox::validate_surface_id;
use blyte_cli::surface_dir::{
    entries_path, is_running, list_records, new_surface_id, pick_name, read_record, record_path,
    surface_dir, surfaces_dir, validate_surface_name, write_record, SurfaceRecord,
};

fn sample_record(id: &str, name: &str, port: u16, started_at: u64) -> SurfaceRecord {
    SurfaceRecord {
        id: id.to_string(),
        name: name.to_string(),
        pid: std::process::id(),
        port,
        session_id: format!("sess-{id}"),
        started_at,
    }
}

fn spawn_health_server(body: String) -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut req_bytes = Vec::new();
        let mut buf = [0u8; 1024];
        while !req_bytes.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            req_bytes.extend_from_slice(&buf[..n]);
        }
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(resp.as_bytes()).unwrap();
        stream.flush().unwrap();

        let request = String::from_utf8(req_bytes).unwrap();
        let (head, _) = request
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("incomplete HTTP request: {request:?}"));
        let mut lines = head.split("\r\n");
        assert_eq!(
            lines.next(),
            Some("GET /api/health HTTP/1.1"),
            "unexpected request: {request:?}"
        );
        let headers: Vec<(String, String)> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        let expected_host = format!("127.0.0.1:{port}");
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "host" && *v == expected_host),
            "missing or unexpected Host header in request: {request:?}"
        );
        assert!(
            headers
                .iter()
                .any(|(k, v)| k == "connection" && v.eq_ignore_ascii_case("close")),
            "missing Connection: close header in request: {request:?}"
        );
    });
    (port, handle)
}

#[test]
fn new_surface_id_generates_distinct_valid_ids() {
    let id1 = new_surface_id().unwrap();
    let id2 = new_surface_id().unwrap();
    assert_ne!(id1, id2);
    validate_surface_id(&id1).unwrap();
    validate_surface_id(&id2).unwrap();
}

#[test]
fn validate_surface_name_accepts_valid_and_rejects_invalid() {
    for ok in ["blyte", "blyte-2", "a.b_c", "9", &"a".repeat(64)] {
        assert!(validate_surface_name(ok).is_ok(), "expected ok for {ok:?}");
    }

    let too_long = "a".repeat(65);
    for bad in ["", "-x", ".x", "a b", "a/b", too_long.as_str()] {
        let err = validate_surface_name(bad).unwrap_err();
        assert_eq!(
            err,
            format!(
                "invalid surface name: {bad} (use letters, digits, '.', '_' and '-', starting with a letter or digit)"
            )
        );
    }
}

#[test]
fn path_helpers_build_expected_paths() {
    let checkout = Path::new("/tmp/example-checkout");
    let id = "s0123456789abcdef";
    assert_eq!(surfaces_dir(checkout), checkout.join(".wt/surfaces"));
    assert_eq!(
        surface_dir(checkout, id),
        checkout.join(".wt/surfaces/s0123456789abcdef")
    );
    assert_eq!(
        record_path(checkout, id),
        checkout.join(".wt/surfaces/s0123456789abcdef/surface.json")
    );
    assert_eq!(
        entries_path(checkout, id),
        checkout.join(".wt/surfaces/s0123456789abcdef/entries.jsonl")
    );
}

#[test]
fn write_record_then_read_record_round_trips_with_no_temp_files() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = sample_record("s0123456789abcdef", "blyte", 18080, 1_700_000_000);
    write_record(tmp.path(), &rec).unwrap();

    let got = read_record(tmp.path(), &rec.id).unwrap();
    assert_eq!(got, rec);

    let dir = surface_dir(tmp.path(), &rec.id);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["surface.json".to_string()]);

    let raw = std::fs::read_to_string(record_path(tmp.path(), &rec.id)).unwrap();
    assert!(raw.ends_with('\n'));
}

#[test]
fn read_record_errors_when_inner_id_differs() {
    let tmp = tempfile::tempdir().unwrap();
    let dir_id = "s0123456789abcdef";
    let inner_id = "sfedcba9876543210";
    let dir = surface_dir(tmp.path(), dir_id);
    std::fs::create_dir_all(&dir).unwrap();
    let path = record_path(tmp.path(), dir_id);
    let rec = sample_record(inner_id, "blyte", 18080, 1_700_000_000);
    std::fs::write(&path, serde_json::to_string_pretty(&rec).unwrap()).unwrap();

    let err = read_record(tmp.path(), dir_id).unwrap_err();
    assert_eq!(
        err,
        format!(
            "surface record {} has id {inner_id}, expected {dir_id}",
            path.display()
        )
    );
}

#[test]
fn list_records_handles_missing_dir_sorting_stray_files_and_errors() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(list_records(tmp.path()).unwrap(), Vec::new());

    let r_later = sample_record("s0000000000000002", "blyte-2", 18082, 200);
    let r_earlier = sample_record("s0000000000000001", "blyte", 18081, 100);
    let r_tie_b = sample_record("s000000000000000b", "blyte-b", 18084, 150);
    let r_tie_a = sample_record("s000000000000000a", "blyte-a", 18083, 150);
    write_record(tmp.path(), &r_later).unwrap();
    write_record(tmp.path(), &r_tie_b).unwrap();
    write_record(tmp.path(), &r_earlier).unwrap();
    write_record(tmp.path(), &r_tie_a).unwrap();

    std::fs::write(surfaces_dir(tmp.path()).join("stray.txt"), "ignore me\n").unwrap();

    let listed = list_records(tmp.path()).unwrap();
    assert_eq!(listed, vec![r_earlier, r_tie_a, r_tie_b, r_later]);

    let missing_json_dir = surface_dir(tmp.path(), "s0000000000000003");
    std::fs::create_dir_all(&missing_json_dir).unwrap();
    let err = list_records(tmp.path()).unwrap_err();
    assert!(
        err.contains(&missing_json_dir.display().to_string()),
        "{err}"
    );
    std::fs::remove_dir_all(&missing_json_dir).unwrap();

    let bad_dir = surfaces_dir(tmp.path()).join("not-a-surface-id");
    std::fs::create_dir_all(&bad_dir).unwrap();
    let err = list_records(tmp.path()).unwrap_err();
    assert!(err.contains(&bad_dir.display().to_string()), "{err}");
}

#[test]
fn is_running_matches_surface_id_and_rejects_mismatch_or_closed_port() {
    let body = r#"{"surface_id":"s0123456789abcdef","session_id":"x"}"#.to_string();

    let (port, handle) = spawn_health_server(body.clone());
    let matching = sample_record("s0123456789abcdef", "blyte", port, 100);
    assert!(is_running(&matching));
    handle.join().unwrap();

    let (port2, handle2) = spawn_health_server(body);
    let other_id = sample_record("sfedcba9876543210", "blyte", port2, 100);
    assert!(!is_running(&other_id));
    handle2.join().unwrap();

    let unused_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = unused_listener.local_addr().unwrap().port();
    drop(unused_listener);
    let closed = sample_record("s0123456789abcdef", "blyte", closed_port, 100);
    assert!(!is_running(&closed));
}

#[test]
fn pick_name_handles_defaults_suffixes_conflicts_and_invalid_bases() {
    assert_eq!(pick_name("blyte", None, &[]).unwrap(), "blyte");
    assert_eq!(
        pick_name("blyte", None, &["blyte".to_string(), "blyte-2".to_string()]).unwrap(),
        "blyte-3"
    );

    let err = pick_name("blyte", Some("x"), &["x".to_string()]).unwrap_err();
    assert_eq!(
        err,
        "a running surface is already named x; pass a different --name"
    );

    let err = pick_name("my repo", None, &[]).unwrap_err();
    assert_eq!(
        err,
        "the repository directory name my repo cannot be a surface name; pass --name"
    );

    assert_eq!(pick_name("my repo", Some("ok"), &[]).unwrap(), "ok");
}
