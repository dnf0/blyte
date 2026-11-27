//! Persistent sessions: the harness's commands match a fresh process exactly, the model's shell
//! keeps its directory and environment, and failures recover.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use blyte_agent::fs::{Fs, LocalFs, SessionFs};
use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

const T: Duration = Duration::from_secs(30);

fn setup() -> (tempfile::TempDir, PathBuf, PersistentShell) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    let p = PersistentShell::local(root.clone());
    (d, root, p)
}

#[test]
fn harness_commands_match_a_fresh_process_exactly() {
    let (_d, root, p) = setup();
    let cmds = [
        "echo hi",
        "printf 'no newline'",
        "echo out; echo err >&2; exit 3",
        "cd / && pwd",
        "cat",
        "seq 1 20000",
        "echo héllo wörld",
        "false",
        "echo $((6*7))",
        "printf 'a\\tb\\n\\n\\n'",
        "for i in 1 2 3; do echo line $i; done",
    ];
    let (olds, news) = std::thread::scope(|s| {
        let old_handles: Vec<_> = cmds
            .iter()
            .map(|&cmd| {
                let root = &root;
                s.spawn(move || LocalShell.exec(cmd, root, T).unwrap())
            })
            .collect();
        let new_handle = s.spawn(|| {
            cmds.iter()
                .map(|&cmd| p.exec(cmd, &root, T).unwrap())
                .collect::<Vec<_>>()
        });
        let olds: Vec<_> = old_handles.into_iter().map(|h| h.join().unwrap()).collect();
        (olds, new_handle.join().unwrap())
    });
    for ((cmd, old), new) in cmds.iter().zip(olds).zip(news) {
        assert_eq!(new, old, "{cmd}");
    }
}

#[test]
fn harness_commands_never_persist_state() {
    let (_d, root, p) = setup();
    p.exec("cd sub; export FOO=1", &root, T).unwrap();
    let o = p.exec("pwd; echo ${FOO:-unset}", &root, T).unwrap();
    assert_eq!(o.stdout, format!("{}\nunset\n", root.display()));
}

#[test]
fn the_models_shell_keeps_its_directory_and_environment() {
    let (_d, root, p) = setup();
    p.exec_user("cd sub && export FOO=bar", &root, T).unwrap();
    let o = p.exec_user("pwd; echo $FOO", &root, T).unwrap();
    assert_eq!(o.stdout, format!("{}/sub\nbar\n", root.display()));
    // ...and none of it leaks into the harness's own commands.
    let h = p.exec("pwd; echo ${FOO:-unset}", &root, T).unwrap();
    assert_eq!(h.stdout, format!("{}\nunset\n", root.display()));
}

#[test]
fn a_timeout_kills_the_command_and_keeps_its_output() {
    let (_d, root, p) = setup();
    p.exec_user("true", &root, T).unwrap();
    let o = p
        .exec_user("echo start; sleep 10", &root, Duration::from_secs(2))
        .unwrap();
    assert_eq!(o.exit_code, 124);
    assert_eq!(o.stdout, "start\n");
    assert!(
        o.stderr.ends_with("[killed by blyte after 2s]"),
        "{:?}",
        o.stderr
    );
    assert_eq!(
        p.exec_user("echo alive", &root, T).unwrap().stdout,
        "alive\n"
    );
}

#[test]
fn exit_ends_the_shell_and_the_next_command_gets_a_fresh_one() {
    let (_d, root, p) = setup();
    p.exec_user("cd sub", &root, T).unwrap();
    let o = p.exec_user("echo bye; exit 7", &root, T).unwrap();
    assert_eq!(o.stdout, "bye\n");
    assert!(o.stderr.contains("the shell exited"), "{:?}", o.stderr);
    let fresh = p.exec_user("pwd", &root, T).unwrap();
    assert_eq!(fresh.stdout, format!("{}\n", root.display()));
}

#[test]
fn a_background_job_does_not_hold_the_command() {
    let (_d, root, p) = setup();
    p.exec_user("true", &root, T).unwrap();
    let t = Instant::now();
    let o = p.exec_user("sleep 30 & echo started", &root, T).unwrap();
    assert_eq!(o.stdout, "started\n");
    assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
    let _ = p.exec_user("kill $! 2>/dev/null; wait $! 2>/dev/null || true", &root, T);
}

#[test]
fn reuses_a_single_shell_process_across_commands() {
    let (_d, root, p) = setup();
    assert!(p.is_persistent());

    let harness_pid: u32 = p
        .exec("echo $$", &root, T)
        .unwrap()
        .stdout
        .trim()
        .parse()
        .unwrap();
    for _ in 0..10 {
        let pid: u32 = p
            .exec("echo $$", &root, T)
            .unwrap()
            .stdout
            .trim()
            .parse()
            .unwrap();
        assert_eq!(pid, harness_pid);
    }

    let user_pid: u32 = p
        .exec_user("echo $$", &root, T)
        .unwrap()
        .stdout
        .trim()
        .parse()
        .unwrap();
    assert_ne!(user_pid, harness_pid);
    p.exec_user("cd sub && SESSION_VAR=kept", &root, T).unwrap();
    let next = p
        .exec_user(
            "printf '%s\\n%s\\n%s\\n' \"$$\" \"$PWD\" \"$SESSION_VAR\"",
            &root,
            T,
        )
        .unwrap();
    assert_eq!(
        next.stdout,
        format!("{user_pid}\n{}/sub\nkept\n", root.display())
    );
}

#[test]
fn session_files_write_and_read_exact_bytes() {
    let (_d, root, _p) = setup();
    let fs = SessionFs::local(root.clone());
    let content = "quotes ' \" `ticks` $HOME \\ back\nünïcode ✓\nno trailing newline";
    let path = root.join("deep/dir/f.txt");
    fs.write(&path, content).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
    assert_eq!(fs.read(&path).unwrap(), LocalFs.read(&path).unwrap());
    std::os::unix::fs::symlink(&path, root.join("link")).unwrap();
    assert_eq!(
        fs.canonicalize(&root.join("link")).unwrap(),
        LocalFs.canonicalize(&root.join("link")).unwrap()
    );
    assert!(fs.read(&root.join("missing.txt")).is_err());
    let big = "x".repeat(200_000);
    fs.write(&root.join("big.txt"), &big).unwrap();
    assert_eq!(fs.read(&root.join("big.txt")).unwrap(), big);
}

#[test]
fn a_models_command_sees_the_marker_and_a_harness_command_does_not() {
    let (_d, root, p) = setup();
    let user_out = p
        .exec_user("echo \"$BLYTE_MODEL_SHELL\"", &root, T)
        .unwrap();
    assert_eq!(user_out.stdout, "1\n");
    let harness_out = p.exec("echo \"$BLYTE_MODEL_SHELL\"", &root, T).unwrap();
    assert_eq!(harness_out.stdout, "\n");

    let local_user = LocalShell
        .exec_user("echo \"$BLYTE_MODEL_SHELL\"", &root, T)
        .unwrap();
    assert_eq!(local_user.stdout, "1\n");
    let local_host = LocalShell
        .exec_host("echo \"$BLYTE_MODEL_SHELL\"", &root, T)
        .unwrap();
    assert_eq!(local_host.stdout, "1\n");
    let local_harness = LocalShell
        .exec("echo \"$BLYTE_MODEL_SHELL\"", &root, T)
        .unwrap();
    assert_eq!(local_harness.stdout, "\n");
}
