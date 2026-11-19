use std::path::Path;
use std::process::Command;

fn init_repo(dir: &Path) {
    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(dir)
        .output()
        .unwrap();
    std::fs::write(dir.join("README.md"), "initial").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "initial", "--no-gpg-sign"])
        .current_dir(dir)
        .output()
        .unwrap();
}

#[test]
fn test_dispatch_failing_gate_stops_as_blocked_before_model() {
    let temp = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Do something").unwrap();

    let blyte_bin = env!("CARGO_BIN_EXE_blyte");
    let out = Command::new(blyte_bin)
        .current_dir(repo_dir)
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env("GEMINI_API_KEY", "dummy-key-never-called")
        .args([
            "dispatch",
            "T_BASE_FAIL",
            task_file.to_str().unwrap(),
            "--gate",
            "exit 3",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();

    assert_eq!(
        out.status.code(),
        Some(5),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let summary: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "Failed to parse JSON: {}\nStdout: {}\nStderr: {}",
            e,
            stdout,
            String::from_utf8_lossy(&out.stderr)
        )
    });

    assert_eq!(summary["outcome"], "blocked");
    assert_eq!(summary["turns"], 0);
    assert_eq!(summary["exit"], 5);
    let final_msg = summary["final_message"].as_str().unwrap_or("");
    assert!(
        final_msg.contains("--no-base-check"),
        "expected final_message to mention --no-base-check, got: {final_msg}"
    );
    let err_msg = summary["error"].as_str().unwrap_or("");
    assert!(
        err_msg.contains("the check fails before any change"),
        "expected error to mention 'the check fails before any change', got: {err_msg}"
    );
    assert!(
        !err_msg.contains('\n'),
        "expected summary error to be a single line, got: {err_msg}"
    );

    let start_json_str =
        std::fs::read_to_string(repo_dir.join(".wt/T_BASE_FAIL.start.json")).unwrap();
    let start_json: serde_json::Value = serde_json::from_str(&start_json_str).unwrap();
    assert_eq!(start_json["base_check"], true);
}

#[test]
fn test_dispatch_no_base_check_flag_skips_base_check() {
    let temp = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Do something").unwrap();

    let blyte_bin = env!("CARGO_BIN_EXE_blyte");
    let out = Command::new(blyte_bin)
        .current_dir(repo_dir)
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .args([
            "dispatch",
            "T_NO_BASE_CHECK",
            task_file.to_str().unwrap(),
            "--gate",
            "exit 3",
            "--no-base-check",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let summary: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_ne!(summary["outcome"], "blocked");

    let start_json_str =
        std::fs::read_to_string(repo_dir.join(".wt/T_NO_BASE_CHECK.start.json")).unwrap();
    let start_json: serde_json::Value = serde_json::from_str(&start_json_str).unwrap();
    assert_eq!(start_json["base_check"], false);
}

#[test]
fn test_dispatch_config_base_check_false_skips_base_check() {
    let temp = tempfile::tempdir().unwrap();
    let repo_dir = temp.path();
    init_repo(repo_dir);

    std::fs::write(
        repo_dir.join("blyte.toml"),
        "[dispatch]\nbase_check = false\n",
    )
    .unwrap();
    Command::new("git")
        .args(["add", "blyte.toml"])
        .current_dir(repo_dir)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "add blyte.toml", "--no-gpg-sign"])
        .current_dir(repo_dir)
        .output()
        .unwrap();

    let task_file = repo_dir.join("task.md");
    std::fs::write(&task_file, "Do something").unwrap();

    let blyte_bin = env!("CARGO_BIN_EXE_blyte");
    let out = Command::new(blyte_bin)
        .current_dir(repo_dir)
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .args([
            "dispatch",
            "T_CFG_NO_BASE_CHECK",
            task_file.to_str().unwrap(),
            "--gate",
            "exit 3",
            "--sandbox",
            "off",
        ])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&out.stdout);
    let summary: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_ne!(summary["outcome"], "blocked");

    let start_json_str =
        std::fs::read_to_string(repo_dir.join(".wt/T_CFG_NO_BASE_CHECK.start.json")).unwrap();
    let start_json: serde_json::Value = serde_json::from_str(&start_json_str).unwrap();
    assert_eq!(start_json["base_check"], false);
}
