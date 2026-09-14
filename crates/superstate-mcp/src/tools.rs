//! MCP Gateway Tool Definitions for Superstate.
//!
//! Exposes strictly the 4 Gateway tools defined in ADR 0002:
//! 1. `superstate_handoff`: Sole execution entrypoint for autonomous detached runs.
//! 2. `superstate_status`: Status and telemetry inspection for active state machines.
//! 3. `superstate_cancel`: Graceful termination signal for background daemons.
//! 4. `superstate_doctor`: Fail-closed preflight and environment verification probe.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Represents a single property within an MCP tool input schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolProperty {
    pub r#type: String,
    pub description: String,
}

/// JSON Schema definition for MCP tool parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpToolInputSchema {
    pub r#type: String,
    pub properties: Value,
    pub required: Vec<String>,
}

/// Metadata and input schema specification for an MCP tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema", alias = "input_schema")]
    pub input_schema: McpToolInputSchema,
}

/// Constructs and returns the strict 4-tool Gateway catalog.
///
/// In accordance with ADR 0002, internal state mutation tools (`state_init`, `state_next`,
/// `state_prompt`, `worktree_*`, etc.) are eliminated from the public MCP catalog.
///
/// # Returns
/// A vector of exactly 4 `McpTool` definitions representing the public Gateway.
pub fn list_all_mcp_tools() -> Vec<McpTool> {
    vec![
        // Gateway Tool 1: superstate_handoff
        // WHY: Autonomous execution requires launching a detached runner in an isolated worktree.
        // WHAT: Initializes state, provisions isolated worktrees, and launches background daemon.
        // HOW: Takes goal, milestones, verification invariant, and defaults to detached execution.
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
                    "from_agent": {
                        "type": "string",
                        "description": "Source agent performing handoff (e.g. claude, agy, human)"
                    },
                    "to_agent": {
                        "type": "string",
                        "description": "Target agent receiving handoff (e.g. agy, claude, daemon, any)"
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
                        "description": "Enable Git worktree isolation for tasks (default: true)"
                    },
                    "no_worktree": {
                        "type": "boolean",
                        "description": "If true, execute in-place without creating a worktree"
                    },
                    "detach": {
                        "type": "boolean",
                        "description": "If true, launch background daemon runner and return immediately (default: true)"
                    },
                    "driver": {
                        "type": "string",
                        "description": "Headless coding driver command or agent (e.g. claude, agy, or command)"
                    }
                }),
                required: vec![],
            },
        },
        // Gateway Tool 2: superstate_status
        // WHY: Operators and client agents require observability into detached task progress.
        // WHAT: Interrogates state machine progress, current milestone, and daemon health.
        // HOW: Reads branch-scoped state file, epistemic ledger, and daemon PID status.
        McpTool {
            name: "superstate_status".to_string(),
            description: "Display state machine status, daemon execution status, and epistemic ledger".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "state": {
                        "type": "string",
                        "description": "Path to state JSON file (default: .agent-state/state.json)"
                    },
                    "json": {
                        "type": "boolean",
                        "description": "Emit machine-readable JSON format"
                    }
                }),
                required: vec![],
            },
        },
        // Gateway Tool 3: superstate_cancel
        // WHY: Operators need to abort errant or runaway background workers cleanly.
        // WHAT: Sends termination signal (SIGTERM escalating to SIGKILL) and cleans up PID files.
        // HOW: Resolves PID by state path or branch slug, terminates the process, and unlinks PID.
        McpTool {
            name: "superstate_cancel".to_string(),
            description: "Signal and terminate active background daemon processes by state path, branch, or fleet".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "slug": {
                        "type": "string",
                        "description": "Slug of the daemon runner to cancel"
                    },
                    "state": {
                        "type": "string",
                        "description": "Path to state JSON file (default: .agent-state/state.json)"
                    },
                    "branch": {
                        "type": "string",
                        "description": "Branch slug of daemon to cancel"
                    },
                    "all": {
                        "type": "boolean",
                        "description": "Cancel all running daemons in the repository"
                    }
                }),
                required: vec![],
            },
        },
        // Gateway Tool 4: superstate_doctor
        // WHY: Fail-closed architecture requires verifying runtime prerequisites before work begins.
        // WHAT: Probes environment binaries, git baseline integrity, and headless driver availability.
        // HOW: Checks binary paths, validates git working tree, and optionally executes driver probe.
        McpTool {
            name: "superstate_doctor".to_string(),
            description: "Diagnose and verify environment readiness, binary integrity, and state health".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "driver": {
                        "type": "string",
                        "description": "Optional driver preset or command to probe (e.g. agy, claude)"
                    },
                    "state": {
                        "type": "string",
                        "description": "Path to state JSON file (default: .agent-state/state.json)"
                    },
                    "plan": {
                        "type": "string",
                        "description": "Optional path to plan markdown to verify against state"
                    },
                    "repair": {
                        "type": "boolean",
                        "description": "Automatically repair detected minor state inconsistencies"
                    },
                    "json": {
                        "type": "boolean",
                        "description": "Emit machine-readable JSON"
                    }
                }),
                required: vec![],
            },
        },
    ]
}
