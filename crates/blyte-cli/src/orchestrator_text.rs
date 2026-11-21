use std::path::Path;

use blyte_agent::orchestrator::{assemble, resolve_sources, start_lines, user_rules_path, Answers};
use blyte_agent::state::State;

pub const REFERENCE: &str = include_str!("../assets/orchestrator/reference.md");
pub const REFERENCE_DISPATCH_TOOL: &str =
    include_str!("../assets/orchestrator/reference-dispatch-tool.md");
pub const DEFAULT_PROCESS: &str = include_str!("../assets/orchestrator/process.md");

pub struct Built {
    pub instructions: String,
    pub start_lines: [String; 2],
}

pub fn build(dispatch_tool: bool, answers: Answers, root: &Path) -> Result<Built, String> {
    let (reference, default_process) = if dispatch_tool {
        (REFERENCE_DISPATCH_TOOL, None)
    } else {
        (REFERENCE, Some(DEFAULT_PROCESS))
    };
    let user_path = user_rules_path();
    let sources = resolve_sources(user_path.as_deref(), root, default_process)?;
    let instructions = assemble(reference, answers, &sources);
    let start_lines = start_lines(&sources);
    Ok(Built {
        instructions,
        start_lines,
    })
}

pub fn set_state_instructions(
    state: &mut State,
    dispatch_tool: bool,
    root: &Path,
) -> Result<[String; 2], String> {
    let answers = if state.runtime.conversation.is_some() {
        Answers::Person
    } else {
        Answers::Nobody
    };
    let built = build(dispatch_tool, answers, root)?;
    state.orchestrator.instructions = Some(built.instructions);
    Ok(built.start_lines)
}
