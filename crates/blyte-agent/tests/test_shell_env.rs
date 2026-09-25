//! Commands blyte runs never see its own configuration or provider credentials. The model could
//! read GEMINI_API_KEY with `env`, and BLYTE_MODEL broke the repository's own tests.
#![cfg(unix)]

use std::time::Duration;

use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

const T: Duration = Duration::from_secs(30);

#[test]
fn credentials_and_blyte_configuration_do_not_reach_commands() {
    // One test owns these variables for this binary, so there is no race with other tests.
    std::env::set_var("GEMINI_API_KEY", "secret-not-real");
    std::env::set_var("ANTHROPIC_API_KEY", "secret-not-real");
    std::env::set_var("BLYTE_MODEL", "gemini-3.8-flash-high");
    std::env::set_var("BLYTE_ANYTHING_ELSE", "x");
    std::env::set_var("UNRELATED_USER_VAR", "kept");
    let probe = "echo ${GEMINI_API_KEY:-unset} ${ANTHROPIC_API_KEY:-unset} \
                 ${BLYTE_MODEL:-unset} ${BLYTE_ANYTHING_ELSE:-unset} ${UNRELATED_USER_VAR:-unset}";
    let expected = "unset unset unset unset kept\n";

    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    assert_eq!(
        LocalShell.exec(probe, &root, T).unwrap().stdout,
        expected,
        "LocalShell"
    );
    let p = PersistentShell::local(root.clone());
    assert_eq!(
        p.exec(probe, &root, T).unwrap().stdout,
        expected,
        "harness session"
    );
    assert_eq!(
        p.exec_user(probe, &root, T).unwrap().stdout,
        expected,
        "model's session"
    );
}
