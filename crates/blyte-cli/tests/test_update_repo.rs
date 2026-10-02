//! Repository URL parsing tests for `blyte update`.

use blyte_cli::update::{parse_repo, repo};

#[test]
fn parser_maps_github_urls_and_rejects_non_github() {
    assert_eq!(
        parse_repo("https://github.com/acme/blyte"),
        Some("acme/blyte")
    );
    assert_eq!(
        parse_repo("https://github.com/acme/blyte/"),
        Some("acme/blyte")
    );
    assert_eq!(
        parse_repo("https://github.com/acme/blyte.git"),
        Some("acme/blyte")
    );
    assert_eq!(parse_repo("https://gitlab.com/acme/blyte"), None);
    assert_eq!(repo(), parse_repo(env!("CARGO_PKG_REPOSITORY")).unwrap());
}
