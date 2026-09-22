//! A command must not be able to hang the harness.

use std::time::{Duration, Instant};

use superstate_agent::shell::{LocalShell, Shell};

#[test]
fn a_runaway_command_is_killed_at_the_deadline() {
    // Regression for the first live run. The model opened with
    // `which pytest; which python3; find / -name pytest 2>/dev/null` and the `find /` walked
    // the entire filesystem for three and a half minutes until it was killed by hand. The
    // timeout parameter existed and was ignored.
    let d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let out = LocalShell
        .exec("sleep 30", d.path(), Duration::from_secs(1))
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(10),
        "must not wait for the command: {elapsed:?}"
    );
    assert_eq!(
        out.exit_code, 124,
        "conventional timeout code, as GNU timeout(1) uses"
    );
    assert!(
        out.stderr.contains("killed by superstate"),
        "the kill must be stated: {out:?}"
    );
}

#[test]
fn output_produced_before_the_kill_is_kept() {
    // A killed command that returns nothing tells the model only that something went wrong.
    let d = tempfile::tempdir().unwrap();
    let out = LocalShell
        .exec(
            "echo progress-so-far; sleep 30",
            d.path(),
            Duration::from_secs(2),
        )
        .unwrap();
    assert!(
        out.stdout.contains("progress-so-far"),
        "partial output must survive: {out:?}"
    );
}

#[test]
fn a_command_that_reads_stdin_does_not_block_forever() {
    // Inheriting stdin lets a bare `python3`, a `git commit` opening an editor, or an npm
    // prompt wait forever on a terminal nobody is watching.
    let d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let out = LocalShell
        .exec("cat", d.path(), Duration::from_secs(5))
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "stdin must be closed, not inherited: waited {:?}",
        started.elapsed()
    );
    assert_eq!(
        out.exit_code, 0,
        "cat on empty stdin exits cleanly: {out:?}"
    );
}

#[test]
fn a_command_producing_more_output_than_the_pipe_buffer_does_not_deadlock() {
    // Reading only after waiting deadlocks against a full pipe, turning a noisy command into
    // a hang -- the same symptom as the timeout bug, from a different cause.
    let d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let out = LocalShell
        .exec("seq 1 200000", d.path(), Duration::from_secs(30))
        .unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(
        out.stdout.lines().count() > 199_000,
        "all output must be drained"
    );
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "must not have deadlocked"
    );
}

#[test]
fn a_normal_command_is_unaffected() {
    let d = tempfile::tempdir().unwrap();
    let out = LocalShell
        .exec("echo hello", d.path(), Duration::from_secs(30))
        .unwrap();
    assert_eq!(out.stdout.trim(), "hello");
    assert_eq!(out.exit_code, 0);
}

#[test]
fn grandchild_process_tree_is_killed_at_deadline() {
    // If only the parent bash process is killed, background grandchildren
    // (such as `find /` or subshells) stay alive, keep pipe handles open,
    // and cause reader thread joins to hang.
    let d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let out = LocalShell
        .exec(
            "sh -c '(sleep 30) & wait'",
            d.path(),
            Duration::from_secs(1),
        )
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(4),
        "must not deadlock on grandchild pipe handles: took {elapsed:?}"
    );
    assert_eq!(out.exit_code, 124, "expected exit code 124 on timeout");
    assert!(
        out.stderr.contains("killed by superstate"),
        "kill notice missing from stderr: {:?}",
        out.stderr
    );
}

#[test]
fn container_shell_enforces_timeout_on_hung_execution() {
    use std::path::PathBuf;
    use superstate_agent::shell::ContainerShell;

    let shell = ContainerShell {
        container_id: "non-existent-container-id-12345".into(),
        workdir: PathBuf::from("/workspace"),
    };
    let d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let res = shell.exec("sleep 30", d.path(), Duration::from_millis(500));
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(3),
        "container exec must obey timeout bound: took {elapsed:?}"
    );
    match res {
        Ok(out) => {
            assert!(out.exit_code != 0);
        }
        Err(e) => {
            assert!(!e.is_empty());
        }
    }
}
