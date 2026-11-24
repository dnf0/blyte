use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use blyte_cli::publish::{open_pull_request_with_gh, push_branch};

fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap_or_else(|e| panic!("failed to run git {}: {e}", args.join(" ")));
    assert!(
        out.status.success(),
        "git {} failed in {}: stdout={} stderr={}",
        args.join(" "),
        dir.display(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

struct TemplateRepo {
    _tmp: tempfile::TempDir,
    bare: PathBuf,
    work: PathBuf,
    head_sha: String,
    remote_alt_sha: String,
    local_feature_sha: String,
}

fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let ty = entry.file_type().unwrap();
        let dest = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn main_template() -> &'static TemplateRepo {
    static MAIN: OnceLock<TemplateRepo> = OnceLock::new();
    MAIN.get_or_init(|| {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("origin.git");
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();

        run_git(&work, &["init", "-b", "main"]);
        let work_git = work.join(".git");
        let config_path = work_git.join("config");
        let base_cfg = std::fs::read_to_string(&config_path).unwrap();
        let work_cfg = format!(
            "{base_cfg}\n[user]\n\temail = test@example.com\n\tname = Test User\n[remote \"origin\"]\n\turl = {}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n[branch \"main\"]\n\tremote = origin\n\tmerge = refs/heads/main\n",
            bare.display()
        );
        std::fs::write(&config_path, work_cfg).unwrap();

        run_git(
            &work,
            &[
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
                "commit",
                "--no-gpg-sign",
                "--allow-empty",
                "-m",
                "initial commit",
            ],
        );
        let head_sha = read_ref(&work_git, "refs/heads/main");

        let empty_tree = run_git(&work, &["rev-parse", "HEAD^{tree}"]);
        let remote_alt_sha = run_git(
            &work,
            &["commit-tree", &empty_tree, "-p", &head_sha, "-m", "first"],
        );

        copy_dir_all(&work_git, &bare);
        let bare_cfg = base_cfg.replace("bare = false", "bare = true");
        std::fs::write(bare.join("config"), bare_cfg).unwrap();
        let _ = std::fs::remove_file(bare.join("index"));

        let local_feature_sha = run_git(
            &work,
            &[
                "commit-tree",
                &empty_tree,
                "-p",
                &head_sha,
                "-m",
                "add feature",
            ],
        );

        let origin_refs = work_git.join("refs/remotes/origin");
        std::fs::create_dir_all(&origin_refs).unwrap();
        std::fs::write(origin_refs.join("main"), format!("{head_sha}\n")).unwrap();
        std::fs::write(origin_refs.join("HEAD"), "ref: refs/remotes/origin/main\n").unwrap();

        TemplateRepo {
            _tmp: tmp,
            bare,
            work,
            head_sha,
            remote_alt_sha,
            local_feature_sha,
        }
    })
}

fn setup_repo_with_bare_origin(default_branch: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tpl = main_template();
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("origin.git");
    let work = tmp.path().join("work");
    copy_dir_all(&tpl.bare, &bare);
    copy_dir_all(&tpl.work, &work);

    let cfg_path = work.join(".git/config");
    let mut cfg = std::fs::read_to_string(&cfg_path)
        .unwrap()
        .replace(tpl.bare.to_str().unwrap(), bare.to_str().unwrap());

    if default_branch != "main" {
        std::fs::write(
            bare.join("HEAD"),
            format!("ref: refs/heads/{default_branch}\n"),
        )
        .unwrap();
        std::fs::rename(
            bare.join("refs/heads/main"),
            bare.join(format!("refs/heads/{default_branch}")),
        )
        .unwrap();
        std::fs::write(
            work.join(".git/HEAD"),
            format!("ref: refs/heads/{default_branch}\n"),
        )
        .unwrap();
        std::fs::rename(
            work.join(".git/refs/heads/main"),
            work.join(format!(".git/refs/heads/{default_branch}")),
        )
        .unwrap();
        std::fs::rename(
            work.join(".git/refs/remotes/origin/main"),
            work.join(format!(".git/refs/remotes/origin/{default_branch}")),
        )
        .unwrap();
        std::fs::write(
            work.join(".git/refs/remotes/origin/HEAD"),
            format!("ref: refs/remotes/origin/{default_branch}\n"),
        )
        .unwrap();
        cfg = cfg
            .replace(
                "[branch \"main\"]",
                &format!("[branch \"{default_branch}\"]"),
            )
            .replace("refs/heads/main", &format!("refs/heads/{default_branch}"));
    }

    std::fs::write(&cfg_path, cfg).unwrap();

    (tmp, bare, work)
}

fn write_ref(git_dir: &Path, rel_ref: &str, sha: &str) {
    let ref_path = git_dir.join(rel_ref);
    if let Some(parent) = ref_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(ref_path, format!("{sha}\n")).unwrap();
}

fn checkout_new_branch_at(work: &Path, branch: &str, sha: &str) {
    write_ref(&work.join(".git"), &format!("refs/heads/{branch}"), sha);
    std::fs::write(
        work.join(".git/HEAD"),
        format!("ref: refs/heads/{branch}\n"),
    )
    .unwrap();
}

fn checkout_new_branch_at_head(work: &Path, branch: &str) {
    checkout_new_branch_at(work, branch, &main_template().head_sha);
}

fn read_ref(git_dir: &Path, rel_ref: &str) -> String {
    std::fs::read_to_string(git_dir.join(rel_ref))
        .unwrap()
        .trim()
        .to_string()
}

fn write_executable(path: &Path, content: &str) {
    std::fs::write(path, content).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[test]
fn push_branch_pushes_work_branch_to_local_bare_origin_and_sets_upstream() {
    let (_tmp, bare, work) = setup_repo_with_bare_origin("main");
    let local_sha = main_template().local_feature_sha.clone();
    checkout_new_branch_at(&work, "feat/publish", &local_sha);

    push_branch(&work, "feat/publish").unwrap();

    let remote_sha = read_ref(&bare, "refs/heads/feat/publish");
    assert_eq!(remote_sha, local_sha);

    let cfg = std::fs::read_to_string(work.join(".git/config")).unwrap();
    assert!(
        cfg.contains("[branch \"feat/publish\"]")
            && cfg.contains("remote = origin")
            && cfg.contains("merge = refs/heads/feat/publish"),
        "expected upstream tracking in git config:\n{cfg}"
    );
}

#[test]
fn push_branch_never_uses_force_on_diverged_remote() {
    let (_tmp, bare, work) = setup_repo_with_bare_origin("main");
    let pushed_sha = main_template().remote_alt_sha.clone();
    let diverged_sha = main_template().local_feature_sha.clone();
    write_ref(&bare, "refs/heads/feat/diverge", &pushed_sha);
    checkout_new_branch_at(&work, "feat/diverge", &diverged_sha);

    let err = push_branch(&work, "feat/diverge").unwrap_err();
    assert!(err.contains("git push"), "unexpected error: {err}");
    assert_eq!(read_ref(&bare, "refs/heads/feat/diverge"), pushed_sha);
}

#[test]
fn push_branch_refuses_default_branch_read_from_origin_head() {
    let (_tmp, bare, work) = setup_repo_with_bare_origin("trunk");
    let before_sha = read_ref(&bare, "refs/heads/trunk");
    write_ref(
        &work.join(".git"),
        "refs/heads/trunk",
        &main_template().local_feature_sha,
    );

    let err = push_branch(&work, "trunk").unwrap_err();
    assert!(
        err.contains("default branch") && err.contains("trunk"),
        "unexpected error: {err}"
    );
    assert_eq!(read_ref(&bare, "refs/heads/trunk"), before_sha);
}

#[test]
fn push_branch_errors_on_missing_branch() {
    let (_tmp, _bare, work) = setup_repo_with_bare_origin("main");

    let err = push_branch(&work, "no-such-branch").unwrap_err();
    assert!(
        err.contains("no-such-branch") && err.contains("does not exist"),
        "unexpected error: {err}"
    );
}

#[test]
fn push_branch_errors_on_missing_origin_remote() {
    let (_tmp, _bare, work) = setup_repo_with_bare_origin("main");
    let cfg_path = work.join(".git/config");
    let cfg = std::fs::read_to_string(&cfg_path).unwrap();
    let stripped: String = cfg.split("[remote \"origin\"]").next().unwrap().to_string();
    std::fs::write(&cfg_path, stripped).unwrap();
    std::fs::remove_dir_all(work.join(".git/refs/remotes/origin")).unwrap();
    checkout_new_branch_at_head(&work, "feat/local");

    let err = push_branch(&work, "feat/local").unwrap_err();
    assert!(err.contains("origin"), "unexpected error: {err}");
}

#[test]
fn push_branch_errors_on_unreadable_default_branch() {
    let (_tmp, _bare, work) = setup_repo_with_bare_origin("main");
    checkout_new_branch_at_head(&work, "feat/no-head");
    let origin_head = work.join(".git/refs/remotes/origin/HEAD");
    std::fs::remove_file(&origin_head).unwrap();

    let err = push_branch(&work, "feat/no-head").unwrap_err();
    assert!(err.contains("origin/HEAD"), "unexpected error: {err}");

    std::fs::write(&origin_head, format!("{}\n", main_template().head_sha)).unwrap();
    let detached_err = push_branch(&work, "feat/no-head").unwrap_err();
    assert!(
        detached_err.contains("origin/HEAD"),
        "unexpected error: {detached_err}"
    );
}

#[test]
fn open_pull_request_runs_gh_with_expected_args_and_returns_url() {
    let bin_tmp = tempfile::tempdir().unwrap();

    let (_tmp, _bare, work) = setup_repo_with_bare_origin("trunk");
    checkout_new_branch_at_head(&work, "feat/open-pr");

    let args_file = bin_tmp.path().join("gh-args.bin");
    let fake_gh = bin_tmp.path().join("gh");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > '{}'\nprintf 'https://github.com/example/repo/pull/42\\n'\n",
        args_file.display()
    );
    write_executable(&fake_gh, &script);

    let title = "Add publish helpers ($(not-a-subshell))";
    let body = "- first line with `backticks`\n- second line with 'quotes' and \"double quotes\"";
    let url =
        open_pull_request_with_gh(&work, "feat/open-pr", title, body, fake_gh.as_os_str()).unwrap();
    assert_eq!(url, "https://github.com/example/repo/pull/42");

    let raw = std::fs::read(&args_file).unwrap();
    let mut parts: Vec<String> = raw
        .split(|&b| b == 0)
        .map(|slice| String::from_utf8(slice.to_vec()).unwrap())
        .collect();
    assert_eq!(parts.pop().as_deref(), Some(""));
    assert_eq!(
        parts,
        vec![
            "pr",
            "create",
            "--base",
            "trunk",
            "--head",
            "feat/open-pr",
            "--title",
            title,
            "--body",
            body,
        ]
    );
}

#[test]
fn open_pull_request_errors_when_gh_is_missing() {
    let bin_tmp = tempfile::tempdir().unwrap();
    let missing_gh = bin_tmp.path().join("gh");

    let (_tmp, _bare, work) = setup_repo_with_bare_origin("main");
    checkout_new_branch_at_head(&work, "feat/missing-gh");

    let err = open_pull_request_with_gh(
        &work,
        "feat/missing-gh",
        "Title",
        "Body",
        missing_gh.as_os_str(),
    )
    .unwrap_err();
    assert!(
        err.contains("gh") && (err.contains("not installed") || err.contains("not found")),
        "unexpected error: {err}"
    );
}

#[test]
fn open_pull_request_errors_when_gh_is_not_signed_in() {
    let bin_tmp = tempfile::tempdir().unwrap();

    let (_tmp, _bare, work) = setup_repo_with_bare_origin("main");
    checkout_new_branch_at_head(&work, "feat/not-logged-in");

    let fake_gh = bin_tmp.path().join("gh");
    write_executable(
        &fake_gh,
        "#!/bin/sh\necho 'To get started with GitHub CLI, please run:  gh auth login' >&2\nexit 4\n",
    );

    let err = open_pull_request_with_gh(
        &work,
        "feat/not-logged-in",
        "Title",
        "Body",
        fake_gh.as_os_str(),
    )
    .unwrap_err();
    assert!(
        err.contains("not signed in") && err.contains("gh"),
        "unexpected error: {err}"
    );
}

#[test]
fn open_pull_request_refuses_empty_title_and_default_branch_before_running_gh() {
    let bin_tmp = tempfile::tempdir().unwrap();

    let (_tmp, _bare, work) = setup_repo_with_bare_origin("main");
    checkout_new_branch_at_head(&work, "feat/work");

    let marker = bin_tmp.path().join("gh-ran");
    let fake_gh = bin_tmp.path().join("gh");
    let script = format!("#!/bin/sh\n: > '{}'\nexit 0\n", marker.display());
    write_executable(&fake_gh, &script);

    let empty_err =
        open_pull_request_with_gh(&work, "feat/work", "   \n\t ", "Body", fake_gh.as_os_str())
            .unwrap_err();
    assert!(!marker.exists(), "gh should not run when title is empty");
    let default_err =
        open_pull_request_with_gh(&work, "main", "Valid title", "Body", fake_gh.as_os_str())
            .unwrap_err();
    assert!(empty_err.contains("title"), "unexpected error: {empty_err}");
    assert!(
        default_err.contains("default branch") && default_err.contains("main"),
        "unexpected error: {default_err}"
    );
    assert!(
        !marker.exists(),
        "gh should not run when branch is the default branch"
    );
}
