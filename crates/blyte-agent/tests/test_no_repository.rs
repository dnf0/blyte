use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use blyte_agent::fs::LocalFs;
use blyte_agent::sandbox::{conversation_paths, is_git_repository};
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, set_blyte_tool, set_publish_tool, PublishTool};
use serde_json::json;

static TEST_LOCK: Mutex<()> = Mutex::new(());

struct EnvVarGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, val: &Path) -> Self {
        let prev = std::env::var_os(key);
        std::env::set_var(key, val);
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

struct CleanupDir(Option<PathBuf>);

impl Drop for CleanupDir {
    fn drop(&mut self) {
        if let Some(ref p) = self.0 {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

struct BlyteToolGuard {
    _lock: MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
}

impl BlyteToolGuard {
    fn acquire_script(body: &str) -> Self {
        let lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-blyte");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        set_blyte_tool(Some(script));
        Self {
            _lock: lock,
            _dir: dir,
        }
    }
}

impl Drop for BlyteToolGuard {
    fn drop(&mut self) {
        set_blyte_tool(None);
    }
}

struct PublishToolGuard {
    _lock: MutexGuard<'static, ()>,
}

impl PublishToolGuard {
    fn set(tool: Arc<dyn PublishTool>) -> Self {
        let lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_publish_tool(Some(tool));
        Self { _lock: lock }
    }
}

impl Drop for PublishToolGuard {
    fn drop(&mut self) {
        set_publish_tool(None);
    }
}

struct RecordingPublish {
    calls: Mutex<Vec<(String, String, String)>>,
}

impl PublishTool for RecordingPublish {
    fn publish(&self, request: &str, title: &str, body: &str) -> Result<String, String> {
        self.calls
            .lock()
            .unwrap()
            .push((request.to_string(), title.to_string(), body.to_string()));
        Ok("https://example.invalid/pull/1".to_string())
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

fn canon(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

#[test]
fn conversation_paths_creates_nothing_in_a_plain_folder() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let holder = tempfile::tempdir_in(tmp_base).unwrap();
    let fake_home = holder.path().join("home");
    let plain = fake_home.join("plain-folder");
    std::fs::create_dir_all(&plain).unwrap();

    let holder_canon = canon(holder.path());
    let _ceiling = EnvVarGuard::set("GIT_CEILING_DIRECTORIES", &holder_canon);
    let _home = EnvVarGuard::set("HOME", &fake_home);

    assert!(!is_git_repository(&plain));
    let paths = conversation_paths(&plain).unwrap();
    let _cleanup = CleanupDir(if paths.created_tmpdir {
        paths.tmpdir.clone()
    } else {
        None
    });

    assert!(!plain.join(".wt").exists());
    let plain_canon = canon(&plain);
    for entry in &paths.allowed {
        let entry_canon = canon(entry);
        assert!(
            !entry_canon.starts_with(&plain_canon),
            "allowed path {} must not be inside plain root {}",
            entry.display(),
            plain_canon.display()
        );
        assert!(
            !plain_canon.starts_with(&entry_canon),
            "plain root {} must not be inside allowed path {}",
            plain_canon.display(),
            entry.display()
        );
    }
}

#[test]
fn under_a_temp_folder_the_private_temp_is_outside_the_root() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let holder = tempfile::tempdir().unwrap();
    let plain = holder.path().join("plain-under-temp");
    std::fs::create_dir_all(&plain).unwrap();

    assert!(!is_git_repository(&plain));
    let paths = conversation_paths(&plain).unwrap();
    let _cleanup = CleanupDir(if paths.created_tmpdir {
        paths.tmpdir.clone()
    } else {
        None
    });

    let tmpdir = paths
        .tmpdir
        .as_ref()
        .expect("private tmpdir must be set when root is inside a temp folder");
    assert!(tmpdir.is_dir(), "tmpdir {} must exist", tmpdir.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(tmpdir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    let plain_canon = canon(&plain);
    let tmpdir_canon = canon(tmpdir);
    assert!(
        !tmpdir_canon.starts_with(&plain_canon),
        "private tmpdir {} must not be inside root {}",
        tmpdir_canon.display(),
        plain_canon.display()
    );
    assert!(
        !plain_canon.starts_with(&tmpdir_canon),
        "root {} must not be inside private tmpdir {}",
        plain_canon.display(),
        tmpdir_canon.display()
    );
    for entry in &paths.allowed {
        let entry_canon = canon(entry);
        assert!(
            !plain_canon.starts_with(&entry_canon),
            "root {} must not be inside allowed path {}",
            plain_canon.display(),
            entry.display()
        );
    }
    assert!(!plain.join(".wt").exists());
}

#[test]
fn in_a_repository_nothing_changes() {
    let _lock = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let repo = tempfile::tempdir().unwrap();
    init_git(repo.path());

    assert!(is_git_repository(repo.path()));
    let paths = conversation_paths(repo.path()).unwrap();
    let _cleanup = CleanupDir(if paths.created_tmpdir {
        paths.tmpdir.clone()
    } else {
        None
    });

    let expected_tasks = canon(repo.path()).join(".wt").join("tasks");
    assert!(
        paths.allowed.iter().any(|p| canon(p) == expected_tasks),
        "expected {} in {:?}",
        expected_tasks.display(),
        paths.allowed
    );
}

#[test]
fn blyte_tool_refuses_dispatch_without_a_repository_and_creates_nothing() {
    let _g = BlyteToolGuard::acquire_script("echo should-not-run");
    let root = tempfile::tempdir().unwrap();

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1", "x.md"], "background": true}),
    );
    assert!(out.is_error, "expected error, got: {}", out.content);
    assert!(out.content.contains("run store"), "{}", out.content);
    assert!(!root.path().join(".wt").exists());

    for subcmd in ["plan", "init", "clean", "edit-start", "edit-finish", "undo"] {
        let res = execute(
            root.path(),
            &LocalShell,
            &LocalFs,
            "blyte",
            &json!({"args": [subcmd]}),
        );
        assert!(res.is_error, "{subcmd} should fail without a repository");
        assert!(
            res.content.contains("not a git repository"),
            "{subcmd}: {}",
            res.content
        );
        assert!(
            res.content.contains("git init"),
            "{subcmd}: {}",
            res.content
        );
    }
    assert!(!root.path().join(".wt").exists());
}

#[test]
fn blyte_tool_works_again_after_git_init() {
    let _g = BlyteToolGuard::acquire_script("printf 'reached-binary:%s\\n' \"$*\"");
    let root = tempfile::tempdir().unwrap();

    let before = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["plan", "plan.md"]}),
    );
    assert!(before.is_error);
    assert!(before.content.contains("not a git repository"));

    init_git(root.path());

    let after = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["plan", "plan.md"]}),
    );
    assert!(!after.is_error, "{}", after.content);
    assert!(
        after.content.contains("reached-binary:plan plan.md"),
        "{}",
        after.content
    );
}

#[test]
fn publish_refuses_without_a_repository() {
    let recorder = Arc::new(RecordingPublish {
        calls: Mutex::new(Vec::new()),
    });
    let _g = PublishToolGuard::set(recorder.clone());
    let root = tempfile::tempdir().unwrap();

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "publish",
        &json!({
            "request": "req-1",
            "title": "feat: publish something",
            "body": "Summary of changes"
        }),
    );
    assert!(out.is_error, "expected error, got: {}", out.content);
    assert!(
        out.content.contains("not a git repository"),
        "{}",
        out.content
    );
    assert!(out.content.contains("git init"), "{}", out.content);
    assert!(
        recorder.calls.lock().unwrap().is_empty(),
        "publish seam must not be called without a repository"
    );
}
