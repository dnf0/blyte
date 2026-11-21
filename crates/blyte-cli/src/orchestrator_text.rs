use std::path::Path;

use blyte_agent::orchestrator::{assemble, resolve_sources, start_lines, user_rules_path, Answers};
use blyte_agent::state::State;

pub const REFERENCE: &str = include_str!("../assets/orchestrator/reference.md");
pub const REFERENCE_DISPATCH_TOOL: &str =
    include_str!("../assets/orchestrator/reference-dispatch-tool.md");
pub const DEFAULT_PROCESS: &str = include_str!("../assets/orchestrator/process.md");

pub const SANDBOX_ON_PARAGRAPH: &str = "The shell is sandboxed: it can write only in the repository and temporary directories, cannot read\ncredentials and has no network. A command that must leave the sandbox (for example `git push`) takes\n`unsandboxed` with a reason and runs only when the user approves.";
pub const SANDBOX_OFF_PARAGRAPH: &str = "The shell is not sandboxed.";

pub fn sandbox_start_line(sandbox_on: bool, allowed_in_advance: usize) -> String {
    if sandbox_on {
        format!(
            "sandbox: on (a command leaves it only with your approval; {allowed_in_advance} allowed in advance)"
        )
    } else {
        "sandbox: off".to_string()
    }
}

pub struct Built {
    pub instructions: String,
    pub start_lines: [String; 3],
}

pub fn build_with_sandbox(
    dispatch_tool: bool,
    answers: Answers,
    root: &Path,
    sandbox_on: bool,
    allowed_in_advance: usize,
) -> Result<Built, String> {
    let (base_reference, default_process) = if dispatch_tool {
        (REFERENCE_DISPATCH_TOOL, None)
    } else {
        (REFERENCE, Some(DEFAULT_PROCESS))
    };
    let sandbox_paragraph = if sandbox_on {
        SANDBOX_ON_PARAGRAPH
    } else {
        SANDBOX_OFF_PARAGRAPH
    };
    let reference = format!("{}\n\n{sandbox_paragraph}", base_reference.trim());
    let user_path = user_rules_path();
    let sources = resolve_sources(user_path.as_deref(), root, default_process)?;
    let instructions = assemble(&reference, answers, &sources);
    let [process_line, repo_line] = start_lines(&sources);
    let sandbox_line = sandbox_start_line(sandbox_on, allowed_in_advance);
    Ok(Built {
        instructions,
        start_lines: [process_line, repo_line, sandbox_line],
    })
}

pub fn build(dispatch_tool: bool, answers: Answers, root: &Path) -> Result<Built, String> {
    let allowed = crate::approval_file::load()?.len();
    build_with_sandbox(dispatch_tool, answers, root, true, allowed)
}

pub fn set_state_instructions(
    state: &mut State,
    dispatch_tool: bool,
    root: &Path,
    sandbox_on: bool,
    allowed_in_advance: usize,
) -> Result<[String; 3], String> {
    let answers = if state.runtime.conversation.is_some() {
        Answers::Person
    } else {
        Answers::Nobody
    };
    let built = build_with_sandbox(dispatch_tool, answers, root, sandbox_on, allowed_in_advance)?;
    state.orchestrator.instructions = Some(built.instructions);
    Ok(built.start_lines)
}
