use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, set_blyte_tool, specs, BLYTE_TOOL_COMMANDS};
use serde_json::json;

static BLYTE_TOOL_LOCK: Mutex<()> = Mutex::new(());

struct BlyteToolGuard {
    _lock: MutexGuard<'static, ()>,
}

impl BlyteToolGuard {
    fn acquire(exe: Option<PathBuf>) -> Self {
        let lock = BLYTE_TOOL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_blyte_tool(exe);
        Self { _lock: lock }
    }
}

impl Drop for BlyteToolGuard {
    fn drop(&mut self) {
        set_blyte_tool(None);
    }
}

fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

#[test]
fn specs_offers_blyte_tool_only_when_set() {
    {
        let _g = BlyteToolGuard::acquire(None);
        let all = specs();
        assert!(!all.iter().any(|t| t.name == "blyte"));
    }

    {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(dir.path(), "fake-blyte", "#!/bin/sh\nexit 0\n");
        let _g = BlyteToolGuard::acquire(Some(script));
        let all = specs();
        let spec = all.iter().find(|t| t.name == "blyte").unwrap();
        let props = spec.parameters["properties"].as_object().unwrap();
        assert_eq!(props["args"]["type"], "array");
        assert_eq!(props["args"]["items"]["type"], "string");
        assert_eq!(props["background"]["type"], "boolean");
        assert_eq!(spec.parameters["required"], json!(["args"]));
        assert_eq!(
            BLYTE_TOOL_COMMANDS,
            ["dispatch", "wait", "status", "review", "plan", "init", "clean"]
        );
    }
}

#[test]
fn runs_allowed_command_with_args_cwd_and_parent_env() {
    let root = tempfile::tempdir().unwrap();
    let scripts = tempfile::tempdir().unwrap();
    let script = write_script(
        scripts.path(),
        "fake-blyte",
        "#!/bin/sh\nprintf 'args=%s\\n' \"$*\"\nprintf 'cwd=%s\\n' \"$(pwd)\"\nprintf 'parent=%s\\n' \"$BLYTE_DISPATCH_PARENT\"\n",
    );
    let _g = BlyteToolGuard::acquire(Some(script));

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["status", "T1"]}),
    );
    assert!(!out.is_error, "{}", out.content);
    let expected_cwd = std::fs::canonicalize(root.path()).unwrap();
    let mut seen_args = None;
    let mut seen_cwd = None;
    let mut seen_parent = None;
    for line in out.content.lines() {
        if let Some(v) = line.strip_prefix("args=") {
            seen_args = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("cwd=") {
            seen_cwd = Some(PathBuf::from(v));
        } else if let Some(v) = line.strip_prefix("parent=") {
            seen_parent = Some(v.to_string());
        }
    }
    assert_eq!(seen_args.as_deref(), Some("status T1"));
    assert_eq!(
        seen_cwd.map(|p| std::fs::canonicalize(p).unwrap()),
        Some(expected_cwd)
    );
    assert!(
        seen_parent.as_deref().is_some_and(|p| !p.is_empty()),
        "expected non-empty BLYTE_DISPATCH_PARENT, got {:?}",
        out.content
    );
}

#[test]
fn rejects_empty_or_disallowed_commands_without_running_executable() {
    let root = tempfile::tempdir().unwrap();
    let scripts = tempfile::tempdir().unwrap();
    let marker = scripts.path().join("ran");
    let script = write_script(
        scripts.path(),
        "fake-blyte",
        &format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    );
    let _g = BlyteToolGuard::acquire(Some(script));

    let expected_err =
        "the blyte tool runs only: dispatch, wait, status, review, plan, init, clean";
    for bad in [
        json!([]),
        json!(["comments"]),
        json!(["--orchestrate", "x"]),
    ] {
        let out = execute(
            root.path(),
            &LocalShell,
            &LocalFs,
            "blyte",
            &json!({"args": bad}),
        );
        assert!(out.is_error, "expected error for {bad}");
        assert_eq!(out.content, expected_err);
        assert!(!marker.exists(), "executable ran for {bad}");
    }
}

#[test]
fn background_rules_and_async_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let scripts = tempfile::tempdir().unwrap();
    let marker = scripts.path().join("done");
    let script = write_script(
        scripts.path(),
        "fake-blyte",
        &format!("#!/bin/sh\nsleep 2\ntouch '{}'\n", marker.display()),
    );
    let _g = BlyteToolGuard::acquire(Some(script));

    let non_dispatch = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["wait", "T1"], "background": true}),
    );
    assert!(non_dispatch.is_error);
    assert_eq!(
        non_dispatch.content,
        "only dispatch can run in the background"
    );

    let missing_id = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch"], "background": true}),
    );
    assert!(missing_id.is_error);
    assert_eq!(missing_id.content, "dispatch needs an id");

    let start = Instant::now();
    let bg = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1", "t.md"], "background": true}),
    );
    let elapsed = start.elapsed();
    assert!(!bg.is_error, "{}", bg.content);
    assert_eq!(
        bg.content,
        "started T1 in the background; use wait to learn when it ends"
    );
    assert!(
        elapsed < Duration::from_secs(1),
        "background dispatch blocked for {elapsed:?}"
    );
    assert!(!marker.exists(), "marker appeared too early");

    let wait_until = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < wait_until {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(marker.exists(), "background script never created marker");
}

#[test]
fn nonzero_exit_returns_error_with_stderr_text() {
    let root = tempfile::tempdir().unwrap();
    let scripts = tempfile::tempdir().unwrap();
    let script = write_script(
        scripts.path(),
        "fake-blyte",
        "#!/bin/sh\necho 'something went wrong on stderr' >&2\nexit 3\n",
    );
    let _g = BlyteToolGuard::acquire(Some(script));

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["status", "T1"]}),
    );
    assert!(out.is_error);
    assert!(
        out.content.contains("something went wrong on stderr"),
        "content was: {:?}",
        out.content
    );
    assert!(
        out.content.contains("exit 3"),
        "content was: {:?}",
        out.content
    );
}
