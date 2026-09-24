use blyte_rules::render::{render_agents_md, render_cursor_rules, RulesConfig};

#[test]
fn test_render_agents_md() {
    let config = RulesConfig {
        repo_name: "test-repo".to_string(),
        language: "rust".to_string(),
        strictness: "balanced".to_string(),
    };
    let content = render_agents_md(&config).unwrap();
    assert!(content.contains("# test-repo Engineering Agent Guidance"));
    assert!(content.contains("Superpowers Integration"));
    assert!(content.contains("stateful-execution"));
}

#[test]
fn test_render_cursor_rules() {
    let config = RulesConfig {
        repo_name: "test-repo".to_string(),
        language: "rust".to_string(),
        strictness: "balanced".to_string(),
    };
    let content = render_cursor_rules(&config).unwrap();
    assert!(content.contains("description: Blyte state-machine execution rules"));
    assert!(content.contains("globs: *"));
}
