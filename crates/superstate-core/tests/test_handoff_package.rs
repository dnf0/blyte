use std::path::{Path, PathBuf};
use superstate_core::handoff::{
    generate_handoff_markdown, generate_resumption_command, write_handoff_package, HandoffSpec,
};
use superstate_core::maintenance::{resume_briefing, resume_briefing_with_root};
use superstate_core::models::{ExecutionState, LedgerEntry, TaskSpec};
use tempfile::tempdir;

#[test]
fn test_generate_resumption_command_claude() {
    let state_path = Path::new(".agent-state/state.json");

    // In-place (no worktree)
    let cmd = generate_resumption_command("claude", None, state_path);
    assert_eq!(cmd, "claude");

    // With worktree
    let wt = Path::new(".wt/feat-test");
    let cmd_wt = generate_resumption_command("claude", Some(wt), state_path);
    assert_eq!(cmd_wt, "cd .wt/feat-test && claude");
}

#[test]
fn test_generate_resumption_command_agy() {
    let state_path = Path::new(".agent-state/state.json");

    // In-place
    let cmd = generate_resumption_command("agy", None, state_path);
    assert_eq!(cmd, "agy");

    // With worktree
    let wt = Path::new(".wt/feat-test");
    let cmd_wt = generate_resumption_command("agy", Some(wt), state_path);
    assert_eq!(cmd_wt, "cd .wt/feat-test && agy");
}

#[test]
fn test_generate_resumption_command_daemon() {
    let state_path = Path::new(".agent-state/state.json");

    // Daemon command formats state daemon invocation with claude driver
    let cmd = generate_resumption_command("daemon", None, state_path);
    assert_eq!(
        cmd,
        "superstate state daemon --state .agent-state/state.json --driver claude --detach"
    );

    let wt = Path::new(".wt/feat-test");
    let cmd_wt = generate_resumption_command("daemon", Some(wt), state_path);
    assert_eq!(
        cmd_wt,
        "superstate state daemon --state .agent-state/state.json --driver claude --detach"
    );
}

#[test]
fn test_generate_resumption_command_fallback() {
    let state_path = Path::new(".agent-state/state.json");

    // Fallback for human or unspecified agent
    let cmd = generate_resumption_command("human", None, state_path);
    assert_eq!(
        cmd,
        "superstate state resume --state .agent-state/state.json"
    );

    let wt = Path::new(".wt/feat-test");
    let cmd_wt = generate_resumption_command("any", Some(wt), state_path);
    assert_eq!(
        cmd_wt,
        "cd .wt/feat-test && superstate state resume --state .agent-state/state.json"
    );
}

#[test]
fn test_generate_handoff_markdown_claude_to_agy() {
    let state = ExecutionState {
        goal: Some("Implement resilient cross-agent handoffs".to_string()),
        active_task: Some(TaskSpec {
            id: "task-2".to_string(),
            title: "Standardized Handoff Package".to_string(),
            depends_on: vec!["task-1".to_string()],
            files_modify: vec![
                PathBuf::from("crates/superstate-core/src/handoff.rs"),
                PathBuf::from("crates/superstate-core/src/maintenance.rs"),
            ],
            verification_cmd: Some("cargo test -p superstate-core".to_string()),
        }),
        active_worktree: Some(PathBuf::from(".wt/feat-handoff")),
        ..Default::default()
    };

    let spec = HandoffSpec {
        from_agent: Some("claude".to_string()),
        to_agent: Some("agy".to_string()),
        state_path: PathBuf::from(".agent-state/state.json"),
        worktree_path: Some(PathBuf::from(".wt/feat-handoff")),
        slug: "feat-handoff".to_string(),
    };

    let notes = vec![LedgerEntry::new(
        "claude",
        "Discovered permission requirements for non-interactive runners\n\nMust include bypass flags.",
    )];

    let md = generate_handoff_markdown(&state, &spec, &notes);

    // Verify source and target headers
    assert!(md.contains("- **Source Agent**: claude"));
    assert!(md.contains("- **Target Agent**: agy"));

    // Verify job slug and state file
    assert!(md.contains("- **Job Slug**: feat-handoff"));
    assert!(md.contains("- **State File**: `.agent-state/state.json`"));

    // Verify active task specification
    assert!(md.contains("- **Task ID**: `task-2`"));
    assert!(md.contains("- **Title**: Standardized Handoff Package"));
    assert!(md.contains("- **Verification Invariant**: `cargo test -p superstate-core`"));
    assert!(md.contains("crates/superstate-core/src/handoff.rs"));
    assert!(md.contains("crates/superstate-core/src/maintenance.rs"));

    // Verify epistemic ledger notes (compacted)
    assert!(md.contains("Discovered permission requirements for non-interactive runners"));
    // Paragraph break should have dropped second paragraph
    assert!(!md.contains("Must include bypass flags."));

    // Verify ready-to-run resumption snippets
    assert!(md.contains("cd .wt/feat-handoff && claude"));
    assert!(md.contains("cd .wt/feat-handoff && agy"));
    assert!(md.contains(
        "superstate state daemon --state .agent-state/state.json --driver claude --detach"
    ));
}

