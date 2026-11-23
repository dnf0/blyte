use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use blyte_agent::approval::{Approval, ApprovalConfig, Approver};
use blyte_agent::fs::{Fs, LocalFs};
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, set_approval, set_write_scope, WriteScope};
use serde_json::json;

static SCOPE_LOCK: Mutex<()> = Mutex::new(());

struct ScopeGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl ScopeGuard {
    fn install(scope: Option<WriteScope>, approval: Option<ApprovalConfig>) -> Self {
        let lock = SCOPE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_write_scope(scope);
        set_approval(approval);
        Self { _lock: lock }
    }
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        set_write_scope(None);
        set_approval(None);
    }
}

struct RecordingApprover {
    decision: Mutex<Approval>,
    calls: Mutex<Vec<(String, String)>>,
}

impl RecordingApprover {
    fn new(decision: Approval) -> Self {
        Self {
            decision: Mutex::new(decision),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn set_decision(&self, decision: Approval) {
        *self.decision.lock().unwrap() = decision;
    }

    fn take_calls(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}

impl Approver for RecordingApprover {
    fn ask(&self, command: &str, reason: &str) -> Approval {
        self.calls
            .lock()
            .unwrap()
            .push((command.to_string(), reason.to_string()));
        *self.decision.lock().unwrap()
    }
}

struct UncanonicalizableFs;

impl Fs for UncanonicalizableFs {
    fn read(&self, path: &Path) -> Result<String, String> {
        Err(format!("no read {}", path.display()))
    }
    fn write(&self, path: &Path, _content: &str) -> Result<(), String> {
        Err(format!("unexpected write {}", path.display()))
    }
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, String> {
        Err(format!("simulated failure for {}", path.display()))
    }
}

#[test]
fn write_and_edit_enforce_write_scope_and_approval() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let allowed_dir = root.join("A");
    std::fs::create_dir_all(&allowed_dir).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    let allowed_canon = allowed_dir.canonicalize().unwrap();

    let approver = Arc::new(RecordingApprover::new(Approval::Denied));
    let _guard = ScopeGuard::install(
        Some(WriteScope {
            allowed: vec![allowed_canon.clone()],
            edit_worktrees: None,
        }),
        Some(ApprovalConfig {
            rules: Vec::new(),
            approver: approver.clone(),
        }),
    );

    let shell = LocalShell::new();
    let fs = LocalFs;

    let r = execute(
        root,
        &shell,
        &fs,
        "write",
        &json!({ "path": "A/task.md", "content": "hello\n" }),
    );
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(
        std::fs::read_to_string(allowed_dir.join("task.md")).unwrap(),
        "hello\n"
    );
    assert!(approver.take_calls().is_empty());

    let r = execute(
        root,
        &shell,
        &fs,
        "edit",
        &json!({ "path": "A/task.md", "old": "hello", "new": "updated" }),
    );
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(
        std::fs::read_to_string(allowed_dir.join("task.md")).unwrap(),
        "updated\n"
    );
    assert!(approver.take_calls().is_empty());

    let r = execute(
        root,
        &shell,
        &fs,
        "write",
        &json!({ "path": "A/newsub/deep/note.md", "content": "nested\n" }),
    );
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(
        std::fs::read_to_string(allowed_dir.join("newsub/deep/note.md")).unwrap(),
        "nested\n"
    );
    assert!(approver.take_calls().is_empty());

    for rel in [
        "src/x.rs",
        ".git/refs/heads/x",
        ".wt/passed/x.json",
        ".wt/combined/r.json",
    ] {
        let r = execute(
            root,
            &shell,
            &fs,
            "write",
            &json!({ "path": rel, "content": "blocked\n" }),
        );
        assert!(r.is_error, "write to {rel} must fail when refused");
        assert!(
            r.content.contains("write was not made"),
            "{rel}: {}",
            r.content
        );
        assert!(
            r.content.contains(&allowed_canon.display().to_string()),
            "{rel}: {}",
            r.content
        );
        assert!(r.content.contains("approve"), "{rel}: {}", r.content);
        assert!(!root.join(rel).exists(), "{rel} must not be created");
        let calls = approver.take_calls();
        assert_eq!(calls.len(), 1, "{rel}: {calls:?}");
        assert!(calls[0].0.starts_with("write "), "{calls:?}");
        assert!(calls[0].0.ends_with(rel), "{calls:?}");
        assert!(
            calls[0]
                .1
                .contains("outside the directories this conversation may write"),
            "{calls:?}"
        );
        assert!(
            calls[0].1.contains(&allowed_canon.display().to_string()),
            "{calls:?}"
        );
    }

    approver.set_decision(Approval::Approved);
    let r = execute(
        root,
        &shell,
        &fs,
        "write",
        &json!({ "path": "src/x.rs", "content": "fn main() {}\n" }),
    );
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(
        std::fs::read_to_string(root.join("src/x.rs")).unwrap(),
        "fn main() {}\n"
    );
    let calls = approver.take_calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].0.starts_with("write "));

