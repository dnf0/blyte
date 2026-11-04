use std::path::Path;
use std::process::Command;

use blyte_cli::skill::{
    remove_auto_compact_window, set_auto_compact_window, AutoCompact, AUTO_COMPACT_WINDOW, SKILL_MD,
};
use blyte_cli::update::refresh_skills;

fn blyte(cwd: &Path, home: &Path, state_dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("BLYTE_STATE_DIR", state_dir)
        .env("BLYTE_ENV_FILE", home.join(".config/blyte/env"))
        .output()
        .unwrap()
}

#[test]
fn pure_set_and_remove_auto_compact_window() {
    // Empty input -> sets 200000
    let res = set_auto_compact_window("").unwrap();
    assert_eq!(
        res,
        AutoCompact::Set("{\n  \"autoCompactWindow\": 200000\n}\n".to_string())
    );
    assert_eq!(AUTO_COMPACT_WINDOW, 200_000);

    // Empty object -> sets 200000
    let res_obj = set_auto_compact_window("{}").unwrap();
    assert_eq!(
        res_obj,
        AutoCompact::Set("{\n  \"autoCompactWindow\": 200000\n}\n".to_string())
    );

    // Other keys preserved
    let with_other = set_auto_compact_window(r#"{"model":"opus","z":1}"#).unwrap();
    let AutoCompact::Set(text) = with_other else {
        panic!("expected Set, got {with_other:?}");
    };
    let val: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(val["model"], "opus");
    assert_eq!(val["z"], 1);
    assert_eq!(val["autoCompactWindow"], 200_000);
    assert!(text.ends_with('\n'));

    // Top-level key kept (and checked first when others are also present)
    let kept_top = set_auto_compact_window(
        r#"{
            "autoCompactWindow": 350000,
            "modelSettings": { "claude-opus-5-5": { "autoCompactWindow": 400000 } },
            "env": { "CLAUDE_CODE_AUTO_COMPACT_WINDOW": "500000" }
        }"#,
    )
    .unwrap();
    assert_eq!(
        kept_top,
        AutoCompact::Kept {
            key: "autoCompactWindow".to_string(),
            value: "350000".to_string(),
        }
    );

    // modelSettings key kept (checked before env)
    let kept_model = set_auto_compact_window(
        r#"{
            "modelSettings": { "claude-opus-5-5": { "autoCompactWindow": 400000 } },
            "env": { "CLAUDE_CODE_AUTO_COMPACT_WINDOW": "500000" }
        }"#,
    )
    .unwrap();
    assert_eq!(
        kept_model,
        AutoCompact::Kept {
            key: "modelSettings.claude-opus-5-5.autoCompactWindow".to_string(),
            value: "400000".to_string(),
        }
    );

    // env key kept
    let kept_env = set_auto_compact_window(
        r#"{
            "modelSettings": { "claude-opus-5-5": { "other": true } },
            "env": { "CLAUDE_CODE_AUTO_COMPACT_WINDOW": "500000" }
        }"#,
    )
    .unwrap();
    assert_eq!(
        kept_env,
        AutoCompact::Kept {
            key: "env.CLAUDE_CODE_AUTO_COMPACT_WINDOW".to_string(),
            value: "500000".to_string(),
        }
    );

    // Non-object and broken JSON -> Err
    assert!(set_auto_compact_window("[]").is_err());
    assert!(set_auto_compact_window("\"hello\"").is_err());
    assert!(set_auto_compact_window("{not json").is_err());
    assert!(remove_auto_compact_window("[]").is_err());
    assert!(remove_auto_compact_window("{not json").is_err());

    // remove with 200000
    let removed = remove_auto_compact_window(&text)
        .unwrap()
        .expect("should remove 200000");
    let removed_val: serde_json::Value = serde_json::from_str(&removed).unwrap();
    assert!(removed_val.get("autoCompactWindow").is_none());
    assert_eq!(removed_val["model"], "opus");
    assert_eq!(removed_val["z"], 1);

    // remove with another value -> None
    assert_eq!(
        remove_auto_compact_window(r#"{"autoCompactWindow": 300000}"#),
        Ok(None)
    );
    assert_eq!(
        remove_auto_compact_window(r#"{"autoCompactWindow": "200000"}"#),
        Ok(None)
    );

    // remove with no key -> None
    assert_eq!(remove_auto_compact_window(r#"{"model": "opus"}"#), Ok(None));
    assert_eq!(remove_auto_compact_window(""), Ok(None));
}

#[test]
fn install_writes_key_and_marker_once_and_respects_manual_removal() {
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let out1 = blyte(cwd.path(), home.path(), state.path(), &["skill", "install"]);
    assert_eq!(
        out1.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out1.stderr)
    );
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    assert!(
        stdout1.contains(
            "auto-compact: set autoCompactWindow to 200000 in ~/.claude/settings.json (Claude Code compacts at 200k tokens; delete the key to use the model's full window)"
        ),
        "unexpected stdout:\n{stdout1}"
    );

    let settings_path = home.path().join(".claude/settings.json");
    let settings_val: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(settings_val["autoCompactWindow"], 200_000);
    assert_eq!(
        settings_val["statusLine"]["command"],
        serde_json::json!("blyte statusline")
    );

    let marker_path = state.path().join("autocompact-window");
    assert_eq!(
        std::fs::read_to_string(&marker_path).unwrap().trim(),
        "200000"
    );

    // Second install prints nothing about auto-compact.
    let out2 = blyte(cwd.path(), home.path(), state.path(), &["skill", "install"]);
    assert_eq!(out2.status.code(), Some(0));
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(
        !stdout2.contains("auto-compact:"),
        "expected no auto-compact line on second install, got:\n{stdout2}"
    );

    // User removes autoCompactWindow by hand; next install leaves it removed because marker exists.
    std::fs::write(
        &settings_path,
        "{\n  \"statusLine\": {\n    \"type\": \"command\",\n    \"command\": \"blyte statusline\",\n    \"refreshInterval\": 30\n  }\n}\n",
    )
    .unwrap();
    let out3 = blyte(cwd.path(), home.path(), state.path(), &["skill", "install"]);
    assert_eq!(out3.status.code(), Some(0));
    let stdout3 = String::from_utf8_lossy(&out3.stdout);
    assert!(!stdout3.contains("auto-compact:"), "{stdout3}");
    let after_val: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert!(
        after_val.get("autoCompactWindow").is_none(),
        "manual removal should stay removed: {after_val}"
    );
}

#[test]
fn existing_model_settings_window_is_kept_and_no_marker_written() {
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let settings_path = claude_dir.join("settings.json");
    std::fs::write(
        &settings_path,
        r#"{"modelSettings":{"claude-opus-5-5":{"autoCompactWindow":400000}}}"#,
    )
    .unwrap();

    let out = blyte(cwd.path(), home.path(), state.path(), &["skill", "install"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(
            "auto-compact: keeping your modelSettings.claude-opus-5-5.autoCompactWindow (400000)"
        ),
        "unexpected stdout:\n{stdout}"
    );

    let val: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert!(val.get("autoCompactWindow").is_none());
    assert_eq!(
        val["modelSettings"]["claude-opus-5-5"]["autoCompactWindow"],
        400_000
    );
    assert!(!state.path().join("autocompact-window").exists());
}

#[test]
fn no_autocompact_flag_leaves_key_and_marker_absent() {
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let out = blyte(
        cwd.path(),
        home.path(),
        state.path(),
        &["skill", "install", "--no-autocompact"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("auto-compact:"), "{stdout}");

    let settings_path = home.path().join(".claude/settings.json");
    let val: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert!(val.get("autoCompactWindow").is_none());
    assert!(!state.path().join("autocompact-window").exists());
}

#[test]
fn uninstall_removes_key_and_marker_unless_user_changed_value() {
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let settings_path = home.path().join(".claude/settings.json");
    let marker_path = state.path().join("autocompact-window");

    // 1. Install then uninstall: both key and marker are removed.
    let out = blyte(cwd.path(), home.path(), state.path(), &["skill", "install"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(marker_path.exists());

    let un = blyte(
        cwd.path(),
        home.path(),
        state.path(),
        &["skill", "uninstall"],
    );
    assert_eq!(
        un.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&un.stderr)
    );
    assert!(!marker_path.exists());
    let val: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert!(val.get("autoCompactWindow").is_none());

    // 2. Install, user changes value to 300000, then uninstall: key stays, marker goes.
    let out = blyte(cwd.path(), home.path(), state.path(), &["skill", "install"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(marker_path.exists());
    std::fs::write(&settings_path, "{\n  \"autoCompactWindow\": 300000\n}\n").unwrap();

    let un2 = blyte(
        cwd.path(),
        home.path(),
        state.path(),
        &["skill", "uninstall"],
    );
    assert_eq!(
        un2.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&un2.stderr)
    );
    assert!(!marker_path.exists());
    let val2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(val2["autoCompactWindow"], 300_000);

    // 3. Without a marker, even 200000 is never touched by uninstall.
    std::fs::write(&settings_path, "{\n  \"autoCompactWindow\": 200000\n}\n").unwrap();
    let un3 = blyte(
        cwd.path(),
        home.path(),
        state.path(),
        &["skill", "uninstall"],
    );
    assert_eq!(un3.status.code(), Some(0));
    let val3: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(val3["autoCompactWindow"], 200_000);
}

#[test]
fn broken_json_in_settings_makes_install_and_uninstall_fail_naming_the_file() {
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let claude_dir = home.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let settings_path = claude_dir.join("settings.json");
    std::fs::write(&settings_path, "{broken json").unwrap();

    for flags in [
        &["skill", "install"][..],
        &["skill", "install", "--no-autocompact"][..],
        &["skill", "install", "--no-statusline"][..],
        &["skill", "uninstall"][..],
    ] {
        let out = blyte(cwd.path(), home.path(), state.path(), flags);
        assert_ne!(
            out.status.code(),
            Some(0),
            "expected failure for {flags:?}, got success"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains(&settings_path.display().to_string()),
            "expected stderr for {flags:?} to name {}, got:\n{stderr}",
            settings_path.display()
        );
    }
}

#[test]
fn marker_write_failure_names_marker_and_explains_key_was_set() {
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    // Make state_dir a regular file so creating `<state_dir>/autocompact-window` fails.
    let bad_state_dir = state.path().join("not-a-dir");
    std::fs::write(&bad_state_dir, "blocking file").unwrap();

    let out = blyte(
        cwd.path(),
        home.path(),
        &bad_state_dir,
        &["skill", "install"],
    );
    assert_ne!(out.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&out.stderr);
    let expected_marker = bad_state_dir.join("autocompact-window");
    assert!(
        stderr.contains(&expected_marker.display().to_string()),
        "expected stderr to name {}, got:\n{stderr}",
        expected_marker.display()
    );
    assert!(
        stderr.contains("uninstall will not remove it"),
        "expected stderr to explain uninstall behaviour, got:\n{stderr}"
    );
}

#[test]
fn refresh_skills_forwards_autocompact_line_and_reports_stderr_cause_on_failure() {
    let home = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    std::env::set_var("BLYTE_STATE_DIR", state.path());

    // Set up an unedited installed skill as if an older version installed it (without marker).
    let skill_dir = home.path().join(".claude/skills/blyte-implementer");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), SKILL_MD).unwrap();
    std::fs::write(skill_dir.join(".blyte-installed"), SKILL_MD).unwrap();

    let exe = Path::new(env!("CARGO_BIN_EXE_blyte"));
    let lines = refresh_skills(exe, home.path());
    assert_eq!(
        lines,
        vec![
            "skill refreshed for Claude Code".to_string(),
            "auto-compact: set autoCompactWindow to 200000 in ~/.claude/settings.json (Claude Code compacts at 200k tokens; delete the key to use the model's full window)".to_string(),
        ]
    );

    // Second refresh: marker exists, so only "skill refreshed for Claude Code" is returned.
    let lines2 = refresh_skills(exe, home.path());
    assert_eq!(lines2, vec!["skill refreshed for Claude Code".to_string()]);

    // Broken settings.json: refresh fails and appends the child's stderr cause.
    let settings_path = home.path().join(".claude/settings.json");
    std::fs::write(&settings_path, "{broken json").unwrap();
    let lines_err = refresh_skills(exe, home.path());
    assert_eq!(lines_err.len(), 1, "{lines_err:?}");
    assert!(
        lines_err[0].starts_with(
            "skill not refreshed for Claude Code: blyte skill install failed; run it by hand"
        ) && lines_err[0].contains(&settings_path.display().to_string()),
        "expected cause in refresh failure line, got: {:?}",
        lines_err[0]
    );
}