#[test]
fn test_generate_handoff_markdown_agy_to_claude() {
    let state = ExecutionState {
        goal: Some("Migrate workflow from Antigravity to Claude Code".to_string()),
        active_task: Some(TaskSpec {
            id: "task-3".to_string(),
            title: "CLI handoff subcommand".to_string(),
            depends_on: vec![],
            files_modify: vec![PathBuf::from(
                "crates/superstate-cli/src/commands/handoff.rs",
            )],
            verification_cmd: Some("cargo test -p superstate".to_string()),
        }),
        ..Default::default()
    };

    let spec = HandoffSpec {
        from_agent: Some("agy".to_string()),
        to_agent: Some("claude".to_string()),
        state_path: PathBuf::from(".agent-state/state-alpha.json"),
        worktree_path: None,
        slug: "alpha".to_string(),
    };

    let notes = vec![LedgerEntry::new("agy", "Tested headless preset generation")];

    let md = generate_handoff_markdown(&state, &spec, &notes);

    assert!(md.contains("- **Source Agent**: agy"));
    assert!(md.contains("- **Target Agent**: claude"));
    assert!(md.contains("- **Job Slug**: alpha"));
    assert!(md.contains("- **State File**: `.agent-state/state-alpha.json`"));
    assert!(md.contains("- **Working Directory**: `repository root (in-place)`"));
    assert!(md.contains("Tested headless preset generation"));
    assert!(md.contains("claude"));
    assert!(md.contains("agy"));
}

#[test]
fn test_write_handoff_package_writes_both_files() {
    let tmp = tempdir().unwrap();
    let repo_root = tmp.path();

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Initialize package".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("cargo check".to_string()),
        }),
        ..Default::default()
    };

    let spec = HandoffSpec {
        from_agent: Some("claude".to_string()),
        to_agent: Some("agy".to_string()),
        state_path: repo_root.join(".agent-state/state.json"),
        worktree_path: None,
        slug: "init-pkg".to_string(),
    };

    let handoff_path = write_handoff_package(repo_root, &state, &spec, &[]).unwrap();

    // Verifies write_handoff_package writes both .agent-state/handoff.md and .agent-state/handoffs/<slug>.md
    assert_eq!(handoff_path, repo_root.join(".agent-state/handoff.md"));
    assert!(handoff_path.exists());

    let scoped_path = repo_root.join(".agent-state/handoffs/init-pkg.md");
    assert!(scoped_path.exists());

    let main_content = std::fs::read_to_string(&handoff_path).unwrap();
    let scoped_content = std::fs::read_to_string(&scoped_path).unwrap();
    assert_eq!(main_content, scoped_content);
    assert!(main_content.contains("Initialize package"));
}

#[test]
fn test_resume_briefing_surfaces_active_handoff_when_present() {
    let tmp = tempdir().unwrap();
    let repo_root = tmp.path();

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Test task".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("cargo check".to_string()),
        }),
        ..Default::default()
    };

    let spec = HandoffSpec {
        from_agent: Some("claude".to_string()),
        to_agent: Some("agy".to_string()),
        state_path: repo_root.join(".agent-state/state.json"),
        worktree_path: Some(PathBuf::from(".wt/test-wt")),
        slug: "test-task".to_string(),
    };

    // When handoff is written
    write_handoff_package(repo_root, &state, &spec, &[]).unwrap();

    let briefing = resume_briefing_with_root(&state, &[], &[], Some(repo_root));
    assert!(briefing.contains("## 🤝 Active Handoff"));
    assert!(briefing.contains("handoff.md"));
    assert!(briefing.contains("agy"));
}

