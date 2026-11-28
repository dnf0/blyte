use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{execute, set_approval, set_write_scope};
use blyte_agent::write_guard::protected_write;
use serde_json::json;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
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
        set_write_scope(None);
        set_approval(None);
        prev
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        set_write_scope(None);
        set_approval(None);
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

#[test]
fn each_protected_kind_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().canonicalize().unwrap();
    let state_dir = home_path.join(".local/state/blyte");

    let cases: &[(&str, &str)] = &[
        (".ssh/id_ed25519", "a credential store"),
        ("project/.env", "an environment file"),
        (".zshrc", "a shell start-up file"),
        (".config/fish/config.fish", "a shell start-up file"),
        (".config/blyte/config.toml", "blyte's own files"),
        (".local/state/blyte/ledger.json", "blyte's own files"),
        (".gitconfig", "git configuration"),
        (".config/git/ignore", "git configuration"),
        (
            "Library/LaunchAgents/com.example.plist",
            "a place that runs programs at login",
        ),
        (
            ".config/systemd/user/app.service",
            "a place that runs programs at login",
        ),
        (
            ".config/autostart/app.desktop",
            "a place that runs programs at login",
        ),
        (".cargo/bin/tool", "a folder of programs on PATH"),
        (".local/bin/tool", "a folder of programs on PATH"),
        ("go/bin/tool", "a folder of programs on PATH"),
        (".bun/bin/tool", "a folder of programs on PATH"),
        (".deno/bin/tool", "a folder of programs on PATH"),
        (".npmrc", "a credential store"),
        (".pgpass", "a credential store"),
        (".my.cnf", "a credential store"),
        (".vault-token", "a credential store"),
        (".claude/settings.json", "a credential store"),
    ];

    for (rel, expected_reason) in cases {
        let target = home_path.join(rel);
        assert_eq!(
            protected_write(&target, &home_path, Some(&state_dir)),
            Some(*expected_reason),
            "target {rel}"
        );
    }
}

#[test]
fn a_file_under_a_protected_folder_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().canonicalize().unwrap();

    assert_eq!(
        protected_write(&home_path.join(".ssh/config"), &home_path, None),
        Some("a credential store")
    );
    assert_eq!(
        protected_write(
            &home_path.join(".config/blyte/allow-unsandboxed"),
            &home_path,
            None
        ),
        Some("blyte's own files")
    );
}

#[test]
fn a_similar_name_is_not_protected() {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().canonicalize().unwrap();
    let state_dir = home_path.join(".local/state/blyte");

    for rel in [".sshx/a", ".envrc", "notes/.bashrc.txt", "bin/tool"] {
        assert_eq!(
            protected_write(&home_path.join(rel), &home_path, Some(&state_dir)),
            None,
            "target {rel} must not be protected"
        );
    }
}

#[test]
fn the_state_directory_is_protected_when_given_and_not_when_absent() {
    let home = tempfile::tempdir().unwrap();
    let home_path = home.path().canonicalize().unwrap();
    let state_dir = home_path.join("custom-state");
    let target = state_dir.join("runs/r1.json");

    assert_eq!(
        protected_write(&target, &home_path, Some(&state_dir)),
        Some("blyte's own files")
    );
    assert_eq!(protected_write(&target, &home_path, None), None);
}

#[test]
fn write_tool_refuses_a_protected_path_in_a_background_run() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    let shell = LocalShell::new();
    let fs = LocalFs;
    let out = execute(
        home.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": ".zshrc", "content": "export PWN=1\n" }),
    );
    assert!(out.is_error, "{}", out.content);
    assert_eq!(
        out.content,
        "write: .zshrc is protected (a shell start-up file); blyte does not write there"
    );
    assert!(!home.path().join(".zshrc").exists());
}

