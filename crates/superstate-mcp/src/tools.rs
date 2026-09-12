use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolProperty {
    pub r#type: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolInputSchema {
    pub r#type: String,
    pub properties: Value,
    pub required: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema", alias = "input_schema")]
    pub input_schema: McpToolInputSchema,
}

pub fn list_all_mcp_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "state_init".to_string(),
            description: "Initialize a DFA execution state machine from a markdown implementation plan, goal, or milestones".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "plan": { "type": "string", "description": "Path to implementation plan markdown file" },
                    "state": { "type": "string", "description": "Path to state JSON output (default .agent-state/state.json)" },
                    "goal": { "type": "string", "description": "Goal description for direct initialization without a plan" },
                    "verify_cmd": { "type": "string", "description": "Verification command required when goal is provided" },
                    "milestones": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "description": "Milestone title or task name" },
                                "verify_cmd": { "type": "string", "description": "Verification command that must exit 0" },
                                "files": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Optional target files to modify"
                                }
                            },
                            "required": ["title", "verify_cmd"]
                        },
                        "description": "Optional array of milestones for direct multi-task DAG initialization"
                    },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of files to modify"
                    },
                    "worktree": {
                        "type": "boolean",
                        "description": "Enable Git worktree isolation for tasks (default: false, runs in-place)"
                    },
                    "no_worktree": {
                        "type": "boolean",
                        "description": "Disable git worktree creation; operate in-place in workspace root"
                    }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_show".to_string(),
            description: "Show execution state in markdown or JSON format".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "json": { "type": "boolean", "description": "Emit machine-readable JSON" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_status".to_string(),
            description: "Display state machine status and epistemic ledger".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "json": { "type": "boolean", "description": "Emit machine-readable JSON" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_next".to_string(),
            description: "Advance active task in state machine with verification gates".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "verify_stage": {
                        "type": "string",
                        "enum": ["milestone", "final"],
                        "description": "Verification stage: 'milestone' (default) executes scoped checks; 'final' enforces comprehensive verification before completing execution state"
                    },
                    "verify_cmd": { "type": "string", "description": "Optional shell command to verify" },
                    "verify_git_clean": { "type": "boolean", "description": "Require clean git working tree" },
                    "ignore_untracked_glob": { "type": "string", "description": "Optional glob pattern of untracked files to ignore during git clean verification" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_ready".to_string(),
            description: "Display tasks whose dependencies are satisfied and may run now".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "json": { "type": "boolean", "description": "Emit a JSON array of task titles" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_checkpoint".to_string(),
            description: "Create a git checkpoint guarding the current step".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_rollback".to_string(),
            description: "Reset the working tree and untracked files to a checkpoint".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "reference": { "type": "string", "description": "Checkpoint to restore (defaults to the current step's checkpoint)" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_prompt".to_string(),
            description: "Generate bounded turnkey subagent prompt for active task".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "task": { "type": "string", "description": "Task to prompt for, by id or title (defaults to active task)" },
                    "with_graph": { "type": "boolean", "description": "Include knowledge-graph context in the prompt" },
                    "json": { "type": "boolean", "description": "Emit machine-readable JSON" },
                    "no_worktree": { "type": "boolean", "description": "Disable git worktree creation; operate in-place in workspace root" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_sync".to_string(),
            description: "Reconcile state with a revised plan, preserving completed tasks".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "plan": { "type": "string", "description": "Path to revised plan markdown file" },
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "out": { "type": "string", "description": "Write the reconciled state here instead of in place" }
                }),
                required: vec!["plan".to_string()],
            },
        },
        McpTool {
            name: "state_exec".to_string(),
            description: "Run a verification command and atomically advance or record failure".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "verify_cmd": { "type": "string", "description": "Verification shell command to execute" },
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "verify_git_clean": { "type": "boolean", "description": "Require clean git working tree" },
                    "checkpoint": { "type": "boolean", "description": "Checkpoint before running, so a failure can be rolled back" },
                    "rollback_on_fail": { "type": "boolean", "description": "Restore the checkpoint if verification fails" },
                    "advance": { "type": "boolean", "description": "Advance step on successful verification (default: true)" },
                    "json": { "type": "boolean", "description": "Emit machine-readable JSON" }
                }),
                required: vec!["verify_cmd".to_string()],
            },
        },
        McpTool {
            name: "state_doctor".to_string(),
            description: "Diagnose plan/state drift and optionally repair it".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "plan": { "type": "string", "description": "Optional plan markdown file for drift diagnosis" },
                    "repair": { "type": "boolean", "description": "Automatically repair detected state inconsistencies" },
                    "json": { "type": "boolean", "description": "Emit machine-readable JSON" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_validate".to_string(),
            description: "Validate a state JSON file against the execution schema".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_apply".to_string(),
            description: "Apply a state patch (null deletes) and/or append a note".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "patch": {
                        "description": "RFC 7396 JSON merge patch object or file path/JSON string",
                        "oneOf": [
                            { "type": "object" },
                            { "type": "string" }
                        ]
                    },
                    "note": { "type": "string", "description": "Optional note to append to the epistemic ledger" },
                    "out": { "type": "string", "description": "Write modified state here instead of in place" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_resume".to_string(),
            description: "Executive briefing for resuming after context loss".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_metrics".to_string(),
            description: "Task telemetry and efficiency metrics".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "json": { "type": "boolean", "description": "Emit machine-readable JSON" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_list".to_string(),
            description: "List active and completed branch-scoped states".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": { "type": "string", "description": "Optional path to state file or directory to scan" },
                    "json": { "type": "boolean", "description": "Emit machine-readable JSON" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_note".to_string(),
            description: "Append discovery, hypothesis, or constraint to epistemic ledger".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "note": { "type": "string", "description": "The note content" },
                    "state": { "type": "string", "description": "Path to state JSON file (default: .agent-state/state.json)" },
                    "author": { "type": "string", "description": "Author attribution for note (default: agent)" }
                }),
                required: vec!["note".to_string()],
            },
        },
        McpTool {
            name: "worktree_create".to_string(),
            description: "Create an isolated Git worktree for parallel task execution".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "branch": { "type": "string", "description": "Branch name to create or checkout" },
                    "base": { "type": "string", "description": "Optional base branch to branch off from (e.g. main)" },
                    "path": { "type": "string", "description": "Optional custom destination path for worktree" },
                    "caches": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Cache directory names to reflink-copy (e.g. .venv target node_modules)"
                    },
                    "no_cache": { "type": "boolean", "description": "Disable auto-detection and copying of cache directories" },
                    "repo": { "type": "string", "description": "Base repository root (defaults to current dir)" }
                }),
                required: vec!["branch".to_string()],
            },
        },
        McpTool {
            name: "worktree_merge".to_string(),
            description: "Merge an isolated Git worktree branch into the base branch".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "branch": { "type": "string", "description": "Branch name of the worktree to merge" },
                    "base": { "type": "string", "description": "Optional base branch to merge into" },
                    "message": { "type": "string", "description": "Optional commit message for the merge" },
                    "repo": { "type": "string", "description": "Base repository root (defaults to current dir)" },
                    "verify_full_suite": {
                        "type": "string",
                        "description": "Optional pre-merge verification command executed in the isolated worktree before merging into base"
                    }
                }),
                required: vec!["branch".to_string()],
            },
        },
        McpTool {
            name: "worktree_list".to_string(),
            description: "List active worktrees in the repository".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "repo": { "type": "string", "description": "Base repository root (defaults to current dir)" },
                    "json": { "type": "boolean", "description": "Emit machine-readable JSON array" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "worktree_remove".to_string(),
            description: "Remove a worktree and clean up its tracking".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "branch": { "type": "string", "description": "Branch name of the worktree to remove" },
                    "force": { "type": "boolean", "description": "Force removal even if worktree is dirty" },
                    "repo": { "type": "string", "description": "Base repository root (defaults to current dir)" }
                }),
                required: vec!["branch".to_string()],
            },
        },
        McpTool {
            name: "worktree_clean".to_string(),
            description: "Clean orphaned worktree directories and prune tracking".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "repo": { "type": "string", "description": "Base repository root (defaults to current dir)" },
                    "force": { "type": "boolean", "description": "Force removal of dirty or unmerged orphaned branches" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "superstate_review".to_string(),
            description: "Adversarially review git diff for regression risks, memory leaks, and architectural boundary violations".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "commit_range": { "type": "string", "description": "Git commit range (e.g. HEAD~1..HEAD)" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "superstate_preflight".to_string(),
            description: "Validate a verification command on the OS in a dry-run without modifying state or code".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "verify_cmd": {
                        "type": "string",
                        "description": "Shell command to execute in dry-run validation"
                    },
                    "verify_cmds": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of shell commands to execute in dry-run validation"
                    }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "superstate_handoff".to_string(),
            description: "Directly initialize state and provision turnkey prompt from a goal and verification invariant without plan.md".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "goal": {
                        "type": "string",
                        "description": "Concise statement of capability delivered"
                    },
                    "verify_cmd": {
                        "type": "string",
                        "description": "Shell command that must exit 0 to pass"
                    },
                    "milestones": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "description": "Milestone title or task name" },
                                "verify_cmd": { "type": "string", "description": "Verification command that must exit 0" },
                                "files": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Optional target files to modify"
                                }
                            },
                            "required": ["title", "verify_cmd"]
                        },
                        "description": "Optional array of milestones for direct multi-task DAG initialization"
                    },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional target files to modify"
                    },
                    "worktree": {
                        "type": "boolean",
                        "description": "Enable Git worktree isolation for tasks (default: false, runs in-place)"
                    },
                    "no_worktree": {
                        "type": "boolean",
                        "description": "If true, execute in-place without creating a worktree"
                    }
                }),
                required: vec![],
            },
        },
    ]
}
