use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::sandbox::agent_sockets;
use blyte_agent::shell::{withheld_env, LocalShell, Shell};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard {
    home: Option<OsString>,
    ssh_auth_sock: Option<OsString>,
    ssh_agent_pid: Option<OsString>,
}

impl EnvGuard {
    fn set(home: Option<&Path>, ssh_auth_sock: Option<&Path>, ssh_agent_pid: Option<&str>) -> Self {
        let prev = Self {
            home: std::env::var_os("HOME"),
            ssh_auth_sock: std::env::var_os("SSH_AUTH_SOCK"),
            ssh_agent_pid: std::env::var_os("SSH_AGENT_PID"),
        };
        match home {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        match ssh_auth_sock {
            Some(p) => std::env::set_var("SSH_AUTH_SOCK", p),
            None => std::env::remove_var("SSH_AUTH_SOCK"),
        }
        match ssh_agent_pid {
            Some(v) => std::env::set_var("SSH_AGENT_PID", v),
            None => std::env::remove_var("SSH_AGENT_PID"),
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
        match &self.ssh_auth_sock {
            Some(v) => std::env::set_var("SSH_AUTH_SOCK", v),
            None => std::env::remove_var("SSH_AUTH_SOCK"),
        }
        match &self.ssh_agent_pid {
            Some(v) => std::env::set_var("SSH_AGENT_PID", v),
            None => std::env::remove_var("SSH_AGENT_PID"),
        }
    }
}

#[test]
fn agent_sockets_returns_five_paths_in_order_and_four_without_sock() {
    let home = Path::new("/Users/someone");
    let sock = Path::new("/tmp/ssh-agent.sock");

    let with_sock = agent_sockets(home, Some(sock));
    assert_eq!(
        with_sock,
        vec![
            PathBuf::from("/tmp/ssh-agent.sock"),
            home.join(".gnupg/S.gpg-agent.ssh"),
            home.join(".gnupg/S.gpg-agent"),
            home.join(".1password/agent.sock"),
            home.join("Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"),
        ]
    );

    let without_sock = agent_sockets(home, None);
    assert_eq!(
        without_sock,
        vec![
            home.join(".gnupg/S.gpg-agent.ssh"),
            home.join(".gnupg/S.gpg-agent"),
            home.join(".1password/agent.sock"),
            home.join("Library/Group Containers/2BUA8C4S2C.com.1password/t/agent.sock"),
        ]
    );

    let dup_sock = home.join(".1password/agent.sock");
    let deduped = agent_sockets(home, Some(&dup_sock));
    assert_eq!(deduped.len(), 4);
    assert_eq!(deduped[0], dup_sock);
}

#[test]
fn ssh_agent_env_vars_are_scrubbed_and_listed_in_withheld_env() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let ws = tempfile::tempdir().unwrap();
    let _guard = EnvGuard::set(
        std::env::var_os("HOME").as_deref().map(Path::new),
        Some(Path::new("/tmp/test-agent.sock")),
        Some("4242"),
    );

    let withheld = withheld_env();
    assert!(
        withheld.contains(&"SSH_AUTH_SOCK".to_string()),
        "{withheld:?}"
    );
    assert!(
        withheld.contains(&"SSH_AGENT_PID".to_string()),
        "{withheld:?}"
    );

    let sh = LocalShell::new();
    let out = sh
        .exec(
            "printf 'sock=%s;pid=%s' \"${SSH_AUTH_SOCK-UNSET}\" \"${SSH_AGENT_PID-UNSET}\"",
            ws.path(),
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(out.exit_code, 0, "{}", out.merged());
    assert_eq!(out.stdout, "sock=UNSET;pid=UNSET");
}

#[cfg(target_os = "macos")]
mod macos {
    use std::os::unix::net::UnixListener;
    use std::path::Path;
    use std::time::Duration;

    use blyte_agent::sandbox::{conversation_profile_net, default_sandbox_net, Profile};
    use blyte_agent::shell::{LocalShell, Shell};

    use super::{EnvGuard, ENV_LOCK};

    fn assert_benign_read(sh: &LocalShell, benign: &Path, cwd: &Path) -> bool {
        let out = sh
            .exec(
                &format!("cat '{}'", benign.display()),
                cwd,
                Duration::from_secs(30),
            )
            .unwrap();
        if out.exit_code == 71 && out.merged().contains("Operation not permitted") {
            eprintln!("skipped: sandbox cannot nest here");
            return false;
        }
        assert_eq!(out.exit_code, 0, "benign read failed: {}", out.merged());
        assert!(out.merged().contains("benign-ok"), "{}", out.merged());
        true
    }

    fn connect_unix(sh: &LocalShell, sock: &Path, cwd: &Path) -> (i32, String) {
        let cmd = format!(
            "python3 -c 'import socket, sys; s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(sys.argv[1]); s.close()' '{}'",
            sock.display()
        );
        let out = sh.exec(&cmd, cwd, Duration::from_secs(30)).unwrap();
        (out.exit_code, out.merged())
    }

    fn assert_agent_socket_blocked(
        profile: Profile,
        tmpdir: Option<std::path::PathBuf>,
        ws: &Path,
        benign: &Path,
        agent: &Path,
        other: &Path,
    ) {
        let sh = LocalShell::sandboxed(profile).with_tmpdir(tmpdir);
        if !assert_benign_read(&sh, benign, ws) {
            return;
        }
        let (code, out) = connect_unix(&sh, other, ws);
        assert_eq!(code, 0, "unlisted unix socket connect failed: {out}");

        let (code, out) = connect_unix(&sh, agent, ws);
        assert_ne!(
            code, 0,
            "agent unix socket connect unexpectedly succeeded: {out}"
        );
    }

    #[test]
    fn seatbelts_deny_agent_sockets_after_network_allows_and_skip_home_when_unset() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir_in("/tmp").unwrap();
        let ws = tempfile::tempdir().unwrap();
        let sock_dir = tempfile::tempdir_in("/tmp").unwrap();
        let agent_sock = sock_dir.path().join("a.sock");
        let _listener = UnixListener::bind(&agent_sock).unwrap();
        let agent_canon = agent_sock.canonicalize().unwrap();

        {
            let _guard = EnvGuard::set(Some(home.path()), Some(&agent_sock), None);
            for profile in [
                default_sandbox_net(ws.path(), &[], true)
                    .expect("sandbox")
                    .profile,
                default_sandbox_net(ws.path(), &[], false)
                    .expect("sandbox")
                    .profile,
                conversation_profile_net(ws.path(), &[], true).unwrap(),
                conversation_profile_net(ws.path(), &[], false).unwrap(),
            ] {
                let Profile::Seatbelt(sbpl) = profile else {
                    panic!("expected Seatbelt profile");
                };
                let raw_rule = format!(
                    r#"(remote unix-socket (path-literal "{}"))"#,
                    agent_sock.display()
                );
                let canon_rule = format!(
                    r#"(remote unix-socket (path-literal "{}"))"#,
                    agent_canon.display()
                );
                let raw_pos = sbpl
                    .rfind(&raw_rule)
                    .unwrap_or_else(|| panic!("missing {raw_rule} in {sbpl}"));
                let canon_pos = sbpl
                    .rfind(&canon_rule)
                    .unwrap_or_else(|| panic!("missing {canon_rule} in {sbpl}"));
                if let Some(allow_pos) = sbpl.rfind("(allow network-outbound") {
                    assert!(raw_pos > allow_pos && canon_pos > allow_pos, "{sbpl}");
                }
                for fixed in blyte_agent::sandbox::agent_sockets(home.path(), None) {
                    let rule = format!(
                        r#"(remote unix-socket (path-literal "{}"))"#,
                        fixed.display()
                    );
                    let pos = sbpl
                        .rfind(&rule)
                        .unwrap_or_else(|| panic!("missing {rule} in {sbpl}"));
                    if let Some(allow_pos) = sbpl.rfind("(allow network-outbound") {
                        assert!(pos > allow_pos, "{sbpl}");
                    }
                }
            }
        }

        {
            let _guard = EnvGuard::set(None, None, None);
            let Profile::Seatbelt(sbpl) = default_sandbox_net(ws.path(), &[], true)
                .expect("sandbox")
                .profile
            else {
                panic!("expected Seatbelt profile");
            };
            assert!(
                !sbpl.contains("S.gpg-agent") && !sbpl.contains("agent.sock"),
                "unset HOME must not invent default agent socket paths: {sbpl}"
            );
        }
    }

    #[test]
    fn implementer_conversation_and_offline_profiles_block_agent_socket_only() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let sock_dir = tempfile::tempdir_in("/tmp").unwrap();

        let benign = ws.path().join("benign.txt");
        std::fs::write(&benign, "benign-ok\n").unwrap();

        let agent_sock = sock_dir.path().join("a.sock");
        let other_sock = sock_dir.path().join("b.sock");
        let _agent_listener = UnixListener::bind(&agent_sock).unwrap();
        let _other_listener = UnixListener::bind(&other_sock).unwrap();

        let _guard = EnvGuard::set(Some(home.path()), Some(&agent_sock), Some("9999"));

        let default_on = default_sandbox_net(ws.path(), &[], true).expect("sandbox");
        assert_agent_socket_blocked(
            default_on.profile,
            default_on.tmpdir,
            ws.path(),
            &benign,
            &agent_sock,
            &other_sock,
        );
        assert_agent_socket_blocked(
            conversation_profile_net(ws.path(), &[], true).unwrap(),
            None,
            ws.path(),
            &benign,
            &agent_sock,
            &other_sock,
        );
        let default_off = default_sandbox_net(ws.path(), &[], false).expect("sandbox");
        assert_agent_socket_blocked(
            default_off.profile,
            default_off.tmpdir,
            ws.path(),
            &benign,
            &agent_sock,
            &other_sock,
        );
        assert_agent_socket_blocked(
            conversation_profile_net(ws.path(), &[], false).unwrap(),
            None,
            ws.path(),
            &benign,
            &agent_sock,
            &other_sock,
        );
    }
}
