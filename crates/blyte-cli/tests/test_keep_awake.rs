use blyte_cli::keep_awake;

#[test]
fn command_matches_platform() {
    let cmd = keep_awake::command(123);

    #[cfg(target_os = "macos")]
    {
        let cmd = cmd.expect("expected Some(Command) on macOS");
        assert_eq!(cmd.get_program(), "/usr/bin/caffeinate");
        let args: Vec<&str> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args, vec!["-i", "-w", "123"]);
    }

    #[cfg(not(target_os = "macos"))]
    {
        assert!(cmd.is_none(), "expected None on non-macOS systems");
    }
}

#[test]
fn hold_no_keep_awake_env_does_nothing() {
    std::env::set_var("BLYTE_NO_KEEP_AWAKE", "1");
    let res = keep_awake::hold(999_999);
    std::env::remove_var("BLYTE_NO_KEEP_AWAKE");
    assert_eq!(res, Ok(()));
}
