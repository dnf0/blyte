use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use superstate_mcp::server::handle_tool_call;
use superstate_mcp::tools::list_all_mcp_tools;
use tempfile::tempdir;

fn setup_test_repo() -> (tempfile::TempDir, PathBuf) {
    let temp = tempdir().unwrap();
    let repo_root = temp.path().to_path_buf();

    Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test Agent"])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@agent.local"])
        .current_dir(&repo_root)
        .output()
        .unwrap();

    let readme = repo_root.join("README.md");
    fs::write(&readme, "# Progressive Verification Test Repo\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "chore: initial commit"])
        .current_dir(&repo_root)
        .output()
        .unwrap();

    (temp, repo_root)
}

#[test]
fn test_mcp_progressive_verification_e2e_lifecycle() {
    let (_dir, repo_root) = setup_test_repo();

    // 1. Initialize multi-milestone plan via superstate_handoff
    let handoff_args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "goal": "Implement feature with progressive staged verification",
        "milestones": [
            {
                "id": "task-1",
                "title": "Milestone 1: Core Algorithm",
                "verify_cmd": "echo 'milestone 1 ok'"
            },
            {
                "id": "task-2",
                "title": "Milestone 2: Final Polish",
                "verify_cmd": "echo 'milestone 2 ok'"
            }
        ],
        "no_worktree": true
    });
    let handoff_res = handle_tool_call("superstate_handoff", &handoff_args)
        .expect("superstate_handoff should succeed");
    assert_eq!(handoff_res["status"], "ready");

    // 2. Create worktree via worktree_create
    let create_args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "branch": "wt/task-1-progressive"
    });
    let create_res =
        handle_tool_call("worktree_create", &create_args).expect("worktree_create should succeed");
    let wt_path = PathBuf::from(create_res["worktree_path"].as_str().unwrap());
    assert!(wt_path.exists(), "worktree path must exist on disk");

    // 3. Make changes in worktree
    let feature_file = wt_path.join("feature.txt");
    fs::write(&feature_file, "initial feature implementation").unwrap();

    // 4. Call state_next with verify_stage: "milestone"
    let next_args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "verify_stage": "milestone"
    });
    let next_res =
        handle_tool_call("state_next", &next_args).expect("state_next milestone should succeed");
    assert_eq!(next_res["status"], "success");
    assert_eq!(next_res["stage"], "milestone");
    assert_eq!(next_res["active_task"]["id"], "task-2");

    // 5. Call worktree_merge with failing verify_full_suite
    //    -> verify error, base branch unchanged, worktree intact
    let merge_fail_args = json!({
        "repo": repo_root.to_str().unwrap(),
        "branch": "wt/task-1-progressive",
        "verify_full_suite": "echo 'Regression check failed!' >&2 && exit 1"
    });
    let merge_err = handle_tool_call("worktree_merge", &merge_fail_args)
        .expect_err("worktree_merge must fail when verify_full_suite returns non-zero");
    assert!(
        merge_err.contains("Pre-merge verification failed"),
        "Expected pre-merge verification failure message, got: {}",
        merge_err
    );
    assert!(
        merge_err.contains("Regression check failed!"),
        "Expected stderr in error output, got: {}",
        merge_err
    );
    assert!(
        wt_path.exists(),
        "Worktree directory must remain intact following verification failure"
    );
    assert!(
        !repo_root.join("feature.txt").exists(),
        "Base branch must not contain worktree changes after failed pre-merge verification"
    );

    // 6. Fix issue in worktree
    fs::write(&feature_file, "fixed feature implementation").unwrap();

    // 7. Call worktree_merge with passing verify_full_suite
    //    -> succeeds, changes merged to base
    let merge_pass_args = json!({
        "repo": repo_root.to_str().unwrap(),
        "branch": "wt/task-1-progressive",
        "verify_full_suite": "echo 'Regression check passed!'"
    });
    let merge_res = handle_tool_call("worktree_merge", &merge_pass_args)
        .expect("worktree_merge must succeed when verify_full_suite passes");
    assert_eq!(merge_res["status"], "success");
    assert_eq!(merge_res["branch"], "wt/task-1-progressive");
    assert!(
        repo_root.join("feature.txt").exists(),
        "Base branch must contain merged feature.txt after successful merge"
    );
    let merged_content = fs::read_to_string(repo_root.join("feature.txt")).unwrap();
    assert_eq!(merged_content, "fixed feature implementation");

    // 8. Call state_next with verify_stage: "final"
    //    -> succeeds, state marks completed
    let final_args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "verify_stage": "final"
    });
    let final_res = handle_tool_call("state_next", &final_args)
        .expect("state_next with final stage must succeed");
    assert_eq!(final_res["status"], "success");
    assert_eq!(final_res["stage"], "final");
    assert!(
        final_res["active_task"].is_null(),
        "active_task must be null after final task advancement"
    );

    // Verify overall state machine status is completed
    let show_args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "json": true
    });
    let show_res = handle_tool_call("state_show", &show_args).expect("state_show should succeed");
    assert_eq!(show_res["status"], "completed");
}

