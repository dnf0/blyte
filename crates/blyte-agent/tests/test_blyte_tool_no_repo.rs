use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use blyte_agent::approval::{Approval, ApprovalConfig, Approver};
use blyte_agent::folder_lock::{acquire, Holder};
use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{
    check_write_scope, execute, set_approval, set_blyte_request, set_blyte_run_store,
    set_blyte_started_by, set_blyte_surface, set_blyte_tasks_dir, set_blyte_tool,
    set_in_place_lock_path, set_publish_tool, set_write_scope, PublishTool, WriteScope,
};
use serde_json::json;

static TOOL_LOCK: Mutex<()> = Mutex::new(());

struct ToolEnvGuard {
    _lock: MutexGuard<'static, ()>,
    _script_dir: Option<tempfile::TempDir>,
}

impl ToolEnvGuard {
    fn new(script_body: Option<&str>) -> Self {
        let lock = TOOL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_blyte_tool(None);
        set_blyte_tasks_dir(None);
        set_blyte_run_store(None);
        set_in_place_lock_path(None);
        set_blyte_request(None);
        set_blyte_surface(None);
        set_blyte_started_by(None);
        set_write_scope(None);
        set_approval(None);
        set_publish_tool(None);

        let script_dir = script_body.map(|body| {
            let dir = tempfile::tempdir().unwrap();
            let exe = write_executable(dir.path(), "blyte-stub", body);
            set_blyte_tool(Some(exe));
            dir
        });

        Self {
            _lock: lock,
            _script_dir: script_dir,
        }
    }
}

impl Drop for ToolEnvGuard {
    fn drop(&mut self) {
        set_blyte_tool(None);
        set_blyte_tasks_dir(None);
        set_blyte_run_store(None);
        set_in_place_lock_path(None);
        set_blyte_request(None);
        set_blyte_surface(None);
        set_blyte_started_by(None);
        set_write_scope(None);
        set_approval(None);
        set_publish_tool(None);
    }
}

fn write_executable(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    let content = format!("#!/bin/sh\nset -eu\n{body}\n");
    std::fs::write(&path, content).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path
}

fn init_git(dir: &Path) {
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(status.success());
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
        Ok("https://example.invalid/pr/1".to_string())
    }
}

struct CountingApprover {
    decision: Approval,
    calls: AtomicUsize,
}

impl Approver for CountingApprover {
    fn ask(&self, _command: &str, _reason: &str) -> Approval {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.decision
    }
}

#[test]
fn outside_repository_allowed_and_refused_commands() {
    let _g = ToolEnvGuard::new(Some("printf 'ARGS:%s\\n' \"$*\""));
    let recorder = Arc::new(RecordingPublish {
        calls: Mutex::new(Vec::new()),
    });
    set_publish_tool(Some(recorder.clone()));
    let root = tempfile::tempdir().unwrap();

    for cmd in [
        vec!["status", "T1"],
        vec!["review", "T1"],
        vec!["wait", "--any", "T1"],
    ] {
        let out = execute(
            root.path(),
            &LocalShell,
            &LocalFs,
            "blyte",
            &json!({"args": cmd}),
        );
        assert!(!out.is_error, "{cmd:?}: {}", out.content);
        let expected = format!("ARGS:{}", cmd.join(" "));
        assert!(out.content.contains(&expected), "{}", out.content);
    }

    for refused in ["plan", "init", "clean", "edit-start", "edit-finish", "undo"] {
        let out = execute(
            root.path(),
            &LocalShell,
            &LocalFs,
            "blyte",
            &json!({"args": [refused, "T1"]}),
        );
        assert!(out.is_error, "{refused} must be refused");
        assert!(
            out.content.contains("dispatch, wait, status and review"),
            "{refused}: {}",
            out.content
        );
        assert!(
            out.content.contains("blyte undo <id>"),
            "{refused}: {}",
            out.content
        );
    }

    let pub_out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "publish",
        &json!({
            "request": "req-1",
            "title": "feat: test",
            "body": "body"
        }),
    );
    assert!(pub_out.is_error, "{}", pub_out.content);
    assert!(recorder.calls.lock().unwrap().is_empty());
    assert!(!root.path().join(".wt").exists());
}

