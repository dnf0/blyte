use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Mutex};

use blyte_agent::approval::{Approval, ApprovalConfig, Approver};
use blyte_agent::fs::LocalFs;
use blyte_agent::sandbox::{is_protected_read, protected_read_paths, surface_token_path};
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, set_approval, set_read_roots, set_write_scope, WriteScope};
use serde_json::json;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    home: Option<OsString>,
    xdg_state: Option<OsString>,
    blyte_state: Option<OsString>,
}

impl EnvGuard {
    fn set(home: Option<&Path>, xdg_state: Option<&Path>, blyte_state: Option<&Path>) -> Self {
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
        match xdg_state {
            Some(p) => std::env::set_var("XDG_STATE_HOME", p),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        match blyte_state {
            Some(p) => std::env::set_var("BLYTE_STATE_DIR", p),
            None => std::env::remove_var("BLYTE_STATE_DIR"),
        }
        prev
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        set_write_scope(None);
        set_approval(None);
        set_read_roots(Vec::new());
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

#[test]
fn read_refuses_ssh_key_inside_root() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(home.path().join(".ssh/id_ed25519"), "PRIVATE_KEY_SECRET\n").unwrap();
    std::fs::write(home.path().join("notes.txt"), "hello notes\n").unwrap();

    let shell = LocalShell::new();
    let fs = LocalFs;

    let refused = execute(
        home.path(),
        &shell,
        &fs,
        "read",
        &json!({ "path": ".ssh/id_ed25519" }),
    );
    assert!(refused.is_error, "{}", refused.content);
    assert!(
        refused.content.contains("read was not made:"),
        "{}",
        refused.content
    );
    assert!(
        refused.content.contains(".ssh/id_ed25519"),
        "{}",
        refused.content
    );
    assert!(
        refused
            .content
            .contains("is a credential path that this conversation cannot read"),
        "{}",
        refused.content
    );
    assert!(
        !refused.content.contains("PRIVATE_KEY_SECRET"),
        "{}",
        refused.content
    );

    let ok = execute(
        home.path(),
        &shell,
        &fs,
        "read",
        &json!({ "path": "notes.txt" }),
    );
    assert!(!ok.is_error, "{}", ok.content);
    assert!(ok.content.contains("hello notes"), "{}", ok.content);
}

#[test]
fn read_refuses_dot_env_but_reads_dot_env_example() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    std::fs::write(ws.path().join(".env"), "API_KEY=secret\n").unwrap();
    std::fs::write(ws.path().join(".env.example"), "API_KEY=\n").unwrap();

    let shell = LocalShell::new();
    let fs = LocalFs;

    let refused = execute(ws.path(), &shell, &fs, "read", &json!({ "path": ".env" }));
    assert!(refused.is_error, "{}", refused.content);
    assert!(
        refused.content.contains("read was not made:"),
        "{}",
        refused.content
    );
    assert!(refused.content.contains(".env"), "{}", refused.content);
    assert!(
        !refused.content.contains("API_KEY=secret"),
        "{}",
        refused.content
    );

    let ok = execute(
        ws.path(),
        &shell,
        &fs,
        "read",
        &json!({ "path": ".env.example" }),
    );
    assert!(!ok.is_error, "{}", ok.content);
    assert!(ok.content.contains("API_KEY="), "{}", ok.content);
}

#[cfg(unix)]
#[test]
fn read_refuses_a_symlink_into_a_protected_folder() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    std::fs::create_dir_all(home.path().join(".aws")).unwrap();
    std::fs::write(
        home.path().join(".aws/credentials"),
        "aws_secret_access_key=TOPSECRET\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        home.path().join(".aws/credentials"),
        home.path().join("link"),
    )
    .unwrap();

    let shell = LocalShell::new();
    let fs = LocalFs;

    let refused = execute(home.path(), &shell, &fs, "read", &json!({ "path": "link" }));
    assert!(refused.is_error, "{}", refused.content);
    assert!(
        refused.content.contains("read was not made:"),
        "{}",
        refused.content
    );
    assert!(
        refused
            .content
            .contains("is a credential path that this conversation cannot read"),
        "{}",
        refused.content
    );
    assert!(
        !refused.content.contains("TOPSECRET"),
        "{}",
        refused.content
    );
}