    let r = execute(
        root,
        &shell,
        &fs,
        "edit",
        &json!({ "path": "src/x.rs", "old": "main", "new": "run" }),
    );
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(
        std::fs::read_to_string(root.join("src/x.rs")).unwrap(),
        "fn run() {}\n"
    );
    let calls = approver.take_calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].0.starts_with("edit "));

    approver.set_decision(Approval::Denied);
    let r = execute(
        root,
        &shell,
        &fs,
        "edit",
        &json!({ "path": "src/x.rs", "old": "run", "new": "hacked" }),
    );
    assert!(r.is_error);
    assert_eq!(
        std::fs::read_to_string(root.join("src/x.rs")).unwrap(),
        "fn run() {}\n"
    );
    let calls = approver.take_calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].0.starts_with("edit "));

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join(".git"), allowed_dir.join("link")).unwrap();
        let r = execute(
            root,
            &shell,
            &fs,
            "write",
            &json!({ "path": "A/link/config", "content": "bad\n" }),
        );
        assert!(r.is_error, "symlink escape must be refused");
        assert!(!root.join(".git/config").exists());
        let calls = approver.take_calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].0.contains(".git/config"), "{calls:?}");

        std::os::unix::fs::symlink(
            root.join(".git/does_not_exist"),
            allowed_dir.join("dangling"),
        )
        .unwrap();
        let r = execute(
            root,
            &shell,
            &fs,
            "write",
            &json!({ "path": "A/dangling", "content": "bad\n" }),
        );
        assert!(r.is_error, "dangling symlink must error");
        assert!(r.content.contains("dangling"), "{}", r.content);
        assert!(!root.join(".git/does_not_exist").exists());
        assert!(approver.take_calls().is_empty());
    }

    let r = execute(
        root,
        &shell,
        &fs,
        "write",
        &json!({ "path": "A/../src/escaped.rs", "content": "bad\n" }),
    );
    assert!(r.is_error, ".. escape must ask and be refused");
    assert!(!root.join("src/escaped.rs").exists());
    let calls = approver.take_calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].0.ends_with("src/escaped.rs"), "{calls:?}");

    set_approval(None);
    let r = execute(
        root,
        &shell,
        &fs,
        "write",
        &json!({ "path": "src/no_approver.rs", "content": "bad\n" }),
    );
    assert!(r.is_error);
    assert!(r.content.contains("write was not made"), "{}", r.content);
    assert!(
        r.content.contains(&allowed_canon.display().to_string()),
        "{}",
        r.content
    );
    assert!(r.content.contains("approve"), "{}", r.content);
    assert!(!root.join("src/no_approver.rs").exists());

    let bad_fs = UncanonicalizableFs;
    let r = execute(
        root,
        &shell,
        &bad_fs,
        "write",
        &json!({ "path": "A/fail.md", "content": "nope\n" }),
    );
    assert!(r.is_error);
    assert!(
        r.content.contains(&root.display().to_string()) || r.content.contains("fail.md"),
        "{}",
        r.content
    );

    set_write_scope(None);
    set_approval(Some(ApprovalConfig {
        rules: Vec::new(),
        approver: approver.clone(),
    }));
    for rel in [
        "src/unscoped.rs",
        ".git/refs/heads/x",
        ".wt/passed/x.json",
        ".wt/combined/r.json",
        "A/../src/escaped.rs",
    ] {
        let r = execute(
            root,
            &shell,
            &fs,
            "write",
            &json!({ "path": rel, "content": "ok\n" }),
        );
        assert!(!r.is_error, "{rel}: {}", r.content);
        assert!(root.join(rel).exists(), "{rel} should exist");
    }
    #[cfg(unix)]
    {
        let r = execute(
            root,
            &shell,
            &fs,
            "write",
            &json!({ "path": "A/link/config", "content": "ok\n" }),
        );
        assert!(!r.is_error, "{}", r.content);
        assert!(root.join(".git/config").exists());
    }
    assert!(approver.take_calls().is_empty());
}

