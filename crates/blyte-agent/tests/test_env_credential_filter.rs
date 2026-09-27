//! Commands blyte runs do not see credential-looking variables: the old scrub removed only blyte's
//! own provider keys, so `env` showed the model any token the user had exported (2026-09-27).

use std::time::Duration;

use blyte_agent::shell::{credential_like, set_pass_env, withheld_env, LocalShell, Shell};

#[test]
fn names_with_a_credential_segment_are_credential_like() {
    for name in [
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_ACCESS_KEY_ID",
        "AWS_SESSION_TOKEN",
        "SSH_AUTH_SOCK",
        "NPM_TOKEN",
        "DB_PASSWORD",
        "hf_token",
    ] {
        assert!(credential_like(name, "x"), "{name}");
    }
    for name in [
        "PATH",
        "HOME",
        "TOKENIZERS_PARALLELISM",
        "SECURITYSESSIONID",
        "CARGO_HOME",
        "KEYBOARD_LAYOUT",
        "AWS_REGION",
    ] {
        assert!(!credential_like(name, "x"), "{name}");
    }
}

#[test]
fn a_url_with_an_inline_password_is_credential_like() {
    assert!(credential_like(
        "DATABASE_URL",
        "postgres://u:p@host:5432/db"
    ));
    assert!(credential_like(
        "X",
        "see https://user:secret@example.com/path"
    ));
    assert!(!credential_like("DATABASE_URL", "postgres://host:5432/db"));
    assert!(!credential_like("X", "https://example.com/a:b@c"));
    assert!(!credential_like("X", "https://user@example.com/"));
}

// One test touches the process environment and the process-wide pass list, so it is the only
// test in this binary that does, and it runs as a single #[test].
#[test]
fn withheld_variables_are_removed_from_commands_and_passed_ones_are_kept() {
    std::env::set_var("ZZ_BLYTE_TEST_TOKEN", "t0ken-value");
    std::env::set_var("ZZ_BLYTE_TEST_PASS_TOKEN", "kept-value");
    std::env::set_var("ZZ_BLYTE_TEST_PLAIN", "plain-value");
    set_pass_env(vec!["ZZ_BLYTE_TEST_PASS_TOKEN".into()]);

    let withheld = withheld_env();
    assert!(
        withheld.contains(&"ZZ_BLYTE_TEST_TOKEN".to_string()),
        "{withheld:?}"
    );
    assert!(
        !withheld.contains(&"ZZ_BLYTE_TEST_PASS_TOKEN".to_string()),
        "{withheld:?}"
    );
    assert!(
        !withheld.iter().any(|n| n == "GEMINI_API_KEY"),
        "HARNESS_ENV names are not listed"
    );

    let dir = tempfile::tempdir().unwrap();
    let out = LocalShell
        .exec("env", dir.path(), Duration::from_secs(30))
        .unwrap();
    let env = out.merged();
    assert!(!env.contains("t0ken-value"), "withheld variable leaked");
    assert!(env.contains("ZZ_BLYTE_TEST_PASS_TOKEN=kept-value"));
    assert!(env.contains("ZZ_BLYTE_TEST_PLAIN=plain-value"));
}
