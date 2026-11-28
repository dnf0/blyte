use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use blyte_cli::surface_dir::{is_running, probe, Liveness, SurfaceRecord};
use blyte_cli::surface_registry::{
    entry_path, live_entries, write_entry, RegistryEntry, SurfaceState,
};

fn dead_pid() -> u32 {
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id();
    let status = child.wait().unwrap();
    assert!(status.success());
    pid
}

struct HealthListener {
    port: u16,
    stop: Arc<AtomicBool>,
    accepted: Arc<AtomicUsize>,
    handle: Option<thread::JoinHandle<()>>,
}

impl HealthListener {
    fn start(surface_id: &str, delay: Duration) -> Self {
        Self::start_with_delays(surface_id, delay, delay, false)
    }

    fn start_retry_slow(surface_id: &str, retry_delay: Duration) -> Self {
        Self::start_with_delays(surface_id, Duration::ZERO, retry_delay, true)
    }

    fn start_with_delays(
        surface_id: &str,
        first_delay: Duration,
        retry_delay: Duration,
        hold_first_until_retry: bool,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let accepted = Arc::new(AtomicUsize::new(0));
        let accepted_bg = Arc::clone(&accepted);
        let body = format!(r#"{{"surface_id":"{surface_id}"}}"#);
        let handle = thread::spawn(move || {
            let mut workers = Vec::new();
            for stream in listener.incoming() {
                if stop_bg.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream else {
                    break;
                };
                let conn_num = accepted_bg.fetch_add(1, Ordering::SeqCst) + 1;
                let body = body.clone();
                let stop_worker = Arc::clone(&stop_bg);
                let accepted_worker = Arc::clone(&accepted_bg);
                workers.push(thread::spawn(move || {
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
                    let mut req = Vec::new();
                    let mut buf = [0u8; 512];
                    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                        match stream.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => req.extend_from_slice(&buf[..n]),
                        }
                    }
                    if conn_num == 1 && hold_first_until_retry {
                        while accepted_worker.load(Ordering::SeqCst) < 2
                            && !stop_worker.load(Ordering::Relaxed)
                        {
                            thread::sleep(Duration::from_millis(10));
                        }
                        return;
                    }
                    let delay = if conn_num == 1 {
                        first_delay
                    } else {
                        retry_delay
                    };
                    if !delay.is_zero() {
                        thread::sleep(delay);
                    }
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                }));
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Self {
            port,
            stop,
            accepted,
            handle: Some(handle),
        }
    }

    fn accepted_count(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}

impl Drop for HealthListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct SilentListener {
    port: u16,
    stop: Arc<AtomicBool>,
    accepted: Arc<AtomicUsize>,
    handle: Option<thread::JoinHandle<()>>,
}

impl SilentListener {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = Arc::clone(&stop);
        let accepted = Arc::new(AtomicUsize::new(0));
        let accepted_bg = Arc::clone(&accepted);
        let handle = thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                if stop_bg.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = stream else {
                    break;
                };
                accepted_bg.fetch_add(1, Ordering::SeqCst);
                held.push(stream);
            }
            drop(held);
        });
        Self {
            port,
            stop,
            accepted,
            handle: Some(handle),
        }
    }

    fn accepted_count(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }
}

impl Drop for SilentListener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[test]
fn port_zero_is_not_running() {
    assert_eq!(probe(std::process::id(), 0, "abc"), Liveness::NotRunning);
}

#[test]
fn closed_port_is_not_running() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    assert_eq!(probe(std::process::id(), port, "abc"), Liveness::NotRunning);
}

#[test]
fn dead_pid_is_not_running_without_connecting() {
    let server = HealthListener::start("abc", Duration::ZERO);
    let pid = dead_pid();
    assert_eq!(probe(pid, server.port, "abc"), Liveness::NotRunning);
    assert_eq!(server.accepted_count(), 0);
}

#[test]
fn right_id_is_running() {
    let server = HealthListener::start("abc", Duration::ZERO);
    assert_eq!(
        probe(std::process::id(), server.port, "abc"),
        Liveness::Running
    );
}

#[test]
fn other_id_is_not_running() {
    let server = HealthListener::start("abc", Duration::ZERO);
    assert_eq!(
        probe(std::process::id(), server.port, "xyz"),
        Liveness::NotRunning
    );
}

#[test]
fn silent_listener_is_unknown() {
    let silent = SilentListener::start();
    let start = Instant::now();
    let result = probe(std::process::id(), silent.port, "abc");
    let elapsed = start.elapsed();
    assert_eq!(result, Liveness::Unknown);
    assert_eq!(silent.accepted_count(), 2);
    assert!(
        elapsed < Duration::from_secs(60),
        "expected probe to return within 60s, took {elapsed:?}"
    );
}

#[test]
fn slow_answer_is_running() {
    let server = HealthListener::start_retry_slow("abc", Duration::from_millis(650));
    assert_eq!(
        probe(std::process::id(), server.port, "abc"),
        Liveness::Running
    );
    assert_eq!(server.accepted_count(), 2);
}

#[test]
fn live_entries_keeps_a_stalled_entry_and_removes_a_dead_one() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let silent = SilentListener::start();

    let stalled = RegistryEntry {
        id: "s00000000000000a1".to_string(),
        name: "stalled".to_string(),
        root: PathBuf::from("/tmp/stalled-repo"),
        branch: Some("main".to_string()),
        pid: std::process::id(),
        port: silent.port,
        started_at: 100,
        doing: "idle".to_string(),
        state: SurfaceState::Idle,
    };
    let dead = RegistryEntry {
        id: "s00000000000000b2".to_string(),
        name: "dead".to_string(),
        root: PathBuf::from("/tmp/dead-repo"),
        branch: Some("main".to_string()),
        pid: dead_pid(),
        port: silent.port,
        started_at: 200,
        doing: "idle".to_string(),
        state: SurfaceState::Idle,
    };

    write_entry(dir, &stalled).unwrap();
    write_entry(dir, &dead).unwrap();

    let live = live_entries(dir).unwrap();
    assert_eq!(live, vec![stalled]);
    assert!(entry_path(dir, "s00000000000000a1").exists());
    assert!(!entry_path(dir, "s00000000000000b2").exists());
}

#[test]
fn is_running_is_true_when_unknown() {
    let silent = SilentListener::start();
    let record = SurfaceRecord {
        id: "s00000000000000c3".to_string(),
        name: "busy".to_string(),
        pid: std::process::id(),
        port: silent.port,
        session_id: "sess-busy".to_string(),
        started_at: 1_700_000_000,
    };
    assert!(is_running(&record));
}
