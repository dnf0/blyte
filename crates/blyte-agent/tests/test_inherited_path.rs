//! Commands blyte runs find the programs the user's own shell finds first: on macOS the login
//! profile's path_helper moved /usr/local/bin ahead of ~/.pyenv/shims, so `python3` inside blyte
//! lacked pytest and a dispatch spent 20 turns on it (2026-09-27).

use std::time::Duration;

use blyte_agent::shell::{inherited_path_prefix, LocalShell, PersistentShell, Shell};

// One test, because it changes this process's PATH.
#[test]
fn a_program_first_on_the_inherited_path_wins_in_commands_and_sessions() {
    let bin = tempfile::tempdir().unwrap();
    // A fake `ls`: the system one lives in /bin, which a login profile may move to the front.
    let fake = bin.path().join("ls");
    std::fs::write(&fake, "#!/bin/sh\necho fake-ls\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let old = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{old}", bin.path().display()));

    let prefix = inherited_path_prefix();
    assert!(prefix.starts_with("export PATH="), "{prefix}");
    assert!(prefix.ends_with(":\"$PATH\"; "), "{prefix}");

    let ws = tempfile::tempdir().unwrap();
    let out = LocalShell
        .exec("ls", ws.path(), Duration::from_secs(30))
        .unwrap();
    assert_eq!(out.stdout.trim(), "fake-ls", "{}", out.merged());

    let session = PersistentShell::local(ws.path().to_path_buf());
    let out = session
        .exec("ls", ws.path(), Duration::from_secs(30))
        .unwrap();
    assert_eq!(out.stdout.trim(), "fake-ls", "{}", out.merged());

    std::env::set_var("PATH", old);
}
