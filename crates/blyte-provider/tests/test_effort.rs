//! The provider says which effort it runs at (2026-10-01).

use blyte_provider::gemini::Gemini;
use blyte_provider::Provider;

#[test]
fn gemini_reports_its_effort_and_others_report_none() {
    std::env::set_var("GEMINI_API_KEY", "test-key");
    let g = Gemini::from_env()
        .unwrap()
        .with_model("gemini-3.8-flash-medium");
    assert_eq!(g.effort().as_deref(), Some("medium"));
    assert_eq!(g.model_id(), "gemini/gemini-3.8-flash");
    let plain = Gemini::from_env().unwrap().with_model("gemini-3.8-flash");
    assert_eq!(plain.effort(), None);
}
