use std::time::Duration;

use blyte_agent::fs::{Fs, SessionFs};
use blyte_agent::shell::{poll_backoff, POLL_START};

#[test]
fn backoff_doubles_from_one_millisecond_to_fifty() {
    let remaining = Duration::from_secs(10);
    let mut current = POLL_START;
    let mut sleeps = Vec::new();
    for _ in 0..8 {
        let (sleep, next) = poll_backoff(current, remaining);
        sleeps.push(sleep);
        current = next;
    }
    let expected: Vec<Duration> = [1, 2, 4, 8, 16, 32, 50, 50]
        .into_iter()
        .map(Duration::from_millis)
        .collect();
    assert_eq!(sleeps, expected);
}

#[test]
fn backoff_never_sleeps_past_the_deadline() {
    let (sleep, _) = poll_backoff(Duration::from_millis(50), Duration::from_millis(7));
    assert_eq!(sleep, Duration::from_millis(7));

    let (sleep_zero, _) = poll_backoff(Duration::from_millis(50), Duration::ZERO);
    assert_eq!(sleep_zero, Duration::ZERO);
}

#[test]
fn session_write_creates_a_missing_directory_and_reuses_an_existing_one() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let fs = SessionFs::local(root.clone());

    let one = root.join("a/b/one.txt");
    let two = root.join("a/b/two.txt");

    fs.write(&one, "first").unwrap();
    fs.write(&two, "second").unwrap();
    fs.write(&one, "overwritten").unwrap();

    assert_eq!(fs.read(&one).unwrap(), "overwritten");
    assert_eq!(fs.read(&two).unwrap(), "second");
    assert_eq!(std::fs::read_to_string(&one).unwrap(), "overwritten");
    assert_eq!(std::fs::read_to_string(&two).unwrap(), "second");
}

#[test]
fn session_write_fails_when_the_directory_cannot_be_made() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let fs = SessionFs::local(root.clone());

    let blocker = root.join("blocker");
    std::fs::write(&blocker, "not a directory").unwrap();

    let result = fs.write(&blocker.join("x.txt"), "data");
    assert!(result.is_err());
}
