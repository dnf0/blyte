use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use blyte_agent::approval::{
    is_simple_command, matching_rule, Approval, ApprovalConfig, Approver, NoApprover,
};
use blyte_agent::fs::LocalFs;
use blyte_agent::sandbox;
use blyte_agent::shell::{ContainerShell, LocalShell, Output, PersistentShell, Shell};
use blyte_agent::tools::{execute, set_approval, specs};
use serde_json::json;

static APPROVAL_LOCK: Mutex<()> = Mutex::new(());

struct ApprovalGuard {
    _lock: MutexGuard<'static, ()>,
}

impl ApprovalGuard {
    fn acquire(config: Option<ApprovalConfig>) -> Self {
        let lock = APPROVAL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_approval(config);
        Self { _lock: lock }
    }
}

impl Drop for ApprovalGuard {
    fn drop(&mut self) {
        set_approval(None);
    }
}

#[derive(Default)]
struct FakeShell {
    calls: Mutex<Vec<(&'static str, String)>>,
}

impl FakeShell {
    fn recorded(&self) -> Vec<(&'static str, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Shell for FakeShell {
    fn exec(&self, command: &str, _: &Path, _: Duration) -> Result<Output, String> {
        self.calls.lock().unwrap().push(("exec", command.into()));
        Ok(Output {
            stdout: "exec-ok\n".into(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    fn exec_user(&self, command: &str, _: &Path, _: Duration) -> Result<Output, String> {
        self.calls
            .lock()
            .unwrap()
            .push(("exec_user", command.into()));
        Ok(Output {
            stdout: "user-ok\n".into(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    fn exec_host(&self, command: &str, _: &Path, _: Duration) -> Result<Output, String> {
        self.calls
            .lock()
            .unwrap()
            .push(("exec_host", command.into()));
        Ok(Output {
            stdout: "host-ok\n".into(),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    fn describe(&self) -> String {
        String::new()
    }
}

struct FakeApprover {
    verdict: Approval,
    calls: Mutex<Vec<(String, String)>>,
}

impl FakeApprover {
    fn new(verdict: Approval) -> Arc<Self> {
        Arc::new(Self {
            verdict,
            calls: Mutex::new(Vec::new()),
        })
    }

    fn recorded(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Approver for FakeApprover {
    fn ask(&self, command: &str, reason: &str) -> Approval {
        self.calls
            .lock()
            .unwrap()
            .push((command.into(), reason.into()));
        self.verdict
    }
}

#[test]
fn is_simple_command_accepts_plain_commands_and_rejects_shell_syntax() {
    assert!(is_simple_command("git push -u origin my-branch"));
    for bad in [
        "",
        "   ",
        "git push; rm -rf x",
        "a && b",
        "a | b",
        "echo $(x)",
        "echo `x`",
        "a > f",
        "a < f",
        "a\nb",
        "(a)",
        "{ a }",
        r"a\b",
    ] {
        assert!(!is_simple_command(bad), "expected false for {bad:?}");
    }
}

#[test]
fn matching_rule_requires_simple_command_and_word_boundary() {
    let rules = vec!["git push -u origin".to_string()];
    assert_eq!(
        matching_rule("git push -u origin x", &rules),
        Some("git push -u origin")
    );
    assert_eq!(matching_rule("git push -u originx", &rules), None);
    assert_eq!(matching_rule("git push -u origin x; ls", &rules), None);
    assert_eq!(
        matching_rule("git push -u origin", &rules),
        Some("git push -u origin")
    );
    let blank_rules = vec!["".to_string(), "   ".to_string()];
    assert_eq!(matching_rule("git push -u origin x", &blank_rules), None);
}

#[test]
fn bash_unsandboxed_enforces_approval_rules_and_routing() {
    let dir = tempfile::tempdir().unwrap();

    {
        let _g = ApprovalGuard::acquire(None);
        let sh = FakeShell::default();
        let out = execute(
            dir.path(),
            &sh,
            &LocalFs,
            "bash",
            &json!({"command": "git push", "unsandboxed": true, "reason": "push branch"}),
        );
        assert!(out.is_error);
        assert_eq!(
            out.content,
            "this run cannot run commands outside the sandbox"
        );
        assert!(sh.recorded().is_empty());
    }

    {
        let approver = FakeApprover::new(Approval::Approved);
        let _g = ApprovalGuard::acquire(Some(ApprovalConfig {
            approver: approver.clone(),
            rules: vec![],
        }));
        let sh = FakeShell::default();
        let out = execute(
            dir.path(),
            &sh,
            &LocalFs,
            "bash",
            &json!({"command": "git push", "unsandboxed": true, "reason": "   "}),
        );
        assert!(out.is_error);
        assert_eq!(
            out.content,
            "give a one-line reason with an unsandboxed command"
        );
        assert!(approver.recorded().is_empty());
        assert!(sh.recorded().is_empty());
    }

    {
        let approver = FakeApprover::new(Approval::Denied);
        let _g = ApprovalGuard::acquire(Some(ApprovalConfig {
            approver: approver.clone(),
            rules: vec!["git push -u origin".into()],
        }));
        let sh = FakeShell::default();
        let out = execute(
            dir.path(),
            &sh,
            &LocalFs,
            "bash",
            &json!({
                "command": "git push -u origin my-branch",
                "unsandboxed": true,
                "reason": "push branch"
            }),
        );
        assert!(!out.is_error, "{}", out.content);
        assert!(approver.recorded().is_empty());
        assert_eq!(
            sh.recorded(),
            vec![("exec_host", "git push -u origin my-branch".into())]
        );
        assert_eq!(
            out.content.lines().next(),
            Some("ran outside the sandbox (allowed by rule \"git push -u origin\")")
        );
    }

    {
        let approver = FakeApprover::new(Approval::Approved);
        let _g = ApprovalGuard::acquire(Some(ApprovalConfig {
            approver: approver.clone(),
            rules: vec![],
        }));
        let sh = FakeShell::default();
        let out = execute(
            dir.path(),
            &sh,
            &LocalFs,
            "bash",
            &json!({
                "command": "gh pr create",
                "unsandboxed": true,
                "reason": "open pull request"
            }),
        );
        assert!(!out.is_error, "{}", out.content);
        assert_eq!(
            approver.recorded(),
            vec![("gh pr create".into(), "open pull request".into())]
        );
        assert_eq!(sh.recorded(), vec![("exec_host", "gh pr create".into())]);
        assert_eq!(
            out.content.lines().next(),
            Some("ran outside the sandbox (approved by the user)")
        );
    }

    {
        let approver = FakeApprover::new(Approval::Denied);
        let _g = ApprovalGuard::acquire(Some(ApprovalConfig {
            approver: approver.clone(),
            rules: vec![],
        }));
        let sh = FakeShell::default();
        let out = execute(
            dir.path(),
            &sh,
            &LocalFs,
            "bash",
            &json!({
                "command": "gh pr create",
                "unsandboxed": true,
                "reason": "open pull request"
            }),
        );
        assert!(out.is_error);
        assert_eq!(
            out.content,
            "the user refused this command; it was not run. Do not try to run it another way."
        );
        assert_eq!(approver.recorded().len(), 1);
        assert!(sh.recorded().is_empty());
    }

    {
        let _g = ApprovalGuard::acquire(Some(ApprovalConfig {
            approver: Arc::new(NoApprover),
            rules: vec![],
        }));
        let sh = FakeShell::default();
        let out = execute(
            dir.path(),
            &sh,
            &LocalFs,
            "bash",
            &json!({
                "command": "gh pr create",
                "unsandboxed": true,
                "reason": "open pull request"
            }),
        );
        assert!(out.is_error);
        assert_eq!(
            out.content,
            "nobody can approve a command in this run, so it was not run"
        );
        assert!(sh.recorded().is_empty());
    }

    {
        let approver = FakeApprover::new(Approval::Denied);
        let _g = ApprovalGuard::acquire(Some(ApprovalConfig {
            approver: approver.clone(),
            rules: vec![],
        }));
        let sh = FakeShell::default();
        let out = execute(
            dir.path(),
            &sh,
            &LocalFs,
            "bash",
            &json!({"command": "echo hi", "unsandboxed": false}),
        );
        assert!(!out.is_error, "{}", out.content);
        assert!(approver.recorded().is_empty());
        assert_eq!(sh.recorded(), vec![("exec_user", "echo hi".into())]);
    }
}

#[test]
fn bash_spec_exposes_unsandboxed_fields_only_when_approval_is_set() {
    {
        let _g = ApprovalGuard::acquire(None);
        let all = specs();
        let bash = all.iter().find(|t| t.name == "bash").unwrap();
        let props = bash.parameters["properties"].as_object().unwrap();
        assert!(!props.contains_key("unsandboxed"));
        assert!(!props.contains_key("reason"));
    }

    {
        let _g = ApprovalGuard::acquire(Some(ApprovalConfig {
            approver: Arc::new(NoApprover),
            rules: vec![],
        }));
        let all = specs();
        let bash = all.iter().find(|t| t.name == "bash").unwrap();
        let props = bash.parameters["properties"].as_object().unwrap();
        assert_eq!(props["unsandboxed"]["type"], "boolean");
        assert_eq!(props["reason"]["type"], "string");
    }
}

#[test]
fn local_shell_exec_host_runs_outside_sandbox() {
    let dir = tempfile::tempdir().unwrap();
    let profile = sandbox::available().then(|| sandbox::default_profile(dir.path(), &[]));
    let sh = match profile.clone() {
        Some(p) => LocalShell::sandboxed(p),
        None => LocalShell::new(),
    };
    let out = sh
        .exec_host("echo hi", dir.path(), Duration::from_secs(30))
        .unwrap();
    assert_eq!(out.exit_code, 0, "{}", out.merged());
    assert_eq!(out.stdout.trim(), "hi");

    let persistent = match profile {
        Some(p) => PersistentShell::local_sandboxed(dir.path().to_path_buf(), p),
        None => PersistentShell::local(dir.path().to_path_buf()),
    };
    let out = persistent
        .exec_host("echo hi", dir.path(), Duration::from_secs(30))
        .unwrap();
    assert_eq!(out.exit_code, 0, "{}", out.merged());
    assert_eq!(out.stdout.trim(), "hi");

    let container = ContainerShell {
        container_id: "nonexistent".into(),
        workdir: PathBuf::from("/work"),
    };
    assert_eq!(
        container
            .exec_host("echo hi", dir.path(), Duration::from_secs(5))
            .unwrap_err(),
        "this shell cannot run a command outside the sandbox"
    );
}
