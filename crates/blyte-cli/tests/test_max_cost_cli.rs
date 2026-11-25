//! `--max-cost` is refused for a model with no known price, before any model call.

use std::process::Command;

#[test]
fn a_cap_on_an_unpriced_model_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args([
            "--max-cost",
            "1.0",
            "--model",
            "ollama/qwen3:14b",
            "--max-turns",
            "1",
            "do nothing",
        ])
        .current_dir(d.path())
        .env("OLLAMA_HOST", "http://127.0.0.1:9")
        .env_remove("GEMINI_API_KEY")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr) + String::from_utf8_lossy(&out.stdout);
    assert!(err.contains("--max-cost needs a known price"), "{err}");
}

#[test]
fn dispatch_help_and_config_template_state_default_max_cost() {
    let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
        .args(["dispatch", "--help"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    let max_cost_section = help
        .split("--max-cost")
        .nth(1)
        .expect("--max-cost in dispatch --help");
    let first_chunk = &max_cost_section[..max_cost_section.len().min(200)];
    assert!(
        first_chunk.contains("25") && first_chunk.to_ascii_lowercase().contains("default"),
        "expected default 25 in --max-cost help: {first_chunk}"
    );

    let rendered = blyte_cli::config::render(&["true".to_string()]);
    assert!(
        rendered.contains("# max_cost = 25.0   # USD per run; this is the default"),
        "rendered template: {rendered}"
    );
}
