//! The dashboard wears blyte's own palette, type and mark, with its fonts served locally
//! (2026-09-30).

use blyte_cli::dashboard::{route, Sources};

const PAGE: &str = include_str!("../assets/dashboard/index.html");

fn src() -> Sources {
    Sources {
        roots: vec![],
        runs_dirs: vec![],
        hours: 24,
    }
}

#[test]
fn the_favicon_and_fonts_are_served_locally() {
    let s = src();
    let (code, ctype, body) = route("/favicon.svg", &s);
    assert_eq!((code, ctype), (200, "image/svg+xml"));
    assert!(String::from_utf8_lossy(&body).contains("<svg"));
    for f in ["archivo", "plex-mono-400", "plex-mono-500"] {
        let (code, ctype, body) = route(&format!("/fonts/{f}.woff2"), &s);
        assert_eq!((code, ctype), (200, "font/woff2"), "{f}");
        assert_eq!(&body[..4], b"wOF2", "{f}");
    }
    assert_eq!(route("/fonts/other.woff2", &s).0, 404);
}

#[test]
fn the_page_uses_the_brand_and_only_local_fonts() {
    for needle in [
        "#A6441A",
        "#E27A44",
        "#ECEFF0",
        "#0F1315",
        "url(\"/fonts/archivo.woff2\")",
        "\"IBM Plex Mono\"",
        "href=\"/favicon.svg\"",
        "class=\"brand-mark\"",
        "class=\"brand-word\">blyte<",
    ] {
        assert!(PAGE.contains(needle), "missing {needle}");
    }
    for gone in ["Graphik", "#e52321", "#2bad72"] {
        assert!(!PAGE.contains(gone), "still has {gone}");
    }
}
