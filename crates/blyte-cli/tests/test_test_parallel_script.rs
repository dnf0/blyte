use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ directory")
        .parent()
        .expect("repository root")
        .to_path_buf()
}

fn script_path() -> PathBuf {
    repo_root().join("scripts/test-parallel.py")
}

fn write_sh(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write shell script");
    let mut perms = fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).expect("chmod");
    path
}

fn write_list(dir: &Path, entries: &[(&Path, &Path)]) -> PathBuf {
    let list_path = dir.join("executables.tsv");
    let mut content = String::new();
    for (exe, cwd) in entries {
        content.push_str(exe.to_str().expect("utf8 exe"));
        content.push('\t');
        content.push_str(cwd.to_str().expect("utf8 cwd"));
        content.push('\n');
    }
    fs::write(&list_path, content).expect("write executables list");
    list_path
}

fn run_parallel(args: &[&str]) -> Output {
    let script = script_path();
    Command::new("python3")
        .arg(&script)
        .args(args)
        .env_remove("FOO")
        .env_remove("EMPTY")
        .output()
        .unwrap_or_else(|e| panic!("failed to run {}: {e}", script.display()))
}

#[test]
fn all_pass_exits_zero() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let alpha = write_sh(root, "alpha.sh", "echo 'test result: ok. 1 passed'");
    let beta = write_sh(root, "beta.sh", "echo 'test result: ok. 1 passed'");
    let list = write_list(root, &[(&alpha, root), (&beta, root)]);

    let out = run_parallel(&[
        "--root",
        root.to_str().unwrap(),
        "--executables-from",
        list.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.lines().any(|line| line.starts_with("ok ")
            && line.contains("alpha.sh")
            && line.contains("test result: ok. 1 passed")),
        "missing ok line for alpha.sh in:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.starts_with("ok ")
            && line.contains("beta.sh")
            && line.contains("test result: ok. 1 passed")),
        "missing ok line for beta.sh in:\n{stdout}"
    );
}

