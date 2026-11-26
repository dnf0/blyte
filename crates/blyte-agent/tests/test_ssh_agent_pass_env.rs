use std::ffi::OsString;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use blyte_agent::shell::{set_pass_env, withheld_env, LocalShell, Shell};

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
fn pass_env_ssh_auth_sock_keeps_var_and_lifts_deny_for_that_path_only() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_pass_env(vec!["SSH_AUTH_SOCK".to_string()]);

    let home = tempfile::tempdir_in("/tmp").unwrap();
    let ws = tempfile::tempdir().unwrap();
    let sock_dir = tempfile::tempdir_in("/tmp").unwrap();
    let agent_sock = sock_dir.path().join("pass.sock");

    let _guard = EnvGuard::set(Some(home.path()), Some(&agent_sock), Some("4242"));

    let withheld = withheld_env();
    assert!(
        !withheld.contains(&"SSH_AUTH_SOCK".to_string()),
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
    assert_eq!(
        out.stdout,
        format!("sock={};pid=UNSET", agent_sock.display())
    );

    #[cfg(target_os = "macos")]
    {
        use std::os::unix::net::UnixListener;

        use blyte_agent::sandbox::{
            agent_sockets, conversation_profile_net, default_profile_net, Profile,
        };

        let _agent_listener = UnixListener::bind(&agent_sock).unwrap();
        let fixed_dir = home.path().join(".1password");
        std::fs::create_dir_all(&fixed_dir).unwrap();
        let fixed_sock = fixed_dir.join("agent.sock");
        let _fixed_listener = UnixListener::bind(&fixed_sock).unwrap();

        let sock_str = agent_sock.to_string_lossy().into_owned();
        let sock_canon_str = agent_sock
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        let profiles = [
            default_profile_net(ws.path(), &[], true),
            default_profile_net(ws.path(), &[], false),
            conversation_profile_net(ws.path(), &[], true).unwrap(),
            conversation_profile_net(ws.path(), &[], false).unwrap(),
        ];

        for profile in &profiles {
            let Profile::Seatbelt(sbpl) = profile else {
                panic!("expected Seatbelt profile");
            };
            assert!(
                !sbpl.contains(&sock_str) && !sbpl.contains(&sock_canon_str),
                "passed-through SSH_AUTH_SOCK must not be denied: {sbpl}"
            );
            for fixed in agent_sockets(home.path(), None) {
                let fixed_str = fixed.to_string_lossy();
                assert!(
                    sbpl.contains(fixed_str.as_ref()),
                    "fixed agent socket {} must still be denied: {sbpl}",
                    fixed.display()
                );
            }
        }

        let benign = ws.path().join("benign.txt");
        std::fs::write(&benign, "benign-ok\n").unwrap();

        let sh = LocalShell::sandboxed(default_profile_net(ws.path(), &[], true));
        let probe = sh
            .exec(
                &format!("cat '{}'", benign.display()),
                ws.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        if probe.exit_code == 71 && probe.merged().contains("Operation not permitted") {
            eprintln!("skipped: sandbox cannot nest here");
            return;
        }
        assert_eq!(probe.exit_code, 0, "benign read failed: {}", probe.merged());

        let connect = |sock: &Path| {
            let cmd = format!(
                "python3 -c 'import socket, sys; s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(sys.argv[1]); s.close()' '{}'",
                sock.display()
            );
            sh.exec(&cmd, ws.path(), Duration::from_secs(30)).unwrap()
        };

        let ok = connect(&agent_sock);
        assert_eq!(
            ok.exit_code,
            0,
            "passed-through SSH_AUTH_SOCK connect failed: {}",
            ok.merged()
        );

        let blocked = connect(&fixed_sock);
        assert_ne!(
            blocked.exit_code,
            0,
            "fixed agent socket unexpectedly allowed: {}",
            blocked.merged()
        );
    }
}
