use std::path::Path;
use std::process::Command;

use blyte_cli::config::{comment_mode, load, CommentMode};
use blyte_cli::dispatch::{comment_step, gate_with_comment_step, DISPATCH_RULES};

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn rev_parse(dir: &Path, rev: &str) -> String {
    let out = Command::new("git")
        .args(["rev-parse", rev])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git rev-parse {rev}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn setup_repo(r: &Path) {
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "init", "--no-gpg-sign"]);
    std::fs::write(r.join("task.md"), "Do nothing.\n").unwrap();
}

fn dispatch(r: &Path, id: &str, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["dispatch", id, "task.md", "--sandbox", "off"])
        .args(extra)
        .current_dir(r)
        .env("BLYTE_USAGE_LOG", "off")
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("HOME", r)
        .env("XDG_CONFIG_HOME", r)
        .env_remove("BLYTE_ENV_FILE")
        .env_remove("BLYTE_MODEL")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .output()
        .unwrap()
}

fn blyte(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(dir)
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("BLYTE_ENV_FILE")
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir)
        .output()
        .unwrap()
}

#[test]
fn comment_mode_parses_accepted_values_and_rejects_others() {
    assert_eq!(comment_mode(None), Ok(CommentMode::Gate));
    assert_eq!(comment_mode(Some("gate")), Ok(CommentMode::Gate));
    assert_eq!(comment_mode(Some("review")), Ok(CommentMode::Review));
    assert_eq!(comment_mode(Some("off")), Ok(CommentMode::Off));
    assert_eq!(
        comment_mode(Some("flag")),
        Err(
            "blyte.toml: [dispatch] comments must be \"gate\", \"review\" or \"off\", not \"flag\""
                .to_string()
        )
    );
}

#[test]
fn comment_step_quotes_path_and_escapes_single_quotes() {
    assert_eq!(
        comment_step(Path::new("/opt/my tools/blyte"), "abc123"),
        "'/opt/my tools/blyte' comments --base abc123"
    );
    assert_eq!(
        comment_step(Path::new("/opt/a'b/blyte"), "deadbeef"),
        r"'/opt/a'\''b/blyte' comments --base deadbeef"
    );
}

#[test]
fn gate_with_comment_step_prepends_when_gate_is_non_blank() {
    assert_eq!(
        gate_with_comment_step("cargo fmt\ncargo test", "S"),
        "S\ncargo fmt\ncargo test"
    );
    assert_eq!(gate_with_comment_step("", "S"), "");
    assert_eq!(gate_with_comment_step("\n  \n", "S"), "\n  \n");
}

#[test]
fn dispatch_rules_include_no_comments_rule() {
    assert!(DISPATCH_RULES.contains("Write no comments in code"));
}

#[test]
fn blyte_toml_parses_comments_and_rejects_unknown_keys() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(
        d.path().join("blyte.toml"),
        "[dispatch]\ncomments = \"review\"\n",
    )
    .unwrap();
    let cfg = load(d.path()).unwrap().unwrap();
    assert_eq!(cfg.dispatch.comments.as_deref(), Some("review"));

    std::fs::write(
        d.path().join("blyte.toml"),
        "[dispatch]\ncomments = \"review\"\nunknown_key = 1\n",
    )
    .unwrap();
    let err = load(d.path()).unwrap_err();
    assert!(err.contains("unknown_key"), "{err}");
}

#[test]
fn dispatch_prepends_comment_step_by_default_and_honours_comments_setting() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    setup_repo(r);
    let base_sha = rev_parse(r, "HEAD");

    std::fs::write(r.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n").unwrap();
    let _out = dispatch(r, "C1", &["--no-base-check"]);
    let start1: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".wt/C1.start.json")).unwrap())
            .unwrap();
    assert_eq!(start1["comments"], "gate");
    let gate1 = start1["gate"].as_str().unwrap();
    let mut lines1 = gate1.lines();
    let first1 = lines1.next().unwrap();
    assert!(
        first1.ends_with(&format!(" comments --base {base_sha}")),
        "first line was: {first1}"
    );
    assert_eq!(lines1.next(), Some("true"));

    std::fs::write(
        r.join("blyte.toml"),
        "[dispatch]\ngate = \"true\"\ncomments = \"review\"\n",
    )
    .unwrap();
    let _out2 = dispatch(r, "C2", &["--no-base-check"]);
    let start2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".wt/C2.start.json")).unwrap())
            .unwrap();
    assert_eq!(start2["comments"], "review");
    assert_eq!(start2["gate"], "true");

    std::fs::write(
        r.join("blyte.toml"),
        "[dispatch]\ngate = \"true\"\ncomments = \"bogus\"\n",
    )
    .unwrap();
    let out3 = dispatch(r, "C3", &["--no-base-check"]);
    assert!(!out3.status.success());
    let stderr3 = String::from_utf8_lossy(&out3.stderr);
    assert!(stderr3.contains("comments must be"), "{stderr3}");

    std::fs::write(r.join("blyte.toml"), "[dispatch]\ngate = \"true\"\n").unwrap();
    let _out4 = dispatch(r, "C4", &["--research", "--no-brief-check"]);
    let start4: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(r.join(".wt/C4.start.json")).unwrap())
            .unwrap();
    assert_eq!(start4["comments"], "off");
}

#[test]
fn review_flags_added_comments_unless_comments_is_off() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    std::fs::write(r.join("f.txt"), "a\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "i", "--no-gpg-sign"]);
    let base_sha = rev_parse(r, "HEAD");

    git(r, &["worktree", "add", "-q", "-b", "wip/RC1", ".wt/RC1"]);
    std::fs::write(
        r.join("task.md"),
        "Files: `src/a.rs`\nCommit: `feat: add a`\n",
    )
    .unwrap();
    std::fs::write(
        r.join(".wt/RC1.start.json"),
        serde_json::json!({
            "id": "RC1",
            "base_sha": base_sha,
            "task_file": "task.md"
        })
        .to_string(),
    )
    .unwrap();

    let wt = r.join(".wt/RC1");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::write(
        wt.join("src/a.rs"),
        format!("{} explain why\npub fn f() {{}}\n", "//"),
    )
    .unwrap();
    git(&wt, &["add", "src/a.rs"]);
    git(&wt, &["commit", "-qm", "feat: add a", "--no-gpg-sign"]);

    let out = blyte(r, &["review", "RC1"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("comments added (1): src/a.rs:1"), "{text}");

    std::fs::write(
        r.join(".wt/RC1.start.json"),
        serde_json::json!({
            "id": "RC1",
            "base_sha": base_sha,
            "task_file": "task.md",
            "comments": "off"
        })
        .to_string(),
    )
    .unwrap();
    let out_off = blyte(r, &["review", "RC1"]);
    assert_eq!(out_off.status.code(), Some(0));
    let text_off = String::from_utf8_lossy(&out_off.stdout);
    assert!(!text_off.contains("comments added"), "{text_off}");
}
