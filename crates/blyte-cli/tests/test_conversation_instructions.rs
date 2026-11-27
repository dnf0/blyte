use std::ffi::OsString;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

use blyte_agent::context::Conversational;
use blyte_agent::conversation::Answers;
use blyte_agent::fs::LocalFs;
use blyte_agent::run::{Budget, Runner, CONVERSATION_FIXED};
use blyte_agent::shell::LocalShell;
use blyte_agent::state::State;
use blyte_cli::conversation_text::{
    build_with_sandbox, DEFAULT_PROCESS, REFERENCE, REFERENCE_DISPATCH_TOOL,
};
use blyte_cli::repl::{render_banner_to, ReplConfig, ReplSession};
use blyte_provider::{Provider, ProviderError, Request, Response};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct XdgGuard {
    _lock: MutexGuard<'static, ()>,
    prev: Option<OsString>,
}

impl XdgGuard {
    fn set(dir: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", dir);
        Self { _lock: lock, prev }
    }
}

impl Drop for XdgGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
}

struct RecordingProvider {
    seen: Mutex<Vec<Request>>,
}

impl Provider for RecordingProvider {
    fn complete(&self, req: &Request) -> Result<Response, ProviderError> {
        self.seen.lock().unwrap().push(req.clone());
        Ok(Response {
            text: "done".into(),
            ..Default::default()
        })
    }

    fn model_id(&self) -> String {
        "recording".into()
    }
}

#[test]
fn shell_mode_gives_the_reference_and_the_default_process() {
    let root = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let _guard = XdgGuard::set(xdg.path());

    let built = build_with_sandbox(false, Answers::Nobody, root.path(), true, 0).unwrap();
    let ref_first = REFERENCE.lines().next().unwrap();
    let proc_first = DEFAULT_PROCESS.lines().next().unwrap();
    assert!(
        built.instructions.contains(ref_first),
        "{}",
        built.instructions
    );
    assert!(
        built.instructions.contains(proc_first),
        "{}",
        built.instructions
    );
    assert_eq!(built.start_lines[0], "process: default (built in)");
    assert_eq!(built.start_lines[1], "repository rules: none");
}

#[test]
fn tool_mode_gives_the_tool_reference_and_no_process() {
    let root = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let _guard = XdgGuard::set(xdg.path());

    let built = build_with_sandbox(true, Answers::Nobody, root.path(), true, 0).unwrap();
    let tool_first = REFERENCE_DISPATCH_TOOL.lines().next().unwrap();
    assert!(
        built.instructions.contains(tool_first),
        "{}",
        built.instructions
    );
    assert!(
        !built.instructions.contains("## Process"),
        "{}",
        built.instructions
    );
    assert_eq!(built.start_lines[0], "process: none");
    assert_eq!(built.start_lines[1], "repository rules: none");
}

#[test]
fn instructions_do_not_contain_the_skill_or_the_old_preamble() {
    let root = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let _guard = XdgGuard::set(xdg.path());

    for dispatch_tool in [false, true] {
        let built =
            build_with_sandbox(dispatch_tool, Answers::Person, root.path(), true, 0).unwrap();
        assert!(
            !built.instructions.contains("# blyte as your implementer"),
            "{}",
            built.instructions
        );
        assert!(
            !built.instructions.contains("Start on the work at once"),
            "{}",
            built.instructions
        );
        assert!(
            !built
                .instructions
                .contains("Get oriented with a few cheap calls"),
            "{}",
            built.instructions
        );
    }
}

