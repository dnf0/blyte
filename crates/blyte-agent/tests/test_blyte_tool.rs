use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use blyte_agent::fs::LocalFs;
use blyte_agent::shell::LocalShell;
use blyte_agent::tools::{
    blyte_command_timeout, execute, set_blyte_request, set_blyte_tool, specs, BLYTE_TOOL_COMMANDS,
    COMMAND_TIMEOUT, EDIT_FINISH_TIMEOUT, OUTPUT_LINE_BUDGET,
};
use serde_json::json;

static BLYTE_TOOL_LOCK: Mutex<()> = Mutex::new(());
static DISPATCHER: OnceLock<(tempfile::TempDir, PathBuf, PathBuf)> = OnceLock::new();

fn dispatcher() -> &'static (tempfile::TempDir, PathBuf, PathBuf) {
    DISPATCHER.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("fake-blyte");
        let body = dir.path().join("body.sh");
        std::fs::write(&exe, format!("#!/bin/sh\n. '{}'\n", body.display())).unwrap();
        let mut perms = std::fs::metadata(&exe).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&exe, perms).unwrap();
        (dir, exe, body)
    })
}

struct BlyteToolGuard {
    _lock: MutexGuard<'static, ()>,
}

impl BlyteToolGuard {
    fn acquire_none() -> Self {
        let lock = BLYTE_TOOL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_blyte_tool(None);
        Self { _lock: lock }
    }

    fn acquire_script(body: &str) -> Self {
        let lock = BLYTE_TOOL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (_, exe, body_path) = dispatcher();
        std::fs::write(body_path, body).unwrap();
        set_blyte_tool(Some(exe.clone()));
        Self { _lock: lock }
    }

    fn set_script(&self, body: &str) {
        let (_, exe, body_path) = dispatcher();
        std::fs::write(body_path, body).unwrap();
        set_blyte_tool(Some(exe.clone()));
    }
}

impl Drop for BlyteToolGuard {
    fn drop(&mut self) {
        set_blyte_tool(None);
    }
}

#[test]
fn specs_offers_blyte_tool_only_when_set() {
    {
        let _g = BlyteToolGuard::acquire_none();
        let all = specs();
        assert!(!all.iter().any(|t| t.name == "blyte"));
    }

    {
        let _g = BlyteToolGuard::acquire_script("exit 0\n");
        let all = specs();
        let spec = all.iter().find(|t| t.name == "blyte").unwrap();
        let props = spec.parameters["properties"].as_object().unwrap();
        assert_eq!(props["args"]["type"], "array");
        assert_eq!(props["args"]["items"]["type"], "string");
        assert_eq!(props["background"]["type"], "boolean");
        assert_eq!(spec.parameters["required"], json!(["args"]));
        assert_eq!(
            BLYTE_TOOL_COMMANDS,
            [
                "dispatch",
                "wait",
                "status",
                "review",
                "plan",
                "init",
                "clean",
                "edit-start",
                "edit-finish",
            ]
        );
    }
}

#[test]
fn runs_allowed_command_with_args_cwd_and_parent_env() {
    let root = tempfile::tempdir().unwrap();
    let _g = BlyteToolGuard::acquire_script(
        "printf 'args=%s\\n' \"$*\"\nprintf 'cwd=%s\\n' \"$PWD\"\nprintf 'parent=%s\\n' \"$BLYTE_DISPATCH_PARENT\"\n",
    );

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["status", "T1"]}),
    );
    assert!(!out.is_error, "{}", out.content);
    let expected_cwd = std::fs::canonicalize(root.path()).unwrap();
    let mut seen_args = None;
    let mut seen_cwd = None;
    let mut seen_parent = None;
    for line in out.content.lines() {
        if let Some(v) = line.strip_prefix("args=") {
            seen_args = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("cwd=") {
            seen_cwd = Some(PathBuf::from(v));
        } else if let Some(v) = line.strip_prefix("parent=") {
            seen_parent = Some(v.to_string());
        }
    }
    assert_eq!(seen_args.as_deref(), Some("status T1"));
    assert_eq!(
        seen_cwd.map(|p| std::fs::canonicalize(p).unwrap()),
        Some(expected_cwd)
    );
    assert!(
        seen_parent.as_deref().is_some_and(|p| !p.is_empty()),
        "expected non-empty BLYTE_DISPATCH_PARENT, got {:?}",
        out.content
    );
}

