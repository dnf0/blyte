use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use blyte_agent::approval::Approval;
use blyte_agent::orchestrator::Answers;
use blyte_agent::sandbox;
use blyte_agent::shell::Shell;
use blyte_cli::approval_file;
use blyte_cli::cli::Cli;
use blyte_cli::instructions_cmd;
use blyte_cli::orchestrator_text;
use blyte_cli::repl::{render_banner_to, ReplConfig, ReplSession};
use blyte_cli::task_runner::{
    build_local_shell, build_sandbox_profile, run_task_with_state_and_events, TaskConfig,
};
use blyte_cli::tty_approver::decide;
use clap::Parser;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    prev_xdg: Option<OsString>,
    prev_home: Option<OsString>,
}

impl EnvGuard {
    fn set(xdg: &Path, home: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("XDG_CONFIG_HOME", xdg);
        std::env::set_var("HOME", home);
        Self {
            _lock: lock,
            prev_xdg,
            prev_home,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.prev_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match &self.prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

fn blyte_cmd(root: &Path, home: &Path, xdg: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_blyte"));
    cmd.current_dir(root)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", xdg)
        .env("BLYTE_STATE_DIR", home.join("state"))
        .env("BLYTE_NO_UPDATE_CHECK", "1")
        .env("BLYTE_ENV_FILE", "")
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("OLLAMA_HOST");
    cmd
}

#[test]
fn decide_accepts_only_y_or_yes() {
    assert_eq!(decide("y"), Approval::Approved);
    assert_eq!(decide("Y"), Approval::Approved);
    assert_eq!(decide(" yes "), Approval::Approved);
    assert_eq!(decide("n"), Approval::Denied);
    assert_eq!(decide(""), Approval::Denied);
    assert_eq!(decide("yep"), Approval::Denied);
}

#[test]
fn approval_file_load_handles_missing_valid_and_unreadable() {
    let home = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(xdg.path(), home.path());

    assert_eq!(approval_file::load().unwrap(), Vec::<String>::new());

    let allow_path = xdg.path().join("blyte").join("allow-unsandboxed");
    std::fs::create_dir_all(allow_path.parent().unwrap()).unwrap();
    std::fs::write(&allow_path, "git push\n\n  gh pr  \n").unwrap();
    assert_eq!(
        approval_file::load().unwrap(),
        vec!["git push".to_string(), "gh pr".to_string()]
    );

    std::fs::remove_file(&allow_path).unwrap();
    std::fs::create_dir(&allow_path).unwrap();
    let err = approval_file::load().unwrap_err();
    assert!(
        err.contains(&allow_path.display().to_string()),
        "expected path in error, got: {err}"
    );
}

#[test]
fn orchestrate_task_config_sandbox_and_network_defaults() {
    let cli = Cli::try_parse_from(["blyte", "--orchestrate"]).unwrap();
    let cfg = TaskConfig::from(&cli);
    assert_eq!(cfg.sandbox, "on");
    assert_eq!(cfg.network, "off");

    let cli_net_on = Cli::try_parse_from(["blyte", "--orchestrate", "--network", "on"]).unwrap();
    let cfg_net_on = TaskConfig::from(&cli_net_on);
    assert_eq!(cfg_net_on.sandbox, "on");
    assert_eq!(cfg_net_on.network, "on");

    let cli_sb_off = Cli::try_parse_from(["blyte", "--orchestrate", "--sandbox", "off"]).unwrap();
    let cfg_sb_off = TaskConfig::from(&cli_sb_off);
    assert_eq!(cfg_sb_off.sandbox, "off");
}

#[test]
fn orchestrator_shell_allows_wt_tasks_and_blocks_root_git_and_wt_passed() {
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let root = tempfile::tempdir_in(tmp_base).unwrap();
    let outside = tempfile::tempdir_in(tmp_base).unwrap();

    let git_ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .status()
        .unwrap();
    assert!(git_ok.success());
    let tasks = root.path().join(".wt/tasks");
    let passed = root.path().join(".wt/passed");
    std::fs::create_dir_all(&tasks).unwrap();
    std::fs::create_dir_all(&passed).unwrap();

    let cli = Cli::try_parse_from(["blyte", "--orchestrate"]).unwrap();
    let mut cfg = TaskConfig::from(&cli);
    cfg.root = root.path().to_path_buf();
    assert_eq!(cfg.sandbox, "on");

    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root_canon = canon(root.path());
    let tasks_canon = canon(&tasks);
    let git_canon = canon(&root.path().join(".git"));
    let wt_canon = canon(&root.path().join(".wt"));
    match build_sandbox_profile(&cfg) {
        sandbox::Profile::Seatbelt(s) => {
            assert!(
                s.contains(&format!(
                    "(allow file-write* (subpath \"{}\"))",
                    tasks_canon.display()
                )),
                "{s}"
            );
            assert!(
                !s.contains(&format!(
                    "(allow file-write* (subpath \"{}\"))",
                    root_canon.display()
                )),
                "{s}"
            );
            assert!(
                !s.contains(&format!(
                    "(allow file-write* (subpath \"{}\"))",
                    git_canon.display()
                )),
                "{s}"
            );
            assert!(
                !s.contains(&format!(
                    "(allow file-write* (subpath \"{}\"))",
                    wt_canon.display()
                )),
                "{s}"
            );
        }
        sandbox::Profile::LandlockConfined { write, .. }
        | sandbox::Profile::Landlock(write)
        | sandbox::Profile::LandlockOffline(write) => {
            assert!(write.iter().any(|p| canon(p) == tasks_canon), "{write:?}");
            assert!(!write.iter().any(|p| canon(p) == root_canon), "{write:?}");
            assert!(!write.iter().any(|p| canon(p) == git_canon), "{write:?}");
            assert!(!write.iter().any(|p| canon(p) == wt_canon), "{write:?}");
        }
    }

    if !sandbox::available() {
        return;
    }

    let shell = build_local_shell(&cfg);

    let task_file = tasks.join("task.md");
    let probe = shell
        .exec(
            &format!("echo inside > '{}'", task_file.display()),
            root.path(),
            Duration::from_secs(30),
        )
        .unwrap();
    if probe.exit_code == 71 && probe.merged().contains("Operation not permitted") {
        return;
    }
    assert_eq!(probe.exit_code, 0, "{}", probe.merged());
    assert!(task_file.exists());

    for denied_file in [
        root.path().join("inside.txt"),
        root.path().join(".git/forged"),
        passed.join("forged.json"),
        outside.path().join("escaped.txt"),
    ] {
        let denied = shell
            .exec(
                &format!("echo no > '{}'", denied_file.display()),
                root.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(
            denied.exit_code,
            0,
            "write to {} must fail",
            denied_file.display()
        );
        assert!(
            !denied_file.exists(),
            "{} must not be created",
            denied_file.display()
        );
    }
}

#[test]
fn orchestrator_shell_in_linked_worktree_allows_only_main_wt_tasks() {
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let main_repo = tempfile::tempdir_in(tmp_base).unwrap();
    let outside = tempfile::tempdir_in(tmp_base).unwrap();

    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());
    std::fs::write(main_repo.path().join("README.md"), "init\n").unwrap();
    assert!(Command::new("git")
        .args(["add", "README.md"])
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "--no-gpg-sign", "-q", "-m", "init"])
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());

    let wt_dir = main_repo.path().join(".wt");
    let tasks_dir = wt_dir.join("tasks");
    let passed_dir = wt_dir.join("passed");
    std::fs::create_dir_all(&tasks_dir).unwrap();
    std::fs::create_dir_all(&passed_dir).unwrap();
    let linked = wt_dir.join("orch");
    assert!(Command::new("git")
        .args(["worktree", "add", "-q", "-b", "orch"])
        .arg(&linked)
        .current_dir(main_repo.path())
        .status()
        .unwrap()
        .success());

    let cli = Cli::try_parse_from(["blyte", "--orchestrate"]).unwrap();
    let mut cfg = TaskConfig::from(&cli);
    cfg.root = linked.clone();

    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let main_tasks_canon = canon(&tasks_dir);
    let linked_canon = canon(&linked);
    let wt_canon = canon(&wt_dir);
    let git_canon = canon(&main_repo.path().join(".git"));
    match build_sandbox_profile(&cfg) {
        sandbox::Profile::Seatbelt(s) => {
            assert!(
                s.contains(&format!(
                    "(allow file-write* (subpath \"{}\"))",
                    main_tasks_canon.display()
                )),
                "{s}"
            );
            assert!(
                !s.contains(&format!(
                    "(allow file-write* (subpath \"{}\"))",
                    linked_canon.display()
                )),
                "{s}"
            );
            assert!(
                !s.contains(&format!(
                    "(allow file-write* (subpath \"{}\"))",
                    wt_canon.display()
                )),
                "{s}"
            );
            assert!(
                !s.contains(&format!(
                    "(allow file-write* (subpath \"{}\"))",
                    git_canon.display()
                )),
                "{s}"
            );
        }
        sandbox::Profile::LandlockConfined { write, .. }
        | sandbox::Profile::Landlock(write)
        | sandbox::Profile::LandlockOffline(write) => {
            assert!(
                write.iter().any(|p| canon(p) == main_tasks_canon),
                "{write:?}"
            );
            assert!(!write.iter().any(|p| canon(p) == linked_canon), "{write:?}");
            assert!(!write.iter().any(|p| canon(p) == wt_canon), "{write:?}");
            assert!(!write.iter().any(|p| canon(p) == git_canon), "{write:?}");
        }
    }

    if !sandbox::available() {
        return;
    }

    let shell = build_local_shell(&cfg);

    let task_file = tasks_dir.join("from-wt.md");
    let wt_write = shell
        .exec(
            &format!("echo ok > '{}'", task_file.display()),
            &linked,
            Duration::from_secs(30),
        )
        .unwrap();
    if wt_write.exit_code == 71 && wt_write.merged().contains("Operation not permitted") {
        return;
    }
    assert_eq!(wt_write.exit_code, 0, "{}", wt_write.merged());
    assert!(task_file.exists());

    for denied_file in [
        wt_dir.join("marker.txt"),
        passed_dir.join("forged.json"),
        linked.join("wt.txt"),
        main_repo.path().join(".git/forged"),
        outside.path().join("no.txt"),
    ] {
        let denied = shell
            .exec(
                &format!("echo no > '{}'", denied_file.display()),
                &linked,
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(
            denied.exit_code,
            0,
            "write to {} must fail",
            denied_file.display()
        );
        assert!(
            !denied_file.exists(),
            "{} must not be created",
            denied_file.display()
        );
    }
}

#[test]
fn blyte_instructions_includes_unsandboxed_and_switches_sandbox_paragraph() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .status()
        .unwrap()
        .success());

