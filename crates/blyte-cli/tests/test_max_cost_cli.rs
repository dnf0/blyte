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
