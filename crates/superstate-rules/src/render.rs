use minijinja::Environment;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RenderError {
    #[error("Template rendering error: {0}")]
    Template(#[from] minijinja::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesConfig {
    pub repo_name: String,
    pub language: String,
    pub strictness: String,
}

const AGENTS_MD_TEMPLATE: &str = r#"# {{ repo_name }} Engineering Agent Guidance

Provider target: Universal

## Superpowers Integration
This repository leverages structured state-machine execution via Superstate and vendored superpowers skills.
Planning, execution, and verification operate under strict DFA state gates.

## Core Rules
- Language target: {{ language }}
- Operational strictness: {{ strictness }}
- Multi-step plan execution must follow the `stateful-execution` pattern.
- Advance tasks using state verification gates before declaring work complete.
"#;

const CURSOR_RULES_TEMPLATE: &str = r#"---
description: Superstate state-machine execution rules
globs: *
---
# {{ repo_name }} Cursor Rules

- All plan execution is tracked via `.agent-state/state.json`.
- Do not make speculative edits without updating or checking the state engine.
"#;

pub fn render_agents_md(config: &RulesConfig) -> Result<String, RenderError> {
    let mut env = Environment::new();
    env.add_template("agents.md", AGENTS_MD_TEMPLATE)?;
    let tmpl = env.get_template("agents.md")?;
    let rendered = tmpl.render(config)?;
    Ok(rendered)
}

pub fn render_cursor_rules(config: &RulesConfig) -> Result<String, RenderError> {
    let mut env = Environment::new();
    env.add_template("cursor.rules", CURSOR_RULES_TEMPLATE)?;
    let tmpl = env.get_template("cursor.rules")?;
    let rendered = tmpl.render(config)?;
    Ok(rendered)
}
