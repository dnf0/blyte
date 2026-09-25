# Lean Session Protocol and Native Glob Implementation Plan

> **For agentic workers:** Execution is delegated to headless `agy`, one task per dispatch; Claude reviews each task before the next is sent. Steps use checkbox (`- [ ]`) syntax for tracking. Do NOT `git push`, open PRs, publish crates, or call any model API; commit locally only.

**Goal:** Take the remaining harness overhead out of commands and globs.

**Where it stands** (`harness_bench`, 200 turns, after #63): local 2.2s, container 5.2s. Local `bash` still costs about 25 ms and `glob` 26 ms; container `glob` costs 84 ms.
- **Commands:** the session protocol spawns about six helper processes per command (`base64` to decode, `base64` and `tr` twice to encode, `rm`). That is cheap on Linux but about 4 ms each on macOS.
- **Globs:** every `glob` goes through the shell.

**Design:**
1. **Lean protocol.** Only the command itself runs as a process.
   - **In:** the command reaches the session through a heredoc read by the `read` builtin (no process). The delimiter is unique per command.
   - **Out:** stdout and stderr still go to per-command files (so background jobs and timeouts behave as now). They are read back with the `read` builtin and returned with one `printf` behind a header of byte lengths (`__BLYTE_<id>__ <rc> <stdout bytes> <stderr bytes>`), measured under `LC_ALL=C`. The previous `LC_ALL` is restored.
   - **Reader:** it becomes byte-based.
   - **`set -e`:** the protocol's own `read`s end in `|| true`, so a model's `set -e` cannot kill the session through them.
   - **No `base64` needed:** sessions start in environments without `base64`.
   - **Container writes** (`SessionFs::write`) use the same heredoc route; the content's added final newline is stripped, so bytes are exact.
   - **Known limit:** the `read` builtin stops at a NUL byte, so output containing NUL (binary) is cut there. `cat` of a binary file already yields unusable text for a model.
2. **Native glob** for local workspaces: `search::glob_paths` emulates bash with `globstar`, `dotglob` and `nullglob`:
   - `**` crosses directories, `*` matches dotfiles, directories match, symlinked directories are not followed;
   - it uses `globset` over an `ignore` walk with no ignore rules, starting at the pattern's literal leading directories, depth-limited when there is no `**`;
   - results are sorted bytewise, at most `MAX_MATCHES`.
   
   Patterns it cannot emulate exactly (absolute, `.`/`..` components, ending in `/`) still go to the shell.

**Guardrail:** `test_persistent_shell.rs` (byte-identical to `bash -lc`) must pass unchanged. New tests cover delimiter-like content, 1 MB outputs, `set -e`, `LC_ALL` restoration, exact bytes for container writes, and glob results equal to the shell's for the same patterns (compared as sets, since the shell's order depends on locale).

**Tech Stack:** Rust 2021; new direct dependency `globset` (already a transitive dependency of `ignore`).

## Global Constraints

- Base branch: `feat/lean-protocol` (from `main`). Each task runs in `.wt/<task>` on branch `wip/<task>`. Claude merges back. Task 2 builds on Task 1.
- Do not run Docker. `Cargo.lock` is gitignored; do not force-add it.
- `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets` must pass. Known timing-flaky tests: `blyte-mcp`'s `test_blyte_status_and_cancel_lifecycle`, `blyte-cli`'s `test_cli_daemon_detached_forwarding`, and `blyte-core`'s `test_daemon_advisory_review_clean_diff_advances`. If one alone fails, re-run once and report both results.
- Existing tests must pass unmodified, except the edit named in Task 2 Step 5.
- Commit with `--no-gpg-sign` and the exact message in each task.

---

### Task 1: Lean session protocol

**Files:**
- Modify: `crates/blyte-agent/src/shell.rs`, `crates/blyte-agent/src/fs.rs`
- Test: `crates/blyte-agent/tests/test_session_protocol.rs` (create)

- [ ] **Step 1: Write the new tests.** Create `crates/blyte-agent/tests/test_session_protocol.rs`:

```rust
//! The lean session protocol: exact bytes in and out, robust to what the model's own shell state
//! (set -e, LC_ALL) and output look like.
#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use blyte_agent::fs::{Fs, SessionFs};
use blyte_agent::shell::{LocalShell, PersistentShell, Shell};

const T: Duration = Duration::from_secs(30);

fn setup() -> (tempfile::TempDir, PathBuf, PersistentShell) {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().canonicalize().unwrap();
    let p = PersistentShell::local(root.clone());
    (d, root, p)
}

#[test]
fn awkward_commands_and_large_outputs_are_exact() {
    let (_d, root, p) = setup();
    for cmd in [
        "printf '%s\\n' '__BLYTE_CMD_0_0__' \"a'b\" '$HOME' '`x`'",
        "head -c 1000000 /dev/zero | tr '\\0' a",
        "printf 'tail without newline'; printf 'err without newline' >&2",
        "cat <<'EOF'\nline one\n  indented $NOT_EXPANDED\nEOF",
        "true",
    ] {
        let old = LocalShell.exec(cmd, &root, T).unwrap();
        assert_eq!(p.exec(cmd, &root, T).unwrap(), old, "exec: {cmd}");
        assert_eq!(p.exec_user(cmd, &root, T).unwrap(), old, "exec_user: {cmd}");
    }
}

#[test]
fn the_models_set_e_does_not_break_the_session() {
    let (_d, root, p) = setup();
    p.exec_user("set -e", &root, T).unwrap();
    assert_eq!(p.exec_user("echo alive", &root, T).unwrap().stdout, "alive\n");
    assert_eq!(p.exec_user("echo again", &root, T).unwrap().stdout, "again\n");
}

#[test]
fn the_models_locale_is_restored_after_each_command() {
    let (_d, root, p) = setup();
    p.exec_user("export LC_ALL=POSIX", &root, T).unwrap();
    assert_eq!(p.exec_user("echo $LC_ALL", &root, T).unwrap().stdout, "POSIX\n");
    p.exec_user("unset LC_ALL", &root, T).unwrap();
    assert_eq!(
        p.exec_user("echo ${LC_ALL-unset}", &root, T).unwrap().stdout,
        "unset\n"
    );
}

#[test]
fn container_style_writes_are_exact_for_awkward_content() {
    let (_d, root, _p) = setup();
    let fs = SessionFs::local(root.clone());
    for (i, content) in [
        "",
        "ends with newline\n",
        "no trailing newline",
        "__BLYTE_CONTENT_0__\nlooks like a delimiter\n",
        "two trailing newlines\n\n",
        "quotes ' \" ` $ \\ and ✓",
    ]
    .iter()
    .enumerate()
    {
        let path = root.join(format!("w{i}.txt"));
        fs.write(&path, content).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), *content, "case {i}");
        assert_eq!(fs.read(&path).unwrap(), *content, "case {i}");
    }
}
```

- [ ] **Step 2: Run the new and existing session tests.** Run: `cargo test -p blyte-agent --test test_session_protocol --test test_persistent_shell`. Record the results: some of the new tests may already pass under the current protocol. (This is a refactor with a byte-identical contract, so no failing-first requirement applies.)

- [ ] **Step 3: The protocol.** In `crates/blyte-agent/src/shell.rs`:
  1. Delete the `B64` constant and the `b64_encode` and `b64_decode` functions (their only other user, `SessionFs::write`, changes in Step 4).
  2. In the imports, replace `use std::io::{BufRead, BufReader, Write};` with `use std::io::{Read, Write};`. If `exec_command` has its own inner `use std::io::Read;`, remove that inner import.
  3. Replace the whole `pub struct Session { ... }` definition, the `const READY` line, and the whole `impl Session { ... }` block (keep `Launcher`, `RunResult`, `PersistentShell` and the rest unchanged) with:

```rust
/// One long-lived `bash -l`. A command arrives through a heredoc read by the `read` builtin; its
/// stdout and stderr go to their own files and come back with one `printf` behind a header of
/// byte lengths. Only the command itself runs as a process: the previous protocol spawned about
/// six helpers per command (`base64`, `tr`, `rm`), about 25 ms on macOS.
pub struct Session {
    child: Child,
    stdin: ChildStdin,
    chunks: Receiver<Vec<u8>>,
    buf: Vec<u8>,
    tmp: String,
    next: u64,
    nonce: String,
}

const READY: &str = "__BLYTE_READY__ ";