#[test]
fn edit_tool_refuses_a_protected_path() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    let gitconfig = home.path().join(".gitconfig");
    let original = "[user]\n\tname = Original\n";
    std::fs::write(&gitconfig, original).unwrap();

    let shell = LocalShell::new();
    let fs = LocalFs;
    let out = execute(
        home.path(),
        &shell,
        &fs,
        "edit",
        &json!({ "path": ".gitconfig", "old": "Original", "new": "Changed" }),
    );
    assert!(out.is_error, "{}", out.content);
    assert_eq!(
        out.content,
        "edit: .gitconfig is protected (git configuration); blyte does not write there"
    );
    assert_eq!(std::fs::read_to_string(&gitconfig).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn a_symlink_to_a_protected_path_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    let ssh_dir = home.path().join(".ssh");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    let target = ssh_dir.join("authorized_keys");
    let original = "ssh-ed25519 AAAA original\n";
    std::fs::write(&target, original).unwrap();
    std::os::unix::fs::symlink(".ssh/authorized_keys", home.path().join("link")).unwrap();

    let shell = LocalShell::new();
    let fs = LocalFs;
    let out = execute(
        home.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": "link", "content": "ssh-ed25519 BBBB evil\n" }),
    );
    assert!(out.is_error, "{}", out.content);
    assert_eq!(
        out.content,
        "write: link is protected (a credential store); blyte does not write there"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), original);
}

#[test]
fn an_ordinary_file_is_still_written() {
    let home = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    let shell = LocalShell::new();
    let fs = LocalFs;
    let out = execute(
        home.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": "notes.txt", "content": "ordinary\n" }),
    );
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(
        std::fs::read_to_string(home.path().join("notes.txt")).unwrap(),
        "ordinary\n"
    );
}

#[test]
fn a_dotenv_in_a_repository_worktree_is_refused() {
    let home = tempfile::tempdir().unwrap();
    let worktree = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, None);

    let shell = LocalShell::new();
    let fs = LocalFs;
    let out = execute(
        worktree.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": ".env", "content": "SECRET=1\n" }),
    );
    assert!(out.is_error, "{}", out.content);
    assert_eq!(
        out.content,
        "write: .env is protected (an environment file); blyte does not write there"
    );
    assert!(!worktree.path().join(".env").exists());
}

#[test]
fn without_home_the_write_is_an_error_naming_home() {
    let ws = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(None, None, None);

    let shell = LocalShell::new();
    let fs = LocalFs;
    let unset = execute(
        ws.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": "notes.txt", "content": "nope\n" }),
    );
    assert!(unset.is_error, "{}", unset.content);
    assert!(unset.content.contains("HOME"), "{}", unset.content);
    assert!(!ws.path().join("notes.txt").exists());

    std::env::set_var("HOME", "");
    let empty = execute(
        ws.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": "notes.txt", "content": "nope\n" }),
    );
    assert!(empty.is_error, "{}", empty.content);
    assert!(empty.content.contains("HOME"), "{}", empty.content);
    assert!(!ws.path().join("notes.txt").exists());
}

#[test]
fn write_tool_refuses_blyte_state_dir_in_a_background_run() {
    let home = tempfile::tempdir().unwrap();
    let custom_state = home.path().join("custom_state");
    std::fs::create_dir_all(&custom_state).unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, Some(&custom_state));

    let shell = LocalShell::new();
    let fs = LocalFs;
    let out = execute(
        home.path(),
        &shell,
        &fs,
        "write",
        &json!({ "path": "custom_state/x", "content": "nope\n" }),
    );
    assert!(out.is_error, "{}", out.content);
    assert_eq!(
        out.content,
        "write: custom_state/x is protected (blyte's own files); blyte does not write there"
    );
    assert!(!custom_state.join("x").exists());
}

