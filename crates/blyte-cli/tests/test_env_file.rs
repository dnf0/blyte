//! blyte reads provider keys from ~/.config/blyte/env when they are not exported: a Claude Code
//! session started without GEMINI_API_KEY failed every dispatch at turn 0 (2026-09-29).

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use blyte_cli::env_file::{load, parse};

#[test]
fn parse_reads_names_and_values() {
    let text =
        "# keys\nexport GEMINI_API_KEY=abc\nB=\"q v\"\nC='s'\n\nnot a line\n1-BAD=x\nD = e \n";
    assert_eq!(
        parse(text),
        vec![
            ("GEMINI_API_KEY".to_string(), "abc".to_string()),
            ("B".to_string(), "q v".to_string()),
            ("C".to_string(), "s".to_string()),
            ("D".to_string(), "e".to_string()),
        ]
    );
}

// The one test in this binary that touches the process environment.
#[test]
fn load_sets_only_unset_variables_and_refuses_a_file_others_can_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("env");
    std::fs::write(
        &path,
        "ZZ_BLYTE_ENVFILE_NEW=from-file\nZZ_BLYTE_ENVFILE_SET=from-file\n",
    )
    .unwrap();
    std::env::set_var("ZZ_BLYTE_ENVFILE_SET", "from-env");

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load(&path).unwrap_err().contains("chmod 600"));
    assert!(std::env::var("ZZ_BLYTE_ENVFILE_NEW").is_err());

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        load(&path).unwrap(),
        vec!["ZZ_BLYTE_ENVFILE_NEW".to_string()]
    );
    assert_eq!(std::env::var("ZZ_BLYTE_ENVFILE_NEW").unwrap(), "from-file");
    assert_eq!(std::env::var("ZZ_BLYTE_ENVFILE_SET").unwrap(), "from-env");

    assert_eq!(
        load(&dir.path().join("missing")).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn the_binary_loads_the_file_named_by_blyte_env_file() {
    // Network-free: a file other users can read is refused with a warning, even for --version,
    // so the binary did look at it; an empty BLYTE_ENV_FILE turns the file off.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("env");
    std::fs::write(&path, "ZZ_UNUSED=1\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let stderr = |file: &std::ffi::OsStr| {
        let out = Command::new(env!("CARGO_BIN_EXE_blyte"))
            .arg("--version")
            .env("BLYTE_ENV_FILE", file)
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    assert!(stderr(path.as_os_str()).contains("chmod 600"));
    assert!(!stderr(std::ffi::OsStr::new("")).contains("chmod 600"));
}
