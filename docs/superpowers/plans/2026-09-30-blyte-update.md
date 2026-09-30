# `blyte update` — Plan

> **For agentic workers:** Implement exactly one task. Do NOT `git push`, open PRs, publish crates, start any paid run, or call any model API; commit locally only. This plan may be shadow-run: agy and blyte each implement it independently.

**Why (2026-09-30):** colleagues install blyte from a release binary or with `cargo install`, and
had no way to update except reinstalling. Every merge to main now produces a release with static
binaries, so `blyte update` installs the latest one in place: it downloads this machine's binary
through the GitHub CLI (`gh`, which already holds the user's access to the private repository, so
blyte handles no token), checks it runs, keeps the old binary as `blyte.prev`, swaps it in, and
refreshes the Claude Code skill when the installed copy is this version's own, unedited (keeping
whether blyte is the default). `--check` only shows the versions; `--rollback` swaps back.
Tests use a stand-in `gh` (`BLYTE_GH`) and run a copy of blyte, so the real binary is never replaced.

## Global Constraints

- Work only in your own worktree. Existing tests pass unmodified.
- Checks: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --all-targets`
- Commit with `--no-gpg-sign` and the exact message below.

### Task 1: `blyte update`

**Files:** create `crates/blyte-cli/src/update.rs`, `crates/blyte-cli/tests/test_update.rs`; modify
`crates/blyte-cli/src/cli.rs`, `crates/blyte-cli/src/lib.rs`, `crates/blyte-cli/src/main.rs`,
`docs/getting-started.md`, `docs/reference/cli.md`.

- [ ] **Step 1: apply this patch exactly.** Write it to a file outside the repository (for example
  `/tmp/blyte-update.patch`) in one go, then run `git apply /tmp/blyte-update.patch` from the
  repository root. Expected: no output, and `git status` shows the seven files above.

````diff
diff --git a/crates/blyte-cli/src/cli.rs b/crates/blyte-cli/src/cli.rs
index b0dad84..95c6bef 100644
--- a/crates/blyte-cli/src/cli.rs
+++ b/crates/blyte-cli/src/cli.rs
@@ -23,6 +23,15 @@ pub enum Command {
     },
     /// Install or print the Claude Code skill for the plan, delegate, review workflow
     Skill(SkillArgs),
+    /// Replace this binary with the latest release (the old one is kept as blyte.prev)
+    Update {
+        /// Only show the installed and latest versions
+        #[arg(long, conflicts_with = "rollback")]
+        check: bool,
+        /// Swap back to the previous build (blyte.prev)
+        #[arg(long)]
+        rollback: bool,
+    },
 }
 
 #[derive(clap::Args, Debug, PartialEq, Clone)]
diff --git a/crates/blyte-cli/src/lib.rs b/crates/blyte-cli/src/lib.rs
index 4934275..7933719 100644
--- a/crates/blyte-cli/src/lib.rs
+++ b/crates/blyte-cli/src/lib.rs
@@ -6,3 +6,4 @@ pub mod repl;
 pub mod report;
 pub mod skill;
 pub mod task_runner;
+pub mod update;
diff --git a/crates/blyte-cli/src/main.rs b/crates/blyte-cli/src/main.rs
index 2f33c1c..687d816 100644
--- a/crates/blyte-cli/src/main.rs
+++ b/crates/blyte-cli/src/main.rs
@@ -83,6 +83,9 @@ fn main() {
             blyte_cli::cli::Command::Status { id, json } => {
                 std::process::exit(blyte_cli::dispatch::run_status(&id, json));
             }
+            blyte_cli::cli::Command::Update { check, rollback } => {
+                std::process::exit(blyte_cli::update::run(check, rollback));
+            }
             blyte_cli::cli::Command::Skill(args) => match args.action {
                 blyte_cli::cli::SkillAction::Install {
                     project,
diff --git a/crates/blyte-cli/src/update.rs b/crates/blyte-cli/src/update.rs
new file mode 100644
index 0000000..55f7f2b
--- /dev/null
+++ b/crates/blyte-cli/src/update.rs
@@ -0,0 +1,227 @@
+//! `blyte update`: replace this binary with the latest GitHub release (2026-09-30).
+//!
+//! Releases carry one static binary per platform (release-on-merge.yml). The repository is private,
+//! so the download goes through the GitHub CLI (`gh`), which already holds the user's access;
+//! blyte handles no token. The new binary must run before it replaces the old one, the old one is
+//! kept beside it as `blyte.prev` (`blyte update --rollback` swaps them back), and the Claude Code
+//! skill is refreshed when the installed copy is this version's, unedited.
+
+use std::path::{Path, PathBuf};
+use std::process::Command;
+
+/// The repository whose releases are installed.
+pub const REPO: &str = "climate-x-org/blyte";
+
+/// The release asset for this machine, or `None` where no binary is built.
+pub fn asset_name() -> Option<&'static str> {
+    match (std::env::consts::OS, std::env::consts::ARCH) {
+        ("macos", "aarch64") => Some("blyte-aarch64-apple-darwin"),
+        ("macos", "x86_64") => Some("blyte-x86_64-apple-darwin"),
+        ("linux", "x86_64") => Some("blyte-x86_64-unknown-linux-musl"),
+        _ => None,
+    }
+}
+
+/// `v0.2.1` or `0.2.1` as `(0, 2, 1)`.
+pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
+    let mut parts = s.trim().trim_start_matches('v').split('.');
+    let v = (
+        parts.next()?.parse().ok()?,
+        parts.next()?.parse().ok()?,
+        parts
+            .next()?
+            .split(|c: char| !c.is_ascii_digit())
+            .next()?
+            .parse()
+            .ok()?,
+    );
+    Some(v)
+}
+
+/// The GitHub CLI; `BLYTE_GH` names another (the tests use a stand-in).
+fn gh() -> String {
+    std::env::var("BLYTE_GH").unwrap_or_else(|_| "gh".to_string())
+}
+
+fn latest_tag() -> Result<String, String> {
+    let out = Command::new(gh())
+        .args([
+            "release", "view", "--repo", REPO, "--json", "tagName", "--jq", ".tagName",
+        ])
+        .output()
+        .map_err(|e| {
+            format!("could not run the GitHub CLI (gh): {e}; install it and run `gh auth login`")
+        })?;
+    if !out.status.success() {
+        return Err(format!(
+            "gh could not read the latest release of {REPO}: {}",
+            String::from_utf8_lossy(&out.stderr).trim()
+        ));
+    }
+    let tag = String::from_utf8_lossy(&out.stdout).trim().to_string();
+    if parse_version(&tag).is_none() {
+        return Err(format!("unexpected release tag: {tag:?}"));
+    }
+    Ok(tag)
+}
+
+/// The installed binary: this one, or its `blyte` sibling when this is `blyte.prev` (so a rollback
+/// also works from the previous build).
+fn current_exe() -> Result<PathBuf, String> {
+    let exe = std::env::current_exe()
+        .and_then(|p| p.canonicalize())
+        .map_err(|e| format!("cannot find this blyte binary: {e}"))?;
+    Ok(if exe.file_name().is_some_and(|n| n == "blyte.prev") {
+        exe.with_file_name("blyte")
+    } else {
+        exe
+    })
+}
+
+fn prev_path(exe: &Path) -> PathBuf {
+    exe.with_file_name("blyte.prev")
+}
+
+fn version_of(bin: &Path) -> Option<String> {
+    let out = Command::new(bin).arg("--version").output().ok()?;
+    out.status
+        .success()
+        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
+}
+
+/// Swap `exe` and `blyte.prev`.
+fn rollback(exe: &Path) -> Result<String, String> {
+    let prev = prev_path(exe);
+    if !prev.is_file() {
+        return Err(format!(
+            "no previous build to roll back to ({} is missing)",
+            prev.display()
+        ));
+    }
+    let swap = exe.with_file_name(".blyte-swap");
+    std::fs::rename(exe, &swap).map_err(|e| format!("rollback failed: {e}"))?;
+    std::fs::rename(&prev, exe).map_err(|e| format!("rollback failed: {e}"))?;
+    std::fs::rename(&swap, &prev).map_err(|e| format!("rollback failed: {e}"))?;
+    Ok(version_of(exe).unwrap_or_else(|| "blyte (version unknown)".into()))
+}
+
+/// Refresh the installed Claude Code skill with `new_exe` when the installed copy is this
+/// version's own, unedited, keeping whether blyte is the default. Returns a line to print.
+fn refresh_skill(new_exe: &Path) -> Option<String> {
+    let home = std::env::var_os("HOME")?;
+    let skill = PathBuf::from(&home).join(".claude/skills/blyte-implementer/SKILL.md");
+    let installed = std::fs::read_to_string(&skill).ok()?;
+    if installed != crate::skill::SKILL_MD {
+        return Some(format!(
+            "skill not refreshed: {} has been edited (run `blyte skill install --force` to replace it)",
+            skill.display()
+        ));
+    }
+    let claude_md = PathBuf::from(&home).join(".claude/CLAUDE.md");
+    let default_on = std::fs::read_to_string(claude_md)
+        .map(|t| t.contains("<!-- blyte:default -->"))
+        .unwrap_or(false);
+    let mut args = vec!["skill", "install", "--force"];
+    if !default_on {
+        args.push("--no-default");
+    }
+    let ok = Command::new(new_exe)
+        .args(&args)
+        .output()
+        .map(|o| o.status.success())
+        .unwrap_or(false);
+    Some(if ok {
+        "skill refreshed (new Claude Code sessions pick it up)".to_string()
+    } else {
+        "skill not refreshed: `blyte skill install` failed; run it by hand".to_string()
+    })
+}
+
+fn update(check: bool) -> Result<(), String> {
+    let current = env!("CARGO_PKG_VERSION");
+    let tag = latest_tag()?;
+    let (now, latest) = (parse_version(current), parse_version(&tag));
+    println!(
+        "installed: blyte {current}\nlatest:    blyte {}",
+        tag.trim_start_matches('v')
+    );
+    if latest <= now {
+        println!("blyte is up to date");
+        return Ok(());
+    }
+    if check {
+        println!("run `blyte update` to install it");
+        return Ok(());
+    }
+    let asset = asset_name().ok_or_else(|| {
+        format!(
+            "no release binary for {}-{}; build it with `cargo install --git https://github.com/{REPO} blyte`",
+            std::env::consts::OS,
+            std::env::consts::ARCH
+        )
+    })?;
+    let exe = current_exe()?;
+    let tmp = exe.with_file_name(format!(".blyte-update-{}", std::process::id()));
+    let out = Command::new(gh())
+        .args([
+            "release",
+            "download",
+            &tag,
+            "--repo",
+            REPO,
+            "--pattern",
+            asset,
+            "--clobber",
+            "-O",
+        ])
+        .arg(&tmp)
+        .output()
+        .map_err(|e| format!("could not run the GitHub CLI (gh): {e}"))?;
+    if !out.status.success() || !tmp.is_file() {
+        let _ = std::fs::remove_file(&tmp);
+        return Err(format!(
+            "download failed: {}",
+            String::from_utf8_lossy(&out.stderr).trim()
+        ));
+    }
+    #[cfg(unix)]
+    {
+        use std::os::unix::fs::PermissionsExt;
+        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
+            .map_err(|e| format!("could not make the download executable: {e}"))?;
+    }
+    let Some(new_version) = version_of(&tmp) else {
+        let _ = std::fs::remove_file(&tmp);
+        return Err("the downloaded binary does not run; the installed blyte is unchanged".into());
+    };
+    let prev = prev_path(&exe);
+    std::fs::copy(&exe, &prev).map_err(|e| format!("could not keep the previous build: {e}"))?;
+    std::fs::rename(&tmp, &exe).map_err(|e| format!("could not replace {}: {e}", exe.display()))?;
+    println!(
+        "updated to {new_version}; the previous build is kept as {}",
+        prev.display()
+    );
+    println!("`blyte update --rollback` restores it");
+    if let Some(line) = refresh_skill(&exe) {
+        println!("{line}");
+    }
+    Ok(())
+}
+
+/// `blyte update [--check | --rollback]`; returns the process exit code.
+pub fn run(check: bool, rollback_requested: bool) -> i32 {
+    let result = if rollback_requested {
+        current_exe()
+            .and_then(|exe| rollback(&exe))
+            .map(|v| println!("rolled back to {v}"))
+    } else {
+        update(check)
+    };
+    match result {
+        Ok(()) => 0,
+        Err(e) => {
+            eprintln!("error: {e}");
+            1
+        }
+    }
+}
diff --git a/crates/blyte-cli/tests/test_update.rs b/crates/blyte-cli/tests/test_update.rs
new file mode 100644
index 0000000..a9b290b
--- /dev/null
+++ b/crates/blyte-cli/tests/test_update.rs
@@ -0,0 +1,173 @@
+//! `blyte update` replaces the binary with the latest release, keeps the old one as blyte.prev,
+//! rolls back, and refreshes an unedited skill (2026-09-30). A stand-in `gh` (BLYTE_GH) serves the
+//! release, and the tests run a copy of blyte, so the real binary is never replaced.
+
+use std::os::unix::fs::PermissionsExt;
+use std::path::{Path, PathBuf};
+use std::process::Command;
+
+use blyte_cli::skill::SKILL_MD;
+use blyte_cli::update::{asset_name, parse_version};
+
+struct Env {
+    _dir: tempfile::TempDir,
+    bin: PathBuf,
+    gh: PathBuf,
+    home: PathBuf,
+    log: PathBuf,
+}
+
+fn exe(path: &Path, body: &str) {
+    std::fs::write(path, body).unwrap();
+    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
+}
+
+/// A copy of blyte in a temp dir, a stand-in gh serving `tag` and a release binary that prints
+/// `new_version` (or fails when `broken`), and an empty HOME.
+fn setup(tag: &str, broken: bool) -> Env {
+    let dir = tempfile::tempdir().unwrap();
+    let bin = dir.path().join("bin/blyte");
+    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
+    std::fs::copy(env!("CARGO_BIN_EXE_blyte"), &bin).unwrap();
+    let log = dir.path().join("calls.log");
+    let release = dir.path().join("release-blyte");
+    if broken {
+        exe(&release, "#!/bin/sh\nexit 1\n");
+    } else {
+        exe(
+            &release,
+            &format!(
+                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'blyte 9.9.9 (test)'; exit 0; fi\necho \"$@\" >> {}\n",
+                log.display()
+            ),
+        );
+    }
+    let gh = dir.path().join("gh");
+    exe(
+        &gh,
+        &format!(
+            "#!/bin/sh\ncase \"$2\" in\n  view) echo '{tag}' ;;\n  download) while [ $# -gt 0 ]; do [ \"$1\" = -O ] && out=$2; shift; done; cp {} \"$out\" ;;\nesac\n",
+            release.display()
+        ),
+    );
+    let home = dir.path().join("home");
+    std::fs::create_dir_all(&home).unwrap();
+    Env {
+        _dir: dir,
+        bin,
+        gh,
+        home,
+        log,
+    }
+}
+
+fn run(e: &Env, args: &[&str]) -> (i32, String) {
+    let out = Command::new(&e.bin)
+        .arg("update")
+        .args(args)
+        .env("BLYTE_GH", &e.gh)
+        .env("HOME", &e.home)
+        .output()
+        .unwrap();
+    let text =
+        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
+    (out.status.code().unwrap_or(-1), text)
+}
+
+fn version(bin: &Path) -> String {
+    String::from_utf8_lossy(&Command::new(bin).arg("--version").output().unwrap().stdout)
+        .into_owned()
+}
+
+#[test]
+fn versions_parse_and_this_machine_has_an_asset() {
+    assert_eq!(parse_version("v0.2.1"), Some((0, 2, 1)));
+    assert_eq!(parse_version("0.10.3"), Some((0, 10, 3)));
+    assert_eq!(parse_version("v1.2"), None);
+    assert!(
+        asset_name().is_some(),
+        "the test machines are macOS or Linux x86_64"
+    );
+}
+
+#[test]
+fn check_reports_and_changes_nothing() {
+    let e = setup("v9.9.9", false);
+    let before = std::fs::read(&e.bin).unwrap();
+    let (code, text) = run(&e, &["--check"]);
+    assert_eq!(code, 0, "{text}");
+    assert!(text.contains("latest:    blyte 9.9.9"), "{text}");
+    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
+}
+
+#[test]
+fn update_replaces_the_binary_keeps_the_old_one_and_rolls_back() {
+    let e = setup("v9.9.9", false);
+    let before = std::fs::read(&e.bin).unwrap();
+    let (code, text) = run(&e, &[]);
+    assert_eq!(code, 0, "{text}");
+    assert!(version(&e.bin).contains("9.9.9"));
+    let prev = e.bin.with_file_name("blyte.prev");
+    assert_eq!(std::fs::read(&prev).unwrap(), before);
+
+    // Roll back with the previous build (the real blyte), which is now blyte.prev.
+    let out = Command::new(&prev)
+        .args(["update", "--rollback"])
+        .output()
+        .unwrap();
+    assert!(
+        out.status.success(),
+        "{}",
+        String::from_utf8_lossy(&out.stderr)
+    );
+    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
+}
+
+#[test]
+fn an_up_to_date_install_is_left_alone() {
+    let e = setup(&format!("v{}", env!("CARGO_PKG_VERSION")), false);
+    let before = std::fs::read(&e.bin).unwrap();
+    let (code, text) = run(&e, &[]);
+    assert_eq!(code, 0, "{text}");
+    assert!(text.contains("up to date"), "{text}");
+    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
+}
+
+#[test]
+fn a_download_that_does_not_run_changes_nothing() {
+    let e = setup("v9.9.9", true);
+    let before = std::fs::read(&e.bin).unwrap();
+    let (code, text) = run(&e, &[]);
+    assert_eq!(code, 1, "{text}");
+    assert!(text.contains("does not run"), "{text}");
+    assert_eq!(std::fs::read(&e.bin).unwrap(), before);
+    assert!(!e.bin.with_file_name("blyte.prev").exists());
+}
+
+#[test]
+fn an_unedited_skill_is_refreshed_keeping_the_default_setting() {
+    let e = setup("v9.9.9", false);
+    let skill = e.home.join(".claude/skills/blyte-implementer/SKILL.md");
+    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
+    std::fs::write(&skill, SKILL_MD).unwrap();
+    let (code, text) = run(&e, &[]);
+    assert_eq!(code, 0, "{text}");
+    assert!(text.contains("skill refreshed"), "{text}");
+    // No default block in CLAUDE.md, so the refresh keeps "only when asked".
+    assert_eq!(
+        std::fs::read_to_string(&e.log).unwrap().trim(),
+        "skill install --force --no-default"
+    );
+}
+
+#[test]
+fn an_edited_skill_is_not_touched() {
+    let e = setup("v9.9.9", false);
+    let skill = e.home.join(".claude/skills/blyte-implementer/SKILL.md");
+    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
+    std::fs::write(&skill, "my own edits\n").unwrap();
+    let (code, text) = run(&e, &[]);
+    assert_eq!(code, 0, "{text}");
+    assert!(text.contains("has been edited"), "{text}");
+    assert!(!e.log.exists());
+}
diff --git a/docs/getting-started.md b/docs/getting-started.md
index e91b4cf..520aa9d 100644
--- a/docs/getting-started.md
+++ b/docs/getting-started.md
@@ -10,9 +10,20 @@ blyte is a single Rust binary, for macOS and Linux. With a recent stable Rust to
 
 ```bash
 cargo install --git https://github.com/climate-x-org/blyte blyte --locked
-blyte --version        # blyte 0.1.0 (<commit>)
+blyte --version        # blyte <version> (<commit>)
 ```
 
