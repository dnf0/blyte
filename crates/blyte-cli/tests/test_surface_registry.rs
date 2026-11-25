use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use blyte_cli::surface_registry::{
    entry_path, find_by_name, first_line, live_entries, read_entries, remove_entry, set_doing,
    write_entry, RegistryEntry,
};

struct MockHealthServer {
    port: u16,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockHealthServer {
    fn start(surface_id: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let body = format!(r#"{{"surface_id":"{surface_id}"}}"#);
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                if stop_bg.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream else {
                    break;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut req = Vec::new();
                let mut buf = [0u8; 512];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        Self {
            port,
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for MockHealthServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn sample_entry(id: &str, name: &str, port: u16, started_at: u64) -> RegistryEntry {
    RegistryEntry {
        id: id.to_string(),
        name: name.to_string(),
        root: PathBuf::from("/tmp/example-repo"),
        branch: "main".to_string(),
        pid: 4242,
        port,
        started_at,
        doing: "idle".to_string(),
    }
}

#[test]
fn write_entry_then_read_entries_round_trips_and_sets_mode_0600() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("surfaces");
    let entry = sample_entry("s00000000000000aa", "alpha", 18080, 1_700_000_000);
    write_entry(&dir, &entry).unwrap();

    let entries = read_entries(&dir).unwrap();
    assert_eq!(entries, vec![entry.clone()]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(entry_path(&dir, &entry.id))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn read_entries_on_missing_directory_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("does-not-exist");
    assert_eq!(read_entries(&missing).unwrap(), Vec::new());
}

#[test]
fn read_entries_ignores_non_json_and_errors_on_corrupt_json() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let entry = sample_entry("s00000000000000aa", "alpha", 18080, 100);
    write_entry(dir, &entry).unwrap();
    std::fs::write(dir.join("x.tmp"), "not json").unwrap();

    assert_eq!(read_entries(dir).unwrap(), vec![entry]);

    let bad = dir.join("bad.json");
    std::fs::write(&bad, "{").unwrap();
    let err = read_entries(dir).unwrap_err();
    assert!(err.contains(&bad.display().to_string()), "{err}");
}

#[test]
fn read_entries_errors_when_id_differs_from_file_name() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let mismatched = sample_entry("s00000000000000bb", "beta", 18081, 100);
    let path = entry_path(dir, "s00000000000000aa");
    std::fs::write(&path, serde_json::to_string_pretty(&mismatched).unwrap()).unwrap();

    let err = read_entries(dir).unwrap_err();
    assert!(err.contains(&path.display().to_string()), "{err}");
}

#[test]
fn write_entry_rejects_invalid_id_and_name_without_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("surfaces");

    let bad_id = sample_entry("nope", "alpha", 18080, 100);
    assert!(write_entry(&dir, &bad_id).is_err());
    assert!(!dir.exists());

    let bad_name = sample_entry("s00000000000000aa", "a/b", 18080, 100);
    assert!(write_entry(&dir, &bad_name).is_err());
    assert!(!dir.exists());
}

#[test]
fn remove_entry_removes_file_and_is_ok_when_absent() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let entry = sample_entry("s00000000000000aa", "alpha", 18080, 100);
    write_entry(dir, &entry).unwrap();
    assert!(entry_path(dir, &entry.id).exists());

    remove_entry(dir, &entry.id).unwrap();
    assert!(!entry_path(dir, &entry.id).exists());

    remove_entry(dir, &entry.id).unwrap();
}

#[test]
fn live_entries_keeps_live_and_removes_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let live_server = MockHealthServer::start("s00000000000000aa");
    let live = sample_entry("s00000000000000aa", "alpha", live_server.port, 100);
    let stale = sample_entry("s00000000000000bb", "beta", closed_port(), 200);
    write_entry(dir, &live).unwrap();
    write_entry(dir, &stale).unwrap();

    let got = live_entries(dir).unwrap();
    assert_eq!(got, vec![live]);
    assert!(entry_path(dir, "s00000000000000aa").exists());
    assert!(!entry_path(dir, "s00000000000000bb").exists());
}

#[test]
fn live_entries_treats_mismatched_surface_id_listener_as_stale() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let wrong_server = MockHealthServer::start("s00000000000000bb");
    let entry = sample_entry("s00000000000000aa", "alpha", wrong_server.port, 100);
    write_entry(dir, &entry).unwrap();

    let got = live_entries(dir).unwrap();
    assert!(got.is_empty());
    assert!(!entry_path(dir, "s00000000000000aa").exists());
}

#[test]
fn find_by_name_matches_name_or_id_and_errors_on_unknown() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let server = MockHealthServer::start("s00000000000000aa");
    let entry = sample_entry("s00000000000000aa", "alpha", server.port, 100);
    write_entry(dir, &entry).unwrap();

    assert_eq!(find_by_name(dir, "alpha").unwrap(), entry);
    assert_eq!(find_by_name(dir, "s00000000000000aa").unwrap(), entry);

    let err = find_by_name(dir, "ghost").unwrap_err();
    assert_eq!(err, "no running surface named ghost");
}

#[test]
fn find_by_name_errors_when_multiple_live_entries_share_a_name() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let server_a = MockHealthServer::start("s00000000000000aa");
    let server_b = MockHealthServer::start("s00000000000000bb");
    let a = sample_entry("s00000000000000aa", "shared", server_a.port, 100);
    let b = sample_entry("s00000000000000bb", "shared", server_b.port, 200);
    write_entry(dir, &a).unwrap();
    write_entry(dir, &b).unwrap();

    let err = find_by_name(dir, "shared").unwrap_err();
    assert_eq!(
        err,
        "more than one running surface is named shared; use an id: s00000000000000aa, s00000000000000bb"
    );
}

#[test]
fn set_doing_updates_only_doing_and_errors_when_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let entry = sample_entry("s00000000000000aa", "alpha", 18080, 100);
    write_entry(dir, &entry).unwrap();

    set_doing(dir, &entry.id, "fixing tests").unwrap();
    let updated = read_entries(dir).unwrap();
    let mut expected = entry;
    expected.doing = "fixing tests".to_string();
    assert_eq!(updated, vec![expected]);

    let missing_path = entry_path(dir, "s00000000000000bb");
    let err = set_doing(dir, "s00000000000000bb", "idle").unwrap_err();
    assert!(err.contains(&missing_path.display().to_string()), "{err}");
}

#[test]
fn first_line_trims_skips_blanks_and_truncates_by_chars() {
    assert_eq!(first_line("\n\n  fix the parser  \nmore"), "fix the parser");
    let accents = "é".repeat(200);
    let cut = first_line(&accents);
    assert_eq!(cut.chars().count(), 120);
    assert_eq!(cut, "é".repeat(120));
    assert_eq!(first_line("  \n"), "");
}