#[test]
fn rejects_empty_or_disallowed_commands_without_running_executable() {
    let root = tempfile::tempdir().unwrap();
    let scripts = tempfile::tempdir().unwrap();
    let marker = scripts.path().join("ran");
    let _g = BlyteToolGuard::acquire_script(&format!(": > '{}'\n", marker.display()));

    let expected_err =
        "the blyte tool runs only: dispatch, wait, status, review, plan, init, clean, edit-start, edit-finish";
    for bad in [json!([]), json!(["comments"]), json!(["--converse", "x"])] {
        let out = execute(
            root.path(),
            &LocalShell,
            &LocalFs,
            "blyte",
            &json!({"args": bad}),
        );
        assert!(out.is_error, "expected error for {bad}");
        assert_eq!(out.content, expected_err);
        assert!(!marker.exists(), "executable ran for {bad}");
    }
}

#[test]
fn background_rules_and_async_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let scripts = tempfile::tempdir().unwrap();
    let gate = scripts.path().join("go");
    let marker = scripts.path().join("done");
    let _g = BlyteToolGuard::acquire_script(&format!(
        "until [ -f '{}' ]; do sleep 0.01; done\n: > '{}'\n",
        gate.display(),
        marker.display()
    ));

    let non_dispatch = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["wait", "T1"], "background": true}),
    );
    assert!(non_dispatch.is_error);
    assert_eq!(
        non_dispatch.content,
        "only dispatch can run in the background"
    );

    let missing_id = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch"], "background": true}),
    );
    assert!(missing_id.is_error);
    assert_eq!(missing_id.content, "dispatch needs an id");

    let start = Instant::now();
    let bg = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1", "t.md"], "background": true}),
    );
    let elapsed = start.elapsed();
    let expected_out = root.path().join(".wt").join("T1.dispatch.out");
    assert!(!bg.is_error, "{}", bg.content);
    assert_eq!(
        bg.content,
        format!(
            "started T1 in the background; use wait to learn when it ends; output: {}",
            expected_out.display()
        )
    );
    assert!(
        elapsed < Duration::from_secs(4),
        "background dispatch blocked for {elapsed:?}"
    );
    assert!(!marker.exists(), "marker appeared too early");

    std::fs::write(&gate, "").unwrap();
    let wait_until = Instant::now() + Duration::from_secs(5);
    while !marker.exists() && Instant::now() < wait_until {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(marker.exists(), "background script never created marker");
}

#[test]
fn nonzero_exit_returns_error_with_stderr_text() {
    let root = tempfile::tempdir().unwrap();
    let _g = BlyteToolGuard::acquire_script("echo 'something went wrong on stderr' >&2\nexit 3\n");

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["status", "T1"]}),
    );
    assert!(out.is_error);
    assert!(
        out.content.contains("something went wrong on stderr"),
        "content was: {:?}",
        out.content
    );
    assert!(
        out.content.contains("exit 3"),
        "content was: {:?}",
        out.content
    );
}

#[test]
fn wait_exit_2_is_still_waiting_not_error() {
    let root = tempfile::tempdir().unwrap();
    let _g = BlyteToolGuard::acquire_script("echo 'still running: IS1' >&2\nexit 2\n");

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["wait", "--any", "IS1"]}),
    );
    assert!(!out.is_error, "expected non-error, got: {:?}", out.content);
    assert_eq!(out.content.lines().next(), Some("still waiting"));
    assert!(
        out.content.contains("still running: IS1"),
        "content was: {:?}",
        out.content
    );
}

