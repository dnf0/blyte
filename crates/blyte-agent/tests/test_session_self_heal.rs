//! A command that deletes the session's temp directory must not break every later command: in a
//! shadow run the model did exactly that and spent 250 turns unable to run even `pwd`.

use std::time::Duration;

use blyte_agent::shell::{PersistentShell, Shell};

#[test]
fn the_session_survives_its_temp_directory_being_deleted() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let s = PersistentShell::local(root.clone());
    let t = Duration::from_secs(30);

    let before = s.exec_user("echo before", &root, t).unwrap();
    assert_eq!(before.exit_code, 0);

    let rm = s.exec_user(r#"rm -rf "$__blyte_t""#, &root, t).unwrap();
    assert_eq!(rm.exit_code, 0);

    let after = s.exec_user("echo after", &root, t).unwrap();
    assert_eq!(after.exit_code, 0, "{}", after.merged());
    assert!(after.merged().contains("after"));
}

#[test]
fn the_session_survives_the_variable_being_unset_too() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let s = PersistentShell::local(root.clone());
    let t = Duration::from_secs(30);
    s.exec_user(r#"rm -rf "$__blyte_t"; unset __blyte_t"#, &root, t)
        .unwrap();
    let after = s.exec_user("echo still-here", &root, t).unwrap();
    assert_eq!(after.exit_code, 0, "{}", after.merged());
    assert!(after.merged().contains("still-here"));
}