+Or, without Rust, download the ready-built binary from the latest release with the GitHub CLI
+(`blyte-x86_64-apple-darwin` on an Intel Mac, `blyte-x86_64-unknown-linux-musl` on Linux):
+
+```bash
+gh release download --repo climate-x-org/blyte --pattern blyte-aarch64-apple-darwin -O ~/.local/bin/blyte
+chmod +x ~/.local/bin/blyte
+```
+
+To update later, `blyte update` installs the latest release in place (`--check` only shows the
+versions; `--rollback` restores the previous build, kept as `blyte.prev`).
+
 ## 2. Give it a model
 
 blyte's model does the implementation, so pick a cheap one that follows instructions well. The
diff --git a/docs/reference/cli.md b/docs/reference/cli.md
index 5d055df..3b37cbe 100644
--- a/docs/reference/cli.md
+++ b/docs/reference/cli.md
@@ -11,6 +11,7 @@ Commands:
   dispatch  Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary
   review    Show what a dispatched run did: commits, files, tree state, report, and optionally the diff
   skill     Install or print the Claude Code skill for the plan, delegate, review workflow
+  update    Replace this binary with the latest release (the old one is kept as blyte.prev)
   help      Print this message or the help of the given subcommand(s)
 
 Arguments:
@@ -107,6 +108,7 @@ Commands:
   dispatch  Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary
   review    Show what a dispatched run did: commits, files, tree state, report, and optionally the diff
   skill     Install or print the Claude Code skill for the plan, delegate, review workflow