#[test]
fn wait_exit_1_and_non_wait_exit_2_remain_errors() {
    let root = tempfile::tempdir().unwrap();
    let _g = BlyteToolGuard::acquire_script(
        "if [ \"$1\" = \"wait\" ]; then echo 'no dispatch IS1 here' >&2; exit 1; fi\nexit 2\n",
    );

    let wait_err = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["wait", "--any", "IS1"]}),
    );
    assert!(wait_err.is_error);
    assert_eq!(wait_err.content.lines().next(), Some("exit 1"));

    let status_err = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["status", "IS1"]}),
    );
    assert!(status_err.is_error);
    assert_eq!(status_err.content.lines().next(), Some("exit 2"));
}

struct RequestReset;

impl Drop for RequestReset {
    fn drop(&mut self) {
        set_blyte_request(None);
    }
}

#[test]
fn dispatch_injects_current_request_and_rejects_model_supplied_request() {
    let root = tempfile::tempdir().unwrap();
    let bg_log = root.path().join("bg.args");
    let bg_tmp = root.path().join("bg.args.tmp");
    let refused_marker = root.path().join("refused.ran");
    let g = BlyteToolGuard::acquire_script("for a in \"$@\"; do printf '%s\\n' \"$a\"; done\n");
    let _reset = RequestReset;

    set_blyte_request(Some("r1700000000-1".to_string()));

    let fg = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1", "t.md"]}),
    );
    assert!(!fg.is_error, "{}", fg.content);
    let fg_lines: Vec<&str> = fg.content.lines().collect();
    assert_eq!(
        fg_lines,
        vec!["dispatch", "T1", "t.md", "--request", "r1700000000-1"]
    );

    let with_sep = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T1", "t.md", "--", "extra"]}),
    );
    assert!(!with_sep.is_error, "{}", with_sep.content);
    let sep_lines: Vec<&str> = with_sep.content.lines().collect();
    assert_eq!(
        sep_lines,
        vec![
            "dispatch",
            "T1",
            "t.md",
            "--request",
            "r1700000000-1",
            "--",
            "extra"
        ]
    );

    let status_out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["status", "T1"]}),
    );
    assert!(!status_out.is_error, "{}", status_out.content);
    let status_lines: Vec<&str> = status_out.content.lines().collect();
    assert_eq!(status_lines, vec!["status", "T1"]);

    let _ = std::fs::remove_file(&bg_log);
    g.set_script(&format!(
        "printf '%s\\n' \"$@\" > '{}' && mv '{}' '{}'\n",
        bg_tmp.display(),
        bg_tmp.display(),
        bg_log.display()
    ));
    let bg = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T2", "t.md"], "background": true}),
    );
    assert!(!bg.is_error, "{}", bg.content);
    let deadline = Instant::now() + Duration::from_secs(5);
    let bg_args = loop {
        if let Ok(raw) = std::fs::read_to_string(&bg_log) {
            if !raw.is_empty() {
                break raw;
            }
        }
        assert!(Instant::now() < deadline, "background script did not run");
        std::thread::sleep(Duration::from_millis(5));
    };
    let bg_lines: Vec<&str> = bg_args.lines().collect();
    assert_eq!(
        bg_lines,
        vec!["dispatch", "T2", "t.md", "--request", "r1700000000-1"]
    );

    g.set_script(&format!(": > '{}'\n", refused_marker.display()));

    for bad_args in [
        json!({"args": ["dispatch", "T3", "t.md", "--request", "spoofed"]}),
        json!({"args": ["dispatch", "T3", "t.md", "--request=spoofed"]}),
        json!({"args": ["dispatch", "--request", "spoofed", "T3", "t.md"]}),
        json!({"args": ["dispatch", "T3", "t.md", "--request", "spoofed"], "background": true}),
    ] {
        let out = execute(root.path(), &LocalShell, &LocalFs, "blyte", &bad_args);
        assert!(out.is_error, "expected error for {bad_args}");
        assert!(
            out.content.contains("--request"),
            "unexpected error for {bad_args}: {}",
            out.content
        );
    }
    assert!(!refused_marker.exists(), "refused dispatch ran executable");

    set_blyte_request(None);
    let refused_no_req = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T4", "t.md", "--request", "spoofed"]}),
    );
    assert!(refused_no_req.is_error);
    assert!(!refused_marker.exists());

    g.set_script("for a in \"$@\"; do printf '%s\\n' \"$a\"; done\n");
    let no_req = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "T5", "t.md"]}),
    );
    assert!(!no_req.is_error, "{}", no_req.content);
    let no_req_lines: Vec<&str> = no_req.content.lines().collect();
    assert_eq!(no_req_lines, vec!["dispatch", "T5", "t.md"]);
}