#[test]
fn write_and_edit_allow_open_direct_edit_worktrees_only() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let wt_dir = root.join(".wt");
    let tasks_dir = wt_dir.join("tasks");
    let edit_wt = wt_dir.join("X");
    let other_wt = wt_dir.join("Y");
    let main_src = root.join("src");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    std::fs::create_dir_all(&edit_wt).unwrap();
    std::fs::create_dir_all(&other_wt).unwrap();
    std::fs::create_dir_all(&main_src).unwrap();

    let tasks_canon = tasks_dir.canonicalize().unwrap();
    let wt_canon = wt_dir.canonicalize().unwrap();

    std::fs::write(wt_dir.join("X.edit.json"), "{}\n").unwrap();

    let approver = Arc::new(RecordingApprover::new(Approval::Denied));
    let _guard = ScopeGuard::install(
        Some(WriteScope {
            allowed: vec![tasks_canon.clone()],
            edit_worktrees: Some(wt_canon.clone()),
        }),
        Some(ApprovalConfig {
            rules: Vec::new(),
            approver: approver.clone(),
        }),
    );

    let shell = LocalShell::new();
    let fs = LocalFs;

    let r = execute(
        root,
        &shell,
        &fs,
        "write",
        &json!({ "path": ".wt/X/src/a.rs", "content": "pub fn a() {}\n" }),
    );
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(
        std::fs::read_to_string(edit_wt.join("src/a.rs")).unwrap(),
        "pub fn a() {}\n"
    );
    assert!(approver.take_calls().is_empty());

    let r = execute(
        root,
        &shell,
        &fs,
        "edit",
        &json!({ "path": ".wt/X/src/a.rs", "old": "fn a()", "new": "fn b()" }),
    );
    assert!(!r.is_error, "{}", r.content);
    assert_eq!(
        std::fs::read_to_string(edit_wt.join("src/a.rs")).unwrap(),
        "pub fn b() {}\n"
    );
    assert!(approver.take_calls().is_empty());

    for rel in [
        ".wt/X/.git",
        ".wt/X/sub/.git/config",
        ".wt/X.json",
        ".wt/X.edit.json",
        ".wt/Y/a.rs",
    ] {
        let r = execute(
            root,
            &shell,
            &fs,
            "write",
            &json!({ "path": rel, "content": "blocked\n" }),
        );
        assert!(r.is_error, "write to {rel} must be refused");
        assert!(
            r.content.contains("write was not made"),
            "{rel}: {}",
            r.content
        );
        let calls = approver.take_calls();
        assert_eq!(calls.len(), 1, "{rel}: {calls:?}");
    }
    assert!(!edit_wt.join(".git").exists());
    assert!(!edit_wt.join("sub/.git/config").exists());
    assert!(!wt_dir.join("X.json").exists());
    assert!(!other_wt.join("a.rs").exists());
    assert_eq!(
        std::fs::read_to_string(wt_dir.join("X.edit.json")).unwrap(),
        "{}\n"
    );

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&edit_wt, tasks_dir.join("to_edit")).unwrap();
        let r = execute(
            root,
            &shell,
            &fs,
            "write",
            &json!({ "path": ".wt/tasks/to_edit/src/via_link.rs", "content": "ok\n" }),
        );
        assert!(!r.is_error, "{}", r.content);
        assert_eq!(
            std::fs::read_to_string(edit_wt.join("src/via_link.rs")).unwrap(),
            "ok\n"
        );
        assert!(approver.take_calls().is_empty());

        std::os::unix::fs::symlink(&main_src, tasks_dir.join("to_main")).unwrap();
        let r = execute(
            root,
            &shell,
            &fs,
            "write",
            &json!({ "path": ".wt/tasks/to_main/escape.rs", "content": "bad\n" }),
        );
        assert!(r.is_error, "symlink to main checkout must be refused");
        assert!(!main_src.join("escape.rs").exists());
        assert_eq!(approver.take_calls().len(), 1);
    }

    std::fs::write(wt_dir.join("X.json"), "{}\n").unwrap();
    let r = execute(
        root,
        &shell,
        &fs,
        "write",
        &json!({ "path": ".wt/X/src/after_finish.rs", "content": "blocked\n" }),
    );
    assert!(r.is_error, "finished edit worktree must be refused");
    assert!(!edit_wt.join("src/after_finish.rs").exists());
    assert_eq!(approver.take_calls().len(), 1);

    std::fs::remove_file(wt_dir.join("X.json")).unwrap();
    std::fs::remove_file(wt_dir.join("X.edit.json")).unwrap();
    let r = execute(
        root,
        &shell,
        &fs,
        "write",
        &json!({ "path": ".wt/X/src/no_edit_file.rs", "content": "blocked\n" }),
    );
    assert!(r.is_error, "worktree without .edit.json must be refused");
    assert!(!edit_wt.join("src/no_edit_file.rs").exists());
    assert_eq!(approver.take_calls().len(), 1);

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(wt_dir.join("Z.edit.json"), wt_dir.join("Z.edit.json")).unwrap();
        let loop_wt = wt_dir.join("Z");
        std::fs::create_dir_all(&loop_wt).unwrap();
        let r = execute(
            root,
            &shell,
            &fs,
            "write",
            &json!({ "path": ".wt/Z/src/a.rs", "content": "blocked\n" }),
        );
        assert!(r.is_error, "unreadable marker must return error");
        assert!(r.content.contains("Z.edit.json"), "{}", r.content);
        assert!(approver.take_calls().is_empty());
        assert!(!loop_wt.join("src/a.rs").exists());
    }

    std::fs::write(wt_dir.join("X.edit.json"), "{}\n").unwrap();
    set_write_scope(Some(WriteScope {
        allowed: vec![tasks_canon],
        edit_worktrees: None,
    }));
    for rel in [
        ".wt/X/src/none_case.rs",
        ".wt/X/.git",
        ".wt/X/sub/.git/config",
        ".wt/X.json",
        ".wt/X.edit.json",
        ".wt/Y/a.rs",
    ] {
        let r = execute(
            root,
            &shell,
            &fs,
            "write",
            &json!({ "path": rel, "content": "blocked\n" }),
        );
        assert!(r.is_error, "edit_worktrees: None must refuse {rel}");
        assert_eq!(approver.take_calls().len(), 1, "{rel}");
    }
    #[cfg(unix)]
    {
        let r = execute(
            root,
            &shell,
            &fs,
            "write",
            &json!({ "path": ".wt/tasks/to_edit/src/none_link.rs", "content": "blocked\n" }),
        );
        assert!(
            r.is_error,
            "edit_worktrees: None must refuse symlink to edit"
        );
        assert!(!edit_wt.join("src/none_link.rs").exists());
        assert_eq!(approver.take_calls().len(), 1);
    }
}