#[test]
fn grep_skips_protected_files_and_says_so() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(home.path().join(".ssh/k"), "NEEDLE_SECRET_VALUE\n").unwrap();
    std::fs::write(home.path().join("a.txt"), "NEEDLE_SECRET_VALUE\n").unwrap();

    let shell = LocalShell::new();
    let fs = LocalFs;

    let out = execute(
        home.path(),
        &shell,
        &fs,
        "grep",
        &json!({ "pattern": "NEEDLE_SECRET_VALUE" }),
    );
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("a.txt"), "{}", out.content);
    assert!(!out.content.contains(".ssh/k"), "{}", out.content);
    assert!(
        out.content.contains("(credential paths skipped: "),
        "{}",
        out.content
    );
}

#[test]
fn glob_does_not_list_protected_files() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(home.path().join(".ssh/id_ed25519"), "key\n").unwrap();
    std::fs::write(home.path().join(".env"), "SECRET=1\n").unwrap();
    std::fs::write(home.path().join("visible.txt"), "ok\n").unwrap();

    let shell = LocalShell::new();
    let fs = LocalFs;

    let out = execute(
        home.path(),
        &shell,
        &fs,
        "glob",
        &json!({ "pattern": "**/*" }),
    );
    assert!(!out.is_error, "{}", out.content);
    assert!(out.content.contains("visible.txt"), "{}", out.content);
    assert!(!out.content.contains(".ssh"), "{}", out.content);
    assert!(!out.content.contains(".env"), "{}", out.content);
    assert!(
        out.content.contains("(credential paths skipped: "),
        "{}",
        out.content
    );
}

#[test]
fn grep_with_a_protected_search_root_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(home.path().join(".ssh/id_ed25519"), "secret\n").unwrap();

    let shell = LocalShell::new();
    let fs = LocalFs;

    let refused = execute(
        home.path(),
        &shell,
        &fs,
        "grep",
        &json!({ "pattern": "secret", "path": ".ssh" }),
    );
    assert!(refused.is_error, "{}", refused.content);
    assert!(
        refused.content.contains("grep was not made:"),
        "{}",
        refused.content
    );
    assert!(refused.content.contains(".ssh"), "{}", refused.content);
    assert!(
        refused
            .content
            .contains("is a credential path that this conversation cannot read"),
        "{}",
        refused.content
    );

    let glob_refused = execute(
        home.path(),
        &shell,
        &fs,
        "glob",
        &json!({ "pattern": "*", "path": ".ssh" }),
    );
    assert!(glob_refused.is_error, "{}", glob_refused.content);
    assert!(
        glob_refused.content.contains("glob was not made:"),
        "{}",
        glob_refused.content
    );
}

#[test]
fn read_refuses_the_surface_token_folder() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    let token_path = surface_token_path(ws.path()).unwrap();
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, "surface-token-secret\n").unwrap();

    let shell = LocalShell::new();
    let fs = LocalFs;

    let rel = token_path.strip_prefix(home.path()).unwrap();
    let refused_root = execute(
        home.path(),
        &shell,
        &fs,
        "read",
        &json!({ "path": rel.to_str().unwrap() }),
    );
    assert!(refused_root.is_error, "{}", refused_root.content);
    assert!(
        refused_root.content.contains("read was not made:"),
        "{}",
        refused_root.content
    );
    assert!(
        !refused_root.content.contains("surface-token-secret"),
        "{}",
        refused_root.content
    );

    set_read_roots(vec![home.path().to_path_buf()]);
    let refused_read_root = execute(
        ws.path(),
        &shell,
        &fs,
        "read",
        &json!({ "path": token_path.to_str().unwrap() }),
    );
    assert!(refused_read_root.is_error, "{}", refused_read_root.content);
    assert!(
        refused_read_root.content.contains("read was not made:"),
        "{}",
        refused_read_root.content
    );
    assert!(
        !refused_read_root.content.contains("surface-token-secret"),
        "{}",
        refused_read_root.content
    );
}

