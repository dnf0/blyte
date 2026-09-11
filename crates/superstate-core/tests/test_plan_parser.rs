use std::fs;
use superstate_core::plan_parser::{init_state_from_plan, parse_plan_markdown};
use tempfile::tempdir;

#[test]
fn test_parse_plan_markdown() {
    let markdown = r#"
# Feature Plan

### Task 1: Scaffolding
- **Depends on:** none
**Files:**
- Create: `src/lib.rs`
**Verification Command:** `cargo test`
**Goal:** Initialize project

### Task 2: Core Logic
- **Depends on:** Task 1
**Files:**
- Modify: `src/lib.rs`
**Verification Command:** `cargo test`
**Goal:** Add core logic
"#;
    let tasks = parse_plan_markdown(markdown).expect("Failed to parse plan");
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].id, "Task 1");
    assert_eq!(tasks[0].title, "Scaffolding");
    assert_eq!(tasks[0].verification_cmd.as_deref(), Some("cargo test"));
    assert_eq!(tasks[1].id, "Task 2");
    assert_eq!(tasks[1].title, "Core Logic");
    assert_eq!(tasks[1].depends_on, vec!["Task 1"]);
}

#[test]
fn test_parse_plan_markdown_unbolded_variations() {
    let markdown = r#"
# Feature Plan

### Task 1: Scaffolding
- Depends on: none
Files:
- Modify: `/app/secret.txt`
- Verification Command: `test -s /app/secret.txt && grep -Eq '^secret\[.+\]$' /app/secret.txt`
Goal: Initialize project

### Task 2: Expire reflogs
- **Depends on:** Task 1
Files:
- Modify: `/app/repo/.git`
- Verification: `test -z "$(git -C /app/repo reflog)"`
"#;
    let tasks =
        parse_plan_markdown(markdown).expect("Failed to parse plan with unbolded variations");
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].id, "Task 1");
    assert_eq!(
        tasks[0].verification_cmd.as_deref(),
        Some("test -s /app/secret.txt && grep -Eq '^secret\\[.+\\]$' /app/secret.txt")
    );
    assert_eq!(tasks[1].id, "Task 2");
    assert_eq!(tasks[1].depends_on, vec!["Task 1"]);
    assert_eq!(
        tasks[1].verification_cmd.as_deref(),
        Some("test -z \"$(git -C /app/repo reflog)\"")
    );
}

#[test]
fn test_init_state_from_plan() {
    let dir = tempdir().unwrap();
    let plan_file = dir.path().join("plan.md");
    let state_file = dir.path().join(".agent-state").join("state.json");

    let markdown = r#"
# My Plan

### Task 1: Step One
- **Depends on:** none
**Files:**
- Create: `step1.txt`

### Task 2: Step Two
- **Depends on:** Task 1
**Files:**
- Create: `step2.txt`
"#;
    fs::write(&plan_file, markdown).unwrap();

    let state =
        init_state_from_plan(&plan_file, &state_file).expect("Failed to init state from plan");
    assert_eq!(state.step, 1);
    assert!(state.active_task.is_some());
    assert_eq!(state.active_task.unwrap().id, "Task 1");
    assert_eq!(state.pending_tasks.len(), 1);
    assert_eq!(state.pending_tasks[0].id, "Task 2");
    assert!(state_file.exists());
}