#[test]
fn outside_repository_dispatch_options_and_injected_flags_match_repository() {
    let _g = ToolEnvGuard::new(Some("for a in \"$@\"; do printf '[%s]\\n' \"$a\"; done"));
    let root = tempfile::tempdir().unwrap();
    let tasks = tempfile::tempdir().unwrap();
    set_blyte_tasks_dir(Some(tasks.path().to_path_buf()));
    set_blyte_request(Some("req-7".to_string()));
    set_blyte_surface(Some("s001".to_string()));
    set_blyte_started_by(Some("peer-a".to_string()));

    let refused = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1", "--gate", "true"], "task": "do x"}),
    );
    assert!(refused.is_error);
    assert!(refused.content.contains("--gate"), "{}", refused.content);

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1"], "task": "do x"}),
    );
    assert!(!out.is_error, "{}", out.content);
    let lines: Vec<&str> = out.content.lines().collect();
    let expected_task = format!("[{}]", tasks.path().join("T1.md").display());
    assert_eq!(
        lines,
        vec![
            "[dispatch]",
            "[T1]",
            expected_task.as_str(),
            "[--request]",
            "[req-7]",
            "[--surface]",
            "[s001]",
            "[--started-by]",
            "[peer-a]",
        ]
    );
}

#[test]
fn inline_task_writes_file_and_enforces_conflicts_and_errors() {
    let _g = ToolEnvGuard::new(Some("for a in \"$@\"; do printf '[%s]\\n' \"$a\"; done"));
    let root = tempfile::tempdir().unwrap();
    let tasks = tempfile::tempdir().unwrap();

    let missing_setter = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1"], "task": "do x"}),
    );
    assert!(missing_setter.is_error);
    assert!(
        missing_setter.content.contains("tasks directory"),
        "{}",
        missing_setter.content
    );
    assert!(!root.path().join(".wt").exists());

    set_blyte_tasks_dir(Some(tasks.path().to_path_buf()));

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1"], "task": "do x"}),
    );
    assert!(!out.is_error, "{}", out.content);
    let task_file = tasks.path().join("T1.md");
    assert_eq!(std::fs::read_to_string(&task_file).unwrap(), "do x");
    let lines: Vec<&str> = out.content.lines().collect();
    assert_eq!(
        lines,
        vec!["[dispatch]", "[T1]", &format!("[{}]", task_file.display())]
    );

    let same_again = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1"], "task": "do x"}),
    );
    assert!(!same_again.is_error, "{}", same_again.content);

    let conflict = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1"], "task": "do y"}),
    );
    assert!(conflict.is_error);
    assert!(
        conflict.content.contains(&task_file.display().to_string()),
        "{}",
        conflict.content
    );
    assert_eq!(std::fs::read_to_string(&task_file).unwrap(), "do x");

    let with_file = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T2", "other.md"], "task": "do x"}),
    );
    assert!(with_file.is_error, "{}", with_file.content);

    let with_plan = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T2", "--plan", "p.md"], "task": "do x"}),
    );
    assert!(with_plan.is_error, "{}", with_plan.content);

    let with_plan_task = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T2", "--plan-task", "Task 1"], "task": "do x"}),
    );
    assert!(with_plan_task.is_error, "{}", with_plan_task.content);

    let non_dispatch = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["status", "T1"], "task": "do x"}),
    );
    assert!(non_dispatch.is_error, "{}", non_dispatch.content);

    struct StateDirEnvGuard {
        prev: Option<OsString>,
    }
    impl Drop for StateDirEnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("BLYTE_STATE_DIR", v),
                None => std::env::remove_var("BLYTE_STATE_DIR"),
            }
        }
    }

    let lock_dir = tempfile::tempdir().unwrap();
    set_in_place_lock_path(Some(lock_dir.path().join("in-place.lock")));
    set_write_scope(Some(WriteScope {
        allowed: Vec::new(),
        edit_worktrees: None,
    }));

    let model_write_tasks = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "write",
        &json!({"path": tasks.path().join("T9.md").display().to_string(), "content": "nope"}),
    );
    assert!(model_write_tasks.is_error, "{}", model_write_tasks.content);
    assert!(
        model_write_tasks.content.contains("escapes the workspace"),
        "{}",
        model_write_tasks.content
    );
    assert!(!tasks.path().join("T9.md").exists());

    let scope_err = check_write_scope(&LocalFs, "write", &tasks.path().join("T9.md")).unwrap_err();
    assert!(
        scope_err.contains("write was not made"),
        "expected write scope refusal, got: {scope_err}"
    );
    assert!(
        scope_err.contains("outside the directories this conversation may write"),
        "expected write scope refusal, got: {scope_err}"
    );

    let state_in_root = root.path().join("blyte-state");
    let tasks_in_root = state_in_root.join("tasks");
    std::fs::create_dir_all(&tasks_in_root).unwrap();
    let prev_state_dir = std::env::var_os("BLYTE_STATE_DIR");
    std::env::set_var("BLYTE_STATE_DIR", &state_in_root);
    let state_guard = StateDirEnvGuard {
        prev: prev_state_dir,
    };
    let protected_write = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "write",
        &json!({"path": "blyte-state/tasks/T9.md", "content": "nope"}),
    );
    assert!(protected_write.is_error, "{}", protected_write.content);
    assert!(
        protected_write
            .content
            .contains("is protected (blyte's own files)"),
        "{}",
        protected_write.content
    );
    assert!(!tasks_in_root.join("T9.md").exists());
    drop(state_guard);

    let scoped_write = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "write",
        &json!({"path": "blyte-state/tasks/T9.md", "content": "nope"}),
    );
    assert!(scoped_write.is_error, "{}", scoped_write.content);
    assert!(
        scoped_write.content.contains("write was not made"),
        "{}",
        scoped_write.content
    );
    assert!(
        scoped_write
            .content
            .contains("outside the directories this conversation may write"),
        "{}",
        scoped_write.content
    );
    assert!(!tasks_in_root.join("T9.md").exists());

    let bad_tasks_dir = tempfile::tempdir().unwrap();
    let blocking_file = bad_tasks_dir.path().join("not-a-dir");
    std::fs::write(&blocking_file, "x").unwrap();
    set_blyte_tasks_dir(Some(blocking_file.clone()));
    let unwritable = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T3"], "task": "do z"}),
    );
    assert!(unwritable.is_error);
    assert!(
        unwritable
            .content
            .contains(&blocking_file.display().to_string()),
        "{}",
        unwritable.content
    );
}