#[test]
fn test_write_handoff_package_sanitizes_slug_with_slashes() {
    let tmp = tempdir().unwrap();
    let repo_root = tmp.path();

    let state = ExecutionState {
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Task with slash branch".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("cargo check".to_string()),
        }),
        active_branch: Some("feat/cross-agent-handoff".to_string()),
        ..Default::default()
    };

    let spec = HandoffSpec {
        from_agent: Some("claude".to_string()),
        to_agent: Some("agy".to_string()),
        state_path: repo_root.join(".agent-state/branches/feat-cross-agent-handoff.json"),
        worktree_path: None,
        slug: "feat/cross-agent-handoff".to_string(),
    };

    let handoff_path = write_handoff_package(repo_root, &state, &spec, &[]).unwrap();
    assert_eq!(handoff_path, repo_root.join(".agent-state/handoff.md"));
    assert!(handoff_path.exists());

    // Scoped file must be sanitized to feat-cross-agent-handoff.md without nested directories
    let scoped_path = repo_root.join(".agent-state/handoffs/feat-cross-agent-handoff.md");
    assert!(
        scoped_path.exists(),
        "Scoped handoff package should exist with sanitized slug"
    );

    // Ensure it did not create a subdirectory 'feat'
    assert!(!repo_root.join(".agent-state/handoffs/feat").exists());
}

#[test]
fn test_resume_briefing_outputs_correct_state_json_path_with_plan() {
    let tmp = tempdir().unwrap();
    let repo_root = tmp.path();

    let state = ExecutionState {
        plan_path: Some(PathBuf::from("docs/plans/feature-plan.md")),
        active_branch: Some("feat/cross-agent-handoff".to_string()),
        active_task: Some(TaskSpec {
            id: "task-2".to_string(),
            title: "Resumption briefing test".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: Some("cargo test".to_string()),
        }),
        ..Default::default()
    };

    let spec = HandoffSpec {
        from_agent: Some("claude".to_string()),
        to_agent: Some("agy".to_string()),
        state_path: repo_root.join(".agent-state/branches/feat-cross-agent-handoff.json"),
        worktree_path: None,
        slug: "feat/cross-agent-handoff".to_string(),
    };

    // Write handoff package in repo_root
    write_handoff_package(repo_root, &state, &spec, &[]).unwrap();

    let briefing = resume_briefing_with_root(&state, &[], &[], Some(repo_root));

    // Verify briefing contains the active handoff section
    assert!(briefing.contains("## 🤝 Active Handoff"));

    // Verify Daemon resumption command uses the JSON state path, NOT the markdown plan path!
    assert!(briefing.contains("--state"));
    assert!(
        briefing.contains(".agent-state/branches/feat-cross-agent-handoff.json"),
        "Briefing should reference the JSON state path in resumption commands"
    );
    assert!(
        !briefing.contains("--state docs/plans/feature-plan.md"),
        "Briefing must NEVER use the markdown plan path as state_path in resumption commands"
    );
    assert!(
        !briefing.contains(".md --driver"),
        "Briefing must not pass a markdown file to --state"
    );

    // Also verify fallback when active_branch is None
    let state_no_branch = ExecutionState {
        plan_path: Some(PathBuf::from("docs/plans/feature-plan.md")),
        active_branch: None,
        active_task: Some(TaskSpec {
            id: "task-1".to_string(),
            title: "Default branch test".to_string(),
            depends_on: vec![],
            files_modify: vec![],
            verification_cmd: None,
        }),
        ..Default::default()
    };

    let briefing_no_branch = resume_briefing_with_root(&state_no_branch, &[], &[], Some(repo_root));
    assert!(
        briefing_no_branch.contains(".agent-state/state.json"),
        "Briefing with no active branch should fall back to state.json"
    );
    assert!(
        !briefing_no_branch.contains("--state docs/plans/feature-plan.md"),
        "Briefing must not use plan_path as state_path"
    );

    // Direct call to resume_briefing without explicit root
    let direct_briefing = resume_briefing(&state, &[], &[]);
    assert!(direct_briefing.contains("Plan Path"));
    assert!(
        !direct_briefing.contains("--state docs/plans/feature-plan.md"),
        "Direct resume_briefing must not use plan_path as state_path"
    );
}
