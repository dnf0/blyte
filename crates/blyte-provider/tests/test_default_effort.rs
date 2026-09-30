//! The default Gemini model runs at high effort: every measured run used `-high` (2026-09-30).

use blyte_provider::gemini::Gemini;
use blyte_provider::Provider;

#[test]
fn the_default_model_is_flash_at_high_effort() {
    std::env::remove_var("BLYTE_MODEL");
    std::env::set_var("GEMINI_API_KEY", "test-key");
    let g = Gemini::from_env().expect("Gemini::from_env");
    assert_eq!(g.model_id(), "gemini/gemini-3.8-flash");
    assert_eq!(g.thinking_level(), Some("high"));
}