    let out_on = blyte_cmd(root.path(), home.path(), xdg.path())
        .args(["instructions"])
        .output()
        .unwrap();
    assert!(
        out_on.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out_on.stderr)
    );
    let stdout_on = String::from_utf8(out_on.stdout).unwrap();
    assert!(stdout_on.contains("unsandboxed"), "{stdout_on}");
    assert!(stdout_on.contains("The shell is sandboxed:"), "{stdout_on}");
    assert!(
        stdout_on.contains(
            "the shell and the write tools can write only in `.wt/tasks` and temporary directories"
        ),
        "{stdout_on}"
    );
    assert!(
        !stdout_on.contains("The shell is not sandboxed."),
        "{stdout_on}"
    );
    assert!(stdout_on.contains("## Tool descriptions"), "{stdout_on}");
    assert!(stdout_on.contains("reason"), "{stdout_on}");

    let out_off = blyte_cmd(root.path(), home.path(), xdg.path())
        .args(["instructions", "--sandbox", "off"])
        .output()
        .unwrap();
    assert!(
        out_off.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out_off.stderr)
    );
    let stdout_off = String::from_utf8(out_off.stdout).unwrap();
    assert!(
        stdout_off.contains("The shell is not sandboxed."),
        "{stdout_off}"
    );
    assert!(
        !stdout_off.contains("The shell is sandboxed:"),
        "{stdout_off}"
    );
    assert!(!stdout_off.contains("unsandboxed"), "{stdout_off}");

    let out_global_off = blyte_cmd(root.path(), home.path(), xdg.path())
        .args(["--sandbox", "off", "instructions"])
        .output()
        .unwrap();
    assert!(out_global_off.status.success());
    let stdout_global_off = String::from_utf8(out_global_off.stdout).unwrap();
    assert!(
        stdout_global_off.contains("The shell is not sandboxed."),
        "{stdout_global_off}"
    );

    let _guard = EnvGuard::set(xdg.path(), home.path());
    let rendered_on =
        instructions_cmd::render_with_sandbox(false, Answers::Person, root.path(), true).unwrap();
    assert!(rendered_on.contains("The shell is sandboxed:"));
    let rendered_off =
        instructions_cmd::render_with_sandbox(false, Answers::Person, root.path(), false).unwrap();
    assert!(rendered_off.contains("The shell is not sandboxed."));
    assert!(!rendered_off.contains("The shell is sandboxed:"));
}