#[test]
fn test_mcp_tool_schemas_parity_progressive_verification() {
    let tools = list_all_mcp_tools();

    // Verify worktree_merge declares verify_full_suite
    let wt_merge = tools
        .iter()
        .find(|t| t.name == "worktree_merge")
        .expect("worktree_merge tool must exist");
    let wt_props = &wt_merge.input_schema.properties;
    assert_eq!(
        wt_props["verify_full_suite"]["type"], "string",
        "worktree_merge must declare verify_full_suite property of type string"
    );
    let wt_desc = wt_props["verify_full_suite"]["description"]
        .as_str()
        .expect("verify_full_suite description must be string");
    assert!(
        wt_desc.contains("verification") || wt_desc.contains("command"),
        "verify_full_suite must describe verification command execution"
    );

    // Verify state_next declares verify_stage
    let state_next = tools
        .iter()
        .find(|t| t.name == "state_next" || t.name == "state_step")
        .expect("state_next or state_step tool must exist");
    let next_props = &state_next.input_schema.properties;
    assert_eq!(
        next_props["verify_stage"]["type"], "string",
        "state_next must declare verify_stage property of type string"
    );
    let enum_vals = next_props["verify_stage"]["enum"]
        .as_array()
        .expect("verify_stage enum array");
    let stages: Vec<&str> = enum_vals.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        stages.contains(&"milestone") && stages.contains(&"final"),
        "verify_stage enum must contain 'milestone' and 'final', got: {:?}",
        stages
    );
}

#[test]
fn test_state_next_infers_impacted_tests_for_empty_verify_cmd_milestone() {
    let (_dir, repo_root) = setup_test_repo();

    // Create a source file and corresponding test file in main
    let src_dir = repo_root.join("src");
    let tests_dir = repo_root.join("tests");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&tests_dir).unwrap();

    let calc_src = src_dir.join("calc.py");
    let calc_test = tests_dir.join("test_calc.py");
    fs::write(&calc_src, "def add(a, b): return a + b\n").unwrap();
    fs::write(&calc_test, "def test_add(): assert True\n").unwrap();

    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "feat: add calc and test"])
        .current_dir(&repo_root)
        .output()
        .unwrap();

    // Initialize state with a milestone whose verify_cmd is empty / missing
    let handoff_args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "goal": "Test impact inference on empty verify_cmd",
        "milestones": [
            {
                "id": "task-1",
                "title": "Impacted Test Inference Task",
                "verify_cmd": "true" // Passed preflight check during handoff
            }
        ],
        "no_worktree": true
    });
    handle_tool_call("superstate_handoff", &handoff_args).unwrap();

    // Clear verification_cmd in state to simulate empty milestone verify_cmd
    let state_file = repo_root.join(".agent-state").join("state.json");
    let mut state: superstate_core::models::ExecutionState =
        superstate_core::state_machine::load_state(&state_file).unwrap();
    if let Some(ref mut task) = state.active_task {
        task.verification_cmd = None;
    }
    superstate_core::state_machine::save_state_atomic(&state_file, &state).unwrap();

    // Modify calc.py
    fs::write(&calc_src, "def add(a, b): return a + b + 0\n").unwrap();

    // Call state_next with verify_stage: "milestone" and no verify_cmd
    let next_args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "verify_stage": "milestone"
    });
    let res = handle_tool_call("state_next", &next_args)
        .expect("state_next should succeed with impacted tests inferred");
    assert_eq!(res["status"], "success");
    assert_eq!(res["stage"], "milestone");

    // Impacted tests should be recorded in response payload
    assert!(
        res.get("impacted_tests").is_some(),
        "Response should include impacted_tests, got: {:?}",
        res
    );
    let impacted_tests = res["impacted_tests"].as_array().unwrap();
    let has_calc_test = impacted_tests
        .iter()
        .any(|v| v.as_str().unwrap().contains("test_calc.py"));
    assert!(
        has_calc_test,
        "impacted_tests should include test_calc.py, got: {:?}",
        impacted_tests
    );

    // Synthesized targeted test command should be present and valid
    assert!(
        res.get("synthesized_verify_cmd").is_some(),
        "Response should include synthesized_verify_cmd, got: {:?}",
        res
    );
    let synth_cmd = res["synthesized_verify_cmd"].as_str().unwrap();
    assert!(
        synth_cmd.starts_with("pytest") && synth_cmd.contains("test_calc.py"),
        "synthesized_verify_cmd should target test_calc.py with pytest, got: {}",
        synth_cmd
    );
}

