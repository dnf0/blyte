//! Finding the project's state from anywhere inside it.

use std::fs;
use superstate_core::state_machine::{enclosing_state_root, resolve_state_path, STATE_DIR};

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join(STATE_DIR);
    fs::create_dir_all(&state_dir).unwrap();
    fs::write(state_dir.join("state.json"), "{}").unwrap();
    fs::create_dir_all(dir.path().join("src/deep")).unwrap();
    dir
}

#[test]
fn a_relative_path_resolves_to_the_projects_state_from_a_subdirectory() {
    // An agent that changes directory must still act on the project's state,
    // not lose it and not start a second one.
    let dir = project();
    let from_deep = dir.path().join("src/deep");
    let resolved = resolve_state_path(std::path::Path::new(".agent-state/state.json"), &from_deep);
    assert_eq!(
        fs::canonicalize(&resolved).unwrap(),
        fs::canonicalize(dir.path().join(STATE_DIR).join("state.json")).unwrap(),
    );
}

#[test]
fn an_absolute_path_is_never_rewritten_by_discovery() {
    let dir = project();
    let explicit = dir.path().join("elsewhere.json");
    let resolved = resolve_state_path(&explicit, dir.path());
    assert_eq!(
        resolved, explicit,
        "an explicit path must be honoured exactly"
    );
}

#[test]
fn an_unfound_relative_path_is_returned_unchanged_so_init_can_create_it() {
    let dir = tempfile::tempdir().unwrap();
    let wanted = std::path::Path::new(".agent-state/state.json");
    assert_eq!(resolve_state_path(wanted, dir.path()), wanted);
}

#[test]
fn an_enclosing_project_is_detected_from_below() {
    let dir = project();
    let found = enclosing_state_root(&dir.path().join("src/deep")).expect("should find the root");
    assert_eq!(
        fs::canonicalize(found).unwrap(),
        fs::canonicalize(dir.path()).unwrap()
    );
}

#[test]
fn no_enclosing_project_is_reported_when_there_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(enclosing_state_root(dir.path()).is_none());
}
