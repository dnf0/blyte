use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use blyte_agent::folder_lock::{acquire, holder, Holder};

fn sample_holder(pid: u32, kind: &str, id: &str) -> Holder {
    Holder {
        pid,
        kind: kind.to_string(),
        id: id.to_string(),
    }
}

#[test]
fn a_second_acquire_is_refused_naming_the_holder() {
    let tmp = tempfile::tempdir().unwrap();
    let lock_path = tmp.path().join("in-place.lock");
    let first = sample_holder(4242, "run", "run-abc");
    let second = sample_holder(9999, "chat", "chat-1");

    let _guard = acquire(&lock_path, &first).unwrap();
    let err = acquire(&lock_path, &second).unwrap_err();
    assert_eq!(
        err,
        "run run-abc (pid 4242) is already working in this folder"
    );
}

#[test]
fn the_lock_is_free_after_the_guard_is_dropped() {
    let tmp = tempfile::tempdir().unwrap();
    let lock_path = tmp.path().join("in-place.lock");
    let first = sample_holder(111, "run", "first");
    let second = sample_holder(222, "run", "second");

    let guard = acquire(&lock_path, &first).unwrap();
    drop(guard);

    let _guard2 = acquire(&lock_path, &second).unwrap();
    assert_eq!(holder(&lock_path).unwrap(), Some(second));
}

#[test]
fn holder_is_none_for_a_missing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let lock_path = tmp.path().join("does-not-exist.lock");

    assert_eq!(holder(&lock_path).unwrap(), None);
    assert!(!lock_path.exists());
}

#[test]
fn holder_is_none_when_nobody_holds_it_and_the_file_is_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let lock_path = tmp.path().join("in-place.lock");
    let previous = sample_holder(777, "run", "finished-run");

    let guard = acquire(&lock_path, &previous).unwrap();
    drop(guard);

    let before = std::fs::read(&lock_path).unwrap();
    assert_eq!(holder(&lock_path).unwrap(), None);
    let after = std::fs::read(&lock_path).unwrap();
    assert_eq!(before, after);
}

#[test]
fn holder_reports_who_holds_it() {
    let tmp = tempfile::tempdir().unwrap();
    let lock_path = tmp.path().join("in-place.lock");
    let current = sample_holder(std::process::id(), "run", "active-1");

    let _guard = acquire(&lock_path, &current).unwrap();
    assert_eq!(holder(&lock_path).unwrap(), Some(current));
}

#[cfg(unix)]
#[test]
fn the_lock_is_released_when_the_holding_process_dies() {
    let tmp = tempfile::tempdir().unwrap();
    let lock_path = tmp.path().join("in-place.lock");

    let script = r#"
import fcntl, json, os, sys, time
path = sys.argv[1]
f = open(path, "w+")
fcntl.flock(f.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
f.truncate(0)
f.seek(0)
f.write(json.dumps({"pid": os.getpid(), "kind": "run", "id": "child-run"}) + "\n")
f.flush()
print("LOCKED", flush=True)
time.sleep(60)
"#;

    let mut child = Command::new("python3")
        .arg("-u")
        .arg("-c")
        .arg(script)
        .arg(&lock_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let child_pid = child.id();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "LOCKED");

    let ours = sample_holder(std::process::id(), "run", "after-crash");
    let err = acquire(&lock_path, &ours).unwrap_err();
    assert_eq!(
        err,
        format!("run child-run (pid {child_pid}) is already working in this folder")
    );
    assert_eq!(
        holder(&lock_path).unwrap(),
        Some(sample_holder(child_pid, "run", "child-run"))
    );

    child.kill().unwrap();
    let _ = child.wait().unwrap();

    let _guard = acquire(&lock_path, &ours).unwrap();
    assert_eq!(holder(&lock_path).unwrap(), Some(ours));
}

#[cfg(unix)]
#[test]
fn an_unparseable_locked_file_names_the_lock_file() {
    use std::os::unix::io::AsRawFd;

    let tmp = tempfile::tempdir().unwrap();
    let lock_path = tmp.path().join("in-place.lock");
    std::fs::write(&lock_path, "not json\n").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    let ret = unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(ret, 0);

    let err = acquire(&lock_path, &sample_holder(1, "run", "x")).unwrap_err();
    assert!(err.contains("locked"), "{err}");
    assert!(err.contains(&lock_path.display().to_string()), "{err}");
    drop(held);
}