/// A per-process token for heredoc delimiters, so no command or file content can end one early.
fn nonce() -> String {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{t:x}{:x}", std::process::id())
}

enum ReadErr {
    Timeout,
    Closed,
}

impl Session {
    pub fn start(launcher: &Launcher) -> Result<Session, String> {
        let mut cmd = launcher.command();
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot start a shell session: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut chunk = vec![0u8; 64 * 1024];
            loop {
                match stdout.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(chunk[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let mut s = Session {
            child,
            stdin,
            chunks: rx,
            buf: Vec::new(),
            tmp: String::new(),
            next: 0,
            nonce: nonce(),
        };
        s.send(
            "__blyte_t=$(mktemp -d 2>/dev/null || { mkdir -p /tmp/blyte.$$ && echo /tmp/blyte.$$; }); \
             echo \"__BLYTE_READY__ $__blyte_t\"\n",
        )?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match s.read_line(deadline) {
                Ok(l) if l.starts_with(READY) => {
                    s.tmp = l[READY.len()..].trim().to_string();
                    return Ok(s);
                }
                Ok(_) => continue, // profile output on stdout
                Err(_) => {
                    s.kill();
                    return Err("the shell session did not start".into());
                }
            }
        }
    }

    fn send(&mut self, script: &str) -> Result<(), String> {
        self.stdin
            .write_all(script.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|e| format!("the shell session closed: {e}"))
    }

    fn fill(&mut self, deadline: Instant) -> Result<(), ReadErr> {
        match self
            .chunks
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(c) => {
                self.buf.extend_from_slice(&c);
                Ok(())
            }
            Err(RecvTimeoutError::Timeout) => Err(ReadErr::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(ReadErr::Closed),
        }
    }

    /// The next line of session output, without its newline.
    fn read_line(&mut self, deadline: Instant) -> Result<String, ReadErr> {
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                return Ok(String::from_utf8_lossy(&line[..i]).into_owned());
            }
            self.fill(deadline)?;
        }
    }

    /// Exactly `n` bytes of session output.
    fn read_exact(&mut self, n: usize, deadline: Instant) -> Result<Vec<u8>, ReadErr> {
        while self.buf.len() < n {
            self.fill(deadline)?;
        }
        Ok(self.buf.drain(..n).collect())
    }

