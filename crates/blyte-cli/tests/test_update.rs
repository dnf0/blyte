//! `blyte update` replaces the binary with the latest release, keeps the old one as blyte.prev,
//! rolls back, and refreshes an unedited skill (2026-09-30). A stand-in HTTP server serves the
//! release metadata and asset, and the tests run a copy of blyte, so the real binary is never
//! replaced.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use blyte_cli::skill::SKILL_MD;
use blyte_cli::update::{asset_name, parse_version, repo};
use sha2::{Digest, Sha256};

static EXEC_LOCK: RwLock<()> = RwLock::new(());

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex
}

struct MockServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    download_requests: Arc<AtomicUsize>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockServer {
    fn start(tag: &str, asset_bytes: Vec<u8>) -> Self {
        let asset = asset_name().expect("test platform has a release asset");
        let checksum = format!("{}  {asset}\n", sha256_bytes(&asset_bytes));
        Self::start_with_checksum(tag, asset_bytes, Some(checksum))
    }

    fn start_with_checksum(tag: &str, asset_bytes: Vec<u8>, checksum: Option<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let download_requests = Arc::new(AtomicUsize::new(0));

        let repo_name = repo();
        let asset = asset_name().expect("test platform has a release asset");
        let latest_path = format!("/repos/{repo_name}/releases/latest");
        let download_path = format!("/{repo_name}/releases/download/{tag}/{asset}");
        let checksum_path = format!("/{repo_name}/releases/download/{tag}/{asset}.sha256");
        let tag_owned = tag.to_string();
        let stop_flag = Arc::clone(&stop);
        let dl_count = Arc::clone(&download_requests);

        let handle = std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                if stop_flag.load(Ordering::SeqCst) {
                    break;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let Ok(cloned) = stream.try_clone() else {
                    continue;
                };
                let mut reader = BufReader::new(cloned);
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() || request_line.is_empty() {
                    continue;
                }
                loop {
                    let mut header = String::new();
                    match reader.read_line(&mut header) {
                        Ok(0) | Err(_) => break,
                        Ok(_) if header == "\r\n" || header == "\n" => break,
                        Ok(_) => {}
                    }
                }
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or("");
                let path = parts.next().unwrap_or("");
                if method == "GET" && path == latest_path {
                    let body = format!(r#"{{"tag_name":"{tag_owned}"}}"#);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                } else if method == "GET" && path == download_path {
                    dl_count.fetch_add(1, Ordering::SeqCst);
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        asset_bytes.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(&asset_bytes);
                } else if method == "GET" && path == checksum_path {
                    if let Some(ref body) = checksum {
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    } else {
                        let body = b"not found";
                        let header = format!(
                            "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(header.as_bytes());
                        let _ = stream.write_all(body);
                    }
                } else {
                    let body = b"not found";
                    let header = format!(
                        "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(body);
                }
                let _ = stream.flush();
            }
        });

        Self {
            addr,
            stop,
            download_requests,
            handle: Some(handle),
        }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn download_requests(&self) -> usize {
        self.download_requests.load(Ordering::SeqCst)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct Env {
    _dir: tempfile::TempDir,
    bin: PathBuf,
    server: MockServer,
    cargo_home: PathBuf,
    home: PathBuf,
    log: PathBuf,
}

fn exe(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn spawn_output(cmd: &mut Command) -> Output {
    let child = {
        let _guard = EXEC_LOCK.read().unwrap_or_else(|e| e.into_inner());
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    child.wait_with_output().unwrap()
}

fn setup_with_checksum(tag: &str, broken: bool, checksum: Option<Option<String>>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin/blyte");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    let log = dir.path().join("calls.log");
    let release_body = if broken {
        "#!/bin/sh\nexit 1\n".to_string()
    } else {
        format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'blyte 9.9.9 (test)'; exit 0; fi\necho \"$@\" >> \"{}\"\n",
            log.display()
        )
    };
    {
        let _guard = EXEC_LOCK.write().unwrap_or_else(|e| e.into_inner());
        std::fs::copy(env!("CARGO_BIN_EXE_blyte"), &bin).unwrap();
    }
    let server = match checksum {
        None => MockServer::start(tag, release_body.into_bytes()),
        Some(c) => MockServer::start_with_checksum(tag, release_body.into_bytes(), c),
    };
    let cargo_home = dir.path().join("cargo-home");
    std::fs::create_dir_all(&cargo_home).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    Env {
        _dir: dir,
        bin,
        server,
        cargo_home,
        home,
        log,
    }
}

/// A copy of blyte in a temp dir, a stand-in HTTP server serving `tag` and a release binary that
/// prints `new_version` (or fails when `broken`), an empty CARGO_HOME, and an empty HOME.
fn setup(tag: &str, broken: bool) -> Env {
    setup_with_checksum(tag, broken, None)
}

fn run(e: &Env, args: &[&str]) -> (i32, String) {
    let url = e.server.url();
    let mut cmd = Command::new(&e.bin);
    cmd.arg("update")
        .args(args)
        .env("BLYTE_UPDATE_API", &url)
        .env("BLYTE_UPDATE_DOWNLOAD", &url)
        .env("CARGO_HOME", &e.cargo_home)
        .env("HOME", &e.home)
        .env("XDG_CONFIG_HOME", &e.home)
        .env("BLYTE_ENV_FILE", "");
    let out = spawn_output(&mut cmd);
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (out.status.code().unwrap_or(-1), text)
}

fn version(bin: &Path) -> String {
    let mut cmd = Command::new(bin);
    cmd.arg("--version");
    String::from_utf8_lossy(&spawn_output(&mut cmd).stdout).into_owned()
}

#[test]
fn versions_parse_and_this_machine_has_an_asset() {
    assert_eq!(parse_version("v0.2.1"), Some((0, 2, 1)));
    assert_eq!(parse_version("0.10.3"), Some((0, 10, 3)));
    assert_eq!(parse_version("v1.2"), None);
    assert!(
        asset_name().is_some(),
        "the test machines are macOS or Linux x86_64"
    );
}

#[test]
fn check_reports_and_changes_nothing() {
    let e = setup("v9.9.9", false);
    let before = std::fs::read(&e.bin).unwrap();
    let (code, text) = run(&e, &["--check"]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("latest:    blyte 9.9.9"), "{text}");
    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
}

#[test]
fn update_replaces_the_binary_keeps_the_old_one_and_rolls_back() {
    let e = setup("v9.9.9", false);
    let before = std::fs::read(&e.bin).unwrap();
    let (code, text) = run(&e, &[]);
    assert_eq!(code, 0, "{text}");
    assert!(version(&e.bin).contains("9.9.9"));
    let prev = e.bin.with_file_name("blyte.prev");
    assert_eq!(std::fs::read(&prev).unwrap(), before);

    // Roll back with the previous build (the real blyte), which is now blyte.prev.
    let mut rollback_cmd = Command::new(&prev);
    rollback_cmd
        .args(["update", "--rollback"])
        .env("HOME", &e.home)
        .env("XDG_CONFIG_HOME", &e.home)
        .env("BLYTE_ENV_FILE", "");
    let out = spawn_output(&mut rollback_cmd);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
}

#[test]
fn an_up_to_date_install_is_left_alone() {
    let e = setup(&format!("v{}", env!("CARGO_PKG_VERSION")), false);
    let before = std::fs::read(&e.bin).unwrap();
    let (code, text) = run(&e, &[]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("up to date"), "{text}");
    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
}

#[test]
fn a_download_that_does_not_run_changes_nothing() {
    let e = setup("v9.9.9", true);
    let before = std::fs::read(&e.bin).unwrap();
    let (code, text) = run(&e, &[]);
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("does not run"), "{text}");
    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
    assert!(!e.bin.with_file_name("blyte.prev").exists());
}

#[test]
fn an_unedited_skill_is_refreshed_keeping_the_default_setting() {
    let e = setup("v9.9.9", false);
    let skill = e.home.join(".claude/skills/blyte-implementer/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, SKILL_MD).unwrap();
    let (code, text) = run(&e, &[]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("skill refreshed"), "{text}");
    // No default block in CLAUDE.md, so the refresh keeps "only when asked".
    assert_eq!(
        std::fs::read_to_string(&e.log).unwrap().trim(),
        "skill install --force --no-default"
    );
}

#[test]
fn an_edited_skill_is_not_touched() {
    let e = setup("v9.9.9", false);
    let skill = e.home.join(".claude/skills/blyte-implementer/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "my own edits\n").unwrap();
    let (code, text) = run(&e, &[]);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("has been edited"), "{text}");
    assert!(!e.log.exists());
}

#[test]
fn rollback_fails_when_restored_binary_exits_nonzero_on_version() {
    let e = setup("v9.9.9", false);
    let prev = e.bin.with_file_name("blyte.prev");
    {
        let _guard = EXEC_LOCK.write().unwrap_or_else(|e| e.into_inner());
        exe(&prev, "#!/bin/sh\necho 'broken binary' >&2\nexit 1\n");
    }
    let (code, text) = run(&e, &["--rollback"]);
    assert_ne!(code, 0, "expected non-zero exit, got 0: {text}");
    assert!(
        text.contains("could not be verified"),
        "expected verification failure message, got: {text}"
    );
    let canonical_bin = e.bin.canonicalize().unwrap_or_else(|_| e.bin.clone());
    assert!(
        text.contains(&canonical_bin.display().to_string()),
        "expected binary path {} in error: {text}",
        canonical_bin.display()
    );
}

#[test]
fn a_cargo_install_is_updated_with_cargo() {
    let dir = tempfile::tempdir().unwrap();
    let cargo_home = dir.path().join("cargo");
    let bin = cargo_home.join("bin/blyte");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(
        cargo_home.join(".crates2.json"),
        r#"{"installs":{"blyte 0.74.1 (sparse+https://index.crates.io/)":{}}}"#,
    )
    .unwrap();
    let args_file = dir.path().join("cargo-args.txt");
    let fake_cargo = dir.path().join("fake-cargo");
    {
        let _guard = EXEC_LOCK.write().unwrap_or_else(|e| e.into_inner());
        std::fs::copy(env!("CARGO_BIN_EXE_blyte"), &bin).unwrap();
        exe(
            &fake_cargo,
            &format!(
                "#!/bin/sh\necho \"$@\" > \"{}\"\nexit 0\n",
                args_file.display()
            ),
        );
    }
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let server = MockServer::start("v9.9.9", b"unused".to_vec());
    let url = server.url();

    let mut cmd = Command::new(&bin);
    cmd.arg("update")
        .env("BLYTE_UPDATE_API", &url)
        .env("BLYTE_UPDATE_DOWNLOAD", &url)
        .env("CARGO_HOME", &cargo_home)
        .env("BLYTE_CARGO", &fake_cargo)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("BLYTE_ENV_FILE", "");
    let out = spawn_output(&mut cmd);
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert_eq!(
        std::fs::read_to_string(&args_file).unwrap().trim(),
        "install blyte --locked"
    );
    assert_eq!(server.download_requests(), 0);
}

#[test]
fn a_failing_cargo_install_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let cargo_home = dir.path().join("cargo");
    let bin = cargo_home.join("bin/blyte");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(
        cargo_home.join(".crates2.json"),
        r#"{"installs":{"blyte 0.74.1 (registry+https://github.com/rust-lang/crates.io-index)":{}}}"#,
    )
    .unwrap();
    let fake_cargo = dir.path().join("fake-cargo");
    {
        let _guard = EXEC_LOCK.write().unwrap_or_else(|e| e.into_inner());
        std::fs::copy(env!("CARGO_BIN_EXE_blyte"), &bin).unwrap();
        exe(&fake_cargo, "#!/bin/sh\nexit 1\n");
    }
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let server = MockServer::start("v9.9.9", b"unused".to_vec());
    let url = server.url();

    let mut cmd = Command::new(&bin);
    cmd.arg("update")
        .env("BLYTE_UPDATE_API", &url)
        .env("BLYTE_UPDATE_DOWNLOAD", &url)
        .env("CARGO_HOME", &cargo_home)
        .env("BLYTE_CARGO", &fake_cargo)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("BLYTE_ENV_FILE", "");
    let out = spawn_output(&mut cmd);
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "{text}");
    assert!(
        text.contains("cargo install blyte --locked failed"),
        "{text}"
    );
}

#[test]
fn an_unreachable_release_api_is_an_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_addr = listener.local_addr().unwrap();
    drop(listener);

    let e = setup("v9.9.9", false);
    let closed_url = format!("http://{closed_addr}");
    let mut cmd = Command::new(&e.bin);
    cmd.arg("update")
        .env("BLYTE_UPDATE_API", &closed_url)
        .env("BLYTE_UPDATE_DOWNLOAD", &closed_url)
        .env("CARGO_HOME", &e.cargo_home)
        .env("HOME", &e.home)
        .env("XDG_CONFIG_HOME", &e.home)
        .env("BLYTE_ENV_FILE", "");
    let out = spawn_output(&mut cmd);
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("could not read the latest release"), "{text}");
}

#[test]
fn a_mismatched_checksum_refuses_the_download_without_running_it() {
    let wrong_digest = "0".repeat(64);
    let asset = asset_name().expect("test platform has a release asset");
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin/blyte");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    let ran_marker = dir.path().join("ran");
    let release_body = format!(
        "#!/bin/sh\ntouch \"{}\"\necho 'blyte 9.9.9'\n",
        ran_marker.display()
    );
    {
        let _guard = EXEC_LOCK.write().unwrap_or_else(|e| e.into_inner());
        std::fs::copy(env!("CARGO_BIN_EXE_blyte"), &bin).unwrap();
    }
    let actual_digest = sha256_bytes(release_body.as_bytes());
    let server = MockServer::start_with_checksum(
        "v9.9.9",
        release_body.into_bytes(),
        Some(format!("{wrong_digest}  {asset}\n")),
    );
    let cargo_home = dir.path().join("cargo-home");
    std::fs::create_dir_all(&cargo_home).unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let env = Env {
        _dir: dir,
        bin,
        server,
        cargo_home,
        home,
        log: PathBuf::new(),
    };

    let before = std::fs::read(&env.bin).unwrap();
    let (code, text) = run(&env, &[]);
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("unchanged"), "{text}");
    assert!(text.contains(asset), "{text}");
    assert!(text.contains(&wrong_digest), "{text}");
    assert!(text.contains(&actual_digest), "{text}");
    assert_eq!(std::fs::read(&env.bin).unwrap(), before);
    assert!(!env.bin.with_file_name("blyte.prev").exists());
    assert!(!ran_marker.exists());
    let leftovers: Vec<_> = std::fs::read_dir(env.bin.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".blyte-update-")
        })
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_missing_checksum_file_refuses_the_download() {
    let asset = asset_name().expect("test platform has a release asset");
    let env = setup_with_checksum("v9.9.9", false, Some(None));
    let before = std::fs::read(&env.bin).unwrap();
    let (code, text) = run(&env, &[]);
    assert_ne!(code, 0, "{text}");
    assert!(text.contains("unchanged"), "{text}");
    assert!(text.contains(&format!("{asset}.sha256")), "{text}");
    assert_eq!(std::fs::read(&env.bin).unwrap(), before);
    assert!(!env.bin.with_file_name("blyte.prev").exists());
    let leftovers: Vec<_> = std::fs::read_dir(env.bin.parent().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(".blyte-update-")
        })
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