#[test]
fn edit_start_and_edit_finish_are_accepted_and_request_injected_on_finish() {
    let root = tempfile::tempdir().unwrap();
    let _g = BlyteToolGuard::acquire_script("for a in \"$@\"; do printf '%s\\n' \"$a\"; done\n");
    let _reset = RequestReset;

    set_blyte_request(Some("r1700000000-42".to_string()));

    let start_out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["edit-start", "E1", "--task", "fix typo"]}),
    );
    assert!(!start_out.is_error, "{}", start_out.content);
    let start_lines: Vec<&str> = start_out.content.lines().collect();
    assert_eq!(start_lines, vec!["edit-start", "E1", "--task", "fix typo"]);

    let finish_with_req = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["edit-finish", "E1"]}),
    );
    assert!(!finish_with_req.is_error, "{}", finish_with_req.content);
    let finish_lines: Vec<&str> = finish_with_req.content.lines().collect();
    assert_eq!(
        finish_lines,
        vec!["edit-finish", "E1", "--request", "r1700000000-42"]
    );

    let spoofed = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["edit-finish", "E1", "--request", "spoofed"]}),
    );
    assert!(spoofed.is_error);
    assert!(spoofed.content.contains("--request"));

    let refused = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["combine", "r1700000000-42"]}),
    );
    assert!(refused.is_error);
    assert!(refused.content.contains(
        "error: blyte combine writes a record only the user or the harness may write, and cannot be run from a model's shell"
    ));

    set_blyte_request(None);
    let finish_no_req = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["edit-finish", "E1"]}),
    );
    assert!(!finish_no_req.is_error, "{}", finish_no_req.content);
    let finish_no_req_lines: Vec<&str> = finish_no_req.content.lines().collect();
    assert_eq!(finish_no_req_lines, vec!["edit-finish", "E1"]);
}

#[test]
fn blyte_command_timeout_gives_edit_finish_3600s_and_others_300s() {
    assert_eq!(EDIT_FINISH_TIMEOUT, Duration::from_secs(3600));
    assert_eq!(COMMAND_TIMEOUT, Duration::from_secs(300));
    assert_eq!(
        blyte_command_timeout("edit-finish"),
        Duration::from_secs(3600)
    );
    assert_eq!(blyte_command_timeout("dispatch"), Duration::from_secs(300));
    assert_eq!(
        blyte_command_timeout("edit-start"),
        Duration::from_secs(300)
    );
    assert_eq!(blyte_command_timeout("status"), Duration::from_secs(300));
}

#[test]
fn blyte_tool_passes_edit_finish_message_with_spaces_and_quotes_intact() {
    let root = tempfile::tempdir().unwrap();
    let _g = BlyteToolGuard::acquire_script("for a in \"$@\"; do printf '%s\\n' \"$a\"; done\n");
    let _reset = RequestReset;

    let msg = "fix: handle \"quoted\" y";
    set_blyte_request(Some("r1700000000-99".to_string()));
    let with_req = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["edit-finish", "X", "--message", msg]}),
    );
    assert!(!with_req.is_error, "{}", with_req.content);
    let lines: Vec<&str> = with_req.content.lines().collect();
    assert_eq!(
        lines,
        vec![
            "edit-finish",
            "X",
            "--message",
            msg,
            "--request",
            "r1700000000-99"
        ]
    );

    set_blyte_request(None);
    let without_req = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["edit-finish", "X", "--message", msg]}),
    );
    assert!(!without_req.is_error, "{}", without_req.content);
    let lines: Vec<&str> = without_req.content.lines().collect();
    assert_eq!(lines, vec!["edit-finish", "X", "--message", msg]);
}