    /// Kill the session and everything it started.
    pub fn kill(&mut self) {
        #[cfg(unix)]
        {
            let pgid = self.child.id() as i32;
            if pgid > 0 {
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Run `command` to completion. `cwd: Some(dir)` runs it in a subshell in `dir`, so nothing
    /// it does persists; `None` runs it in the session itself, so `cd` and `export` carry over.
    pub fn run(&mut self, command: &str, cwd: Option<&Path>, timeout: Duration) -> RunResult {
        let id = self.next;
        self.next += 1;
        let (out, err) = (format!("{}/{id}.o", self.tmp), format!("{}/{id}.e", self.tmp));
        let (qo, qe) = (quote(&out), quote(&err));
        let delim = format!("__BLYTE_CMD_{id}_{}__", self.nonce);
        let body = match cwd {
            Some(d) => format!(
                "( cd {} && eval \"$__blyte_c\" )",
                quote(&d.to_string_lossy())
            ),
            None => "eval \"$__blyte_c\"".to_string(),
        };
        // `|| true` on the protocol's own reads: a model's `set -e` must not end the session
        // because `read` reports end of input. LC_ALL=C makes `${#var}` count bytes; the
        // model's LC_ALL is put back afterwards.
        let script = format!(
            "IFS= read -r -d '' __blyte_c <<'{delim}' || true\n{command}\n{delim}\n\
             {body} </dev/null >{qo} 2>{qe}; __blyte_r=$?; \
             IFS= read -r -d '' __blyte_o <{qo} || true; \
             IFS= read -r -d '' __blyte_e <{qe} || true; \
             __blyte_l=${{LC_ALL-__blyte_unset__}}; LC_ALL=C; \
             printf '__BLYTE_{id}__ %s %s %s\\n%s%s' \"$__blyte_r\" \"${{#__blyte_o}}\" \
             \"${{#__blyte_e}}\" \"$__blyte_o\" \"$__blyte_e\"; \
             if [ \"$__blyte_l\" = __blyte_unset__ ]; then unset LC_ALL; else LC_ALL=$__blyte_l; fi; \
             unset __blyte_c __blyte_o __blyte_e __blyte_l\n"
        );
        if self.send(&script).is_err() {
            return RunResult::Died { out, err };
        }
        let marker = format!("__BLYTE_{id}__ ");
        let deadline = Instant::now() + timeout;
        let fail = |e: ReadErr, out: String, err: String| match e {
            ReadErr::Timeout => RunResult::TimedOut { out, err },
            ReadErr::Closed => RunResult::Died { out, err },
        };
        let header = loop {
            match self.read_line(deadline) {
                Ok(l) => {
                    if let Some(rest) = l.strip_prefix(&marker) {
                        break rest.to_string();
                    }
                }
                Err(e) => return fail(e, out, err),
            }
        };
        let mut f = header.split(' ');
        let exit_code = f.next().and_then(|x| x.parse().ok()).unwrap_or(-1);
        let no: usize = f.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        let ne: usize = f.next().and_then(|x| x.parse().ok()).unwrap_or(0);
        match self.read_exact(no + ne, deadline) {
            Ok(bytes) => RunResult::Done(Output {
                stdout: String::from_utf8_lossy(&bytes[..no]).into_owned(),
                stderr: String::from_utf8_lossy(&bytes[no..]).into_owned(),
                exit_code,
            }),
            Err(e) => fail(e, out, err),
        }
    }

    /// Remove the session's scratch directory, then end it.
    fn close(&mut self) {
        let _ = self.send(&format!("rm -rf {}; exit 0\n", quote(&self.tmp)));
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.kill();
    }
}
```

  4. Update the `SessionFs` doc comment in `fs.rs` (Step 4). Nothing else in `shell.rs` changes.

- [ ] **Step 4: Heredoc writes.** In `crates/blyte-agent/src/fs.rs`, in `impl Fs for SessionFs`, replace the body of `fn write` with:

```rust
        let p = path.to_string_lossy().to_string();
        let dir = path
            .parent()
            .map(|d| d.to_string_lossy().to_string())
            .unwrap_or_default();
        let mkdir = if dir.is_empty() {
            String::new()
        } else {
            format!("mkdir -p -- {} && ", sh_quote(&dir))
        };
        // Content through a heredoc read by the `read` builtin: no process, no encoding. The
        // heredoc adds one final newline, which is stripped, so the bytes are exact.
        let delim = format!(
            "__BLYTE_CONTENT_{}_{}__",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let cmd = format!(
            "IFS= read -r -d '' __blyte_w <<'{delim}' || true\n{content}\n{delim}\n\
             __blyte_w=${{__blyte_w%$'\\n'}}; {mkdir}printf %s \"$__blyte_w\" > {}",
            sh_quote(&p)
        );
        match self.run(&cmd) {
            Some(o) if o.exit_code == 0 => Ok(()),
            Some(_) => Err(format!("cannot write {p} in container")),
            None => self.fallback.write(path, content),
        }
```

  In the `SessionFs` doc comment, replace `Content crosses base64-encoded, so no byte is interpreted on the way.` with `Content crosses through a heredoc read by the \`read\` builtin, so no byte is interpreted on the way.`

- [ ] **Step 5: Run the tests.** Run: `cargo test -p blyte-agent --test test_session_protocol --test test_persistent_shell`. Expected: all pass (4 new, 8 existing). If one fails, report the test, command and both outputs verbatim, and stop.

- [ ] **Step 6: Benchmark.** Run: `cargo run --release -p blyte-agent --example harness_bench -- --turns 200` and report the output verbatim.

- [ ] **Step 7: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed. Run `cargo fmt --all` first if formatting differs.

- [ ] **Step 8: Commit.**

```bash
git add crates/
git commit --no-gpg-sign -m "perf(agent): lean session protocol — builtin heredoc in, length-framed out, no helper processes"
```

---

### Task 2: Native glob for local workspaces

**Files:**
- Modify: `Cargo.toml`, `crates/blyte-agent/Cargo.toml`, `crates/blyte-agent/src/search.rs`, `crates/blyte-agent/src/tools.rs`
- Modify (one test, Step 5): `crates/blyte-agent/tests/test_tools.rs`
- Test: `crates/blyte-agent/tests/test_native_glob.rs` (create)

- [ ] **Step 1: Write the failing tests.** Create `crates/blyte-agent/tests/test_native_glob.rs`:

```rust
//! `glob` on a local workspace expands in-process, matching what the shell expands (bash
//! globstar + dotglob + nullglob, or zsh where bash lacks globstar), without spawning.
#![cfg(unix)]

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::json;
use blyte_agent::fs::LocalFs;
use blyte_agent::search::glob_paths;
use blyte_agent::shell::{LocalShell, Output, Shell};
use blyte_agent::tools::execute;

struct NoShell;
impl Shell for NoShell {
    fn exec(&self, command: &str, _: &Path, _: std::time::Duration) -> Result<Output, String> {
        panic!("glob spawned a command: {command}");
    }
    fn describe(&self) -> String {
        String::new()
    }
}

fn tree() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    for dir in ["src/deep", "src/.dot", "docs"] {
        std::fs::create_dir_all(r.join(dir)).unwrap();
    }
    for f in [
        "a.rs",
        ".hidden.rs",
        "src/b.rs",
        "src/c.rs",
        "src/deep/d.rs",
        "src/.dot/e.rs",
        "docs/readme.md",
    ] {
        std::fs::write(r.join(f), "x").unwrap();
    }
    std::os::unix::fs::symlink(r.join("src"), r.join("link")).unwrap();
    d
}

/// What the harness's shell glob returns for `pattern`, as a set.
fn shell_glob(root: &Path, pattern: &str) -> BTreeSet<String> {
    let o = execute(root, &LocalShell, &NotLocal, "glob", &json!({ "pattern": pattern }));
    o.content
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

struct NotLocal;
impl blyte_agent::fs::Fs for NotLocal {
    fn read(&self, p: &Path) -> Result<String, String> {
        blyte_agent::fs::Fs::read(&LocalFs, p)
    }
    fn write(&self, p: &Path, c: &str) -> Result<(), String> {
        blyte_agent::fs::Fs::write(&LocalFs, p, c)
    }
    fn canonicalize(&self, p: &Path) -> Result<std::path::PathBuf, String> {
        blyte_agent::fs::Fs::canonicalize(&LocalFs, p)
    }
}

#[test]
fn native_expansion_matches_the_shell() {
    let d = tree();
    for pattern in [
        "**/*.rs",
        "*.rs",
        "src/*",
        "src/**/*.rs",
        "**/*.md",
        "src/[bc]*.rs",
        "*",
    ] {
        let native: BTreeSet<String> = glob_paths(d.path(), pattern, 200)
            .expect("emulated")
            .into_iter()
            .collect();
        assert_eq!(native, shell_glob(d.path(), pattern), "{pattern}");
    }
}

#[test]
fn results_are_sorted_capped_and_unmatched_is_empty() {
    let d = tree();
    let all = glob_paths(d.path(), "**/*.rs", 200).unwrap();
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(all, sorted);
    assert_eq!(glob_paths(d.path(), "**/*.rs", 2).unwrap().len(), 2);
    assert_eq!(glob_paths(d.path(), "nomatch*", 200).unwrap(), Vec::<String>::new());
    assert!(glob_paths(d.path(), "/etc/*", 200).is_none());
    assert!(glob_paths(d.path(), "../*", 200).is_none());
    assert!(glob_paths(d.path(), "src/", 200).is_none());
}

#[test]
fn the_glob_tool_uses_it_on_a_local_workspace_without_spawning() {
    let d = tree();
    let o = execute(d.path(), &NoShell, &LocalFs, "glob", &json!({"pattern": "src/*.rs"}));
    assert_eq!(o.content, "2 file(s):\nsrc/b.rs\nsrc/c.rs");
    let none = execute(d.path(), &NoShell, &LocalFs, "glob", &json!({"pattern": "*.zz"}));
    assert_eq!(none.content, "no files match *.zz");
}
```

- [ ] **Step 2: Run to confirm they fail.** Run: `cargo test -p blyte-agent --test test_native_glob`. Expected: a compile error, because `glob_paths` does not exist.

- [ ] **Step 3: Dependency.** Add `globset = "0.4.20"` to the root `[workspace.dependencies]` and `globset.workspace = true` to `crates/blyte-agent/Cargo.toml` `[dependencies]`.

- [ ] **Step 4: `glob_paths`.** Append to `crates/blyte-agent/src/search.rs`:

```rust
/// Paths matching a shell glob, as bash expands it with `globstar`, `dotglob` and `nullglob`,
/// relative to `root`: `**` crosses directories, `*` matches dotfiles, directories match too, and
/// symlinked directories are not followed. `None` for patterns this does not emulate exactly
/// (absolute, `.` or `..` components, or a trailing `/`); callers send those to the shell.
/// Sorted bytewise, at most `max`. No process is spawned: a shell glob cost 26 ms locally and
/// 84 ms in a container (harness_bench).
pub fn glob_paths(root: &Path, pattern: &str, max: usize) -> Option<Vec<String>> {
    if pattern.is_empty()
        || pattern.starts_with('/')
        || pattern.ends_with('/')
        || pattern.split('/').any(|c| c == "." || c == "..")
    {
        return None;
    }
    let is_meta = |c: &str| c.contains(['*', '?', '[', '{']);
    if !is_meta(pattern) {
        return Some(if root.join(pattern).exists() {
            vec![pattern.to_string()]
        } else {
            Vec::new()
        });
    }
    let matcher = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .backslash_escape(true)
        .build()
        .ok()?
        .compile_matcher();
    // Walk from the pattern's literal leading directories, only as deep as it can reach.
    let comps: Vec<&str> = pattern.split('/').collect();
    let lit = comps[..comps.len() - 1]
        .iter()
        .take_while(|c| !is_meta(c))
        .count();
    let base = comps[..lit]
        .iter()
        .fold(root.to_path_buf(), |p, c| p.join(c));
    if !base.is_dir() {
        return Some(Vec::new());
    }
    let mut walk = WalkBuilder::new(&base);
    walk.standard_filters(false).hidden(false).follow_links(false);
    if !pattern.contains("**") {
        walk.max_depth(Some(comps.len() - lit));
    }
    let mut hits: Vec<String> = walk
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() > 0)
        .filter_map(|e| {
            let rel = e.path().strip_prefix(root).ok()?.to_path_buf();
            matcher
                .is_match(&rel)
                .then(|| rel.to_string_lossy().into_owned())
        })
        .collect();
    hits.sort();
    hits.truncate(max);
    Some(hits)
}
```

- [ ] **Step 5: The `glob` tool, and the seam test.**
  1. In `crates/blyte-agent/src/tools.rs`, in the `"glob" =>` arm, directly after `let pattern = s("pattern");`, add:

```rust
            // A local workspace expands in-process; other executors, and patterns the native
            // expansion does not emulate, use the shell as before.
            if fs.is_local() {
                if let Some(hits) = crate::search::glob_paths(root, &pattern, MAX_MATCHES) {
                    return if hits.is_empty() {
                        ToolOutcome::ok(format!("no files match {pattern}"))
                    } else {
                        ToolOutcome::ok(format!("{} file(s):\n{}", hits.len(), hits.join("\n")))
                    };
                }
            }
```

  2. In `crates/blyte-agent/tests/test_tools.rs`, in `search_tools_also_go_through_the_seam`, change the `glob` call's filesystem argument from `&LocalFs` to `&NotLocal` (the `NotLocal` struct already defined in that file), and update the comment above it to say that a container's files are not local, so glob, like grep, must go through its executor. This is the only permitted edit to an existing test.

- [ ] **Step 6: Run the tests.** Run: `cargo test -p blyte-agent --test test_native_glob --test test_tools --test test_native_grep`. Expected: all pass (3 new). If `native_expansion_matches_the_shell` fails for a pattern, report the pattern and both sets verbatim, and stop. Do not change the test.

- [ ] **Step 7: Benchmark.** Run: `cargo run --release -p blyte-agent --example harness_bench -- --turns 200` and report the output verbatim.

- [ ] **Step 8: Full check.** Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`. Expected: clean, 0 failed.

- [ ] **Step 9: Commit.**

```bash
git add Cargo.toml crates/
git commit --no-gpg-sign -m "perf(agent): native glob for local workspaces"
```