#[test]
fn write_into_blyte_state_asks_for_approval() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    let local_dir = home.path().join(".local");
    std::fs::create_dir_all(local_dir.join("state/blyte")).unwrap();
    std::fs::create_dir_all(local_dir.join("share")).unwrap();
    let local_canon = local_dir.canonicalize().unwrap();

    let approver = Arc::new(RecordingApprover::new(Approval::Denied));
    set_write_scope(Some(WriteScope {
        allowed: vec![local_canon],
        edit_worktrees: None,
    }));
    set_approval(Some(ApprovalConfig {
        rules: Vec::new(),
        approver: approver.clone(),
    }));

    let shell = LocalShell::new();
    let fs = LocalFs;

    let refused = execute(
        home.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": ".local/state/blyte/x", "content": "nope\n" }),
    );
    assert!(refused.is_error, "{}", refused.content);
    assert!(
        refused
            .content
            .contains("inside blyte's own state or configuration"),
        "{}",
        refused.content
    );
    assert!(!home.path().join(".local/state/blyte/x").exists());
    let calls = approver.take_calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].1, "inside blyte's own state or configuration");

    let allowed = execute(
        home.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": ".local/share/x", "content": "ok\n" }),
    );
    assert!(!allowed.is_error, "{}", allowed.content);
    assert_eq!(
        std::fs::read_to_string(home.path().join(".local/share/x")).unwrap(),
        "ok\n"
    );
    assert!(approver.take_calls().is_empty());
}

#[test]
fn write_into_blyte_config_asks_for_approval() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    let config_dir = home.path().join(".config");
    std::fs::create_dir_all(config_dir.join("blyte")).unwrap();
    std::fs::create_dir_all(config_dir.join("other")).unwrap();
    let config_canon = config_dir.canonicalize().unwrap();

    let approver = Arc::new(RecordingApprover::new(Approval::Denied));
    set_write_scope(Some(WriteScope {
        allowed: vec![config_canon],
        edit_worktrees: None,
    }));
    set_approval(Some(ApprovalConfig {
        rules: Vec::new(),
        approver: approver.clone(),
    }));

    let shell = LocalShell::new();
    let fs = LocalFs;

    let refused = execute(
        home.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": ".config/blyte/config.toml", "content": "nope\n" }),
    );
    assert!(refused.is_error, "{}", refused.content);
    assert!(
        refused
            .content
            .contains("inside blyte's own state or configuration"),
        "{}",
        refused.content
    );
    assert!(!home.path().join(".config/blyte/config.toml").exists());
    let calls = approver.take_calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].1, "inside blyte's own state or configuration");

    let allowed = execute(
        home.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": ".config/other/ok.toml", "content": "ok\n" }),
    );
    assert!(!allowed.is_error, "{}", allowed.content);
    assert!(approver.take_calls().is_empty());
}

#[test]
fn protected_read_paths_without_home_keeps_non_home_entries_only() {
    let xdg = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(None, Some(xdg.path()), None);

    let paths = protected_read_paths().unwrap();
    let expected = xdg.path().canonicalize().unwrap().join("blyte/surface");
    assert_eq!(paths, vec![expected.clone()]);
    assert!(is_protected_read(&expected.join("abc.token")).unwrap());
    assert!(!is_protected_read(&xdg.path().join("other.txt")).unwrap());
}

#[test]
fn write_into_custom_blyte_state_dir_asks_for_approval() {
    let home = tempfile::tempdir().unwrap();
    let custom_state = home.path().join("custom_state");
    let xdg_state = home.path().join("xdg_state");
    std::fs::create_dir_all(&custom_state).unwrap();
    std::fs::create_dir_all(xdg_state.join("blyte")).unwrap();
    let _guard = EnvGuard::set(Some(home.path()), Some(&xdg_state), Some(&custom_state));

    let home_canon = home.path().canonicalize().unwrap();
    let approver = Arc::new(RecordingApprover::new(Approval::Denied));
    set_write_scope(Some(WriteScope {
        allowed: vec![home_canon],
        edit_worktrees: None,
    }));
    set_approval(Some(ApprovalConfig {
        rules: Vec::new(),
        approver: approver.clone(),
    }));

    let shell = LocalShell::new();
    let fs = LocalFs;

    let refused_custom = execute(
        home.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": "custom_state/x", "content": "nope\n" }),
    );
    assert!(refused_custom.is_error, "{}", refused_custom.content);
    let calls = approver.take_calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].1, "inside blyte's own state or configuration");

    let refused_xdg = execute(
        home.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": "xdg_state/blyte/y", "content": "nope\n" }),
    );
    assert!(refused_xdg.is_error, "{}", refused_xdg.content);
    let calls = approver.take_calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].1, "inside blyte's own state or configuration");
}