#[test]
fn the_tool_refuses_verdict_accepted_fixed_combine_and_model_review() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join("invoked");
    let _g = BlyteToolGuard::acquire_script(&format!(
        "touch '{}'\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done\n",
        marker.display()
    ));

    let cases: &[(&[&str], &str)] = &[
        (
            &["review", "T1", "--verdict", "accepted"],
            "error: blyte review --verdict accepted writes a record only the user or the harness may write, and cannot be run from a model's shell",
        ),
        (
            &["review", "T1", "--verdict=accepted"],
            "error: blyte review --verdict accepted writes a record only the user or the harness may write, and cannot be run from a model's shell",
        ),
        (
            &["review", "T1", "--verdict", "fixed"],
            "error: blyte review --verdict fixed writes a record only the user or the harness may write, and cannot be run from a model's shell",
        ),
        (
            &["review", "T1", "--verdict=fixed"],
            "error: blyte review --verdict fixed writes a record only the user or the harness may write, and cannot be run from a model's shell",
        ),
        (
            &["review", "T1", "--model-review"],
            "error: blyte review --model-review writes a record only the user or the harness may write, and cannot be run from a model's shell",
        ),
        (
            &["combine", "r1700000000-42"],
            "error: blyte combine writes a record only the user or the harness may write, and cannot be run from a model's shell",
        ),
    ];

    for (args, expected) in cases {
        let out = execute(
            root.path(),
            &LocalShell,
            &LocalFs,
            "blyte",
            &json!({"args": args}),
        );
        assert!(out.is_error, "expected refusal for {args:?}");
        assert_eq!(out.content, *expected, "for {args:?}");
        assert!(
            !marker.exists(),
            "fake executable must not run for {args:?}"
        );
    }

    let redone = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["review", "T1", "--verdict", "redone"]}),
    );
    assert!(!redone.is_error, "{}", redone.content);
    assert!(marker.exists());
}

#[test]
fn the_tool_child_does_not_carry_the_marker() {
    let root = tempfile::tempdir().unwrap();
    let env_file = root.path().join("child.env");
    let _g = BlyteToolGuard::acquire_script(&format!(
        "env > '{}'\nprintf 'marker=%s\\n' \"${{BLYTE_MODEL_SHELL:-UNSET}}\"\n",
        env_file.display()
    ));

    struct MarkerGuard(Option<std::ffi::OsString>);
    impl Drop for MarkerGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("BLYTE_MODEL_SHELL", v),
                None => std::env::remove_var("BLYTE_MODEL_SHELL"),
            }
        }
    }
    let _marker = MarkerGuard(std::env::var_os("BLYTE_MODEL_SHELL"));
    std::env::set_var("BLYTE_MODEL_SHELL", "1");

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["status", "T1"]}),
    );
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.content.trim(), "marker=UNSET");
    let recorded = std::fs::read_to_string(&env_file).unwrap();
    assert!(
        !recorded
            .lines()
            .any(|line| line.starts_with("BLYTE_MODEL_SHELL=")),
        "BLYTE_MODEL_SHELL leaked to child: {recorded}"
    );
}

#[test]
fn background_dispatch_early_nonzero_exit_reports_error_and_keeps_output() {
    let root = tempfile::tempdir().unwrap();
    let _g = BlyteToolGuard::acquire_script("printf 'no API key\\n' >&2\nexit 3\n");

    let start = Instant::now();
    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "BD1", "t.md"], "background": true}),
    );
    let elapsed = start.elapsed();

    assert!(out.is_error, "expected error, got: {:?}", out.content);
    assert!(
        out.content
            .contains("dispatch BD1 failed to start (exit 3)"),
        "content was: {:?}",
        out.content
    );
    assert!(
        out.content.contains("no API key"),
        "content was: {:?}",
        out.content
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "early failure waited too long: {elapsed:?}"
    );

    let out_file = root.path().join(".wt").join("BD1.dispatch.out");
    let saved = std::fs::read_to_string(&out_file).unwrap();
    assert!(saved.contains("no API key"), "saved output was: {saved:?}");
}

