//! On Linux, commands read only system directories, the workspace, tool caches and grants:
//! Landlock cannot deny inside an allowed tree, but it can allow-list reads (2026-09-27).

use std::path::{Path, PathBuf};

use blyte_agent::sandbox::read_allowed;

#[test]
fn the_allowlist_has_the_system_and_tool_dirs_but_no_credential_stores() {
    let home = Path::new("/home/someone");
    let r = read_allowed(Some(home), &[PathBuf::from("/data/granted")]);
    for p in ["/usr", "/etc", "/tmp", "/proc"] {
        assert!(r.contains(&PathBuf::from(p)), "{p} missing");
    }
    for p in [
        ".cargo",
        ".rustup",
        ".cache",
        ".config/git",
        ".gitconfig",
        ".docker/buildx",
    ] {
        assert!(r.contains(&home.join(p)), "{p} missing");
    }
    for p in [
        ".ssh",
        ".aws",
        ".config/gh",
        ".codex",
        ".docker/config.json",
        ".npmrc",
        ".local",
        ".config",
    ] {
        assert!(!r.contains(&home.join(p)), "{p} must not be readable");
    }
    assert!(
        !r.contains(&home.to_path_buf()),
        "the home directory itself is not granted"
    );
    assert_eq!(r.last(), Some(&PathBuf::from("/data/granted")));
    assert!(read_allowed(None, &[])
        .iter()
        .all(|p| !p.starts_with("/home")));
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::PathBuf;
    use std::time::Duration;

    use blyte_agent::sandbox::{available, read_allowed, Profile};
    use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

    fn landlock_or_skip() -> bool {
        if available() {
            return true;
        }
        assert!(
            std::env::var_os("CI").is_none(),
            "Landlock unavailable on CI"
        );
        false
    }

    fn confined(ws: &std::path::Path, grants: &[PathBuf]) -> Profile {
        Profile::LandlockConfined {
            write: vec![ws.to_path_buf(), "/dev".into(), "/tmp".into()],
            read: read_allowed(None, grants),
            block_tcp: false,
        }
    }

    #[test]
    fn a_confined_command_reads_the_workspace_and_system_but_not_other_home_files() {
        if !landlock_or_skip() {
            return;
        }
        let ws = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
        std::fs::write(secret.path().join("key"), "s3cret").unwrap();
        let sh = LocalShell::sandboxed(confined(ws.path(), &[]));
        let ok = sh
            .exec(
                "echo hi > f.txt && cat f.txt && ls /usr/bin >/dev/null",
                ws.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        let denied = sh
            .exec(
                &format!("cat '{}'", secret.path().join("key").display()),
                ws.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(denied.exit_code, 0, "{}", denied.merged());
        assert!(!denied.merged().contains("s3cret"));
    }

    #[test]
    fn an_allow_read_grant_makes_a_home_path_readable() {
        if !landlock_or_skip() {
            return;
        }
        let ws = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
        std::fs::write(secret.path().join("key"), "s3cret").unwrap();
        let sh = LocalShell::sandboxed(confined(ws.path(), &[secret.path().to_path_buf()]));
        let out = sh
            .exec(
                &format!("cat '{}'", secret.path().join("key").display()),
                ws.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_eq!(out.exit_code, 0, "{}", out.merged());
        assert!(out.merged().contains("s3cret"));
    }

    #[test]
    fn a_confined_persistent_session_works_and_is_confined() {
        if !landlock_or_skip() {
            return;
        }
        let ws = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
        std::fs::write(secret.path().join("key"), "s3cret").unwrap();
        let sh =
            PersistentShell::local_sandboxed(ws.path().to_path_buf(), confined(ws.path(), &[]));
        let ok = sh
            .exec(
                "echo hi > g.txt && cat g.txt",
                ws.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        let denied = sh
            .exec(
                &format!("cat '{}'", secret.path().join("key").display()),
                ws.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(denied.exit_code, 0, "{}", denied.merged());
    }
}
