//! On macOS blyte's shell refuses writes outside an allow-list: a model once spent 50 minutes
//! deleting a host cache after a gate failure, and a prompt is not enforcement.

use std::path::{Path, PathBuf};

use blyte_agent::sandbox::{conversation_allowed, default_allowed, profile};

#[test]
fn the_profile_denies_writes_then_allows_the_listed_paths() {
    let p = profile(&[PathBuf::from("/tmp/ws-\"q\"")]);
    assert!(
        p.starts_with("(version 1)(allow default)(deny file-write*)"),
        "{p}"
    );
    assert!(
        p.contains(r#"(allow file-write* (subpath "/tmp/ws-\"q\""))"#),
        "{p}"
    );
    assert!(p.contains(r#"(literal "/dev/null")"#), "{p}");
}

#[test]
fn the_default_allow_list_covers_the_workspace_and_its_git_directory() {
    let d = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(d.path())
        .status()
        .unwrap();
    let allowed = default_allowed(d.path());
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root = canon(d.path());
    assert!(allowed.iter().any(|p| canon(p) == root), "{allowed:?}");
    assert!(
        allowed.iter().any(|p| canon(p) == root.join(".git")),
        "{allowed:?}"
    );
}

#[test]
fn conversation_allowed_contains_wt_tasks_and_temp_not_root_git_or_wt() {
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

    let allowed = conversation_allowed(d.path());
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root = canon(d.path());
    let tasks_canon = canon(&tasks);
    let git_canon = canon(&d.path().join(".git"));
    let wt_canon = canon(&d.path().join(".wt"));

    assert!(
        allowed.iter().any(|p| canon(p) == tasks_canon),
        "{allowed:?}"
    );
    assert!(
        allowed
            .iter()
            .any(|p| p == Path::new("/tmp") || p == Path::new("/private/tmp")),
        "{allowed:?}"
    );
    assert!(!allowed.iter().any(|p| canon(p) == root), "{allowed:?}");
    assert!(
        !allowed.iter().any(|p| canon(p) == git_canon),
        "{allowed:?}"
    );
    assert!(!allowed.iter().any(|p| canon(p) == wt_canon), "{allowed:?}");
}

#[test]
fn conversation_allowed_in_linked_worktree_uses_main_wt_tasks() {
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let main_repo = tempfile::tempdir_in(tmp_base).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());
    assert!(std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());
    assert!(std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());
    std::fs::write(main_repo.path().join("README.md"), "init\n").unwrap();
    assert!(std::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());
    assert!(std::process::Command::new("git")
        .args(["commit", "--no-gpg-sign", "-q", "-m", "init"])
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());

    let linked = main_repo.path().join(".wt/linked");
    assert!(std::process::Command::new("git")
        .args(["worktree", "add", "-q", "-b", "linked"])
        .arg(&linked)
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());
    let main_tasks = main_repo.path().join(".wt/tasks");
    std::fs::create_dir_all(&main_tasks).unwrap();

    let allowed = conversation_allowed(&linked);
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let main_root = canon(main_repo.path());
    let linked_root = canon(&linked);
    let main_tasks_canon = canon(&main_tasks);
    let git_common = canon(&main_repo.path().join(".git"));
    let wt_canon = canon(&main_repo.path().join(".wt"));

    assert!(
        allowed.iter().any(|p| canon(p) == main_tasks_canon),
        "{allowed:?}"
    );
    assert!(
        !allowed.iter().any(|p| canon(p) == main_root),
        "{allowed:?}"
    );
    assert!(
        !allowed.iter().any(|p| canon(p) == linked_root),
        "{allowed:?}"
    );
    assert!(
        !allowed.iter().any(|p| canon(p) == git_common),
        "{allowed:?}"
    );
    assert!(!allowed.iter().any(|p| canon(p) == wt_canon), "{allowed:?}");
    assert!(
        !allowed
            .iter()
            .any(|p| canon(p) == linked_root.join(".wt/tasks")),
        "{allowed:?}"
    );
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use blyte_agent::sandbox::{conversation_profile, profile};
    use blyte_agent::shell::{LocalShell, Shell};

    #[test]
    fn a_sandboxed_command_can_write_inside_and_not_outside() {
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // A profile with only `inside` allowed (no temp dirs), so `outside` is refused.
        let sh = LocalShell::sandboxed(profile(&[inside.path().to_path_buf()]));
        let ok = sh
            .exec("echo hi > ok.txt", inside.path(), Duration::from_secs(30))
            .unwrap();
        if ok.exit_code == 71 && ok.merged().contains("Operation not permitted") {
            // Nested sandboxing is not permitted in this environment (e.g. running under an enclosing sandbox).
            return;
        }
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        let target: PathBuf = outside.path().join("no.txt");
        let denied = sh
            .exec(
                &format!("echo hi > {}", target.display()),
                inside.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(denied.exit_code, 0);
        assert!(
            !target.exists(),
            "the write outside the allow-list must not happen"
        );
    }

    #[test]
    fn conversation_profile_writes_only_in_wt_tasks_not_root_git_or_wt_passed() {
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
        if ok.exit_code == 71 && ok.merged().contains("Operation not permitted") {
            return;
        }
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
