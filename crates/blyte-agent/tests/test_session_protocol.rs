//! The lean session protocol: exact bytes in and out, robust to what the model's own shell state
//! (set -e, LC_ALL) and output look like.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use blyte_agent::fs::{Fs, SessionFs};
use blyte_agent::shell::{Output, PersistentShell, Shell};

const T: Duration = Duration::from_secs(30);

fn setup() -> (tempfile::TempDir, PathBuf, PersistentShell) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let p = PersistentShell::local(root.clone());
    (d, root, p)
}

#[test]
fn awkward_commands_and_large_outputs_are_exact() {
    let (_d, root, p) = setup();
    let cases = [
        (
            "printf '%s\\n' '__BLYTE_CMD_0_0__' \"a'b\" '$HOME' '`x`'",
            Output {
                stdout: "__BLYTE_CMD_0_0__\na'b\n$HOME\n`x`\n".into(),
                stderr: String::new(),
                exit_code: 0,
            },
        ),
        (
            "head -c 1000000 /dev/zero | tr '\\0' a",
            Output {
                stdout: "a".repeat(1_000_000),
                stderr: String::new(),
                exit_code: 0,
            },
        ),
        (
            "printf 'tail without newline'; printf 'err without newline' >&2",
            Output {
                stdout: "tail without newline".into(),
                stderr: "err without newline".into(),
                exit_code: 0,
            },
        ),
        (
            "cat <<'EOF'\nline one\n  indented $NOT_EXPANDED\nEOF",
            Output {
                stdout: "line one\n  indented $NOT_EXPANDED\n".into(),
                stderr: String::new(),
                exit_code: 0,
            },
        ),
        (
            "true",
            Output {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            },
        ),
    ];
    let (execs, users) = std::thread::scope(|s| {
        let exec_handle = s.spawn(|| {
            cases
                .iter()
                .map(|(cmd, _)| p.exec(cmd, &root, T).unwrap())
                .collect::<Vec<_>>()
        });
        let user_handle = s.spawn(|| {
            cases
                .iter()
                .map(|(cmd, _)| p.exec_user(cmd, &root, T).unwrap())
                .collect::<Vec<_>>()
        });
        (exec_handle.join().unwrap(), user_handle.join().unwrap())
    });
    for ((cmd, expected), (ex, us)) in cases.iter().zip(execs.into_iter().zip(users)) {
        assert_eq!(&ex, expected, "exec: {cmd}");
        assert_eq!(&us, expected, "exec_user: {cmd}");
    }
}

#[test]
fn the_models_set_e_does_not_break_the_session() {
    let (_d, root, p) = setup();
    p.exec_user("set -e", &root, T).unwrap();
    assert_eq!(
        p.exec_user("echo alive", &root, T).unwrap().stdout,
        "alive\n"
    );
    assert_eq!(
        p.exec_user("echo again", &root, T).unwrap().stdout,
        "again\n"
    );
}

#[test]
fn the_models_locale_is_restored_after_each_command() {
    let (_d, root, p) = setup();
    p.exec_user("export LC_ALL=POSIX", &root, T).unwrap();
    assert_eq!(
        p.exec_user("echo $LC_ALL", &root, T).unwrap().stdout,
        "POSIX\n"
    );
    p.exec_user("unset LC_ALL", &root, T).unwrap();
    assert_eq!(
        p.exec_user("echo ${LC_ALL-unset}", &root, T)
            .unwrap()
            .stdout,
        "unset\n"
    );
}

#[test]
fn container_style_writes_are_exact_for_awkward_content() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let cases = [
        "",
        "ends with newline\n",
        "no trailing newline",
        "__BLYTE_CONTENT_0__\nlooks like a delimiter\n",
        "two trailing newlines\n\n",
        "quotes ' \" ` $ \\ and ✓",
    ];
    std::thread::scope(|s| {
        for (chunk_idx, chunk) in cases.chunks(3).enumerate() {
            let root = &root;
            s.spawn(move || {
                let fs = SessionFs::local(root.clone());
                for (offset, content) in chunk.iter().enumerate() {
                    let i = chunk_idx * 3 + offset;
                    let path = root.join(format!("w{i}.txt"));
                    fs.write(&path, content).unwrap();
                    assert_eq!(
                        std::fs::read_to_string(&path).unwrap(),
                        *content,
                        "case {i}"
                    );
                    assert_eq!(fs.read(&path).unwrap(), *content, "case {i}");
                }
            });
        }
    });
}
