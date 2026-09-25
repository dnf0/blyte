//! The lean session protocol: exact bytes in and out, robust to what the model's own shell state
//! (set -e, LC_ALL) and output look like.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use blyte_agent::fs::{Fs, SessionFs};
use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

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
    for cmd in [
        "printf '%s\\n' '__BLYTE_CMD_0_0__' \"a'b\" '$HOME' '`x`'",
        "head -c 1000000 /dev/zero | tr '\\0' a",
        "printf 'tail without newline'; printf 'err without newline' >&2",
        "cat <<'EOF'\nline one\n  indented $NOT_EXPANDED\nEOF",
        "true",
    ] {
        let old = LocalShell.exec(cmd, &root, T).unwrap();
        assert_eq!(p.exec(cmd, &root, T).unwrap(), old, "exec: {cmd}");
        assert_eq!(p.exec_user(cmd, &root, T).unwrap(), old, "exec_user: {cmd}");
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
    let (_d, root, _p) = setup();
    let fs = SessionFs::local(root.clone());
    for (i, content) in [
        "",
        "ends with newline\n",
        "no trailing newline",
        "__BLYTE_CONTENT_0__\nlooks like a delimiter\n",
        "two trailing newlines\n\n",
        "quotes ' \" ` $ \\ and ✓",
    ]
    .iter()
    .enumerate()
    {
        let path = root.join(format!("w{i}.txt"));
        fs.write(&path, content).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            *content,
            "case {i}"
        );
        assert_eq!(fs.read(&path).unwrap(), *content, "case {i}");
    }
}
