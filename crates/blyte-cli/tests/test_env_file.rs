//! blyte reads provider keys from ~/.config/blyte/env when they are not exported: a Claude Code
//! session started without GEMINI_API_KEY failed every dispatch at turn 0 (2026-09-29).

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use blyte_cli::env_file::{get, load, parse, set};

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

#[test]
fn set_creates_file_with_mode_0600_replaces_existing_and_appends_without_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/blyte/env");

    // Creates parent directories and file with mode 0600.
    set(&path, &[("BLYTE_MODEL", "gemini-4-argon")]).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "BLYTE_MODEL=gemini-4-argon\n"
    );
    assert_eq!(
        get(&path, "BLYTE_MODEL").unwrap(),
        Some("gemini-4-argon".to_string())
    );

    // Replaces existing BLYTE_MODEL line and keeps other lines and comments.
    std::fs::write(
        &path,
        "# comment\nGEMINI_API_KEY=secret\nBLYTE_MODEL=old-model\nOTHER=1\n",
    )
    .unwrap();
    set(
        &path,
        &[
            ("BLYTE_MODEL", "gemini-4-argon"),
            ("BLYTE_MODEL_TIMEOUT_SECS", "600"),
        ],
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# comment\nGEMINI_API_KEY=secret\nBLYTE_MODEL=gemini-4-argon\nOTHER=1\nBLYTE_MODEL_TIMEOUT_SECS=600\n"
    );

    // Appends to a file with no trailing newline.
    let no_nl = dir.path().join("no_nl_env");
    std::fs::write(&no_nl, "GEMINI_API_KEY=secret").unwrap();
    set(&no_nl, &[("BLYTE_MODEL", "gemini-3.8-flash-high")]).unwrap();
    assert_eq!(
        std::fs::read_to_string(&no_nl).unwrap(),
        "GEMINI_API_KEY=secret\nBLYTE_MODEL=gemini-3.8-flash-high\n"
    );
    let no_nl_mode = std::fs::metadata(&no_nl).unwrap().permissions().mode() & 0o777;
    assert_eq!(no_nl_mode, 0o600);
}
