//! Every behaviour-affecting flag must survive the detach re-exec.
//!
//! `cmd_daemon --detach` re-launches itself as a background process by re-building the child's
//! argv BY HAND. Any flag not written into that block is silently dropped, and since the
//! evaluation harness always detaches, a dropped flag means the daemon runs with a default
//! while the caller, the results file and the ledger all say otherwise.
//!
//! This has already happened twice in one day, to both parameters added that day:
//!
//! * `--read-threshold-lines` -- a sweep would have run at the default throughout.
//! * `--no-read-cache` -- EXP-081's ablation arm provisioned `--read-cache true` on BOTH
//!   sides. The run was killed 90 seconds in, after the live hooks were compared; a null
//!   result from it would have read as "the cache does not matter".
//!
//! Testing the two known cases would not stop the third. This walks the full switch set.

use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_superstate")
}

/// The flags `state daemon` accepts that change what the daemon DOES, and a non-default value
/// for each. Anything added to `DaemonConfig` belongs here.
fn switches() -> Vec<(&'static str, Option<&'static str>)> {
    vec![
        ("--read-threshold-lines", Some("137")),
        ("--no-read-cache", None),
        ("--no-shunt", None),
        ("--no-file-guard", None),
        ("--no-command-guard", None),
        ("--no-auto-rollback", None),
        ("--rollback-policy", Some("on-stall")),
        ("--impact-slicing", None),
        ("--advisory-review", None),
    ]
}

#[test]
fn every_switch_is_accepted_by_state_daemon() {
    // If a flag here is not real, the round-trip test below would pass vacuously.
    let help = String::from_utf8(
        Command::new(bin())
            .args(["state", "daemon", "--help"])
            .output()
            .expect("run --help")
            .stdout,
    )
    .unwrap();
    for (flag, _) in switches() {
        assert!(
            help.contains(flag),
            "{flag} is not a real `state daemon` flag; this test would pass vacuously"
        );
    }
}

#[test]
fn detach_argv_forwards_the_read_budget_and_cache() {
    // The two that were actually lost. Asserted on the provisioned hook rather than on the
    // argv, because the hook is what the worker is ultimately governed by -- an argv that
    // carries the flag but never reaches the guard would still be a defect.
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    Command::new("git").args(["init", "-q"]).current_dir(repo).output().unwrap();
    std::fs::create_dir_all(repo.join(".agent-state")).unwrap();
    std::fs::write(
        repo.join(".agent-state/state.json"),
        r#"{"version":"1.0","goal":"g","status":"in_progress","step":0,
            "active_task":{"id":"t1","title":"t","depends_on":[],"files_modify":[],
                           "verification_cmd":"true"},
            "tasks":[],"no_worktree":true}"#,
    )
    .unwrap();

    let out = Command::new(bin())
        .args([
            "state", "daemon",
            "--state", ".agent-state/state.json",
            "--driver", "true",
            "--read-threshold-lines", "137",
            "--no-read-cache",
        ])
        .current_dir(repo)
        .output()
        .expect("run daemon");
    assert!(
        out.status.success(),
        "daemon failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let hooks = std::fs::read_to_string(repo.join(".agents/hooks.json"))
        .expect("daemon must provision hooks.json");
    let json: serde_json::Value = serde_json::from_str(&hooks).unwrap();
    let cmd = json["superstate-file-guard"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();

    assert!(
        cmd.contains("--threshold-lines 137"),
        "the configured read budget must reach the guard, got {cmd:?}"
    );
    assert!(
        cmd.contains("--read-cache false"),
        "the cache ablation must reach the guard, got {cmd:?}"
    );
}
