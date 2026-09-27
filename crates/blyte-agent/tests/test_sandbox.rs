//! On macOS blyte's shell refuses writes outside an allow-list: a model once spent 50 minutes
//! deleting a host cache after a gate failure, and a prompt is not enforcement.

use std::path::PathBuf;

use blyte_agent::sandbox::{default_allowed, profile};

#[test]
fn the_profile_denies_writes_then_allows_the_listed_paths() {
    let p = profile(&[PathBuf::from("/tmp/ws-\"q\"")]);
    assert!(
        p.starts_with("(version 1)(allow default)(deny file-write*)"),
        "{p}"
    );
    assert!(
        p.contains(r#"(allow file-write* (subpath "/tmp/ws-\"q\""))"#),
        "{p}"
    );
    assert!(p.contains(r#"(literal "/dev/null")"#), "{p}");
}

#[test]
fn the_default_allow_list_covers_the_workspace_and_its_git_directory() {
    let d = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(d.path())
        .status()
        .unwrap();
    let allowed = default_allowed(d.path());
    let canon = |p: &std::path::Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let root = canon(d.path());
    assert!(allowed.iter().any(|p| canon(p) == root), "{allowed:?}");
    assert!(
        allowed.iter().any(|p| canon(p) == root.join(".git")),
        "{allowed:?}"
    );
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::PathBuf;
    use std::time::Duration;

    use blyte_agent::sandbox::profile;
    use blyte_agent::shell::{LocalShell, Shell};

    #[test]
    fn a_sandboxed_command_can_write_inside_and_not_outside() {
        let inside = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // A profile with only `inside` allowed (no temp dirs), so `outside` is refused.
        let sh = LocalShell::sandboxed(profile(&[inside.path().to_path_buf()]));
        let ok = sh
            .exec("echo hi > ok.txt", inside.path(), Duration::from_secs(30))
            .unwrap();
        if ok.exit_code == 71 && ok.merged().contains("Operation not permitted") {
            // Nested sandboxing is not permitted in this environment (e.g. running under an enclosing sandbox).
            return;
        }
        assert_eq!(ok.exit_code, 0, "{}", ok.merged());
        let target: PathBuf = outside.path().join("no.txt");
        let denied = sh
            .exec(
                &format!("echo hi > {}", target.display()),
                inside.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_ne!(denied.exit_code, 0);
        assert!(
            !target.exists(),
            "the write outside the allow-list must not happen"
        );
    }
}