#[test]
fn inline_task_inside_repository_uses_wt_tasks() {
    let _g = ToolEnvGuard::new(Some("for a in \"$@\"; do printf '[%s]\\n' \"$a\"; done"));
    let repo = tempfile::tempdir().unwrap();
    init_git(repo.path());

    let out = execute(
        repo.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1"], "task": "in repo task"}),
    );
    assert!(!out.is_error, "{}", out.content);
    let expected_file = repo.path().join(".wt").join("tasks").join("T1.md");
    assert_eq!(
        std::fs::read_to_string(&expected_file).unwrap(),
        "in repo task"
    );
    assert!(
        out.content
            .contains(&format!("[{}]", expected_file.display())),
        "{}",
        out.content
    );
}

#[test]
fn background_dispatch_outside_repository_writes_to_run_store_not_folder() {
    let _g = ToolEnvGuard::new(Some("echo dispatched-bg"));
    let root = tempfile::tempdir().unwrap();
    let tasks = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();

    set_blyte_tasks_dir(Some(tasks.path().to_path_buf()));
    set_blyte_run_store(Some(store.path().to_path_buf()));

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1"], "task": "do bg", "background": true}),
    );
    assert!(!out.is_error, "{}", out.content);
    let out_file = store.path().join("T1.dispatch.out");
    assert!(
        out_file.exists(),
        "expected {} to exist",
        out_file.display()
    );
    // The command runs in the background: wait for its output rather than
    // reading the file the moment it exists.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let text = std::fs::read_to_string(&out_file).unwrap();
        if text.contains("dispatched-bg") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no output from the background command after 60 s: {text:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!root.path().join(".wt").exists());
}

