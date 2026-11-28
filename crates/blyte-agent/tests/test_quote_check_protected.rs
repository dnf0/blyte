use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use blyte_agent::fs::{Fs, LocalFs};
use blyte_agent::quote_expand;
use blyte_agent::run::read_unless_protected;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct HomeGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    home: Option<OsString>,
}

impl HomeGuard {
    fn set(home: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = Self {
            _lock: lock,
            home: std::env::var_os("HOME"),
        };
        std::env::set_var("HOME", home);
        prev
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match &self.home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
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
fn read_unless_protected_refuses_ssh_file() {
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::set(home.path());

    let ssh_dir = home.path().join(".ssh");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    let secret_path = ssh_dir.join("id_ed25519");
    let secret = "PRIVATE_KEY_SECRET_CONTENT\n";
    std::fs::write(&secret_path, secret).unwrap();

    let err = read_unless_protected(&LocalFs, &secret_path).unwrap_err();
    assert!(err.contains("is a protected path"), "{err}");
    assert!(!err.contains("PRIVATE_KEY_SECRET_CONTENT"), "{err}");
}

#[test]
fn read_unless_protected_reads_ordinary_file() {
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::set(home.path());

    let notes_path = home.path().join("notes.txt");
    let body = "ordinary file contents\n";
    std::fs::write(&notes_path, body).unwrap();

    let got = read_unless_protected(&LocalFs, &notes_path).unwrap();
    assert_eq!(got, body);
}

#[test]
fn quote_expand_does_not_paste_protected_file() {
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::set(home.path());

    let ssh_dir = home.path().join(".ssh");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    let secret_path = ssh_dir.join("id_ed25519");
    std::fs::write(&secret_path, "PRIVATE_KEY_SECRET_CONTENT\n").unwrap();

    let report = format!("## Findings\n<<quote {}:1>>\n", secret_path.display());
    let reader = |p: &Path| read_unless_protected(&LocalFs, p);
    let (expanded, problems) = quote_expand::expand(&report, &reader);

    assert!(
        !expanded.contains("PRIVATE_KEY_SECRET_CONTENT"),
        "{expanded}"
    );
    assert_eq!(problems.len(), 1, "{problems:?}");
    let path_str = secret_path.display().to_string();
    assert!(problems[0].contains(&path_str), "{}", problems[0]);
    assert!(
        problems[0].contains("is a protected path"),
        "{}",
        problems[0]
    );
    assert!(
        !problems[0].contains("PRIVATE_KEY_SECRET_CONTENT"),
        "{}",
        problems[0]
    );
}

#[test]
fn read_unless_protected_skips_host_check_for_non_local_fs() {
    let home = tempfile::tempdir().unwrap();
    let _guard = HomeGuard::set(home.path());

    let ssh_dir = home.path().join(".ssh");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    let secret_path = ssh_dir.join("id_ed25519");
    std::fs::write(&secret_path, "HOST_SECRET\n").unwrap();

    let mut files = HashMap::new();
    files.insert(secret_path.clone(), "remote content\n".to_string());
    let remote = FakeRemoteFs { files };

    let got = read_unless_protected(&remote, &secret_path).unwrap();
    assert_eq!(got, "remote content\n");
}
