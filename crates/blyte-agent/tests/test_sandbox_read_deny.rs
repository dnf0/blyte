//! blyte's sandboxed shell refuses to read credential stores and `.env` files: the write sandbox
//! stopped host damage, but a command could still read ~/.aws or ~/.ssh and print it (2026-09-27).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use blyte_agent::fs::LocalFs;
use blyte_agent::sandbox::{
    profile, profile_with, read_denied, surface_token_dir, surface_token_path,
};
use blyte_agent::shell::LocalShell;
use blyte_agent::tools;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    home: Option<OsString>,
    xdg_state: Option<OsString>,
}

impl EnvGuard {
    fn set(home: Option<&Path>, xdg_state: Option<&Path>) -> Self {
        let prev = Self {
            home: std::env::var_os("HOME"),
            xdg_state: std::env::var_os("XDG_STATE_HOME"),
        };
        match home {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        match xdg_state {
            Some(p) => std::env::set_var("XDG_STATE_HOME", p),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        prev
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match &self.xdg_state {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
    }
}

#[test]
fn the_deny_list_covers_the_usual_credential_stores() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = Path::new("/Users/someone");
    let denied = read_denied(home);
    for sub in [
        ".ssh",
        ".aws",
        ".config/blyte/env",
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
fn surface_token_path_errors_without_state_dir_and_hashes_checkouts_deterministically() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    {
        let _guard = EnvGuard::set(None, None);
        assert!(surface_token_dir().is_none());
        let err = surface_token_path(Path::new("/tmp/repo")).unwrap_err();
        assert!(err.contains("HOME"), "{err}");
        assert!(err.contains("XDG_STATE_HOME"), "{err}");
    }

    let home = tempfile::tempdir().unwrap();
    let repo_a = tempfile::tempdir().unwrap();
    let repo_b = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None);

    let p1 = surface_token_path(repo_a.path()).unwrap();
    let p2 = surface_token_path(repo_a.path()).unwrap();
    let p3 = surface_token_path(repo_b.path()).unwrap();
    assert_eq!(p1, p2);
    assert_ne!(p1, p3);
    assert_eq!(p1.parent(), surface_token_dir().as_deref());
    let stem = p1.file_stem().unwrap().to_str().unwrap();
    assert_eq!(stem.len(), 16, "{p1:?}");
    assert!(stem.chars().all(|c| c.is_ascii_hexdigit()), "{p1:?}");
    assert_eq!(p1.extension().and_then(|e| e.to_str()), Some("token"));
}

#[test]
fn read_tool_refuses_surface_token_path() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = tempfile::tempdir().unwrap();
    let repo = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(Some(home.path()), None);

    let token_path = surface_token_path(repo.path()).unwrap();
    std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
    std::fs::write(&token_path, "surface-secret-token-value\n").unwrap();

    let shell = LocalShell::new();
    let fs = LocalFs;
    let outcome = tools::execute(
        repo.path(),
        &shell,
        &fs,
        "read",
        &serde_json::json!({ "path": token_path.to_str().unwrap() }),
    );
    assert!(outcome.is_error, "{}", outcome.content);
    assert!(
        !outcome.content.contains("surface-secret-token-value"),
        "{}",
        outcome.content
    );
}

#[test]
fn the_read_rules_follow_the_write_profile_and_grants_come_last() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    use blyte_agent::sandbox::{
        conversation_profile_net, default_sandbox_net, profile_with, surface_token_dir,
        surface_token_path,
    };
    use blyte_agent::shell::{LocalShell, Shell};

    use super::{EnvGuard, ENV_LOCK};

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
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

    fn assert_benign_read(sh: &LocalShell, benign: &Path, cwd: &Path) -> bool {
        let Some((code, out)) = cat(sh, benign, cwd) else {
            eprintln!("skipped: sandbox cannot nest here");
            return false;
        };
        assert_eq!(code, 0, "benign read failed: {out}");
        assert!(out.contains("benign-ok"), "benign read output: {out}");
        true
    }

    #[test]
    fn conversation_profile_offline_cannot_read_surface_token_or_blyte_env() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(Some(home.path()), None);

        let benign = ws.path().join("benign.txt");
        std::fs::write(&benign, "benign-ok\n").unwrap();

        let token_path = surface_token_path(ws.path()).unwrap();
        std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
        std::fs::write(&token_path, "surface-token-conv-secret\n").unwrap();

        let env_path = home.path().join(".config/blyte/env");
        std::fs::create_dir_all(env_path.parent().unwrap()).unwrap();
        std::fs::write(&env_path, "ANTHROPIC_API_KEY=blyte-env-conv-secret\n").unwrap();