+  update    Replace this binary with the latest release (the old one is kept as blyte.prev)
   help      Print this message or the help of the given subcommand(s)
 
 Arguments:
@@ -162,6 +164,7 @@ Commands:
   dispatch  Run one task in its own git worktree (.wt/<id>, branch wip/<id>) and print a JSON summary
   review    Show what a dispatched run did: commits, files, tree state, report, and optionally the diff
   skill     Install or print the Claude Code skill for the plan, delegate, review workflow
+  update    Replace this binary with the latest release (the old one is kept as blyte.prev)
   help      Print this message or the help of the given subcommand(s)
 
 Arguments:
@@ -206,6 +209,23 @@ Options:
           Print version
 ```
 
+## blyte update
+
+```text
+Replace this binary with the latest release (the old one is kept as blyte.prev)
+
+Usage: blyte update [OPTIONS]
+
+Options:
+      --check     Only show the installed and latest versions
+      --rollback  Swap back to the previous build (blyte.prev)
+  -h, --help      Print help
+```
+
+It downloads this machine's binary from the latest GitHub release through the GitHub CLI (`gh`,
+signed in with access to the repository), checks that it runs, keeps the old binary beside it as
+`blyte.prev`, and refreshes the Claude Code skill if the installed copy is unedited.
+
 ## Environment
 
 | variable | meaning |
````

- [ ] **Step 2: run the update tests.** `cargo test -p blyte --test test_update`. Expected: 7 passed.

- [ ] **Step 3: run the checks and commit.**

```bash
git add crates/blyte-cli/src/update.rs crates/blyte-cli/tests/test_update.rs crates/blyte-cli/src/cli.rs crates/blyte-cli/src/lib.rs crates/blyte-cli/src/main.rs docs/getting-started.md docs/reference/cli.md
git commit --no-gpg-sign -m "feat(cli): blyte update installs the latest release, with --check and --rollback"
```
