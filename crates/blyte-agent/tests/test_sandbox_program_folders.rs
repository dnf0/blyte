use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    home: Option<OsString>,
    xdg_state: Option<OsString>,
    blyte_state: Option<OsString>,
}

impl EnvGuard {
    fn set_home(home: Option<&Path>) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = Self {
            _lock: lock,
            home: std::env::var_os("HOME"),
            xdg_state: std::env::var_os("XDG_STATE_HOME"),
            blyte_state: std::env::var_os("BLYTE_STATE_DIR"),
        };
        match home {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        std::env::remove_var("XDG_STATE_HOME");
        std::env::remove_var("BLYTE_STATE_DIR");
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
        match &self.blyte_state {
            Some(v) => std::env::set_var("BLYTE_STATE_DIR", v),
            None => std::env::remove_var("BLYTE_STATE_DIR"),
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::Path;
    use std::process::Command;
    use std::time::Duration;

    use blyte_agent::sandbox::{self, Profile};
    use blyte_agent::shell::{LocalShell, Shell};

    use super::{EnvGuard, ENV_LOCK};

    fn is_nested_sandbox_error(exit_code: i32, merged: &str) -> bool {
        exit_code == 71 && merged.contains("Operation not permitted")
    }

    fn target_tmp_base() -> &'static Path {
        let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
        std::fs::create_dir_all(base).unwrap();
        base
    }

    fn init_git_repo(dir: &Path) {
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
    }

    #[test]
    fn live_protected_paths_denied_and_caches_writable() {
        if !sandbox::available() {
            return;
        }

        let tmp_base = target_tmp_base();
        let home = tempfile::tempdir_in(tmp_base).unwrap();
        let repo = tempfile::tempdir_in(tmp_base).unwrap();

        let denied_files = [
            ".cargo/bin/x",
            ".cargo/config.toml",
            ".local/bin/x",
            ".local/lib/x",
            ".rustup/x",
            "go/bin/x",
            ".local/share/uv/tools/x",
            ".gradle/init.d/x",
            ".m2/settings.xml",
            ".local/state/blyte/x",
        ];
        let allowed_files = [
            ".cargo/registry/x",
            ".cargo/.package-cache",
            ".cargo/git/x",
            "go/pkg/x",
            ".cache/x",
            ".local/share/uv/python/x",
            ".gradle/caches/x",
            ".m2/repository/x",
        ];

        for rel in denied_files.iter().chain(allowed_files.iter()) {
            let full = home.path().join(rel);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        }

        let _env = EnvGuard::set_home(Some(home.path()));

        for (label, profile) in [
            (
                "conversation_profile",
                sandbox::conversation_profile(repo.path(), &[]).unwrap(),
            ),
            (
                "default_profile",
                sandbox::default_profile(repo.path(), &[]),
            ),
        ] {
            let sh = LocalShell::sandboxed(profile);

            for rel in &allowed_files {
                let target = home.path().join(rel);
                let _ = std::fs::remove_file(&target);
                let out = sh
                    .exec_user(
                        &format!("echo ok > '{}'", target.display()),
                        repo.path(),
                        Duration::from_secs(30),
                    )
                    .unwrap();
                if is_nested_sandbox_error(out.exit_code, &out.merged()) {
                    return;
                }
                assert_eq!(
                    out.exit_code,
                    0,
                    "{label}: write to {rel} should succeed: {}",
                    out.merged()
                );
                assert!(target.exists(), "{label}: {rel} should exist");
            }

            for rel in &denied_files {
                let target = home.path().join(rel);
                let _ = std::fs::remove_file(&target);
                let out = sh
                    .exec_user(
                        &format!("echo no > '{}'", target.display()),
                        repo.path(),
                        Duration::from_secs(30),
                    )
                    .unwrap();
                assert_ne!(
                    out.exit_code,
                    0,
                    "{label}: write to {rel} should fail: {}",
                    out.merged()
                );
                assert!(!target.exists(), "{label}: {rel} must not be created");
            }
        }
    }

    #[test]
    fn live_default_profile_denies_git_hooks_and_config_while_allowing_commit() {
        if !sandbox::available() {
            return;
        }

        let tmp_base = target_tmp_base();
        let home = tempfile::tempdir_in(tmp_base).unwrap();
        let repo = tempfile::tempdir_in(tmp_base).unwrap();
        let _env = EnvGuard::set_home(Some(home.path()));

        init_git_repo(repo.path());
        let hooks_dir = repo.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let hook = hooks_dir.join("pre-commit");
        let git_config = repo.path().join(".git/config");
        let config_before = std::fs::read_to_string(&git_config).unwrap();

        let sh = LocalShell::sandboxed(sandbox::default_profile(repo.path(), &[]));

        let commit = sh
            .exec_user(
                "git commit --allow-empty -m x",
                repo.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        if is_nested_sandbox_error(commit.exit_code, &commit.merged()) {
            return;
        }
        assert_eq!(
            commit.exit_code,
            0,
            "git commit should succeed: {}",
            commit.merged()
        );

        let write_hook = sh
            .exec_user(
                &format!("echo '#!/bin/sh' > '{}'", hook.display()),
                repo.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(
            write_hook.exit_code,
            0,
            "write to .git/hooks/pre-commit should fail: {}",
            write_hook.merged()
        );
        assert!(!hook.exists(), "hook file must not be created");

        let append_config = sh
            .exec_user(
                &format!("echo '# tampered' >> '{}'", git_config.display()),
                repo.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(
            append_config.exit_code,
            0,
            "append to .git/config should fail: {}",
            append_config.merged()
        );
        assert_eq!(
            std::fs::read_to_string(&git_config).unwrap(),
            config_before,
            ".git/config must remain unchanged"
        );
    }

    #[test]
    fn live_default_profile_allows_workspace_inside_protected_folder_but_not_sibling() {
        if !sandbox::available() {
            return;
        }

        let tmp_base = target_tmp_base();
        let home = tempfile::tempdir_in(tmp_base).unwrap();
        let proj = home.path().join(".local/lib/proj");
        std::fs::create_dir_all(&proj).unwrap();
        let _env = EnvGuard::set_home(Some(home.path()));

        let sh = LocalShell::sandboxed(sandbox::default_profile(&proj, &[]));

        let inside = proj.join("inside.txt");
        let allow_out = sh
            .exec_user(
                &format!("echo ok > '{}'", inside.display()),
                &proj,
                Duration::from_secs(30),
            )
            .unwrap();
        if is_nested_sandbox_error(allow_out.exit_code, &allow_out.merged()) {
            return;
        }
        assert_eq!(
            allow_out.exit_code,
            0,
            "write inside workspace root should succeed: {}",
            allow_out.merged()
        );
        assert!(inside.exists());

        let sibling = home.path().join(".local/lib/other");
        let deny_out = sh
            .exec_user(
                &format!("echo no > '{}'", sibling.display()),
                &proj,
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(
            deny_out.exit_code,
            0,
            "write to sibling in .local/lib should fail: {}",
            deny_out.merged()
        );
        assert!(!sibling.exists());
    }

    #[test]
    fn live_cargo_build_offline_succeeds_with_real_home() {
        if !sandbox::available() {
            return;
        }

        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let proj = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(proj.path().join("src")).unwrap();
        std::fs::write(
            proj.path().join("Cargo.toml"),
            "[package]\nname = \"sandbox_smoke\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
        )
        .unwrap();
        std::fs::write(proj.path().join("src/main.rs"), "fn main() {}\n").unwrap();

        let sh = LocalShell::sandboxed(sandbox::default_profile(proj.path(), &[]));

        let probe = sh
            .exec_user("echo ok > probe.txt", proj.path(), Duration::from_secs(30))
            .unwrap();
        if is_nested_sandbox_error(probe.exit_code, &probe.merged()) {
            return;
        }
        assert_eq!(probe.exit_code, 0, "probe failed: {}", probe.merged());

        let build = sh
            .exec_user(
                "cargo build --offline",
                proj.path(),
                Duration::from_secs(120),
            )
            .unwrap();
        assert_eq!(
            build.exit_code,
            0,
            "cargo build --offline should succeed with ~/.rustup read-only: {}",
            build.merged()
        );
    }

    #[test]
    fn profile_text_places_protected_denies_after_parent_allows_and_git_denies_only_in_default() {
        let tmp_base = target_tmp_base();
        let home = tempfile::tempdir_in(tmp_base).unwrap();
        let repo = tempfile::tempdir_in(tmp_base).unwrap();
        for dir in [".cargo", ".local", ".gradle", ".m2", "go/pkg"] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }
        let _env = EnvGuard::set_home(Some(home.path()));
        init_git_repo(repo.path());

        let home_canon = home.path().canonicalize().unwrap();
        let git_canon = repo.path().join(".git").canonicalize().unwrap();

        let pairs = [
            (".cargo", ".cargo/bin"),
            (".cargo", ".cargo/config"),
            (".cargo", ".cargo/config.toml"),
            (".cargo", ".cargo/credentials"),
            (".cargo", ".cargo/credentials.toml"),
            (".local", ".local/bin"),
            (".local", ".local/lib"),
            (".local", ".local/state/blyte"),
            (".local", ".local/share/uv/tools"),
            (".local", ".local/share/mise"),
            (".local", ".local/share/pipx"),
            (".local", ".local/share/keyrings"),
            (".gradle", ".gradle/init.d"),
            (".m2", ".m2/settings.xml"),
        ];

        let Profile::Seatbelt(default_sbpl) = sandbox::default_profile(repo.path(), &[]) else {
            panic!("expected Seatbelt profile on macOS");
        };
        let Profile::Seatbelt(conv_sbpl) = sandbox::conversation_profile(repo.path(), &[]).unwrap()
        else {
            panic!("expected Seatbelt profile on macOS");
        };

        for (label, sbpl) in [
            ("default_profile", &default_sbpl),
            ("conversation_profile", &conv_sbpl),
        ] {
            let go_pkg_allow = format!(
                r#"(allow file-write* (subpath "{}"))"#,
                home_canon.join("go/pkg").display()
            );
            let go_whole_allow = format!(
                r#"(allow file-write* (subpath "{}"))"#,
                home_canon.join("go").display()
            );
            let rustup_allow = format!(
                r#"(allow file-write* (subpath "{}"))"#,
                home_canon.join(".rustup").display()
            );
            assert!(
                sbpl.contains(&go_pkg_allow),
                "{label}: missing {go_pkg_allow}"
            );
            assert!(
                !sbpl.contains(&go_whole_allow),
                "{label}: must not contain {go_whole_allow}"
            );
            assert!(
                !sbpl.contains(&rustup_allow),
                "{label}: must not contain {rustup_allow}"
            );

            for (parent, protected) in pairs {
                let allow_rule = format!(
                    r#"(allow file-write* (subpath "{}"))"#,
                    home_canon.join(parent).display()
                );
                let deny_rule = format!(
                    r#"(deny file-write* (subpath "{}"))"#,
                    home_canon.join(protected).display()
                );
                let allow_idx = sbpl
                    .find(&allow_rule)
                    .unwrap_or_else(|| panic!("{label}: missing allow rule {allow_rule}"));
                let deny_idx = sbpl
                    .rfind(&deny_rule)
                    .unwrap_or_else(|| panic!("{label}: missing deny rule {deny_rule}"));
                assert!(
                    deny_idx > allow_idx,
                    "{label}: deny rule for {protected} must follow allow rule for {parent}"
                );
            }
        }

        let git_allow = format!(r#"(allow file-write* (subpath "{}"))"#, git_canon.display());
        let git_allow_idx = default_sbpl
            .find(&git_allow)
            .expect("default_profile must allow git common dir");
        for sub in ["hooks", "config"] {
            let git_deny = format!(
                r#"(deny file-write* (subpath "{}"))"#,
                git_canon.join(sub).display()
            );
            let deny_idx = default_sbpl
                .rfind(&git_deny)
                .unwrap_or_else(|| panic!("default_profile missing {git_deny}"));
            assert!(
                deny_idx > git_allow_idx,
                "default_profile: git {sub} deny must follow git allow"
            );
            assert!(
                !conv_sbpl.contains(&git_deny),
                "conversation_profile must not contain {git_deny}"
            );
        }
    }

    #[test]
    fn profile_text_reallows_workspace_inside_or_equal_to_protected_path() {
        let tmp_base = target_tmp_base();
        let home = tempfile::tempdir_in(tmp_base).unwrap();
        let lib_dir = home.path().join(".local/lib");
        let proj = lib_dir.join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let _env = EnvGuard::set_home(Some(home.path()));
        init_git_repo(&proj);

        let lib_canon = lib_dir.canonicalize().unwrap();
        let proj_canon = proj.canonicalize().unwrap();
        let lib_deny = format!(r#"(deny file-write* (subpath "{}"))"#, lib_canon.display());

        let Profile::Seatbelt(proj_sbpl) = sandbox::default_profile(&proj, &[]) else {
            panic!("expected Seatbelt profile on macOS");
        };
        let proj_allow = format!(
            r#"(allow file-write* (subpath "{}"))"#,
            proj_canon.display()
        );
        let deny_idx = proj_sbpl.rfind(&lib_deny).expect("lib deny present");
        let allow_idx = proj_sbpl.rfind(&proj_allow).expect("proj allow present");
        assert!(
            allow_idx > deny_idx,
            "workspace inside protected folder must be re-allowed after parent deny"
        );
        for sub in ["hooks", "config"] {
            let git_deny = format!(
                r#"(deny file-write* (subpath "{}"))"#,
                proj_canon.join(".git").join(sub).display()
            );
            let git_deny_idx = proj_sbpl
                .rfind(&git_deny)
                .expect("git deny present for repo inside protected folder");
            assert!(
                git_deny_idx > allow_idx,
                "git {sub} deny must follow workspace re-allow"
            );
        }

        let Profile::Seatbelt(eq_sbpl) = sandbox::default_profile(&lib_dir, &[]) else {
            panic!("expected Seatbelt profile on macOS");
        };
        let lib_allow = format!(r#"(allow file-write* (subpath "{}"))"#, lib_canon.display());
        let eq_deny_idx = eq_sbpl.rfind(&lib_deny).expect("lib deny present");
        let eq_allow_idx = eq_sbpl.rfind(&lib_allow).expect("lib allow present");
        assert!(
            eq_allow_idx > eq_deny_idx,
            "workspace equal to protected folder must be re-allowed after deny"
        );
    }

    #[test]
    fn profile_text_omits_home_denies_when_home_unset_or_empty() {
        let tmp_base = target_tmp_base();
        let repo = tempfile::tempdir_in(tmp_base).unwrap();

        for home_val in [None, Some(Path::new(""))] {
            let _env = EnvGuard::set_home(home_val);
            let Profile::Seatbelt(default_sbpl) = sandbox::default_profile(repo.path(), &[]) else {
                panic!("expected Seatbelt profile on macOS");
            };
            let Profile::Seatbelt(conv_sbpl) =
                sandbox::conversation_profile(repo.path(), &[]).unwrap()
            else {
                panic!("expected Seatbelt profile on macOS");
            };
            for sbpl in [&default_sbpl, &conv_sbpl] {
                assert!(!sbpl.contains(".cargo/bin"), "{sbpl}");
                assert!(!sbpl.contains(".local/bin"), "{sbpl}");
                assert!(!sbpl.contains(".gradle/init.d"), "{sbpl}");
                assert!(!sbpl.contains(".m2/settings.xml"), "{sbpl}");
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use blyte_agent::sandbox::{conversation_allowed, default_allowed};

    use super::EnvGuard;

    #[test]
    fn linux_write_allowlist_excludes_program_folders_and_keeps_caches() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();

        for dir in [
            ".local/bin",
            ".local/lib",
            ".local/share",
            ".rustup",
            "go/bin",
            "go/pkg",
        ] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }

        let _env = EnvGuard::set_home(Some(home.path()));

        for (label, allowed) in [
            ("default_allowed", default_allowed(ws.path())),
            (
                "conversation_allowed",
                conversation_allowed(ws.path()).unwrap(),
            ),
        ] {
            assert!(
                allowed.contains(&home.path().join(".local/share")),
                "{label}: expected .local/share in {allowed:?}"
            );
            assert!(
                allowed.contains(&home.path().join("go/pkg")),
                "{label}: expected go/pkg in {allowed:?}"
            );
            for excluded in [".local/bin", ".local/lib", ".rustup", "go", "go/bin"] {
                assert!(
                    !allowed.contains(&home.path().join(excluded)),
                    "{label}: {excluded} must not be in {allowed:?}"
                );
            }
        }
    }
}
