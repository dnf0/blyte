use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use blyte_agent::approval::Approval;
use blyte_agent::conversation::Answers;
use blyte_agent::sandbox;
use blyte_agent::shell::Shell;
use blyte_cli::approval_file;
use blyte_cli::cli::Cli;
use blyte_cli::conversation_text;
use blyte_cli::instructions_cmd;
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
fn converse_task_config_sandbox_and_network_defaults() {
    let cli = Cli::try_parse_from(["blyte", "--converse"]).unwrap();
    let cfg = TaskConfig::from(&cli);
    assert_eq!(cfg.sandbox, "on");
    assert_eq!(cfg.network, "off");

    let cli_net_on = Cli::try_parse_from(["blyte", "--converse", "--network", "on"]).unwrap();
    let cfg_net_on = TaskConfig::from(&cli_net_on);
    assert_eq!(cfg_net_on.sandbox, "on");
    assert_eq!(cfg_net_on.network, "on");

    let cli_sb_off = Cli::try_parse_from(["blyte", "--converse", "--sandbox", "off"]).unwrap();
    let cfg_sb_off = TaskConfig::from(&cli_sb_off);
    assert_eq!(cfg_sb_off.sandbox, "off");
}

#[test]
fn conversation_shell_allows_wt_tasks_and_blocks_root_git_and_wt_passed() {
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

    let cli = Cli::try_parse_from(["blyte", "--converse"]).unwrap();
    let mut cfg = TaskConfig::from(&cli);
    cfg.root = root.path().to_path_buf();
    assert_eq!(cfg.sandbox, "on");

    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root_canon = canon(root.path());
    let tasks_canon = canon(&tasks);
    let git_canon = canon(&root.path().join(".git"));
    let wt_canon = canon(&root.path().join(".wt"));
    match build_sandbox_profile(&cfg).unwrap() {
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

    let shell = build_local_shell(&cfg).unwrap();

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
fn conversation_shell_in_linked_worktree_allows_only_main_wt_tasks() {
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

    let cli = Cli::try_parse_from(["blyte", "--converse"]).unwrap();
    let mut cfg = TaskConfig::from(&cli);
    cfg.root = linked.clone();

    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let main_tasks_canon = canon(&tasks_dir);
    let linked_canon = canon(&linked);
    let wt_canon = canon(&wt_dir);
    let git_canon = canon(&main_repo.path().join(".git"));
    match build_sandbox_profile(&cfg).unwrap() {
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

    let shell = build_local_shell(&cfg).unwrap();

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
        stdout_on.contains("The shell and the write tools can also write in the worktree of an"),
        "{stdout_on}"
    );
    assert!(
        stdout_on.contains("`git commit` there is not possible (`edit-finish`"),
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
            conversation_text::build_with_sandbox(false, Answers::Person, root.path(), true, 2)
                .unwrap();
        assert_eq!(
            built_on.start_lines[2],
            "sandbox: on (a command leaves it only with your approval; 2 allowed in advance)"
        );
        assert!(built_on.instructions.contains("The shell is sandboxed:"));

        let built_off =
            conversation_text::build_with_sandbox(false, Answers::Person, root.path(), false, 0)
                .unwrap();
        assert_eq!(built_off.start_lines[2], "sandbox: off");
        assert!(built_off
            .instructions
            .contains("The shell is not sandboxed."));
        assert!(!built_off.instructions.contains("The shell is sandboxed:"));

        let session_on = ReplSession::try_new(&ReplConfig {
            repo_root: root.path().to_path_buf(),
            converse: true,
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
            converse: true,
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
        .args(["--converse", "--", "plan the release"])
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
        .args(["--converse", "--sandbox", "off", "--", "plan the release"])
        .output()
        .unwrap();
    let stderr_off = String::from_utf8_lossy(&out_off.stderr);
    assert!(stderr_off.contains("blyte: sandbox: off"), "{stderr_off}");
}

#[test]
fn write_scope_is_set_only_for_converse_with_sandbox_on() {
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
        converse: true,
        sandbox: "on".to_string(),
        ..Default::default()
    })
    .unwrap();
    let tasks_dir = root.path().join(".wt/tasks");
    assert!(tasks_dir.is_dir(), ".wt/tasks must be created on start");
    let tasks_canon = tasks_dir.canonicalize().unwrap();
    let root_canon = root.path().canonicalize().unwrap();
    let scope = blyte_agent::tools::write_scope().expect("converse + sandbox on must set scope");
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
        converse: true,
        sandbox: "off".to_string(),
        ..Default::default()
    })
    .unwrap();
    assert!(blyte_agent::tools::write_scope().is_none());

    let _session_on_again = ReplSession::try_new(&ReplConfig {
        repo_root: root.path().to_path_buf(),
        converse: true,
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
    assert!(!impl_cfg.converse);
    let _ = run_task_with_state_and_events(&impl_cfg, &mut impl_state, &|_| {});
    assert!(
        blyte_agent::tools::write_scope().is_none(),
        "implementer run must not set write scope"
    );

    let mut orch_state = blyte_agent::state::State::new("orch", String::new());
    let cli_orch =
        Cli::try_parse_from(["blyte", "--converse", "--model", "unknown-model-for-test"]).unwrap();
    let mut orch_cfg = TaskConfig::from(&cli_orch);
    orch_cfg.root = root.path().to_path_buf();
    let _ = run_task_with_state_and_events(&orch_cfg, &mut orch_state, &|_| {});
    let scope = blyte_agent::tools::write_scope()
        .expect("run_task with converse + sandbox on must set scope");
    assert!(scope.allowed.contains(&tasks_canon), "{:?}", scope.allowed);

    let mut orch_off_state = blyte_agent::state::State::new("orch-off", String::new());
    let cli_orch_off = Cli::try_parse_from([
        "blyte",
        "--converse",
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
        "run_task with converse + sandbox off must clear scope"
    );

    blyte_agent::tools::set_write_scope(None);
    blyte_agent::tools::set_approval(None);
}

#[test]
fn sandboxed_shell_cannot_write_the_usage_ledger_directory() {
    if !sandbox::available() {
        return;
    }

    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let home = tempfile::tempdir_in(tmp_base).unwrap();
    let xdg = tempfile::tempdir_in(tmp_base).unwrap();
    let repo = tempfile::tempdir_in(tmp_base).unwrap();

    let blyte_state = home.path().join(".local").join("state").join("blyte");
    let local_bin = home.path().join(".local").join("bin");
    std::fs::create_dir_all(&blyte_state).unwrap();
    std::fs::create_dir_all(&local_bin).unwrap();

    let _env = EnvGuard::set(xdg.path(), home.path());
    struct XdgStateGuard(Option<OsString>);
    impl Drop for XdgStateGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
    }
    let _xdg_state = XdgStateGuard(std::env::var_os("XDG_STATE_HOME"));
    std::env::remove_var("XDG_STATE_HOME");

    for profile in [
        sandbox::conversation_profile(repo.path(), &[]).unwrap(),
        sandbox::default_profile(repo.path(), &[]),
    ] {
        let sh = blyte_agent::shell::LocalShell::sandboxed(profile);

        let bin_target = local_bin.join("x");
        let _ = std::fs::remove_file(&bin_target);
        let allowed = sh
            .exec_user(
                &format!("echo ok > '{}'", bin_target.display()),
                repo.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        if allowed.exit_code == 71 && allowed.merged().contains("Operation not permitted") {
            return;
        }
        assert_eq!(
            allowed.exit_code,
            0,
            "write to .local/bin should succeed: {}",
            allowed.merged()
        );
        assert!(bin_target.exists());

        let ledger_target = blyte_state.join("x");
        let _ = std::fs::remove_file(&ledger_target);
        let denied = sh
            .exec_user(
                &format!("echo no > '{}'", ledger_target.display()),
                repo.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(
            denied.exit_code,
            0,
            "write to .local/state/blyte should fail: {}",
            denied.merged()
        );
        assert!(!ledger_target.exists());
    }
}

fn init_git_repo_in_target_tmp() -> tempfile::TempDir {
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let root = tempfile::tempdir_in(tmp_base).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(root.path())
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(root.path())
        .status()
        .unwrap()
        .success());
    std::fs::write(root.path().join("README.md"), "init\n").unwrap();
    assert!(Command::new("git")
        .args(["add", "README.md"])
        .current_dir(root.path())
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args(["commit", "--no-gpg-sign", "-q", "-m", "init"])
        .current_dir(root.path())
        .status()
        .unwrap()
        .success());
    std::fs::create_dir_all(root.path().join(".wt/tasks")).unwrap();
    root
}

fn conversation_config(root: &Path) -> TaskConfig {
    let cli = Cli::try_parse_from(["blyte", "--converse"]).unwrap();
    let mut cfg = TaskConfig::from(&cli);
    cfg.root = root.to_path_buf();
    cfg
}

fn open_edit_worktree(root: &Path, id: &str) -> PathBuf {
    let wt = root.join(".wt").join(id);
    assert!(Command::new("git")
        .args(["worktree", "add", "-q", "-b", &format!("wip/{id}")])
        .arg(&wt)
        .arg("HEAD")
        .current_dir(root)
        .status()
        .unwrap()
        .success());
    std::fs::write(root.join(".wt").join(format!("{id}.edit.json")), "{}\n").unwrap();
    wt
}

#[test]
fn shell_writes_in_an_open_edit_worktree_and_nowhere_else_new() {
    if !sandbox::available() {
        return;
    }
    // The profile depends on HOME, which other tests here change under this lock.
    let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let root = init_git_repo_in_target_tmp();
    let e1 = open_edit_worktree(root.path(), "E1");
    let other = root.path().join(".wt/other");
    std::fs::create_dir_all(&other).unwrap();

    let cfg = conversation_config(root.path());
    let shell = build_local_shell(&cfg).unwrap();

    let e1_file = e1.join("f.txt");
    let ok = shell
        .exec_user(
            &format!("echo ok > '{}'", e1_file.display()),
            root.path(),
            Duration::from_secs(30),
        )
        .unwrap();
    if ok.exit_code == 71 && ok.merged().contains("Operation not permitted") {
        return;
    }
    assert_eq!(
        ok.exit_code,
        0,
        "write in open edit must succeed: {}",
        ok.merged()
    );
    assert_eq!(std::fs::read_to_string(&e1_file).unwrap(), "ok\n");

    #[cfg(target_os = "macos")]
    {
        let e1_git = e1.join(".git");
        let before = std::fs::read_to_string(&e1_git).unwrap();
        let denied_git = shell
            .exec_user(
                &format!("echo tampered > '{}'", e1_git.display()),
                root.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(
            denied_git.exit_code,
            0,
            "write to .wt/E1/.git must fail on macOS: {}",
            denied_git.merged()
        );
        assert_eq!(std::fs::read_to_string(&e1_git).unwrap(), before);
    }

    for (label, target) in [
        (".wt/other/f.txt", other.join("f.txt")),
        ("<root>/f.txt", root.path().join("f.txt")),
        ("<root>/.git/x", root.path().join(".git/x")),
    ] {
        let r = shell
            .exec_user(
                &format!("echo no > '{}'", target.display()),
                root.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(r.exit_code, 0, "write to {label} must fail: {}", r.merged());
        assert!(!target.exists(), "{label} must not be created");
    }

    let head_before = Command::new("git")
        .args(["-C"])
        .arg(&e1)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let commit = shell
        .exec_user(
            &format!(
                "git -C '{}' commit --no-gpg-sign --allow-empty -m nope",
                e1.display()
            ),
            root.path(),
            Duration::from_secs(30),
        )
        .unwrap();
    assert_ne!(
        commit.exit_code,
        0,
        "git commit in open edit worktree must fail: {}",
        commit.merged()
    );
    let head_after = Command::new("git")
        .args(["-C"])
        .arg(&e1)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(head_before.stdout, head_after.stdout);
}

#[test]
fn shell_loses_the_worktree_when_the_edit_finishes() {
    if !sandbox::available() {
        return;
    }
    // The profile depends on HOME, which other tests here change under this lock.
    let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let root = init_git_repo_in_target_tmp();
    let e1 = open_edit_worktree(root.path(), "E1");

    let cfg = conversation_config(root.path());
    let shell = build_local_shell(&cfg).unwrap();

    let first_file = e1.join("a.txt");
    let first = shell
        .exec_user(
            &format!("echo first > '{}'", first_file.display()),
            root.path(),
            Duration::from_secs(30),
        )
        .unwrap();
    if first.exit_code == 71 && first.merged().contains("Operation not permitted") {
        return;
    }
    assert_eq!(first.exit_code, 0, "{}", first.merged());
    assert!(first_file.exists());

    std::fs::write(root.path().join(".wt/E1.json"), "{}\n").unwrap();

    let second_file = e1.join("b.txt");
    let second = shell
        .exec_user(
            &format!("echo second > '{}'", second_file.display()),
            root.path(),
            Duration::from_secs(30),
        )
        .unwrap();
    assert_ne!(
        second.exit_code,
        0,
        "write after edit finishes must fail: {}",
        second.merged()
    );
    assert!(!second_file.exists());
    assert!(
        second
            .merged()
            .contains(blyte_agent::shell::SHELL_RESTART_NOTICE),
        "{}",
        second.merged()
    );
}

#[test]
fn shell_keeps_its_directory_and_says_when_it_restarted() {
    if !sandbox::available() {
        return;
    }
    // The profile depends on HOME, which other tests here change under this lock.
    let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let root = init_git_repo_in_target_tmp();
    let sub = root.path().join("subdir");
    std::fs::create_dir_all(&sub).unwrap();

    let cfg = conversation_config(root.path());
    let shell = build_local_shell(&cfg).unwrap();

    let setup = shell
        .exec_user(
            "cd subdir && export X=1 && pwd",
            root.path(),
            Duration::from_secs(30),
        )
        .unwrap();
    if setup.exit_code == 71 && setup.merged().contains("Operation not permitted") {
        return;
    }
    assert_eq!(setup.exit_code, 0, "{}", setup.merged());
    assert!(
        !setup
            .merged()
            .contains(blyte_agent::shell::SHELL_RESTART_NOTICE),
        "{}",
        setup.merged()
    );
    let expected_pwd = setup.stdout.trim().to_string();
    assert!(expected_pwd.ends_with("subdir"), "{expected_pwd}");

    open_edit_worktree(root.path(), "E1");

    let after_open = shell
        .exec_user(
            r#"printf 'PWD=%s\nX=%s\n' "$(pwd)" "$X""#,
            root.path(),
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(after_open.exit_code, 0, "{}", after_open.merged());
    let lines: Vec<&str> = after_open.stdout.lines().collect();
    assert_eq!(
        lines.first().copied(),
        Some(blyte_agent::shell::SHELL_RESTART_NOTICE),
        "{}",
        after_open.stdout
    );
    assert_eq!(
        lines.get(1).copied(),
        Some(format!("PWD={expected_pwd}").as_str()),
        "{}",
        after_open.stdout
    );
    assert_eq!(lines.get(2).copied(), Some("X="), "{}", after_open.stdout);

    let next = shell
        .exec_user("pwd", root.path(), Duration::from_secs(30))
        .unwrap();
    assert_eq!(next.exit_code, 0, "{}", next.merged());
    assert!(
        !next
            .merged()
            .contains(blyte_agent::shell::SHELL_RESTART_NOTICE),
        "{}",
        next.merged()
    );
    assert_eq!(next.stdout.trim(), expected_pwd);
}

#[test]
fn shell_does_not_restart_when_open_edits_are_unchanged() {
    if !sandbox::available() {
        return;
    }
    // The profile depends on HOME, which other tests here change under this lock.
    let _env_lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let root = init_git_repo_in_target_tmp();
    let cfg = conversation_config(root.path());
    let shell = build_local_shell(&cfg).unwrap();

    let first = shell
        .exec_user("echo $$", root.path(), Duration::from_secs(30))
        .unwrap();
    if first.exit_code == 71 && first.merged().contains("Operation not permitted") {
        return;
    }
    assert_eq!(first.exit_code, 0, "{}", first.merged());
    let pid1 = first.stdout.trim().to_string();
    assert!(!pid1.is_empty());

    let second = shell
        .exec_user("echo $$", root.path(), Duration::from_secs(30))
        .unwrap();
    assert_eq!(second.exit_code, 0, "{}", second.merged());
    assert!(
        !second
            .merged()
            .contains(blyte_agent::shell::SHELL_RESTART_NOTICE),
        "{}",
        second.merged()
    );
    let pid2 = second.stdout.trim().to_string();
    assert_eq!(pid1, pid2);
}

#[test]
fn read_tool_refuses_surface_token_and_blyte_env() {
    let tmp_base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(tmp_base).unwrap();
    let root = tempfile::tempdir_in(tmp_base).unwrap();
    let home = tempfile::tempdir_in(tmp_base).unwrap();
    let xdg = tempfile::tempdir_in(tmp_base).unwrap();
    let _env = EnvGuard::set(xdg.path(), home.path());

    struct XdgStateGuard(Option<OsString>);
    impl Drop for XdgStateGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
        }
    }
    let _xdg_state = XdgStateGuard(std::env::var_os("XDG_STATE_HOME"));
    std::env::remove_var("XDG_STATE_HOME");

    let token_path = sandbox::surface_token_path(root.path()).unwrap();
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, "surface-token-read-tool-secret\n").unwrap();

    let env_path = home.path().join(".config/blyte/env");
    std::fs::create_dir_all(env_path.parent().unwrap()).unwrap();
    std::fs::write(&env_path, "ANTHROPIC_API_KEY=blyte-env-read-tool-secret\n").unwrap();

    let shell = blyte_agent::shell::LocalShell::new();
    let fs = blyte_agent::fs::LocalFs;
    for (path, secret) in [
        (&token_path, "surface-token-read-tool-secret"),
        (&env_path, "blyte-env-read-tool-secret"),
    ] {
        let outcome = blyte_agent::tools::execute(
            root.path(),
            &shell,
            &fs,
            "read",
            &serde_json::json!({ "path": path.to_string_lossy() }),
        );
        assert!(
            outcome.is_error,
            "read tool must refuse {}: {}",
            path.display(),
            outcome.content
        );
        assert!(!outcome.content.contains(secret), "{}", outcome.content);
    }
}

#[test]
fn write_tool_under_temp_dir_asks_for_approval_and_cleans_up_private_tmpdir() {
    use std::sync::Arc;

    struct RecordingApprover {
        calls: Mutex<Vec<String>>,
    }

    impl blyte_agent::approval::Approver for RecordingApprover {
        fn ask(&self, command: &str, _reason: &str) -> Approval {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(command.to_string());
            Approval::Denied
        }
    }

    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .status()
        .unwrap()
        .success());
    let _env = EnvGuard::set(xdg.path(), home.path());

    let approver = Arc::new(RecordingApprover {
        calls: Mutex::new(Vec::new()),
    });
    let session = ReplSession::try_new_with_approver(
        &ReplConfig {
            repo_root: root.path().to_path_buf(),
            converse: true,
            sandbox: "on".to_string(),
            ..Default::default()
        },
        Some(approver.clone()),
    )
    .unwrap();

    let private_tmp = root
        .path()
        .canonicalize()
        .unwrap()
        .join(".wt/tmp")
        .join(std::process::id().to_string());
    assert!(
        private_tmp.is_dir(),
        "private tmpdir {} must exist while session is active",
        private_tmp.display()
    );

    let shell = blyte_agent::shell::LocalShell::new();
    let fs = blyte_agent::fs::LocalFs;
    let denied = blyte_agent::tools::execute(
        root.path(),
        &shell,
        &fs,
        "write",
        &serde_json::json!({
            "path": "src/x.rs",
            "content": "fn main() {}\n",
        }),
    );
    assert!(denied.is_error, "{}", denied.content);
    assert!(!root.path().join("src/x.rs").exists());
    assert!(
        !approver
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty(),
        "write to <root>/src/x.rs must ask for approval"
    );

    let allowed = blyte_agent::tools::execute(
        root.path(),
        &shell,
        &fs,
        "write",
        &serde_json::json!({
            "path": ".wt/tasks/allowed.md",
            "content": "ok\n",
        }),
    );
    assert!(!allowed.is_error, "{}", allowed.content);
    assert!(root.path().join(".wt/tasks/allowed.md").exists());

    drop(session);
    assert!(
        !private_tmp.exists(),
        "private tmpdir {} must be removed when the session exits",
        private_tmp.display()
    );

    blyte_agent::tools::set_write_scope(None);
    blyte_agent::tools::set_approval(None);
}

#[test]
fn transient_policy_error_preserves_active_session_private_tmpdir() {
    let root = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(root.path())
        .status()
        .unwrap()
        .success());
    let _env = EnvGuard::set(xdg.path(), home.path());

    let mut session = ReplSession::try_new(&ReplConfig {
        repo_root: root.path().to_path_buf(),
        converse: true,
        sandbox: "on".to_string(),
        ..Default::default()
    })
    .unwrap();

    let private_tmp = root
        .path()
        .canonicalize()
        .unwrap()
        .join(".wt/tmp")
        .join(std::process::id().to_string());
    assert!(private_tmp.is_dir());
    let sentinel = private_tmp.join("keep.txt");
    std::fs::write(&sentinel, "active\n").unwrap();

    let bad_approval = xdg.path().join("blyte").join("allow-unsandboxed");
    std::fs::create_dir_all(&bad_approval).unwrap();

    let cfg = session.task_config_for_prompt("hi");
    let err = run_task_with_state_and_events(&cfg, &mut session.state, &|_| {}).unwrap_err();
    assert!(
        err.contains(&bad_approval.display().to_string()),
        "expected approval load error, got: {err}"
    );
    assert!(
        private_tmp.is_dir(),
        "active session private tmpdir {} must survive a transient turn error",
        private_tmp.display()
    );
    assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "active\n");

    let fresh_root = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(fresh_root.path())
        .status()
        .unwrap()
        .success());
    let fresh_tmp = fresh_root
        .path()
        .canonicalize()
        .unwrap()
        .join(".wt/tmp")
        .join(std::process::id().to_string());
    let start_err = ReplSession::try_new(&ReplConfig {
        repo_root: fresh_root.path().to_path_buf(),
        converse: true,
        sandbox: "on".to_string(),
        ..Default::default()
    })
    .err()
    .expect("startup must fail when approval file cannot be read");
    assert!(
        start_err.contains(&bad_approval.display().to_string()),
        "expected approval load error, got: {start_err}"
    );
    assert!(
        !fresh_tmp.exists(),
        "failed startup must clean up newly created private tmpdir {}",
        fresh_tmp.display()
    );

    drop(session);
    assert!(
        !private_tmp.exists(),
        "private tmpdir {} must be removed when the session exits",
        private_tmp.display()
    );

    blyte_agent::tools::set_write_scope(None);
    blyte_agent::tools::set_approval(None);
}