#[test]
fn test_state_next_infers_impacted_tests_in_active_worktree_without_wt_prefix() {
    let (_dir, repo_root) = setup_test_repo();

    // Create baseline test structure on main branch
    let src_dir = repo_root.join("src");
    let tests_dir = repo_root.join("tests");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&tests_dir).unwrap();

    let calc_src = src_dir.join("calc.py");
    let calc_test = tests_dir.join("test_calc.py");
    fs::write(&calc_src, "def add(a, b): return a + b\n").unwrap();
    fs::write(&calc_test, "def test_add(): assert True\n").unwrap();

    Command::new("git")
        .args(["add", "."])
        .current_dir(&repo_root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "feat: initial calc and test"])
        .current_dir(&repo_root)
        .output()
        .unwrap();

    // 1. superstate_handoff with no_worktree: false (provisions task worktree)
    let handoff_args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "goal": "Test worktree-scoped impact command generation",
        "milestones": [
            {
                "id": "task-1",
                "title": "Worktree Inferred Test Milestone",
                "verify_cmd": "true"
            }
        ],
        "no_worktree": false
    });
    let handoff_res = handle_tool_call("superstate_handoff", &handoff_args)
        .expect("superstate_handoff should succeed");
    assert_eq!(handoff_res["status"], "ready");
    let wt_path_str = handoff_res["worktree_path"]
        .as_str()
        .expect("worktree_path should be present");
    let wt_path = PathBuf::from(wt_path_str);
    assert!(wt_path.exists(), "worktree path must exist on disk");

    // 2. Clear verification_cmd in state to trigger impacted test inference
    let state_path = repo_root.join(".agent-state").join("state.json");
    let mut state: superstate_core::models::ExecutionState =
        superstate_core::state_machine::load_state(&state_path).unwrap();
    assert!(
        state.active_worktree.is_some(),
        "active_worktree must be populated"
    );
    if let Some(ref mut task) = state.active_task {
        task.verification_cmd = None;
    }
    superstate_core::state_machine::save_state_atomic(&state_path, &state).unwrap();

    // 3. Verify that a failing inferred test blocks state_next
    let wt_calc_src = wt_path.join("src").join("calc.py");
    let wt_calc_test = wt_path.join("tests").join("test_calc.py");
    fs::write(&wt_calc_test, "def test_fail(): assert False\n").unwrap();
    fs::write(&wt_calc_src, "def add(a, b): return a + b\n").unwrap();

    let next_args = json!({
        "repo_root": repo_root.to_str().unwrap(),
        "verify_stage": "milestone"
    });
    let err_res = handle_tool_call("state_next", &next_args);
    assert!(
        err_res.is_err(),
        "state_next must fail when synthesized targeted test fails"
    );
    let err_msg = err_res.unwrap_err();
    assert!(
        err_msg.contains("verify_cmd: pytest tests/test_calc.py") && err_msg.contains("FAILED"),
        "error message should indicate verification_cmd gate failure, got: {}",
        err_msg
    );

    // 4. Fix test in the isolated worktree
    fs::write(&wt_calc_test, "def test_ok(): assert True\n").unwrap();
    fs::write(&wt_calc_src, "def add(a, b): return a + b + 0\n").unwrap();

    // 5. Advance with verify_stage: milestone
    let res = handle_tool_call("state_next", &next_args)
        .expect("state_next should succeed in worktree with inferred tests passing");
    assert_eq!(res["status"], "success");
    assert_eq!(res["stage"], "milestone");

    // 6. Verify synthesized_verify_cmd does NOT contain ".wt/"
    assert!(
        res.get("synthesized_verify_cmd").is_some(),
        "Response should include synthesized_verify_cmd, got: {:?}",
        res
    );
    let synth_cmd = res["synthesized_verify_cmd"].as_str().unwrap();
    assert!(
        !synth_cmd.contains(".wt/"),
        "synthesized_verify_cmd must not contain '.wt/' path prefix: {}",
        synth_cmd
    );
    assert!(
        synth_cmd.starts_with("pytest") && synth_cmd.contains("tests/test_calc.py"),
        "synthesized_verify_cmd must target relative test path inside worktree root: {}",
        synth_cmd
    );
}
