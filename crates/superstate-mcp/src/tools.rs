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
    pub input_schema: McpToolInputSchema,
}

pub fn list_all_mcp_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "state_init".to_string(),
            description: "Initialize a DFA execution state machine from a markdown implementation plan".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "plan": { "type": "string", "description": "Path to implementation plan markdown file" },
                    "state": { "type": "string", "description": "Path to state JSON output (default .agent-state/state.json)" }
                }),
                required: vec!["plan".to_string()],
            },
        },
        McpTool {
            name: "state_next".to_string(),
            description: "Advance active task in state machine with verification gates".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "verify_cmd": { "type": "string", "description": "Optional shell command to verify" },
                    "verify_git_clean": { "type": "boolean", "description": "Require clean git working tree" }
                }),
                required: vec![],
            },
        },
        McpTool {
            name: "state_prompt".to_string(),
            description: "Generate bounded turnkey subagent prompt for active task".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({}),
                required: vec![],
            },
        },
        McpTool {
            name: "state_note".to_string(),
            description: "Append discovery, hypothesis, or constraint to epistemic ledger".to_string(),
            input_schema: McpToolInputSchema {
                r#type: "object".to_string(),
                properties: serde_json::json!({
                    "note": { "type": "string", "description": "The note content" }
                }),
                required: vec!["note".to_string()],
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
                    }
                }),
                required: vec!["verify_cmd".to_string()],
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
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional target files to modify"
                    },
                    "no_worktree": {
                        "type": "boolean",
                        "description": "If true, execute in-place without creating a worktree"
                    }
                }),
                required: vec!["goal".to_string(), "verify_cmd".to_string()],
            },
        },
    ]
}
