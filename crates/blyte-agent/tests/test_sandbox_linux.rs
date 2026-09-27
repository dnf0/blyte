//! On Linux blyte's shell refuses writes outside the allow-list via Landlock: the macOS sandbox
//! (#91) left Linux runs able to write anywhere the user can (2026-09-27).

use std::path::PathBuf;

use blyte_agent::sandbox::{default_profile, Profile};

#[test]
fn a_string_profile_is_a_seatbelt_profile() {
    let p: Profile = String::from("(version 1)").into();
    assert_eq!(p, Profile::Seatbelt("(version 1)".into()));
}

#[test]
fn the_default_profile_matches_the_platform() {
    let d = tempfile::tempdir().unwrap();
    let p = default_profile(d.path(), &[]);
    if cfg!(target_os = "macos") {
        assert!(
            matches!(p, Profile::Seatbelt(ref s) if s.starts_with("(version 1)")),
            "{p:?}"
        );
    } else {
        match p {
            Profile::LandlockConfined {
                write,
                read,
                block_tcp: false,
            } => {
                assert!(write.contains(&d.path().to_path_buf()), "{write:?}");
                assert!(write.contains(&PathBuf::from("/dev")), "{write:?}");
                assert!(read.contains(&PathBuf::from("/usr")), "{read:?}");
            }
            other => panic!("expected LandlockConfined, got {other:?}"),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::time::Duration;

    use blyte_agent::sandbox::{available, Profile};
    use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

    fn landlock_or_skip() -> bool {
        if available() {
            return true;
        }
        // CI (GitHub's Ubuntu runners) must have Landlock, or these tests prove nothing.
        assert!(
            std::env::var_os("CI").is_none(),
            "Landlock unavailable on CI"
        );
        false
    }

    #[test]
    fn a_landlocked_command_can_write_inside_and_not_outside() {
        if !landlock_or_skip() {
            return;
        }
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
        let sh = LocalShell::sandboxed(Profile::Landlock(vec![
            inside.path().to_path_buf(),
            "/dev".into(),
        ]));
        let ok = sh
            .exec(
                "echo hi > ok.txt && echo hi > /dev/null",
                inside.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        let target = outside.path().join("no.txt");
        let denied = sh
            .exec(
                &format!("echo hi > '{}'", target.display()),
                inside.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(denied.exit_code, 0, "{}", denied.merged());
        assert!(!target.exists());
    }

    #[test]
    fn a_landlocked_persistent_session_is_confined_too() {
        if !landlock_or_skip() {
            return;
        }
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir_in(std::env::var("HOME").unwrap()).unwrap();
        let sh = PersistentShell::local_sandboxed(
            inside.path().to_path_buf(),
            Profile::Landlock(vec![
                inside.path().to_path_buf(),
                "/dev".into(),
                "/tmp".into(),
            ]),
        );
        let ok = sh
            .exec("echo hi > ok.txt", inside.path(), Duration::from_secs(30))
            .unwrap();
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        let target = outside.path().join("no.txt");
        let denied = sh
            .exec(
                &format!("echo hi > '{}'", target.display()),
                inside.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(denied.exit_code, 0, "{}", denied.merged());
        assert!(!target.exists());
    }
}
