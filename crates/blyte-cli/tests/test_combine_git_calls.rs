use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use blyte_cli::combine::{CombineRecord, CombineState};

fn real_git_path() -> PathBuf {
    let path_var = std::env::var_os("PATH").expect("PATH is set");
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join("git");
        if let Ok(meta) = std::fs::metadata(&candidate) {
            if meta.is_file() && (meta.permissions().mode() & 0o111 != 0) {
                return candidate;
            }
        }
    }
    panic!("git binary not found on PATH");
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(real_git_path())
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed:\nstdout: {}\nstderr: {}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn fast_import(dir: &Path, stream: &str) {
    let mut child = Command::new(real_git_path())
        .args(["fast-import", "--quiet"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "git fast-import failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_recursive(&src_path, &dst_path);
        } else {
            std::fs::copy(&src_path, &dst_path).unwrap();
        }
    }
}

fn base_repo_template() -> &'static Path {
    static TEMPLATE: OnceLock<tempfile::TempDir> = OnceLock::new();
    TEMPLATE
        .get_or_init(|| {
            let d = tempfile::tempdir().unwrap();
            git(d.path(), &["init", "-q", "-b", "main"]);
            let config_path = d.path().join(".git/config");
            let mut cfg = std::fs::read_to_string(&config_path).unwrap();
            cfg.push_str("\n[user]\n\tname = Test\n\temail = test@example.com\n");
            std::fs::write(&config_path, cfg).unwrap();
            let stream = "commit refs/heads/main\ncommitter Test <test@example.com> 1700000000 +0000\ndata 4\ninit\nM 100644 inline README.md\ndata 6\nhello\n\n";
            fast_import(d.path(), stream);
            git(d.path(), &["reset", "--hard", "-q", "HEAD"]);
            d
        })
        .path()
}

fn init_repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    copy_dir_recursive(base_repo_template(), d.path());
    std::fs::write(d.path().join("blyte.toml"), "[dispatch]\ngate = \"true\"\n").unwrap();
    d
}

fn add_verified_run(
    root: &Path,
    id: &str,
    request: &str,
    base_sha: &str,
    started_at: u64,
    file: &str,
    content: &str,
) -> String {
    let wt_parent = root.join(".wt");
    std::fs::create_dir_all(&wt_parent).unwrap();
    let branch = format!("wip/{id}");
    let msg = format!("change {id}");
    let stream = format!(
        "commit refs/heads/{branch}\ncommitter Test <test@example.com> 1700000001 +0000\ndata {}\n{msg}\nfrom {base_sha}\nM 100644 inline {file}\ndata {}\n{content}\n",
        msg.len(),
        content.len()
    );
    fast_import(root, &stream);
    let tip = std::fs::read_to_string(root.join(format!(".git/refs/heads/{branch}")))
        .unwrap()
        .trim()
        .to_string();

    let start = serde_json::json!({
        "id": id,
        "started_at": started_at,
        "base": base_sha,
        "base_sha": base_sha,
        "request": request,
    });
    std::fs::write(
        wt_parent.join(format!("{id}.start.json")),
        serde_json::to_string_pretty(&start).unwrap(),
    )
    .unwrap();

    let summary = serde_json::json!({
        "id": id,
        "branch": branch,
        "outcome": "verified",
        "exit": 0,
    });
    std::fs::write(
        wt_parent.join(format!("{id}.json")),
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();

    tip
}

struct GitLog {
    _dir: tempfile::TempDir,
    bin_dir: PathBuf,
    log_path: PathBuf,
}

impl GitLog {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let log_path = dir.path().join("git.log");
        let real_git = real_git_path();
        let real_git_quoted = real_git.to_string_lossy().replace('\'', "'\\''");
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$BLYTE_TEST_GIT_LOG\"\nexec '{real_git_quoted}' \"$@\"\n"
        );
        let wrapper = bin_dir.join("git");
        std::fs::write(&wrapper, script).unwrap();
        let mut perms = std::fs::metadata(&wrapper).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&wrapper, perms).unwrap();
        Self {
            _dir: dir,
            bin_dir,
            log_path,
        }
    }

    fn run_combine(&self, root: &Path, request: &str) -> std::process::Output {
        let home = tempfile::tempdir().unwrap();
        let orig_path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.bin_dir.clone()];
        paths.extend(std::env::split_paths(&orig_path));
        let wrapped_path = std::env::join_paths(paths).unwrap();

        Command::new(env!("CARGO_BIN_EXE_blyte"))
            .args(["combine", request])
            .current_dir(root)
            .env("PATH", wrapped_path)
            .env("BLYTE_TEST_GIT_LOG", &self.log_path)
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path())
            .env("BLYTE_STATE_DIR", home.path().join("state"))
            .env("BLYTE_ENV_FILE", "")
            .env("BLYTE_NO_UPDATE_CHECK", "1")
            .env("BLYTE_NO_KEEP_AWAKE", "1")
            .env("BLYTE_USAGE_LOG", "off")
            .output()
            .unwrap()
    }

    fn lines(&self) -> Vec<String> {
        if !self.log_path.exists() {
            return Vec::new();
        }
        std::fs::read_to_string(&self.log_path)
            .unwrap()
            .lines()
            .map(ToString::to_string)
            .collect()
    }
}

