//! Tests for usage entry session fallback to Claude Code session ID.

use blyte_cli::usage_log::session_from;

#[test]
fn test_session_from() {
    assert_eq!(
        session_from(Some("b".into()), Some("c".into())),
        Some("b".into())
    );
    assert_eq!(session_from(None, Some("c".into())), Some("c".into()));
    assert_eq!(
        session_from(Some("".into()), Some("c".into())),
        Some("c".into())
    );
    assert_eq!(session_from(Some("  ".into()), None), None);
    assert_eq!(session_from(None, None), None);
}

#[test]
fn test_session_from_whitespace_blyte_falls_back_to_claude() {
    assert_eq!(
        session_from(Some("  ".into()), Some("c".into())),
        Some("c".into())
    );
}
