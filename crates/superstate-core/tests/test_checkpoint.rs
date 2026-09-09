//! Git checkpoints guarding milestone edits.

use std::path::Path;
use std::process::Command;
use superstate_core::checkpoint::{checkpoint_ref, create_checkpoint, rollback_to};

fn git(dir: &Path, args: &[&str]) {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
}

fn repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "."]);
    git(p, &["config", "user.email", "e@l"]);
    git(p, &["config", "user.name", "E"]);
    std::fs::write(p.join("f.txt"), "original").unwrap();
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "base"]);
    dir
}

#[test]
fn checkpoint_tag_matches_the_reference_naming() {
    assert_eq!(checkpoint_ref(3), "agent-checkpoint/step-3");
}

#[test]
fn rollback_restores_tracked_files_and_removes_untracked_debris() {
    // A milestone that half-creates files and then fails leaves debris. A plain
    // `reset --hard` keeps it, and it would be carried into the next milestone
    // and into the graded diff.
    let dir = repo();
    let p = dir.path();
    create_checkpoint(p, 1).expect("checkpoint");

    std::fs::write(p.join("f.txt"), "modified").unwrap();
    std::fs::write(p.join("junk.txt"), "debris").unwrap();

    rollback_to(p, 1, None).expect("rollback");

    assert_eq!(
        std::fs::read_to_string(p.join("f.txt")).unwrap(),
        "original"
    );
    assert!(
        !p.join("junk.txt").exists(),
        "untracked debris must be removed"
    );
}

#[test]
fn rollback_refuses_an_unknown_reference_without_touching_the_tree() {
    // A typo in --ref must not destroy work by resetting to something unintended.
    let dir = repo();
    let p = dir.path();
    std::fs::write(p.join("wip.txt"), "uncommitted work").unwrap();

    let err = rollback_to(p, 1, Some("no-such-tag")).unwrap_err();

    assert!(err.to_string().contains("does not exist"), "{}", err);
    assert!(
        p.join("wip.txt").exists(),
        "work must survive a refused rollback"
    );
}

#[test]
fn recheckpointing_a_step_moves_the_tag_to_the_current_commit() {
    // A retried milestone should guard its own starting point, not an older one.
    let dir = repo();
    let p = dir.path();
    create_checkpoint(p, 1).expect("first");

    std::fs::write(p.join("f.txt"), "second commit").unwrap();
    git(p, &["add", "-A"]);
    git(p, &["commit", "-q", "-m", "second"]);
    create_checkpoint(p, 1).expect("re-checkpoint must not fail");

    std::fs::write(p.join("f.txt"), "dirty").unwrap();
    rollback_to(p, 1, None).expect("rollback");
    assert_eq!(
        std::fs::read_to_string(p.join("f.txt")).unwrap(),
        "second commit",
        "the tag should guard the newer commit"
    );
}
