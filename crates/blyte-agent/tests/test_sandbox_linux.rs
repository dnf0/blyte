//! On Linux blyte's shell refuses writes outside the allow-list via Landlock: the macOS sandbox
//! (#91) left Linux runs able to write anywhere the user can (2026-09-27).

use std::path::{Path, PathBuf};

use blyte_agent::sandbox::{conversation_profile, default_profile, Profile};

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

#[test]
fn conversation_profile_allows_only_wt_tasks_for_writes() {
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let d = tempfile::tempdir_in(tmp_base).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(d.path())
        .status()
        .unwrap()
        .success());
    let tasks = d.path().join(".wt/tasks");
    std::fs::create_dir_all(&tasks).unwrap();
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root = canon(d.path());
    let tasks_canon = canon(&tasks);
    let git_canon = canon(&d.path().join(".git"));
    let wt_canon = canon(&d.path().join(".wt"));

    let p = conversation_profile(d.path(), &[]);
    if cfg!(target_os = "macos") {
        let Profile::Seatbelt(s) = p else {
            panic!("expected Seatbelt");
        };
        assert!(
            s.contains(&format!(
                "(allow file-write* (subpath \"{}\"))",
                tasks_canon.display()
            )),
            "{s}"
        );
        assert!(
            !s.contains(&format!(
                "(allow file-write* (subpath \"{}\"))",
                root.display()
            )),
            "{s}"
        );
        assert!(
            !s.contains(&format!(
                "(allow file-write* (subpath \"{}\"))",
                git_canon.display()
            )),
            "{s}"
        );
        assert!(
            !s.contains(&format!(
                "(allow file-write* (subpath \"{}\"))",
                wt_canon.display()
            )),
            "{s}"
        );
    } else {
        match p {
            Profile::LandlockConfined { write, read, .. } => {
                assert!(write.iter().any(|p| canon(p) == tasks_canon), "{write:?}");
                assert!(!write.iter().any(|p| canon(p) == root), "{write:?}");
                assert!(!write.iter().any(|p| canon(p) == git_canon), "{write:?}");
                assert!(!write.iter().any(|p| canon(p) == wt_canon), "{write:?}");
                assert!(read.iter().any(|p| canon(p) == root), "{read:?}");
            }
            other => panic!("expected LandlockConfined, got {other:?}"),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;
    use std::time::Duration;

    use blyte_agent::sandbox::{available, conversation_profile, Profile};
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

    #[test]
    fn conversation_profile_writes_only_in_wt_tasks_not_root_git_or_wt_passed() {
        if !landlock_or_skip() {
            return;
        }
        let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
        std::fs::create_dir_all(tmp_base).unwrap();
        let repo = tempfile::tempdir_in(tmp_base).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
        let tasks = repo.path().join(".wt/tasks");
        let passed = repo.path().join(".wt/passed");
        std::fs::create_dir_all(&tasks).unwrap();
        std::fs::create_dir_all(&passed).unwrap();

        let sh = LocalShell::sandboxed(conversation_profile(repo.path(), &[]));
        let ok_file = tasks.join("task.md");
        let ok = sh
            .exec(
                &format!("echo ok > '{}'", ok_file.display()),
                repo.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        assert!(ok_file.exists());

        for denied_path in [
            repo.path().join("root.txt"),
            repo.path().join(".git/forged"),
            passed.join("forged.json"),
        ] {
            let r = sh
                .exec(
                    &format!("echo no > '{}'", denied_path.display()),
                    repo.path(),
                    Duration::from_secs(30),
                )
                .unwrap();
            assert_ne!(
                r.exit_code,
                0,
                "write to {} must fail",
                denied_path.display()
            );
            assert!(
                !denied_path.exists(),
                "{} must not be created",
                denied_path.display()
            );
        }
    }
}
