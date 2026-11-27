use std::path::Path;

use blyte_agent::conversation::{assemble, resolve_sources, start_lines, user_rules_path, Answers};
use blyte_agent::state::State;

pub const REFERENCE: &str = include_str!("../assets/conversation/reference.md");
pub const REFERENCE_DISPATCH_TOOL: &str =
    include_str!("../assets/conversation/reference-dispatch-tool.md");
pub const DEFAULT_PROCESS: &str = include_str!("../assets/conversation/process.md");

pub const SANDBOX_ON_PARAGRAPH: &str = "The shell is sandboxed: the shell and the write tools can write only in `.wt/tasks` and temporary directories,\ncannot read credentials and have no network. The shell and the write tools can also write in the worktree of an\nopen edit started with `blyte edit-start`, until it finishes; `git commit` there is not possible (`edit-finish`\ncommits). A command that must leave the sandbox (for example a package install) takes `unsandboxed` with a reason and\nruns only when the user approves, and a write or edit outside those directories also needs the user's approval.\nTo publish a request, use the `publish` tool; it runs only when the user approves it on the page.";
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
    let sandbox_off_process;
    let (base_reference, default_process) = if dispatch_tool {
        (REFERENCE_DISPATCH_TOOL, None)
    } else if sandbox_on {
        (REFERENCE, Some(DEFAULT_PROCESS))
    } else {
        sandbox_off_process = DEFAULT_PROCESS.replace(
            "unsandboxed commands the user approves",
            "commands the user approves",
        );
        (REFERENCE, Some(sandbox_off_process.as_str()))
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
    state.brief.instructions = Some(built.instructions);
    Ok(built.start_lines)
}