#[test]
fn background_dispatch_still_running_after_startup_wait_reports_started_and_output_path() {
    let root = tempfile::tempdir().unwrap();
    let _g = BlyteToolGuard::acquire_script("exec sleep 5\n");

    let start = Instant::now();
    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "BD2", "t.md"], "background": true}),
    );
    let elapsed = start.elapsed();
    let expected_path = root.path().join(".wt").join("BD2.dispatch.out");

    assert!(!out.is_error, "expected ok, got: {:?}", out.content);
    assert!(
        out.content.contains("started"),
        "content was: {:?}",
        out.content
    );
    assert!(
        out.content.contains(&expected_path.display().to_string()),
        "expected output path {} in {:?}",
        expected_path.display(),
        out.content
    );
    assert!(
        elapsed < Duration::from_secs(4),
        "background dispatch blocked for {elapsed:?}"
    );
    assert!(expected_path.exists(), "output file was not created");
}

#[test]
fn background_dispatch_immediate_zero_exit_reports_finished_with_output() {
    let root = tempfile::tempdir().unwrap();
    let _g = BlyteToolGuard::acquire_script("printf 'ok\\n'\nexit 0\n");

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "BD3", "t.md"], "background": true}),
    );

    assert!(!out.is_error, "expected ok, got: {:?}", out.content);
    assert!(
        out.content.contains("dispatch BD3 finished"),
        "content was: {:?}",
        out.content
    );
    assert!(out.content.contains("ok"), "content was: {:?}", out.content);

    let out_file = root.path().join(".wt").join("BD3.dispatch.out");
    let saved = std::fs::read_to_string(&out_file).unwrap();
    assert!(saved.contains("ok"), "saved output was: {saved:?}");
}

#[test]
fn background_dispatch_fails_when_wt_is_a_regular_file() {
    let root = tempfile::tempdir().unwrap();
    let wt_path = root.path().join(".wt");
    std::fs::write(&wt_path, "not a directory").unwrap();
    let marker = root.path().join("ran");
    let _g = BlyteToolGuard::acquire_script(&format!(": > '{}'\n", marker.display()));

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "BD4", "t.md"], "background": true}),
    );

    assert!(out.is_error, "expected error, got: {:?}", out.content);
    assert!(
        out.content.contains(&wt_path.display().to_string()),
        "expected error naming {}, got: {:?}",
        wt_path.display(),
        out.content
    );
    assert!(!marker.exists(), "stub executable should not have run");
}

#[test]
fn background_dispatch_truncates_previous_output_and_bounds_large_early_failure() {
    let root = tempfile::tempdir().unwrap();
    let wt_dir = root.path().join(".wt");
    std::fs::create_dir_all(&wt_dir).unwrap();
    let out_file = wt_dir.join("BD5.dispatch.out");
    std::fs::write(&out_file, "stale previous output\n").unwrap();

    let total_lines = OUTPUT_LINE_BUDGET + 200;
    let _g = BlyteToolGuard::acquire_script(&format!(
        "i=1\nwhile [ \"$i\" -le {total_lines} ]; do printf 'line %d\\n' \"$i\" >&2; i=$((i + 1)); done\nexit 4\n"
    ));

    let out = execute(
        root.path(),
        &LocalShell,
        &LocalFs,
        "blyte",
        &json!({"args": ["dispatch", "BD5", "t.md"], "background": true}),
    );

    assert!(out.is_error, "expected error, got: {:?}", out.content);
    assert!(
        out.content
            .contains("dispatch BD5 failed to start (exit 4)"),
        "content was: {:?}",
        out.content
    );
    assert!(
        !out.content.contains("stale previous output"),
        "previous output was not truncated"
    );
    assert!(
        out.content.contains("elided by blyte"),
        "large output was not bounded"
    );

    let saved = std::fs::read_to_string(&out_file).unwrap();
    assert!(!saved.contains("stale previous output"));
    assert_eq!(saved.lines().count(), total_lines);
}
