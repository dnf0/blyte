//! blyte's sandboxed shell refuses to read credential stores and `.env` files: the write sandbox
//! stopped host damage, but a command could still read ~/.aws or ~/.ssh and print it (2026-09-27).

use std::path::{Path, PathBuf};

use blyte_agent::sandbox::{profile, profile_with, read_denied};

#[test]
fn the_deny_list_covers_the_usual_credential_stores() {
    let home = Path::new("/Users/someone");
    let denied = read_denied(home);
    for sub in [
        ".ssh",
        ".aws",
        ".config/gh",
        ".codex",
        "Library/Keychains",
        ".docker/config.json",
    ] {
        assert!(
            denied.contains(&home.join(sub)),
            "{sub} missing from {denied:?}"
        );
    }
}

#[test]
fn the_read_rules_follow_the_write_profile_and_grants_come_last() {
    let ws = Path::new("/nonexistent/ws");
    let write = [ws.to_path_buf()];
    let p = profile_with(
        ws,
        &write,
        &[PathBuf::from("/nonexistent/secret")],
        &[PathBuf::from("/nonexistent/secret/ok")],
    );
    assert!(p.starts_with(&profile(&write)), "{p}");
    let deny = p
        .find(r#"(deny file-read-data (subpath "/nonexistent/secret"))"#)
        .expect(&p);
    let env = p
        .find(r##"(deny file-read-data (regex #"/\.env$"))"##)
        .expect(&p);
    let tmp_env = p
        .find(r##"(allow file-read-data (require-all (subpath "/private/tmp") (require-not (subpath "/nonexistent/ws")) (regex #"/\.env$")))"##)
        .expect(&p);
    let var_folders_env = p
        .find(r##"(allow file-read-data (require-all (subpath "/private/var/folders") (require-not (subpath "/nonexistent/ws")) (regex #"/\.env$")))"##)
        .expect(&p);
    let grant = p
        .find(r#"(allow file-read-data (subpath "/nonexistent/secret/ok"))"#)
        .expect(&p);
    assert!(
        deny < env && env < tmp_env && tmp_env < var_folders_env && var_folders_env < grant,
        "{p}"
    );
}

#[test]
fn dot_env_outside_temp_folders_and_workspace_stays_denied_in_profile() {
    let ws = Path::new("/nonexistent/ws");
    let p = profile_with(ws, &[ws.to_path_buf()], &[], &[]);
    assert!(
        p.contains(r##"(deny file-read-data (regex #"/\.env$"))"##),
        "{p}"
    );
    // With no allow_read grants, the only file-read-data allows are for .env under
    // /private/tmp and /private/var/folders (excluding the workspace), so a .env anywhere
    // else (e.g. /Users/someone/other/.env) stays denied by the regex rule above.
    let read_allows: Vec<&str> = p
        .match_indices("(allow file-read-data")
        .map(|(i, _)| &p[i..])
        .collect();
    assert_eq!(read_allows.len(), 2, "{p}");
    assert!(
        read_allows[0].starts_with(
            r##"(allow file-read-data (require-all (subpath "/private/tmp") (require-not (subpath "/nonexistent/ws")) (regex #"/\.env$")))"##
        ),
        "{p}"
    );
    assert!(
        read_allows[1].starts_with(
            r##"(allow file-read-data (require-all (subpath "/private/var/folders") (require-not (subpath "/nonexistent/ws")) (regex #"/\.env$")))"##
        ),
        "{p}"
    );
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::Path;
    use std::time::Duration;

    use blyte_agent::sandbox::profile_with;
    use blyte_agent::shell::{LocalShell, Shell};

    fn cat(sh: &LocalShell, file: &Path, cwd: &Path) -> Option<(i32, String)> {
        let out = sh
            .exec(
                &format!("cat '{}'", file.display()),
                cwd,
                Duration::from_secs(30),
            )
            .unwrap();
        if out.exit_code == 71 && out.merged().contains("Operation not permitted") {
            // Nested sandboxing is not permitted here (e.g. already inside a sandbox).
            return None;
        }
        Some((out.exit_code, out.merged()))
    }

    #[test]
    fn a_denied_file_and_a_dot_env_cannot_be_read_but_other_files_can() {
        let ws = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir().unwrap();
        std::fs::write(secret.path().join("key"), "s3cret").unwrap();
        std::fs::write(ws.path().join(".env"), "TOKEN=x").unwrap();
        std::fs::write(ws.path().join(".env.example"), "TOKEN=").unwrap();
        let sh = LocalShell::sandboxed(profile_with(
            ws.path(),
            &[ws.path().to_path_buf()],
            &[secret.path().to_path_buf()],
            &[],
        ));
        let Some((code, out)) = cat(&sh, &secret.path().join("key"), ws.path()) else {
            return;
        };
        assert_ne!(code, 0, "{out}");
        assert!(!out.contains("s3cret"), "{out}");
        let (code, out) = cat(&sh, &ws.path().join(".env"), ws.path()).unwrap();
        assert!(code != 0 && !out.contains("TOKEN=x"), "{out}");
        let (code, out) = cat(&sh, &ws.path().join(".env.example"), ws.path()).unwrap();
        assert_eq!(code, 0, "{out}");
        // Only contents are refused: pytest stats every path while collecting, `.env` included.
        let stat = sh
            .exec("test -e .env", ws.path(), Duration::from_secs(30))
            .unwrap();
        assert_eq!(stat.exit_code, 0, "{}", stat.merged());
    }

    #[test]
    fn a_dot_env_in_a_separate_temp_folder_can_be_read_while_workspace_dot_env_cannot() {
        let ws = tempfile::tempdir().unwrap();
        let other_tmp = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join(".env"), "WS_SECRET=1").unwrap();
        std::fs::write(other_tmp.path().join(".env"), "TEST_FIXTURE=2").unwrap();
        let sh = LocalShell::sandboxed(profile_with(
            ws.path(),
            &[ws.path().to_path_buf()],
            &[],
            &[],
        ));
        let Some((code, out)) = cat(&sh, &other_tmp.path().join(".env"), ws.path()) else {
            return;
        };
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("TEST_FIXTURE=2"), "{out}");
        let (code, out) = cat(&sh, &ws.path().join(".env"), ws.path()).unwrap();
        assert!(code != 0 && !out.contains("WS_SECRET=1"), "{out}");
    }

    #[test]
    fn an_allow_read_grant_overrides_the_deny_list() {
        let ws = tempfile::tempdir().unwrap();
        let secret = tempfile::tempdir().unwrap();
        std::fs::write(secret.path().join("key"), "s3cret").unwrap();
        let sh = LocalShell::sandboxed(profile_with(
            ws.path(),
            &[ws.path().to_path_buf()],
            &[secret.path().to_path_buf()],
            &[secret.path().to_path_buf()],
        ));
        let Some((code, out)) = cat(&sh, &secret.path().join("key"), ws.path()) else {
            return;
        };
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("s3cret"), "{out}");
    }
}
