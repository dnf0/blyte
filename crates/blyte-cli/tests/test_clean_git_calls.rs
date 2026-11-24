use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn real_git() -> &'static str {
    static GIT_PATH: OnceLock<String> = OnceLock::new();
    GIT_PATH.get_or_init(|| {
        let out = Command::new("which").arg("git").output().unwrap();
        assert!(
            out.status.success(),
            "which git failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    })
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(real_git())
        .args([
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@test.com",
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write_file(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn build_repo_with_runs(k: usize) -> (tempfile::TempDir, Vec<String>) {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q", "-b", "main"]);
    write_file(&root.join("README.md"), "initial\n");
    git(root, &["add", "."]);
    git(
        root,
        &[
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "commit",
            "-qm",
            "initial",
            "--no-gpg-sign",
        ],
    );

    let wt_dir = root.join(".wt");
    let mut ids = Vec::with_capacity(k);
    for i in 1..=k {
        let id = format!("R{i}");
        let wt_path = wt_dir.join(&id);
        git(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("wip/{id}"),
                wt_path.to_str().unwrap(),
            ],
        );
        write_file(
            &wt_path.join(format!("file_{i}.txt")),
            &format!("work {i}\n"),
        );
        git(&wt_path, &["add", "."]);
        git(
            &wt_path,
            &[
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
                "commit",
                "-qm",
                &format!("feat: run {i}"),
                "--no-gpg-sign",
            ],
        );
        write_file(
            &wt_dir.join(format!("{id}.start.json")),
            &serde_json::json!({
                "id": id,
                "pid": 999999,
                "started_at": 1000,
                "base": "main"
            })
            .to_string(),
        );
        ids.push(id);
    }

    (repo, ids)
}

fn create_git_wrapper(dir: &Path) -> PathBuf {
    let bin_dir = dir.join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let wrapper = bin_dir.join("git");
    let escaped = real_git().replace('\'', "'\\''");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$BLYTE_TEST_GIT_LOG\"\nexec '{escaped}' \"$@\"\n"
    );
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin_dir
}

fn run_clean_logged(cwd: &Path) -> (std::process::Output, Vec<String>) {
    let env_dir = tempfile::tempdir().unwrap();
    let bin_dir = create_git_wrapper(env_dir.path());
    let log_file = env_dir.path().join("git.log");
    let existing_path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{existing_path}", bin_dir.display());

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["clean"])
        .current_dir(cwd)
        .env("PATH", new_path)
        .env("BLYTE_TEST_GIT_LOG", &log_file)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_ENV_FILE", "")
        .env("BLYTE_STATE_DIR", env_dir.path().join("state"))
        .env("BLYTE_GATE_SLOT_DIR", env_dir.path().join("slots"))
        .env("HOME", env_dir.path().join("home"))
        .env("XDG_CONFIG_HOME", env_dir.path().join("home/.config"))
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("OLLAMA_HOST")
        .env_remove("BLYTE_MODEL")
        .output()
        .unwrap();

    let calls = std::fs::read_to_string(&log_file)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.to_string())
        .collect();
    (out, calls)
}

fn count_lookup_calls(calls: &[String]) -> usize {
    calls
        .iter()
        .filter(|line| {
            matches!(
                line.split_whitespace().next(),
                Some("rev-parse" | "for-each-ref" | "show-ref" | "symbolic-ref" | "merge-base")
            )
        })
        .count()
}

#[test]
fn lookups_do_not_grow_with_the_number_of_runs() {
    let (repo2, _) = build_repo_with_runs(2);
    let (out2, calls2) = run_clean_logged(repo2.path());
    assert!(
        out2.status.success(),
        "clean (k=2) failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );

    let (repo8, _) = build_repo_with_runs(8);
    let (out8, calls8) = run_clean_logged(repo8.path());
    assert!(
        out8.status.success(),
        "clean (k=8) failed: {}",
        String::from_utf8_lossy(&out8.stderr)
    );

    let lookups2 = count_lookup_calls(&calls2);
    let lookups8 = count_lookup_calls(&calls8);
    assert_eq!(
        lookups2,
        lookups8,
        "lookup calls grew from {lookups2} (k=2) to {lookups8} (k=8)\nk=2:\n{}\nk=8:\n{}",
        calls2.join("\n"),
        calls8.join("\n")
    );
}

#[test]
fn total_calls_grow_by_at_most_three_per_kept_run() {
    let (repo2, _) = build_repo_with_runs(2);
    let (out2, calls2) = run_clean_logged(repo2.path());
    assert!(
        out2.status.success(),
        "clean (k=2) failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );

    let (repo8, _) = build_repo_with_runs(8);
    let (out8, calls8) = run_clean_logged(repo8.path());
    assert!(
        out8.status.success(),
        "clean (k=8) failed: {}",
        String::from_utf8_lossy(&out8.stderr)
    );

    eprintln!(
        "git calls: k=2 -> {}, k=8 -> {}",
        calls2.len(),
        calls8.len()
    );
    let delta = calls8.len().saturating_sub(calls2.len());
    assert!(
        delta <= 18,
        "expected (calls(8) - calls(2)) / 6 <= 3, got ({} - {}) / 6 = {:.2}\nk=2:\n{}\nk=8:\n{}",
        calls8.len(),
        calls2.len(),
        delta as f64 / 6.0,
        calls2.join("\n"),
        calls8.join("\n")
    );
}

#[test]
fn merged_runs_are_still_removed_and_reported_in_order() {
    let (repo, ids) = build_repo_with_runs(3);
    let root = repo.path();

    for id in [&ids[0], &ids[1]] {
        git(
            root,
            &[
                "merge",
                "--no-ff",
                "-m",
                &format!("Merge wip/{id}"),
                "--no-gpg-sign",
                &format!("wip/{id}"),
            ],
        );
    }

    let (out, _calls) = run_clean_logged(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "clean failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    let lines: Vec<&str> = stdout.lines().collect();
    for id in [&ids[0], &ids[1]] {
        let removed_prefix = format!("removed .wt/{id} ");
        let deleted_prefix = format!("deleted branch wip/{id} ");
        let pos = lines
            .iter()
            .position(|l| l.starts_with(&removed_prefix))
            .unwrap_or_else(|| panic!("missing '{removed_prefix}' in stdout:\n{stdout}"));
        let next = lines.get(pos + 1).copied().unwrap_or("");
        assert!(
            next.starts_with(&deleted_prefix),
            "expected '{deleted_prefix}' directly after '{}', got '{next}'\nfull stdout:\n{stdout}",
            lines[pos]
        );
        assert!(!root.join(".wt").join(id).exists());
    }

    let kept_id = &ids[2];
    assert!(
        root.join(".wt").join(kept_id).exists(),
        "unmerged run {kept_id} should be kept"
    );
    assert!(
        stdout.contains(&format!("kept .wt/{kept_id} ")),
        "expected kept line for {kept_id} in stdout:\n{stdout}"
    );

    let wt_list = git(root, &["worktree", "list"]);
    for id in [&ids[0], &ids[1]] {
        assert!(
            !wt_list.contains(&format!(".wt/{id}")),
            "git worktree list still contains .wt/{id}:\n{wt_list}"
        );
    }
    assert!(
        wt_list.contains(&format!(".wt/{kept_id}")),
        "git worktree list should still contain .wt/{kept_id}:\n{wt_list}"
    );
}

#[test]
fn a_base_that_is_a_commit_still_resolves() {
    let (repo, ids) = build_repo_with_runs(1);
    let root = repo.path();
    let id = &ids[0];
    let main_sha = git(root, &["rev-parse", "main"]);

    write_file(
        &root.join(".wt").join(format!("{id}.start.json")),
        &serde_json::json!({
            "id": id,
            "pid": 999999,
            "started_at": 1000,
            "base": main_sha
        })
        .to_string(),
    );

    git(
        root,
        &[
            "merge",
            "--no-ff",
            "-m",
            &format!("Merge wip/{id}"),
            "--no-gpg-sign",
            &format!("wip/{id}"),
        ],
    );

    let (out, _calls) = run_clean_logged(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "clean failed:\nstdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stdout.contains(&format!("removed .wt/{id} ")),
        "expected removed line for {id} in stdout:\n{stdout}"
    );
    assert!(!root.join(".wt").join(id).exists());
}

fn normalize_removed_size(line: &str) -> String {
    if line.starts_with("removed ") && line.ends_with(')') {
        if let Some(comma_pos) = line.rfind(", ") {
            let size_part = &line[comma_pos + 2..line.len() - 1];
            let valid_size = size_part.split_once(' ').is_some_and(|(num, unit)| {
                !num.is_empty()
                    && num.chars().all(|c| c.is_ascii_digit() || c == '.')
                    && matches!(unit, "B" | "KB" | "MB" | "GB")
            });
            if valid_size {
                return format!("{}, <size>)", &line[..comma_pos]);
            }
        }
    }
    line.to_string()
}

#[test]
fn reasons_match_the_wording_before_the_ref_table() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init", "-q", "-b", "main"]);
    write_file(&root.join("README.md"), "initial\n");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "initial", "--no-gpg-sign"]);
    git(root, &["branch", "feature"]);

    let wt_dir = root.join(".wt");
    let create_run = |id: &str, base: &str, commit_msg: &str| {
        let wt_path = wt_dir.join(id);
        git(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("wip/{id}"),
                wt_path.to_str().unwrap(),
                base,
            ],
        );
        write_file(
            &wt_path.join(format!("f_{id}.txt")),
            &format!("content of {id}\n"),
        );
        git(&wt_path, &["add", "."]);
        git(&wt_path, &["commit", "-qm", commit_msg, "--no-gpg-sign"]);
        write_file(
            &wt_dir.join(format!("{id}.start.json")),
            &serde_json::json!({
                "id": id,
                "pid": 999999,
                "started_at": 1000,
                "base": base
            })
            .to_string(),
        );
    };

    for id in ["A", "B", "D", "E", "K"] {
        create_run(id, "main", &format!("feat: run {id}"));
    }
    create_run("C", "feature", "feat: run C");
    create_run("E2", "wip/E", "fix: follow-up E2");

    write_file(&root.join("advance_main.txt"), "advance main\n");
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-qm", "chore: advance main", "--no-gpg-sign"],
    );

    git(root, &["cherry-pick", "wip/A"]);

    git(root, &["merge", "--squash", "wip/B"]);
    git(
        root,
        &[
            "commit",
            "-qm",
            "squash (#1)\n\n* feat: run B",
            "--no-gpg-sign",
        ],
    );

    write_file(&root.join("f_D.txt"), "content of D\n");
    git(root, &["add", "."]);
    git(
        root,
        &["commit", "-qm", "unrelated wording", "--no-gpg-sign"],
    );

    git(root, &["cherry-pick", "wip/E", "wip/E2"]);

    git(root, &["checkout", "-q", "feature"]);
    write_file(&root.join("advance_feature.txt"), "advance feature\n");
    git(root, &["add", "."]);
    git(root, &["commit", "-qm", "advance feature", "--no-gpg-sign"]);
    git(root, &["cherry-pick", "wip/C"]);
    git(root, &["checkout", "-q", "main"]);

    let (out, _calls) = run_clean_logged(root);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "clean failed:\nstdout: {stdout}\nstderr: {stderr}"
    );

    let mut lines: Vec<String> = stdout.lines().map(normalize_removed_size).collect();
    let last = lines.pop().unwrap_or_default();
    assert!(
        last.starts_with("freed "),
        "expected final 'freed ...' line, got '{last}'\nfull stdout:\n{stdout}"
    );

    let expected: Vec<String> = [
        "removed .wt/A (merged into main, <size>)",
        "deleted branch wip/A (merged)",
        "removed .wt/B (merged into main, <size>)",
        "deleted branch wip/B (merged)",
        "removed .wt/C (merged into feature, <size>)",
        "deleted branch wip/C (merged)",
        "removed .wt/D (merged into main, <size>)",
        "deleted branch wip/D (merged)",
        "removed .wt/E (merged into main, <size>)",
        "deleted branch wip/E (merged)",
        "removed .wt/E2 (merged, <size>)",
        "deleted branch wip/E2 (merged)",
        "kept .wt/K (no verdict)",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    assert_eq!(lines, expected, "unexpected clean output:\n{stdout}");
}