#[test]
fn start_lines_and_banner_report_sandbox_state() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();

    let allow_path = xdg.path().join("blyte").join("allow-unsandboxed");
    std::fs::create_dir_all(allow_path.parent().unwrap()).unwrap();
    std::fs::write(&allow_path, "git push\ngh pr\n").unwrap();

    {
        let _guard = EnvGuard::set(xdg.path(), home.path());

        let built_on =
            orchestrator_text::build_with_sandbox(false, Answers::Person, root.path(), true, 2)
                .unwrap();
        assert_eq!(
            built_on.start_lines[2],
            "sandbox: on (a command leaves it only with your approval; 2 allowed in advance)"
        );
        assert!(built_on.instructions.contains("The shell is sandboxed:"));

        let built_off =
            orchestrator_text::build_with_sandbox(false, Answers::Person, root.path(), false, 0)
                .unwrap();
        assert_eq!(built_off.start_lines[2], "sandbox: off");
        assert!(built_off
            .instructions
            .contains("The shell is not sandboxed."));
        assert!(!built_off.instructions.contains("The shell is sandboxed:"));

        let session_on = ReplSession::try_new(&ReplConfig {
            repo_root: root.path().to_path_buf(),
            orchestrate: true,
            ..Default::default()
        })
        .unwrap();
        let mut banner_on = Vec::new();
        render_banner_to(&mut banner_on, &session_on).unwrap();
        let banner_on = String::from_utf8(banner_on).unwrap();
        assert!(
            banner_on.contains(
                "  Sandbox    : on (a command leaves it only with your approval; 2 allowed in advance)"
            ),
            "{banner_on}"
        );

        let session_off = ReplSession::try_new(&ReplConfig {
            repo_root: root.path().to_path_buf(),
            orchestrate: true,
            sandbox: "off".to_string(),
            ..Default::default()
        })
        .unwrap();
        let mut banner_off = Vec::new();
        render_banner_to(&mut banner_off, &session_off).unwrap();
        let banner_off = String::from_utf8(banner_off).unwrap();
        assert!(banner_off.contains("  Sandbox    : off"), "{banner_off}");
    }

    let out_on = blyte_cmd(root.path(), home.path(), xdg.path())
        .args(["--orchestrate", "--", "plan the release"])
        .output()
        .unwrap();
    let stderr_on = String::from_utf8_lossy(&out_on.stderr);
    assert!(
        stderr_on.contains(
            "blyte: sandbox: on (a command leaves it only with your approval; 2 allowed in advance)"
        ),
        "{stderr_on}"
    );

    let out_off = blyte_cmd(root.path(), home.path(), xdg.path())
        .args([
            "--orchestrate",
            "--sandbox",
            "off",
            "--",
            "plan the release",
        ])
        .output()
        .unwrap();
    let stderr_off = String::from_utf8_lossy(&out_off.stderr);
    assert!(stderr_off.contains("blyte: sandbox: off"), "{stderr_off}");
}