        let sh = LocalShell::sandboxed(conversation_profile_net(ws.path(), &[], false).unwrap());
        if !assert_benign_read(&sh, &benign, ws.path()) {
            return;
        }

        let (code, out) = cat(&sh, &token_path, ws.path()).unwrap();
        assert_ne!(code, 0, "{out}");
        assert!(!out.contains("surface-token-conv-secret"), "{out}");

        let (code, out) = cat(&sh, &env_path, ws.path()).unwrap();
        assert_ne!(code, 0, "{out}");
        assert!(!out.contains("blyte-env-conv-secret"), "{out}");
    }

    #[test]
    fn default_profile_online_cannot_read_surface_token_or_blyte_env() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(Some(home.path()), None);

        let benign = ws.path().join("benign.txt");
        std::fs::write(&benign, "benign-ok\n").unwrap();

        let token_path = surface_token_path(ws.path()).unwrap();
        std::fs::create_dir_all(token_path.parent().unwrap()).unwrap();
        std::fs::write(&token_path, "surface-token-default-secret\n").unwrap();

        let env_path = home.path().join(".config/blyte/env");
        std::fs::create_dir_all(env_path.parent().unwrap()).unwrap();
        std::fs::write(&env_path, "ANTHROPIC_API_KEY=blyte-env-default-secret\n").unwrap();

        let sb = default_sandbox_net(ws.path(), &[], true).expect("sandbox");
        let sh = LocalShell::sandboxed(sb.profile).with_tmpdir(sb.tmpdir);
        if !assert_benign_read(&sh, &benign, ws.path()) {
            return;
        }

        let (code, out) = cat(&sh, &token_path, ws.path()).unwrap();
        assert_ne!(code, 0, "{out}");
        assert!(!out.contains("surface-token-default-secret"), "{out}");

        let (code, out) = cat(&sh, &env_path, ws.path()).unwrap();
        assert_ne!(code, 0, "{out}");
        assert!(!out.contains("blyte-env-default-secret"), "{out}");
    }

    #[test]
    fn allow_read_grant_cannot_expose_surface_token_dir() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(Some(home.path()), None);

        let benign = ws.path().join("benign.txt");
        std::fs::write(&benign, "benign-ok\n").unwrap();

        let token_path = surface_token_path(ws.path()).unwrap();
        let token_dir = surface_token_dir().unwrap();
        std::fs::create_dir_all(&token_dir).unwrap();
        std::fs::write(&token_path, "surface-token-grant-secret\n").unwrap();

        let default_sb = default_sandbox_net(ws.path(), std::slice::from_ref(&token_dir), true)
            .expect("sandbox");
        for (profile, tmpdir) in [
            (
                conversation_profile_net(ws.path(), std::slice::from_ref(&token_dir), false)
                    .unwrap(),
                None,
            ),
            (default_sb.profile, default_sb.tmpdir),
        ] {
            let sh = LocalShell::sandboxed(profile).with_tmpdir(tmpdir);
            if !assert_benign_read(&sh, &benign, ws.path()) {
                return;
            }
            let (code, out) = cat(&sh, &token_path, ws.path()).unwrap();
            assert_ne!(code, 0, "{out}");
            assert!(!out.contains("surface-token-grant-secret"), "{out}");
        }
    }

    #[test]
    fn allow_read_grant_cannot_expose_blyte_env() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set(Some(home.path()), None);

        let benign = ws.path().join("benign.txt");
        std::fs::write(&benign, "benign-ok\n").unwrap();

        let config_dir = home.path().join(".config/blyte");
        let env_path = config_dir.join("env");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(&env_path, "ANTHROPIC_API_KEY=blyte-env-grant-secret\n").unwrap();

        for grant in [&config_dir, &env_path] {
            let default_sb =
                default_sandbox_net(ws.path(), std::slice::from_ref(grant), true).expect("sandbox");
            for (profile, tmpdir) in [
                (
                    conversation_profile_net(ws.path(), std::slice::from_ref(grant), false)
                        .unwrap(),
                    None,
                ),
                (default_sb.profile, default_sb.tmpdir),
            ] {
                let sh = LocalShell::sandboxed(profile).with_tmpdir(tmpdir);
                if !assert_benign_read(&sh, &benign, ws.path()) {
                    return;
                }
                let (code, out) = cat(&sh, &env_path, ws.path()).unwrap();
                assert_ne!(code, 0, "grant {} leaked: {out}", grant.display());
                assert!(!out.contains("blyte-env-grant-secret"), "{out}");
            }
        }
    }
}