#[test]
fn write_and_edit_refused_while_folder_lock_held_and_allowed_through_approval_when_free() {
    let _g = ToolEnvGuard::new(Some("echo ok"));
    let root = tempfile::tempdir().unwrap();
    let lock_dir = tempfile::tempdir().unwrap();
    let lock_path = lock_dir.path().join("in-place.lock");
    let target = root.path().join("note.txt");
    std::fs::write(&target, "before\n").unwrap();

    let approver = Arc::new(CountingApprover {
        decision: Approval::Approved,
        calls: AtomicUsize::new(0),
    });
    set_approval(Some(ApprovalConfig {
        approver: approver.clone(),
        rules: Vec::new(),
    }));
    set_write_scope(Some(WriteScope {
        allowed: Vec::new(),
        edit_worktrees: None,
    }));

    let missing_lock = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "write",
        &json!({"path": "note.txt", "content": "nope\n"}),
    );
    assert!(missing_lock.is_error);
    assert!(
        missing_lock.content.contains("lock"),
        "{}",
        missing_lock.content
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "before\n");
    assert_eq!(approver.calls.load(Ordering::SeqCst), 0);

    set_in_place_lock_path(Some(lock_path.clone()));

    let held = acquire(
        &lock_path,
        &Holder {
            pid: std::process::id(),
            kind: "run".to_string(),
            id: "R1".to_string(),
        },
    )
    .unwrap();

    let write_blocked = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "write",
        &json!({"path": "note.txt", "content": "after\n"}),
    );
    assert!(write_blocked.is_error);
    assert_eq!(
        write_blocked.content,
        "run R1 is editing this folder; wait for it to finish"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "before\n");
    assert_eq!(approver.calls.load(Ordering::SeqCst), 0);

    let edit_blocked = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "edit",
        &json!({"path": "note.txt", "old": "before", "new": "after"}),
    );
    assert!(edit_blocked.is_error);
    assert_eq!(
        edit_blocked.content,
        "run R1 is editing this folder; wait for it to finish"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "before\n");
    assert_eq!(approver.calls.load(Ordering::SeqCst), 0);

    drop(held);

    let write_ok = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "write",
        &json!({"path": "note.txt", "content": "after\n"}),
    );
    assert!(!write_ok.is_error, "{}", write_ok.content);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "after\n");
    assert_eq!(approver.calls.load(Ordering::SeqCst), 1);

    let edit_ok = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "edit",
        &json!({"path": "note.txt", "old": "after", "new": "edited"}),
    );
    assert!(!edit_ok.is_error, "{}", edit_ok.content);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "edited\n");
    assert_eq!(approver.calls.load(Ordering::SeqCst), 2);

    let blocking_parent = lock_dir.path().join("not-a-dir");
    std::fs::write(&blocking_parent, "x").unwrap();
    let bad_lock = blocking_parent.join("in-place.lock");
    set_in_place_lock_path(Some(bad_lock.clone()));
    let unreadable = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "write",
        &json!({"path": "note.txt", "content": "blocked\n"}),
    );
    assert!(unreadable.is_error, "{}", unreadable.content);
    assert!(
        unreadable.content.contains(&bad_lock.display().to_string()),
        "{}",
        unreadable.content
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "edited\n");
}
