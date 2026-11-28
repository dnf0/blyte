//! On Linux blyte's shell refuses writes outside the allow-list via Landlock: the macOS sandbox
//! (#91) left Linux runs able to write anywhere the user can (2026-09-27).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use blyte_agent::sandbox::{conversation_profile, default_profile, read_allowed, Profile};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn a_string_profile_is_a_seatbelt_profile() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p: Profile = String::from("(version 1)").into();
    assert_eq!(p, Profile::Seatbelt("(version 1)".into()));
}

#[test]
fn the_default_profile_matches_the_platform() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    let p = conversation_profile(d.path(), &[]).unwrap();
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

#[test]
fn read_allowed_excludes_blyte_state_and_config_dirs() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = Path::new("/home/someone");
    let allowed = read_allowed(Some(home), &[]);
    let state_dir = home.join(".local/state/blyte");
    let config_dir = home.join(".config/blyte");
    for secret_dir in [&state_dir, &config_dir] {
        assert!(
            !allowed.iter().any(|p| secret_dir.starts_with(p)),
            "{} must not be covered by {allowed:?}",
            secret_dir.display()
        );
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use blyte_agent::sandbox::{
        available, conversation_profile, conversation_profile_net, conversation_sandbox,
        conversation_sandbox_net, default_profile_net, default_sandbox_net, landlock_abi,
        surface_token_path, Profile,
    };
    use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

    use super::ENV_LOCK;

    struct EnvVarGuard {
        key: &'static str,
        prev: Option<OsString>,
    }

    impl EnvVarGuard {
        fn remove(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    struct RemoveDirGuard(PathBuf);

    impl Drop for RemoveDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct EnvGuard {
        home: Option<OsString>,
        xdg_state: Option<OsString>,
    }

    impl EnvGuard {
        fn set(home: Option<&Path>, xdg_state: Option<&Path>) -> Self {
            let prev = Self {
                home: std::env::var_os("HOME"),
                xdg_state: std::env::var_os("XDG_STATE_HOME"),
            };
            match home {
                Some(p) => std::env::set_var("HOME", p),
                None => std::env::remove_var("HOME"),
            }
            match xdg_state {
                Some(p) => std::env::set_var("XDG_STATE_HOME", p),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
            prev
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match &self.xdg_state {
                Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
    }

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
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

        let sh = LocalShell::sandboxed(conversation_profile(repo.path(), &[]).unwrap());
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

    #[test]
    fn conversation_profile_under_system_temp_dir_writes_only_in_wt_tasks_and_private_tmpdir() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if !landlock_or_skip() {
            return;
        }
        let repo = tempfile::tempdir().unwrap();
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

        let sb = conversation_sandbox(repo.path(), &[]).unwrap();
        let private_tmp = sb
            .tmpdir
            .clone()
            .expect("repo under system temp dir must get a private TMPDIR");
        let sh = LocalShell::sandboxed(sb.profile).with_tmpdir(sb.tmpdir);

        let ok_file = tasks.join("x");
        let ok = sh
            .exec(
                &format!("echo ok > '{}'", ok_file.display()),
                repo.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        assert!(ok_file.exists());

        let tmp_write = sh
            .exec(
                r#"echo tmp > "$TMPDIR/probe.txt" && printf '%s' "$TMPDIR""#,
                repo.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_eq!(tmp_write.exit_code, 0, "{}", tmp_write.merged());
        assert_eq!(
            std::path::PathBuf::from(tmp_write.stdout.trim())
                .canonicalize()
                .unwrap(),
            private_tmp.canonicalize().unwrap()
        );
        assert!(private_tmp.join("probe.txt").exists());

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

    #[test]
    fn landlock_profiles_cannot_read_surface_token_or_blyte_env() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if !landlock_or_skip() {
            eprintln!("skipped: sandbox cannot nest here");
            return;
        }
        let real_home = std::env::var("HOME").unwrap();
        let fake_home = tempfile::tempdir_in(&real_home).unwrap();
        let repo = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(Some(fake_home.path()), None);

        let benign = repo.path().join("benign.txt");
        std::fs::write(&benign, "benign-ok\n").unwrap();

        let token_path = surface_token_path(repo.path()).unwrap();
        std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
        std::fs::write(&token_path, "surface-token-linux-secret\n").unwrap();

        let env_path = fake_home.path().join(".config/blyte/env");
        std::fs::create_dir_all(env_path.parent().unwrap()).unwrap();
        std::fs::write(&env_path, "ANTHROPIC_API_KEY=blyte-env-linux-secret\n").unwrap();

        let conv_network = landlock_abi() < 4;
        for profile in [
            conversation_profile_net(repo.path(), &[], conv_network).unwrap(),
            default_profile_net(repo.path(), &[], true),
        ] {
            let sh = LocalShell::sandboxed(profile);
            let benign_res = sh
                .exec(
                    &format!("cat '{}'", benign.display()),
                    repo.path(),
                    Duration::from_secs(30),
                )
                .unwrap();
            assert_eq!(benign_res.exit_code, 0, "{}", benign_res.merged());
            assert!(
                benign_res.merged().contains("benign-ok"),
                "{}",
                benign_res.merged()
            );

            let token_res = sh
                .exec(
                    &format!("cat '{}'", token_path.display()),
                    repo.path(),
                    Duration::from_secs(30),
                )
                .unwrap();
            assert_ne!(token_res.exit_code, 0, "{}", token_res.merged());
            assert!(
                !token_res.merged().contains("surface-token-linux-secret"),
                "{}",
                token_res.merged()
            );

            let env_res = sh
                .exec(
                    &format!("cat '{}'", env_path.display()),
                    repo.path(),
                    Duration::from_secs(30),
                )
                .unwrap();
            assert_ne!(env_res.exit_code, 0, "{}", env_res.merged());
            assert!(
                !env_res.merged().contains("blyte-env-linux-secret"),
                "{}",
                env_res.merged()
            );
        }
    }

    #[test]
    fn landlock_default_and_conversation_sandboxes_cannot_read_cargo_credentials() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if !landlock_or_skip() {
            return;
        }
        let real_home = std::env::var("HOME").unwrap();
        let fake_home = tempfile::tempdir_in(&real_home).unwrap();
        let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
        std::fs::create_dir_all(tmp_base).unwrap();
        let repo = tempfile::tempdir_in(tmp_base).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
        let _guard = EnvGuard::set(Some(fake_home.path()), None);
        let _blyte_state = EnvVarGuard::remove("BLYTE_STATE_DIR");

        let cargo_dir = fake_home.path().join(".cargo");
        let registry_file = cargo_dir.join("registry/x");
        std::fs::create_dir_all(registry_file.parent().unwrap()).unwrap();
        std::fs::write(&registry_file, "cargo-registry-ok\n").unwrap();

        let cred_toml = cargo_dir.join("credentials.toml");
        std::fs::write(&cred_toml, "token = \"cargo-toml-secret\"\n").unwrap();

        let cred_plain = cargo_dir.join("credentials");
        std::fs::write(&cred_plain, "token = \"cargo-plain-secret\"\n").unwrap();

        let conv_network = landlock_abi() < 4;
        for sb in [
            default_sandbox_net(repo.path(), &[], true).unwrap(),
            conversation_sandbox_net(repo.path(), &[], conv_network).unwrap(),
        ] {
            let sh = LocalShell::sandboxed(sb.profile).with_tmpdir(sb.tmpdir);

            let ok = sh
                .exec(
                    &format!("cat '{}'", registry_file.display()),
                    repo.path(),
                    Duration::from_secs(30),
                )
                .unwrap();
            assert_eq!(ok.exit_code, 0, "{}", ok.merged());
            assert!(ok.merged().contains("cargo-registry-ok"), "{}", ok.merged());

            for (path, secret) in [
                (&cred_toml, "cargo-toml-secret"),
                (&cred_plain, "cargo-plain-secret"),
            ] {
                let res = sh
                    .exec(
                        &format!("cat '{}'", path.display()),
                        repo.path(),
                        Duration::from_secs(30),
                    )
                    .unwrap();
                assert_ne!(res.exit_code, 0, "{}", res.merged());
                assert!(!res.merged().contains(secret), "{}", res.merged());
            }
        }
    }

    #[test]
    fn landlock_default_sandbox_with_home_under_tmp_gets_private_tmp() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if !landlock_or_skip() {
            return;
        }
        let fake_home = tempfile::tempdir_in("/tmp").unwrap();
        let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
        std::fs::create_dir_all(tmp_base).unwrap();
        let repo = tempfile::tempdir_in(tmp_base).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success());
        let _guard = EnvGuard::set(Some(fake_home.path()), None);
        let _tmpdir = EnvVarGuard::remove("TMPDIR");
        let _blyte_state = EnvVarGuard::remove("BLYTE_STATE_DIR");

        let aws_cred = fake_home.path().join(".aws/credentials");
        std::fs::create_dir_all(aws_cred.parent().unwrap()).unwrap();
        std::fs::write(&aws_cred, "aws-tmp-secret\n").unwrap();

        let sb = default_sandbox_net(repo.path(), &[], true).unwrap();
        let private_tmp = sb
            .tmpdir
            .clone()
            .expect("HOME under /tmp must get a private TMPDIR");
        let _cleanup = RemoveDirGuard(private_tmp.clone());

        assert!(
            !private_tmp.starts_with("/tmp"),
            "private tmpdir {} must be outside /tmp",
            private_tmp.display()
        );
        let canon_tmp = Path::new("/tmp").canonicalize().unwrap();
        let canon_private = private_tmp.canonicalize().unwrap();
        assert!(
            !canon_private.starts_with(&canon_tmp),
            "private tmpdir {} must be outside {}",
            canon_private.display(),
            canon_tmp.display()
        );

        let sh = LocalShell::sandboxed(sb.profile).with_tmpdir(sb.tmpdir);
        let mk = sh
            .exec("mktemp -d", repo.path(), Duration::from_secs(30))
            .unwrap();
        assert_eq!(mk.exit_code, 0, "{}", mk.merged());

        let cred = sh
            .exec(
                r#"cat "$HOME/.aws/credentials""#,
                repo.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(cred.exit_code, 0, "{}", cred.merged());
        assert!(
            !cred.merged().contains("aws-tmp-secret"),
            "{}",
            cred.merged()
        );
    }

    #[test]
    fn default_and_conversation_sandboxes_refuse_root_containing_protected_path() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let real_home = std::env::var("HOME").unwrap();
        let fake_home = tempfile::tempdir_in(&real_home).unwrap();
        let _guard = EnvGuard::set(Some(fake_home.path()), None);
        let _blyte_state = EnvVarGuard::remove("BLYTE_STATE_DIR");

        let default_err = default_sandbox_net(fake_home.path(), &[], true).unwrap_err();
        assert!(default_err.contains("cannot protect"), "{default_err}");

        let conv_err = conversation_sandbox_net(fake_home.path(), &[], true).unwrap_err();
        assert!(conv_err.contains("cannot protect"), "{conv_err}");
    }
}
