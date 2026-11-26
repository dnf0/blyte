use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use blyte_agent::sandbox::{conversation_allowed, conversation_paths};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct TmpdirGuard {
    prev: Option<OsString>,
}

impl TmpdirGuard {
    fn set(path: &Path) -> Self {
        let prev = std::env::var_os("TMPDIR");
        std::env::set_var("TMPDIR", path);
        Self { prev }
    }
}

impl Drop for TmpdirGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("TMPDIR", v),
            None => std::env::remove_var("TMPDIR"),
        }
    }
}

fn init_git(dir: &Path) {
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .unwrap()
        .success());
}

#[test]
fn conversation_paths_filters_ancestors_and_creates_private_tmpdir() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let parent = tempfile::tempdir_in(tmp_base).unwrap();
    let repo = parent.path().join("repo");
    std::fs::create_dir_all(repo.join(".wt/tasks")).unwrap();
    init_git(&repo);
    let elsewhere = tempfile::tempdir_in(tmp_base).unwrap();
    std::fs::create_dir_all(elsewhere.path().join(".wt/tasks")).unwrap();
    init_git(elsewhere.path());

    let _tmpdir = TmpdirGuard::set(parent.path());

    let paths = conversation_paths(&repo).unwrap();
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let repo_canon = canon(&repo);
    let tasks_canon = canon(&repo.join(".wt/tasks"));
    let expected_tmp = repo_canon
        .join(".wt/tmp")
        .join(std::process::id().to_string());

    assert_eq!(paths.tmpdir.as_ref(), Some(&expected_tmp));
    assert!(paths.created_tmpdir);
    let paths_again = conversation_paths(&repo).unwrap();
    assert_eq!(paths_again.tmpdir.as_ref(), Some(&expected_tmp));
    assert!(!paths_again.created_tmpdir);
    assert!(expected_tmp.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&expected_tmp)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }
    for entry in &paths.allowed {
        let entry_canon = canon(entry);
        assert!(
            !repo_canon.starts_with(&entry_canon),
            "ancestor or equal entry {} must not remain in {:?}",
            entry.display(),
            paths.allowed
        );
    }
    assert!(
        paths.allowed.iter().any(|p| canon(p) == tasks_canon),
        "{:?}",
        paths.allowed
    );
    assert!(
        paths.allowed.iter().any(|p| canon(p) == expected_tmp),
        "{:?}",
        paths.allowed
    );

    let other_paths = conversation_paths(elsewhere.path()).unwrap();
    let other_canon_early = canon(elsewhere.path());
    if other_canon_early.starts_with("/private/tmp") || other_canon_early.starts_with("/tmp") {
        // The checkout is under the system temp directory, so this repository is too: it gets
        // a private temp directory like the first one.
        assert_eq!(
            other_paths.tmpdir,
            Some(
                other_canon_early
                    .join(".wt/tmp")
                    .join(std::process::id().to_string())
            )
        );
        assert!(!other_paths.allowed.contains(&PathBuf::from("/private/tmp")));
    } else {
        assert_eq!(other_paths.tmpdir, None);
        assert!(!other_paths.created_tmpdir);
        let other_canon = canon(elsewhere.path());
        assert_eq!(other_paths.allowed[0], other_canon.join(".wt/tasks"));
        assert!(other_paths.allowed.contains(&PathBuf::from("/private/tmp")));
        assert!(other_paths
            .allowed
            .contains(&PathBuf::from("/private/var/folders")));
        assert!(other_paths.allowed.contains(&PathBuf::from("/tmp")));
        assert!(other_paths.allowed.contains(&PathBuf::from("/var/tmp")));
        assert!(other_paths.allowed.contains(&PathBuf::from("/dev")));
        assert!(other_paths.allowed.contains(&parent.path().to_path_buf()));
        assert!(!other_canon.join(".wt/tmp").exists());
    }
    assert_eq!(
        other_paths.allowed,
        conversation_allowed(elsewhere.path()).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn conversation_paths_removes_symlink_alias_of_real_parent() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let holder = tempfile::tempdir_in(tmp_base).unwrap();
    let real_parent = holder.path().join("real");
    let alias_parent = holder.path().join("alias");
    let repo = real_parent.join("repo");
    std::fs::create_dir_all(repo.join(".wt/tasks")).unwrap();
    init_git(&repo);
    std::os::unix::fs::symlink(&real_parent, &alias_parent).unwrap();

    let _tmpdir = TmpdirGuard::set(&alias_parent);
    let paths = conversation_paths(&repo).unwrap();
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let repo_canon = canon(&repo);
    assert!(paths.tmpdir.is_some());
    assert!(!paths.allowed.contains(&alias_parent));
    assert!(!paths.allowed.contains(&real_parent));
    for entry in &paths.allowed {
        assert!(
            !repo_canon.starts_with(canon(entry)),
            "entry {} must not cover {}",
            entry.display(),
            repo_canon.display()
        );
    }

    let under_tmp = tempfile::tempdir_in("/tmp").unwrap();
    init_git(under_tmp.path());
    let tmp_paths = conversation_paths(under_tmp.path()).unwrap();
    let under_tmp_canon = canon(under_tmp.path());
    assert!(tmp_paths.tmpdir.is_some());
    assert!(!tmp_paths.allowed.contains(&PathBuf::from("/tmp")));
    // `/private/tmp` exists only on macOS, where it is what `/tmp` points to; elsewhere it
    // cannot contain the folder, so nothing removes it and the loop below is the check.
    #[cfg(target_os = "macos")]
    assert!(!tmp_paths.allowed.contains(&PathBuf::from("/private/tmp")));
    for entry in &tmp_paths.allowed {
        assert!(
            !under_tmp_canon.starts_with(canon(entry)),
            "entry {} must not cover {}",
            entry.display(),
            under_tmp_canon.display()
        );
    }
}

#[cfg(unix)]
#[test]
fn conversation_paths_errors_on_canonicalization_and_private_tmpdir_failures() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let holder = tempfile::tempdir_in(tmp_base).unwrap();
    let repo = holder.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    init_git(&repo);

    let loop_a = holder.path().join("loop_a");
    let loop_b = holder.path().join("loop_b");
    std::os::unix::fs::symlink(&loop_b, &loop_a).unwrap();
    std::os::unix::fs::symlink(&loop_a, &loop_b).unwrap();
    {
        let _tmpdir = TmpdirGuard::set(&loop_a);
        let err = conversation_paths(&repo).unwrap_err();
        assert!(
            err.contains(&loop_a.display().to_string()),
            "expected failing path in error, got: {err}"
        );
    }

    let blocked_repo = holder.path().join("blocked");
    std::fs::create_dir_all(&blocked_repo).unwrap();
    init_git(&blocked_repo);
    std::fs::write(blocked_repo.join(".wt"), "not a directory").unwrap();
    {
        let _tmpdir = TmpdirGuard::set(holder.path());
        let err = conversation_paths(&blocked_repo).unwrap_err();
        assert!(
            err.contains(".wt/tmp"),
            "expected private temp dir path in error, got: {err}"
        );
    }
}
