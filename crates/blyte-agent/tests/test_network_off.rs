//! `--network=off` stops the commands blyte runs from connecting out: read-deny kept secrets in, but
//! a command could still send workspace code anywhere (2026-09-27).

use std::path::{Path, PathBuf};

use blyte_agent::sandbox::{
    default_sandbox, default_sandbox_net, docker_sockets, offline, Profile,
};

#[test]
fn network_on_is_exactly_todays_profile() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(
        default_sandbox_net(d.path(), &[], true)
            .expect("sandbox")
            .profile,
        default_sandbox(d.path(), &[]).expect("sandbox").profile
    );
}

#[test]
fn the_offline_rules_allow_localhost_and_sockets_then_deny_docker_last() {
    let p = offline(
        "(version 1)".into(),
        &[PathBuf::from("/nonexistent/docker.sock")],
    );
    assert!(p.starts_with("(version 1)"), "{p}");
    let deny = p.find("(deny network-outbound)").expect(&p);
    let local = p
        .find(r#"(allow network-outbound (remote ip "localhost:*"))"#)
        .expect(&p);
    let unix = p
        .find("(allow network-outbound (remote unix-socket))")
        .expect(&p);
    let docker = p
        .find(r#"(deny network-outbound (remote unix-socket (path-literal "/nonexistent/docker.sock"))"#)
        .expect(&p);
    assert!(deny < local && local < unix && unix < docker, "{p}");
}

#[test]
fn the_docker_socket_list_covers_the_usual_runtimes() {
    let s = docker_sockets(Path::new("/Users/x"));
    assert_eq!(s[0], PathBuf::from("/var/run/docker.sock"));
    assert!(
        s.contains(&PathBuf::from("/Users/x/.docker/run/docker.sock")),
        "{s:?}"
    );
    assert!(
        s.contains(&PathBuf::from("/Users/x/.orbstack/run/docker.sock")),
        "{s:?}"
    );
}

#[test]
#[allow(clippy::assertions_on_constants)]
fn the_offline_default_profile_matches_the_platform() {
    let d = tempfile::tempdir().unwrap();
    match default_sandbox_net(d.path(), &[], false)
        .expect("sandbox")
        .profile
    {
        Profile::Seatbelt(s) => {
            assert!(cfg!(target_os = "macos"));
            assert!(s.contains("(deny network-outbound)"), "{s}");
        }
        Profile::LandlockConfined {
            write,
            block_tcp: true,
            ..
        } => {
            assert!(!cfg!(target_os = "macos"));
            assert!(write.contains(&d.path().to_path_buf()), "{write:?}");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    use blyte_agent::sandbox::{offline, profile};
    use blyte_agent::shell::{LocalShell, Shell};

    #[test]
    fn offline_commands_reach_localhost_but_not_the_outside() {
        let ws = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nok");
            }
        });
        let sh = LocalShell::sandboxed(offline(profile(&[ws.path().to_path_buf()]), &[]));
        let local = sh
            .exec(
                &format!("curl -4 -s -m 2 http://localhost:{port}/"),
                ws.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        if local.exit_code == 71 && local.merged().contains("Operation not permitted") {
            return;
        }
        assert_eq!(local.exit_code, 0, "{}", local.merged());
        assert!(local.merged().contains("ok"), "{}", local.merged());
        let out = sh
            .exec(
                "curl -s -m 2 http://192.0.2.1/ ; echo rc=$?",
                ws.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert!(!out.merged().contains("rc=0"), "{}", out.merged());
        assert!(
            out.merged().contains("rc=7"),
            "connect should be refused (curl 7): {}",
            out.merged()
        );
    }
}
