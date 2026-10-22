use blyte_cli::update_notice::{check, notice_line, summarize, Cache};

#[test]
fn summarize_extracts_bullets_and_cuts_over_200_chars() {
    let real = "### Feat\n\n- **agent**: nudge, then stop, fix runs that make no progress (#304)\n";
    assert_eq!(
        summarize(real),
        "agent: nudge, then stop, fix runs that make no progress (#304)"
    );
    assert_eq!(summarize("### Feat\n\nno bullets here\n"), "");

    let long_body = format!(
        "### Feat\n\n- **cli**: {}\n- **agent**: {}\n",
        "a".repeat(120),
        "b".repeat(120)
    );
    let cut = summarize(&long_body);
    assert!(cut.ends_with('…'), "expected ellipsis at end: {cut}");
    assert_eq!(cut.chars().count(), 201);

    // Multi-byte UTF-8 character across byte 200 cuts cleanly on a char boundary.
    let multibyte_body = format!("- {}é{}\n", "a".repeat(199), "b".repeat(50));
    let mb_cut = summarize(&multibyte_body);
    assert!(mb_cut.ends_with('…'), "{mb_cut}");
    assert_eq!(mb_cut, format!("{}…", "a".repeat(199)));
}

#[test]
fn notice_line_compares_versions_and_formats_summary() {
    assert_eq!(
        notice_line("0.35.0", "v0.36.0", "x").as_deref(),
        Some("blyte: 0.36.0 is out (you have 0.35.0): x. Run `blyte update`.")
    );
    assert_eq!(
        notice_line("0.35.0", "v0.36.0", "").as_deref(),
        Some("blyte: 0.36.0 is out (you have 0.35.0). Run `blyte update`.")
    );
    assert_eq!(notice_line("0.36.0", "v0.36.0", "x"), None);
    assert_eq!(notice_line("0.37.0", "v0.36.0", "x"), None);
    assert_eq!(notice_line("0.35.0", "not-a-version", "x"), None);
    assert_eq!(notice_line("not-a-version", "v0.36.0", "x"), None);
}

#[test]
fn check_with_no_cache_fetches_writes_cache_and_returns_notice() {
    let dir = tempfile::tempdir().unwrap();
    let lines = check(dir.path(), "0.35.0", 1_000_000, || {
        Ok((
            "v0.36.0".to_string(),
            "### Feat\n\n- **agent**: nudge, then stop, fix runs that make no progress (#304)\n"
                .to_string(),
        ))
    });
    assert_eq!(
        lines,
        vec![
            "blyte: 0.36.0 is out (you have 0.35.0): agent: nudge, then stop, fix runs that make no progress (#304). Run `blyte update`."
        ]
    );
    let saved: Cache = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("latest-release.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        saved,
        Cache {
            checked_at: 1_000_000,
            tag: Some("v0.36.0".to_string()),
            summary: Some(
                "agent: nudge, then stop, fix runs that make no progress (#304)".to_string()
            ),
            error: None,
        }
    );
}

#[test]
fn check_with_fresh_cache_does_not_call_fetch_and_returns_cached_notice() {
    let dir = tempfile::tempdir().unwrap();
    let _ = check(dir.path(), "0.35.0", 1_000_000, || {
        Ok((
            "v0.36.0".to_string(),
            "- **cli**: faster startup\n".to_string(),
        ))
    });

    let lines = check(dir.path(), "0.35.0", 1_000_000 + 3_600, || {
        panic!("fetch must not be called when cache is fresh")
    });
    assert_eq!(
        lines,
        vec!["blyte: 0.36.0 is out (you have 0.35.0): cli: faster startup. Run `blyte update`."]
    );
}

#[test]
fn check_with_stale_cache_and_failing_fetch_reports_error_once_and_keeps_old_tag() {
    let dir = tempfile::tempdir().unwrap();
    let t0 = 1_000_000;
    let _ = check(dir.path(), "0.35.0", t0, || {
        Ok((
            "v0.36.0".to_string(),
            "- **cli**: faster startup\n".to_string(),
        ))
    });

    let stale_now = t0 + 86_400;
    let lines = check(dir.path(), "0.35.0", stale_now, || {
        Err("HTTP 503".to_string())
    });
    assert_eq!(
        lines,
        vec![
            "blyte: could not check for a newer release: HTTP 503",
            "blyte: 0.36.0 is out (you have 0.35.0): cli: faster startup. Run `blyte update`."
        ]
    );

    let lines_later = check(dir.path(), "0.35.0", stale_now + 60, || {
        panic!("fetch must not be called 60s after a failed check")
    });
    assert_eq!(
        lines_later,
        vec!["blyte: 0.36.0 is out (you have 0.35.0): cli: faster startup. Run `blyte update`."]
    );
}

#[test]
fn check_with_same_version_gives_no_lines() {
    let dir = tempfile::tempdir().unwrap();
    let lines = check(dir.path(), "0.36.0", 1_000_000, || {
        Ok((
            "v0.36.0".to_string(),
            "- **cli**: faster startup\n".to_string(),
        ))
    });
    assert!(lines.is_empty(), "expected no lines, got {lines:?}");
}

#[test]
fn check_reports_unexpected_tag() {
    let dir = tempfile::tempdir().unwrap();
    let lines = check(dir.path(), "0.36.0", 1_000_000, || {
        Ok(("nightly".to_string(), String::new()))
    });
    assert_eq!(
        lines,
        vec!["blyte: could not check for a newer release: unexpected tag nightly"]
    );
}