#[test]
fn run_scratch_workspace_allows_report_write_but_keeps_state_dir_and_dotenv_protected() {
    let home = tempfile::tempdir().unwrap();
    let custom_state = home.path().join("custom_state");
    let scratch = custom_state
        .join("folders")
        .join("0123456789abcdef")
        .join("runs")
        .join("R1.scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, Some(&custom_state));

    let shell = LocalShell::new();
    let fs = LocalFs;

    let ok_write = execute(
        &scratch,
        &shell,
        &fs,
        "write",
        &json!({ "path": "report.md", "content": "# Findings\n" }),
    );
    assert!(!ok_write.is_error, "{}", ok_write.content);
    assert_eq!(
        std::fs::read_to_string(scratch.join("report.md")).unwrap(),
        "# Findings\n"
    );

    let dotenv = execute(
        &scratch,
        &shell,
        &fs,
        "write",
        &json!({ "path": ".env", "content": "SECRET=1\n" }),
    );
    assert!(dotenv.is_error, "{}", dotenv.content);
    assert!(!scratch.join(".env").exists());

    let sibling = execute(
        &scratch,
        &shell,
        &fs,
        "write",
        &json!({ "path": "../R1.json", "content": "{}" }),
    );
    assert!(sibling.is_error, "{}", sibling.content);
    assert!(!scratch.parent().unwrap().join("R1.json").exists());

    let runs_dir = scratch.parent().unwrap();
    let non_scratch = execute(
        runs_dir,
        &shell,
        &fs,
        "write",
        &json!({ "path": "R1.json", "content": "{}" }),
    );
    assert!(non_scratch.is_error, "{}", non_scratch.content);
    assert!(!runs_dir.join("R1.json").exists());

    #[cfg(target_os = "macos")]
    {
        let scratch_canon = scratch.canonicalize().unwrap();
        let state_canon = custom_state.canonicalize().unwrap();
        let blyte_agent::sandbox::Profile::Seatbelt(profile) =
            blyte_agent::sandbox::default_sandbox(&scratch, &[])
                .expect("sandbox")
                .profile
        else {
            panic!("expected Seatbelt profile on macOS");
        };
        let deny_rule = format!(
            r#"(deny file-write* (subpath "{}"))"#,
            state_canon.display()
        );
        let allow_rule = format!(
            r#"(allow file-write* (subpath "{}"))"#,
            scratch_canon.display()
        );
        let deny_idx = profile
            .find(&deny_rule)
            .expect("state dir deny rule present");
        let allow_idx = profile
            .rfind(&allow_rule)
            .expect("scratch dir allow rule present");
        assert!(
            allow_idx > deny_idx,
            "scratch allow rule must follow state dir deny rule in profile: {profile}"
        );

        let blyte_agent::sandbox::Profile::Seatbelt(runs_profile) =
            blyte_agent::sandbox::default_sandbox(runs_dir, &[])
                .expect("sandbox")
                .profile
        else {
            panic!("expected Seatbelt profile on macOS");
        };
        let runs_canon = runs_dir.canonicalize().unwrap();
        let runs_deny_idx = runs_profile
            .find(&deny_rule)
            .expect("state dir deny rule present");
        let runs_allow_rule = format!(
            r#"(allow file-write* (subpath "{}"))"#,
            runs_canon.display()
        );
        assert!(
            runs_profile[runs_deny_idx..]
                .find(&runs_allow_rule)
                .is_none(),
            "non-scratch runs dir must not be re-allowed after state dir deny"
        );
    }
}

#[test]
fn is_run_scratch_workspace_matches_only_named_scratch_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let state_dir = tmp.path().join("state");
    let runs = state_dir
        .join("folders")
        .join("0123456789abcdef")
        .join("runs");

    for rejected in [
        runs.join("T1"),
        runs.join("T1.before"),
        runs.join("T1.after"),
        runs.join(".scratch"),
        runs.join("T1.scratch").join("sub"),
    ] {
        assert!(
            !blyte_agent::sandbox::is_run_scratch_workspace(&rejected, &state_dir),
            "expected false for {}",
            rejected.display()
        );
    }

    for accepted in [runs.join("T1.scratch"), runs.join("T1.before.scratch")] {
        assert!(
            blyte_agent::sandbox::is_run_scratch_workspace(&accepted, &state_dir),
            "expected true for {}",
            accepted.display()
        );
    }
}

#[test]
fn write_tool_refuses_write_from_run_rooted_at_before_dir() {
    let home = tempfile::tempdir().unwrap();
    let custom_state = home.path().join("custom_state");
    let before_dir = custom_state
        .join("folders")
        .join("0123456789abcdef")
        .join("runs")
        .join("T1.before");
    std::fs::create_dir_all(&before_dir).unwrap();
    std::fs::write(before_dir.join("a.txt"), "original\n").unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None, Some(&custom_state));

    let shell = LocalShell::new();
    let fs = LocalFs;
    let out = execute(
        &before_dir,
        &shell,
        &fs,
        "write",
        &json!({ "path": "a.txt", "content": "overwritten\n" }),
    );
    assert!(out.is_error, "{}", out.content);
    assert!(
        out.content.contains("blyte's own files"),
        "expected state dir refusal, got: {}",
        out.content
    );
    assert_eq!(
        std::fs::read_to_string(before_dir.join("a.txt")).unwrap(),
        "original\n"
    );
}