fn setup_two_run_fixture(request: &str) -> (tempfile::TempDir, String, String, String) {
    let repo = init_repo();
    let root = repo.path();
    let base = git(root, &["rev-parse", "HEAD"]);
    let tip1 = add_verified_run(root, "T1", request, &base, 100, "a.txt", "alpha\n");
    let tip2 = add_verified_run(root, "T2", request, &base, 200, "b.txt", "beta\n");
    (repo, base, tip1, tip2)
}

#[test]
fn a_fresh_two_run_combine_starts_at_most_fourteen_git_processes() {
    let (repo, _, _, _) = setup_two_run_fixture("req-count");
    let git_log = GitLog::new();
    let out = git_log.run_combine(repo.path(), "req-count");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines = git_log.lines();
    eprintln!("fresh two-run combine git calls: {}", lines.len());
    for (i, line) in lines.iter().enumerate() {
        eprintln!("  {:2}: git {line}", i + 1);
    }
    assert!(
        lines.len() <= 14,
        "expected at most 14 git processes, got {}:\n{}",
        lines.len(),
        lines.join("\n")
    );
}

#[test]
fn the_common_directory_is_asked_for_once() {
    let (repo, _, _, _) = setup_two_run_fixture("req-common");
    let git_log = GitLog::new();
    let out = git_log.run_combine(repo.path(), "req-common");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines = git_log.lines();
    let common_dir_calls: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains("--git-common-dir"))
        .collect();
    assert!(
        common_dir_calls.len() <= 1,
        "expected at most 1 --git-common-dir call, got {}:\n{:?}",
        common_dir_calls.len(),
        common_dir_calls
    );
}

#[test]
fn the_combined_branch_is_the_same_as_before() {
    let (repo, base, tip1, tip2) = setup_two_run_fixture("req-same");
    let git_log = GitLog::new();
    let out = git_log.run_combine(repo.path(), "req-same");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let commits_raw = git(
        repo.path(),
        &[
            "rev-list",
            "--reverse",
            &format!("{base}..refs/heads/blyte/req-same"),
        ],
    );
    let commits: Vec<&str> = commits_raw.lines().collect();
    assert_eq!(commits.len(), 2);

    let msg1 = git(repo.path(), &["log", "-n", "1", "--format=%B", commits[0]]);
    let msg2 = git(repo.path(), &["log", "-n", "1", "--format=%B", commits[1]]);
    assert!(msg1.starts_with("change T1"), "msg1: {msg1}");
    assert!(
        msg1.trim_end()
            .ends_with(&format!("(cherry picked from commit {tip1})")),
        "msg1: {msg1}"
    );
    assert!(msg2.starts_with("change T2"), "msg2: {msg2}");
    assert!(
        msg2.trim_end()
            .ends_with(&format!("(cherry picked from commit {tip2})")),
        "msg2: {msg2}"
    );

    let record: CombineRecord = serde_json::from_str(
        &std::fs::read_to_string(repo.path().join(".wt/combined/req-same.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record.state, CombineState::Combined);
    let ids: Vec<&str> = record.runs.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["T1", "T2"]);
}

#[test]
fn a_conflict_still_names_the_run_and_files() {
    let repo = init_repo();
    let root = repo.path();
    let base = git(root, &["rev-parse", "HEAD"]);
    add_verified_run(
        root,
        "T1",
        "req-conflict",
        &base,
        100,
        "README.md",
        "from T1\n",
    );
    add_verified_run(
        root,
        "T2",
        "req-conflict",
        &base,
        200,
        "README.md",
        "from T2\n",
    );

    let git_log = GitLog::new();
    let out = git_log.run_combine(root, "req-conflict");
    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let record: CombineRecord = serde_json::from_str(
        &std::fs::read_to_string(root.join(".wt/combined/req-conflict.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record.state, CombineState::Conflict);
    assert_eq!(record.conflicting_run.as_deref(), Some("T2"));
    assert_eq!(record.conflicting_files, vec!["README.md".to_string()]);

    let combine_wt = root.join(".wt/combine-req-conflict");
    let cp_head = git(
        &combine_wt,
        &["rev-parse", "--git-path", "CHERRY_PICK_HEAD"],
    );
    let cp_head_path = if Path::new(&cp_head).is_absolute() {
        PathBuf::from(cp_head)
    } else {
        combine_wt.join(cp_head)
    };
    assert!(
        !cp_head_path.exists(),
        "CHERRY_PICK_HEAD should not exist at {}",
        cp_head_path.display()
    );
    let status = git(&combine_wt, &["status", "--porcelain"]);
    assert!(status.is_empty(), "worktree should be clean: {status}");
}

#[test]
fn recombining_after_a_new_run_replaces_the_branch() {
    let (repo, base, _, _) = setup_two_run_fixture("req-recombine");
    let root = repo.path();
    let git_log = GitLog::new();

    let out1 = git_log.run_combine(root, "req-recombine");
    assert_eq!(
        out1.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out1.stderr)
    );
    let first_tip = git(root, &["rev-parse", "refs/heads/blyte/req-recombine"]);

    add_verified_run(root, "T3", "req-recombine", &base, 300, "c.txt", "gamma\n");

    let out2 = git_log.run_combine(root, "req-recombine");
    assert_eq!(
        out2.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let second_tip = git(root, &["rev-parse", "refs/heads/blyte/req-recombine"]);
    assert_ne!(second_tip, first_tip);

    let count = git(
        root,
        &[
            "rev-list",
            "--count",
            &format!("{base}..refs/heads/blyte/req-recombine"),
        ],
    );
    assert_eq!(count, "3");
}
