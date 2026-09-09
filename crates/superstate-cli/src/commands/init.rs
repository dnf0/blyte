use std::fs;
use std::path::Path;
use superstate_rules::render::{render_agents_md, render_cursor_rules, RulesConfig};

pub fn cmd_init_rules(dest: &Path, language: &str, strictness: &str) -> Result<(), String> {
    let repo_name = dest
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string();

    let config = RulesConfig {
        repo_name,
        language: language.to_string(),
        strictness: strictness.to_string(),
    };

    let agents_md =
        render_agents_md(&config).map_err(|e| format!("Failed to render AGENTS.md: {}", e))?;
    let cursor_rules = render_cursor_rules(&config)
        .map_err(|e| format!("Failed to render cursor rules: {}", e))?;

    fs::write(dest.join("AGENTS.md"), agents_md)
        .map_err(|e| format!("Failed to write AGENTS.md: {}", e))?;

    let cursor_dir = dest.join(".cursor").join("rules");
    fs::create_dir_all(&cursor_dir)
        .map_err(|e| format!("Failed to create .cursor/rules directory: {}", e))?;
    fs::write(cursor_dir.join("superstate.mdc"), cursor_rules)
        .map_err(|e| format!("Failed to write cursor rules: {}", e))?;

    println!("Initialized agent guidance in {}", dest.display());
    println!("  ✓ AGENTS.md");
    println!("  ✓ .cursor/rules/superstate.mdc");
    Ok(())
}
