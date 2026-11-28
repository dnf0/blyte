#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::sandbox::{available, default_sandbox_net};
use blyte_agent::shell::{LocalShell, Shell};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvVarGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvVarGuard {
    fn remove(key: &'static str) -> Self {
        let prev = std::env::var_os(key);
        std::env::remove_var(key);
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

struct EnvGuard {
    home: Option<OsString>,
    xdg_state: Option<OsString>,
}

impl EnvGuard {
    fn set(home: Option<&Path>, xdg_state: Option<&Path>) -> Self {
        let prev = Self {
            home: std::env::var_os("HOME"),
            xdg_state: std::env::var_os("XDG_STATE_HOME"),
        };
        match home {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        match xdg_state {
            Some(p) => std::env::set_var("XDG_STATE_HOME", p),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
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
    }
}

struct RemoveDirGuard(PathBuf);

impl Drop for RemoveDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct RemoveFileGuard(PathBuf);

impl Drop for RemoveFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn landlock_or_skip() -> bool {
    if available() {
        return true;
    }
    assert!(
        std::env::var_os("CI").is_none(),
        "Landlock unavailable on CI"
    );
    false
}

#[test]
fn home_under_var_tmp_is_protected_and_var_stays_readable() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !landlock_or_skip() {
        return;
    }

    let fake_home = tempfile::Builder::new()
        .prefix("blyte-var-home-")
        .tempdir_in("/var/tmp")
        .unwrap();
    let home_name = fake_home.path().file_name().unwrap().to_str().unwrap();
    let sibling = PathBuf::from("/var/tmp").join(format!("{home_name}-sibling.txt"));
    std::fs::write(&sibling, "sibling-ok\n").unwrap();
    let _sibling_cleanup = RemoveFileGuard(sibling.clone());

    let repo = fake_home.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());

    let aws_cred = fake_home.path().join(".aws/credentials");
    std::fs::create_dir_all(aws_cred.parent().unwrap()).unwrap();
    std::fs::write(&aws_cred, "SECRET-VAR-HOME\n").unwrap();

    let _guard = EnvGuard::set(Some(fake_home.path()), None);
    let _tmpdir = EnvVarGuard::remove("TMPDIR");
    let _blyte_state = EnvVarGuard::remove("BLYTE_STATE_DIR");

    let sb = default_sandbox_net(&repo, &[], true).unwrap();
    let _tmp_cleanup = sb.tmpdir.clone().map(RemoveDirGuard);

    let sh = LocalShell::sandboxed(sb.profile).with_tmpdir(sb.tmpdir);

    let cred = sh
        .exec(
            &format!("cat '{}'", aws_cred.display()),
            &repo,
            Duration::from_secs(30),
        )
        .unwrap();
    assert_ne!(cred.exit_code, 0, "{}", cred.merged());
    assert!(
        !cred.merged().contains("SECRET-VAR-HOME"),
        "{}",
        cred.merged()
    );

    let sib = sh
        .exec(
            &format!("cat '{}'", sibling.display()),
            &repo,
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(sib.exit_code, 0, "{}", sib.merged());
    assert_eq!(sib.stdout.trim(), "sibling-ok", "{}", sib.merged());
}