#[test]
fn an_empty_user_file_stops_the_start() {
    let root = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let user_file = xdg.path().join("blyte").join("process.md");
    std::fs::create_dir_all(user_file.parent().unwrap()).unwrap();
    std::fs::write(&user_file, "   \n").unwrap();

    let err = {
        let _guard = XdgGuard::set(xdg.path());
        build_with_sandbox(false, Answers::Nobody, root.path(), true, 0)
            .err()
            .unwrap()
    };
    assert!(err.contains(&user_file.display().to_string()), "{err}");

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .current_dir(root.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .args(["--converse", "--", "plan the release"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(&user_file.display().to_string()),
        "{stderr}"
    );
    assert!(stderr.contains("is empty"), "{stderr}");
}

#[test]
fn repository_rules_for_implementers_are_not_given() {
    let root = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let marker = "IMPLEMENTER_ONLY_MARKER_948271";
    std::fs::write(root.path().join("AGENTS.md"), format!("Rule: {marker}\n")).unwrap();

    let built = {
        let _guard = XdgGuard::set(xdg.path());
        build_with_sandbox(false, Answers::Nobody, root.path(), true, 0).unwrap()
    };

    let mut state = State::new("plan the release", "");
    state.brief.instructions = Some(built.instructions);
    let provider = RecordingProvider {
        seen: Mutex::new(Vec::new()),
    };
    let runner = Runner {
        provider: &provider,
        shell: &LocalShell,
        fs: &LocalFs,
        context: &Conversational,
        root: root.path().to_path_buf(),
        budget: Budget {
            max_turns: 1,
            max_gate_attempts: 1,
            max_duration: None,
        },
        rollback: false,
        on_event: None,
        research: false,
        claim_check: false,
        base_check: blyte_agent::run::BaseCheck::None,
    };
    runner.run(&mut state);

    let system = &provider.seen.lock().unwrap()[0].system;
    assert!(!system.contains(marker), "{system}");
    assert!(system.contains(CONVERSATION_FIXED), "{system}");
}

#[test]
fn interactive_banner_prints_process_and_repo_rules() {
    let root = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let repo_rules = root.path().join(".blyte").join("process.md");
    std::fs::create_dir_all(repo_rules.parent().unwrap()).unwrap();
    std::fs::write(&repo_rules, "Repo rule.").unwrap();

    let session = {
        let _guard = XdgGuard::set(xdg.path());
        ReplSession::try_new(&ReplConfig {
            repo_root: root.path().to_path_buf(),
            converse: true,
            ..Default::default()
        })
        .unwrap()
    };

    let mut out = Vec::new();
    render_banner_to(&mut out, &session).unwrap();
    let banner = String::from_utf8(out).unwrap();
    let mode_pos = banner.find("  Mode       : conversation model").unwrap();
    let process_pos = banner.find("  Process    : default (built in)").unwrap();
    let repo_pos = banner
        .find(&format!("  Repo rules : {}", repo_rules.display()))
        .unwrap();
    assert!(process_pos > mode_pos, "{banner}");
    assert!(repo_pos > process_pos, "{banner}");
}

#[test]
fn one_shot_prints_start_lines_and_not_implementer_rules() {
    let root = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("AGENTS.md"), "Implementer rule.\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .current_dir(root.path())
        .env("XDG_CONFIG_HOME", xdg.path())
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("OLLAMA_HOST")
        .args(["--converse", "--", "plan the release"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("blyte: process: default (built in)"),
        "{stderr}"
    );
    assert!(stderr.contains("blyte: repository rules: none"), "{stderr}");
    assert!(!stderr.contains("blyte: repository rules from"), "{stderr}");
}

#[test]
fn assembled_instructions_explain_working_without_a_git_repository() {
    let root = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let _guard = XdgGuard::set(xdg.path());

    let built = build_with_sandbox(false, Answers::Person, root.path(), true, 0).unwrap();
    assert!(
        built.instructions.contains("git init"),
        "{}",
        built.instructions
    );
    assert!(
        built.instructions.contains("not a git repository"),
        "{}",
        built.instructions
    );

    let built_off = build_with_sandbox(false, Answers::Person, root.path(), false, 0).unwrap();
    assert!(
        built_off.instructions.contains("git init"),
        "{}",
        built_off.instructions
    );
    assert!(
        built_off.instructions.contains("not a git repository"),
        "{}",
        built_off.instructions
    );
    assert!(
        !built_off.instructions.contains("unsandboxed"),
        "{}",
        built_off.instructions
    );
}

#[test]
fn assembled_instructions_for_non_repository_describe_in_place_runs_and_undo() {
    let root = tempfile::tempdir().unwrap();
    let xdg = tempfile::tempdir().unwrap();
    let _guard = XdgGuard::set(xdg.path());

    let built = build_with_sandbox(false, Answers::Person, root.path(), true, 0).unwrap();
    assert!(
        built.instructions.contains("blyte undo"),
        "{}",
        built.instructions
    );
    assert!(
        !built.instructions.contains("there are no runs"),
        "{}",
        built.instructions
    );
    assert!(
        built.instructions.contains("blyte review <id>"),
        "{}",
        built.instructions
    );
    assert!(
        built.instructions.contains("in place"),
        "{}",
        built.instructions
    );

    let built_off = build_with_sandbox(false, Answers::Person, root.path(), false, 0).unwrap();
    assert!(
        built_off.instructions.contains("blyte undo"),
        "{}",
        built_off.instructions
    );
    assert!(
        !built_off.instructions.contains("there are no runs"),
        "{}",
        built_off.instructions
    );
}