#[test]
fn write_scope_is_set_only_for_orchestrate_with_sandbox_on() {
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let root = tempfile::tempdir_in(tmp_base).unwrap();
    let home = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .status()
        .unwrap()
        .success());

    let _guard = EnvGuard::set(xdg.path(), home.path());

    let _session_on = ReplSession::try_new(&ReplConfig {
        repo_root: root.path().to_path_buf(),
        orchestrate: true,
        sandbox: "on".to_string(),
        ..Default::default()
    })
    .unwrap();
    let tasks_dir = root.path().join(".wt/tasks");
    assert!(tasks_dir.is_dir(), ".wt/tasks must be created on start");
    let tasks_canon = tasks_dir.canonicalize().unwrap();
    let root_canon = root.path().canonicalize().unwrap();
    let scope = blyte_agent::tools::write_scope().expect("orchestrate + sandbox on must set scope");
    assert!(scope.allowed.contains(&tasks_canon), "{:?}", scope.allowed);
    assert!(!scope.allowed.contains(&root_canon), "{:?}", scope.allowed);
    assert!(
        !scope.allowed.contains(&root_canon.join(".git")),
        "{:?}",
        scope.allowed
    );
    assert!(
        !scope.allowed.contains(&root_canon.join(".wt")),
        "{:?}",
        scope.allowed
    );

    let _session_off = ReplSession::try_new(&ReplConfig {
        repo_root: root.path().to_path_buf(),
        orchestrate: true,
        sandbox: "off".to_string(),
        ..Default::default()
    })
    .unwrap();
    assert!(blyte_agent::tools::write_scope().is_none());

    let _session_on_again = ReplSession::try_new(&ReplConfig {
        repo_root: root.path().to_path_buf(),
        orchestrate: true,
        sandbox: "on".to_string(),
        ..Default::default()
    })
    .unwrap();
    assert!(blyte_agent::tools::write_scope().is_some());

    let mut impl_state = blyte_agent::state::State::new("impl", String::new());
    let cli_impl = Cli::try_parse_from(["blyte", "--model", "unknown-model-for-test"]).unwrap();
    let mut impl_cfg = TaskConfig::from(&cli_impl);
    impl_cfg.root = root.path().to_path_buf();
    impl_cfg.sandbox = "on".to_string();
    assert!(!impl_cfg.orchestrate);
    let _ = run_task_with_state_and_events(&impl_cfg, &mut impl_state, &|_| {});
    assert!(
        blyte_agent::tools::write_scope().is_none(),
        "implementer run must not set write scope"
    );

    let mut orch_state = blyte_agent::state::State::new("orch", String::new());
    let cli_orch = Cli::try_parse_from([
        "blyte",
        "--orchestrate",
        "--model",
        "unknown-model-for-test",
    ])
    .unwrap();
    let mut orch_cfg = TaskConfig::from(&cli_orch);
    orch_cfg.root = root.path().to_path_buf();
    let _ = run_task_with_state_and_events(&orch_cfg, &mut orch_state, &|_| {});
    let scope = blyte_agent::tools::write_scope()
        .expect("run_task with orchestrate + sandbox on must set scope");
    assert!(scope.allowed.contains(&tasks_canon), "{:?}", scope.allowed);

    let mut orch_off_state = blyte_agent::state::State::new("orch-off", String::new());
    let cli_orch_off = Cli::try_parse_from([
        "blyte",
        "--orchestrate",
        "--sandbox",
        "off",
        "--model",
        "unknown-model-for-test",
    ])
    .unwrap();
    let mut orch_off_cfg = TaskConfig::from(&cli_orch_off);
    orch_off_cfg.root = root.path().to_path_buf();
    let _ = run_task_with_state_and_events(&orch_off_cfg, &mut orch_off_state, &|_| {});
    assert!(
        blyte_agent::tools::write_scope().is_none(),
        "run_task with orchestrate + sandbox off must clear scope"
    );

    blyte_agent::tools::set_write_scope(None);
    blyte_agent::tools::set_approval(None);
}