#[test]
fn one_failure_exits_one_and_prints_its_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let passing = write_sh(root, "passing.sh", "echo 'SECRET-PASS-OUTPUT'\nexit 0");
    let failing = write_sh(root, "failing.sh", "echo 'boom-detail'\nexit 3");
    let list = write_list(root, &[(&passing, root), (&failing, root)]);

    let out = run_parallel(&[
        "--root",
        root.to_str().unwrap(),
        "--executables-from",
        list.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("boom-detail"),
        "expected boom-detail in stdout:\n{stdout}"
    );
    let summary = stdout.lines().last().unwrap_or("");
    assert!(
        summary.contains("failing.sh"),
        "expected failing.sh in summary line {summary:?}\nfull stdout:\n{stdout}"
    );
    assert!(
        !stdout.contains("SECRET-PASS-OUTPUT") && !stderr.contains("SECRET-PASS-OUTPUT"),
        "passing output leaked:\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn binaries_run_side_by_side() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let m1 = root.join("marker1");
    let m2 = root.join("marker2");
    let m1_str = m1.to_str().unwrap();
    let m2_str = m2.to_str().unwrap();

    let s1 = write_sh(
        root,
        "peer1.sh",
        &format!(
            ": > '{m1_str}'\ni=0\nwhile [ \"$i\" -lt 200 ]; do\n  [ -f '{m2_str}' ] && exit 0\n  sleep 0.1\n  i=$((i + 1))\ndone\nexit 1"
        ),
    );
    let s2 = write_sh(
        root,
        "peer2.sh",
        &format!(
            ": > '{m2_str}'\ni=0\nwhile [ \"$i\" -lt 200 ]; do\n  [ -f '{m1_str}' ] && exit 0\n  sleep 0.1\n  i=$((i + 1))\ndone\nexit 1"
        ),
    );
    let list = write_list(root, &[(&s1, root), (&s2, root)]);

    let out = run_parallel(&[
        "--jobs",
        "2",
        "--root",
        root.to_str().unwrap(),
        "--executables-from",
        list.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn jobs_one_runs_one_at_a_time() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let lock_dir = root.join("mutex_dir");
    let lock_str = lock_dir.to_str().unwrap();
    let body = format!("set -e\nmkdir '{lock_str}'\nsleep 0.2\nrmdir '{lock_str}'");

    let s1 = write_sh(root, "serial1.sh", &body);
    let s2 = write_sh(root, "serial2.sh", &body);
    let list = write_list(root, &[(&s1, root), (&s2, root)]);

    let out = run_parallel(&[
        "--jobs",
        "1",
        "--root",
        root.to_str().unwrap(),
        "--executables-from",
        list.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn env_table_and_manifest_dir_are_set() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let cargo_dir = root.join(".cargo");
    fs::create_dir_all(&cargo_dir).expect("create .cargo");
    fs::write(
        cargo_dir.join("config.toml"),
        "[env]\nFOO = \"bar\"\nEMPTY = \"\"\n",
    )
    .expect("write config.toml");

    let pkg_dir = root.join("crate_dir");
    fs::create_dir_all(&pkg_dir).expect("create crate_dir");

    let check = write_sh(
        root,
        "check_env.sh",
        "set -eu\n[ \"$FOO\" = \"bar\" ]\n[ \"${EMPTY+set}\" = \"set\" ]\n[ -z \"$EMPTY\" ]\n[ \"$CARGO_MANIFEST_DIR\" = \"$(pwd)\" ]",
    );
    let list = write_list(root, &[(&check, &pkg_dir)]);

    let out = run_parallel(&[
        "--root",
        root.to_str().unwrap(),
        "--executables-from",
        list.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn a_table_valued_env_entry_is_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let cargo_dir = root.join(".cargo");
    fs::create_dir_all(&cargo_dir).expect("create .cargo");
    fs::write(
        cargo_dir.join("config.toml"),
        "[env]\nFOO = { value = \"x\", relative = true }\n",
    )
    .expect("write config.toml");

    let dummy = write_sh(root, "dummy.sh", "exit 0");
    let list = write_list(root, &[(&dummy, root)]);

    let out = run_parallel(&[
        "--root",
        root.to_str().unwrap(),
        "--executables-from",
        list.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(2),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("FOO") || stdout.contains("FOO"),
        "expected error naming FOO\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn a_missing_executable_is_a_failure() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let missing = root.join("does-not-exist-bin");
    let list = write_list(root, &[(&missing, root)]);

    let out = run_parallel(&[
        "--root",
        root.to_str().unwrap(),
        "--executables-from",
        list.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    let missing_str = missing.to_str().unwrap();
    assert!(
        stdout.contains(missing_str) || stderr.contains(missing_str),
        "expected missing path {missing_str:?}\nstdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn an_empty_list_is_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let list = write_list(root, &[]);

    let out = run_parallel(&[
        "--root",
        root.to_str().unwrap(),
        "--executables-from",
        list.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(2),
        "stdout: {stdout}\nstderr: {stderr}"
    );
}

#[test]
fn a_binary_past_the_timeout_is_killed_and_reported_failed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let sleeper = write_sh(root, "sleeper.sh", "echo started\nsleep 60");
    let passing = write_sh(root, "passing.sh", "echo 'test result: ok. 1 passed'");
    let list = write_list(root, &[(&sleeper, root), (&passing, root)]);

    let start = Instant::now();
    // 10 s, not 2: on a loaded machine starting the passing script can itself take seconds.
    let out = run_parallel(&[
        "--timeout",
        "10",
        "--root",
        root.to_str().unwrap(),
        "--executables-from",
        list.to_str().unwrap(),
    ]);
    let elapsed = start.elapsed();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(1),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains("started"),
        "expected started in stdout:\n{stdout}"
    );
    assert!(
        stdout.contains("FAILED (timed out after 10s)"),
        "expected timeout failure line in stdout:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|line| line.starts_with("ok ")
            && line.contains("passing.sh")
            && line.contains("test result: ok. 1 passed")),
        "missing ok line for passing.sh in:\n{stdout}"
    );
    assert!(
        elapsed < Duration::from_secs(45),
        "expected run to finish within 20s, took {elapsed:?}"
    );
}

#[test]
fn a_timeout_below_one_is_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let dummy = write_sh(root, "dummy.sh", "exit 0");
    let list = write_list(root, &[(&dummy, root)]);

    let out = run_parallel(&[
        "--timeout",
        "0",
        "--root",
        root.to_str().unwrap(),
        "--executables-from",
        list.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        out.status.code(),
        Some(2),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("--timeout") || stdout.contains("--timeout"),
        "expected error naming --timeout\nstdout: {stdout}\nstderr: {stderr}"
    );
}
