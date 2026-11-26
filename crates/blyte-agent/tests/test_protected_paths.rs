use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use blyte_agent::approval::{Approval, ApprovalConfig, Approver};
use blyte_agent::fs::{Fs, LocalFs};
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

#[cfg(unix)]
#[test]
fn glob_and_grep_tolerate_dangling_symlinks() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    std::fs::write(home.path().join("a.txt"), "NEEDLE_VALUE\n").unwrap();
    std::os::unix::fs::symlink(
        "/nonexistent_blyte_target/nowhere",
        home.path().join("broken_link"),
    )
    .unwrap();
    std::os::unix::fs::symlink("user@host.12345", home.path().join(".#lock")).unwrap();
    std::os::unix::fs::symlink(
        home.path().join(".ssh/missing_key"),
        home.path().join("broken_ssh"),
    )
    .unwrap();

    assert!(!is_protected_read(&home.path().join("broken_link")).unwrap());
    assert!(!is_protected_read(&home.path().join(".#lock")).unwrap());
    assert!(is_protected_read(&home.path().join("broken_ssh")).unwrap());

    let shell = LocalShell::new();
    let fs = LocalFs;

    let glob_out = execute(
        home.path(),
        &shell,
        &fs,
        "glob",
        &json!({ "pattern": "**/*" }),
    );
    assert!(!glob_out.is_error, "{}", glob_out.content);
    assert!(glob_out.content.contains("a.txt"), "{}", glob_out.content);
    assert!(
        !glob_out.content.contains("broken_ssh"),
        "{}",
        glob_out.content
    );
    assert!(
        glob_out.content.contains("(credential paths skipped: 1)"),
        "{}",
        glob_out.content
    );

    let grep_out = execute(
        home.path(),
        &shell,
        &fs,
        "grep",
        &json!({ "pattern": "NEEDLE_VALUE" }),
    );
    assert!(!grep_out.is_error, "{}", grep_out.content);
    assert!(grep_out.content.contains("a.txt"), "{}", grep_out.content);
    assert!(
        grep_out.content.contains("(credential paths skipped: 1)"),
        "{}",
        grep_out.content
    );
}

struct FakeRemoteFs {
    files: HashMap<PathBuf, String>,
}

impl Fs for FakeRemoteFs {
    fn read(&self, path: &Path) -> Result<String, String> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| format!("missing remote file {}", path.display()))
    }

    fn write(&self, _path: &Path, _content: &str) -> Result<(), String> {
        Err("read-only".to_string())
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf, String> {
        Ok(path.to_path_buf())
    }
}

#[test]
fn read_with_non_local_fs_does_not_check_host_paths() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(home.path().join(".ssh/id_ed25519"), "HOST_SECRET\n").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(home.path().join("loop_b"), home.path().join("loop_a")).unwrap();
        std::os::unix::fs::symlink(home.path().join("loop_a"), home.path().join("loop_b")).unwrap();
    }

    let mut files = HashMap::new();
    files.insert(
        home.path().join(".ssh/id_ed25519"),
        "container ssh content\n".to_string(),
    );
    files.insert(
        home.path().join("loop_a"),
        "container loop content\n".to_string(),
    );
    files.insert(
        PathBuf::from("/container-only/app.txt"),
        "container app\n".to_string(),
    );
    let remote_fs = FakeRemoteFs { files };
    let shell = LocalShell::new();

    let out_ssh = execute(
        home.path(),
        &shell,
        &remote_fs,
        "read",
        &json!({ "path": ".ssh/id_ed25519" }),
    );
    assert!(!out_ssh.is_error, "{}", out_ssh.content);
    assert!(
        out_ssh.content.contains("container ssh content"),
        "{}",
        out_ssh.content
    );

    #[cfg(unix)]
    {
        let out_loop = execute(
            home.path(),
            &shell,
            &remote_fs,
            "read",
            &json!({ "path": "loop_a" }),
        );
        assert!(!out_loop.is_error, "{}", out_loop.content);
        assert!(
            out_loop.content.contains("container loop content"),
            "{}",
            out_loop.content
        );
    }

    let out_virtual = execute(
        Path::new("/container-only"),
        &shell,
        &remote_fs,
        "read",
        &json!({ "path": "app.txt" }),
    );
    assert!(!out_virtual.is_error, "{}", out_virtual.content);
    assert!(
        out_virtual.content.contains("container app"),
        "{}",
        out_virtual.content
    );
}

#[cfg(unix)]
#[test]
fn dot_env_symlink_to_another_filename_is_protected() {
    let home = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    std::fs::write(ws.path().join(".env.production"), "PROD_SECRET=123\n").unwrap();
    std::fs::write(ws.path().join("notes.txt"), "PROD_SECRET=123\n").unwrap();
    std::os::unix::fs::symlink(ws.path().join(".env.production"), ws.path().join(".env")).unwrap();

    assert!(is_protected_read(&ws.path().join(".env")).unwrap());

    let shell = LocalShell::new();
    let fs = LocalFs;

    let read_refused = execute(ws.path(), &shell, &fs, "read", &json!({ "path": ".env" }));
    assert!(read_refused.is_error, "{}", read_refused.content);
    assert!(
        read_refused.content.contains("read was not made:"),
        "{}",
        read_refused.content
    );
    assert!(
        !read_refused.content.contains("PROD_SECRET"),
        "{}",
        read_refused.content
    );

    let grep_root_refused = execute(
        ws.path(),
        &shell,
        &fs,
        "grep",
        &json!({ "pattern": "PROD_SECRET", "path": ".env" }),
    );
    assert!(grep_root_refused.is_error, "{}", grep_root_refused.content);
    assert!(
        grep_root_refused.content.contains("grep was not made:"),
        "{}",
        grep_root_refused.content
    );

    let glob_out = execute(
        ws.path(),
        &shell,
        &fs,
        "glob",
        &json!({ "pattern": "**/*" }),
    );
    assert!(!glob_out.is_error, "{}", glob_out.content);
    assert!(
        glob_out.content.contains("notes.txt"),
        "{}",
        glob_out.content
    );
    assert!(
        !glob_out.content.lines().any(|l| l.trim() == ".env"),
        "{}",
        glob_out.content
    );
    assert!(
        glob_out.content.contains("(credential paths skipped: 1)"),
        "{}",
        glob_out.content
    );

    let grep_out = execute(
        ws.path(),
        &shell,
        &fs,
        "grep",
        &json!({ "pattern": "PROD_SECRET" }),
    );
    assert!(!grep_out.is_error, "{}", grep_out.content);
    assert!(
        grep_out.content.contains("notes.txt"),
        "{}",
        grep_out.content
    );
    assert!(
        !grep_out.content.contains("./.env:"),
        "{}",
        grep_out.content
    );
    assert!(
        grep_out.content.contains("(credential paths skipped: 1)"),
        "{}",
        grep_out.content
    );
}

#[test]
fn a_regular_file_where_a_credential_folder_would_be_does_not_break_reads() {
    // `~/.docker` as a regular file makes `~/.docker/config.json` "not a directory"; that must
    // count as a path that does not exist, not as an error that refuses every read.
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join(".docker"), "x").unwrap();
    std::fs::write(home.path().join("notes.txt"), "hello").unwrap();
    let old = std::env::var_os("HOME");
    std::env::set_var("HOME", home.path());
    let got = blyte_agent::sandbox::is_protected_read(&home.path().join("notes.txt"));
    match old {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
    assert_eq!(got, Ok(false));
}
